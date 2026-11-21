//! Command lines in each row's own argument forms.
use command_surface::*;
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadRequest {
    target: String,
    #[serde(default)]
    words: Option<String>,
    #[serde(default)]
    view: Option<String>,
    #[serde(default)]
    full: bool,
    #[serde(default)]
    tag: Vec<String>,
    #[serde(default)]
    limit: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecRequest {
    command: Vec<String>,
}

fn read_spec() -> OperationSpec {
    OperationSpec {
        arguments: vec![
            ArgumentSpec::new("target", ArgumentForm::Positional, "what to read"),
            ArgumentSpec::new("words", ArgumentForm::Words, "words to find"),
            ArgumentSpec::new("view", ArgumentForm::Named, "the view").short('v'),
            ArgumentSpec::new("full", ArgumentForm::Flag, "show every field"),
            ArgumentSpec::new("tag", ArgumentForm::Repeated, "a tag"),
            ArgumentSpec::new("out", ArgumentForm::Named, "write here").calling_process(),
        ],
        ..OperationSpec::request(
            "read",
            &["read"],
            "read",
            "Read a target",
            "Reads one target",
        )
    }
}

fn exec_spec() -> OperationSpec {
    OperationSpec {
        arguments: vec![ArgumentSpec::new(
            "command",
            ArgumentForm::Rest,
            "the command",
        )],
        ..OperationSpec::request(
            "exec",
            &["exec"],
            "act",
            "Run a command",
            "Runs one command",
        )
    }
}

fn table() -> DeclarationsBuilder {
    let mut table = OperationDeclarations::builder("example", Deadlines::default());
    for option in RequestOption::ALL {
        table.accept(option);
    }
    for name in ["read", "act"] {
        table
            .declare_tool(ToolFamily {
                name: name.into(),
                description: format!("{name} things"),
                open_world: false,
                idempotent: true,
            })
            .unwrap();
    }
    table
        .declare::<ReadRequest, Value>(read_spec(), |value, _style| format!("{value}\n"))
        .unwrap();
    table
        .declare::<ExecRequest, Value>(exec_spec(), |value, _style| format!("{value}\n"))
        .unwrap();
    table.set_default_operation("read", |_| true).unwrap();
    table
}
fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
}

#[test]
fn structured_mcp_calls_preserve_the_canonical_request_options() {
    let table = table();
    for deadline in [
        Deadline::Default,
        Deadline::None,
        Deadline::Milliseconds(1234),
    ] {
        for budget_tokens in [
            TokenBudget::Default,
            TokenBudget::Unlimited,
            TokenBudget::Tokens(57),
        ] {
            let request = RunnableRequest {
                deadline,
                budget_tokens,
                cursor: Some("opaque +/?# λ".into()),
                fresh: true,
                format: Format::Json,
                ..RunnableRequest::new("read", json!({"target":"session-1"}), "accepted-child")
            };
            fn require_eq(_: &impl Eq) {}
            require_eq(&request);
            let mut arguments = serde_json::to_value(&request).unwrap();
            // The tool has one operation, which its declaration supplies.
            arguments.as_object_mut().unwrap().remove("operation");
            assert_eq!(
                table.mcp_call(&request).unwrap(),
                json!({"name":"example_read", "arguments":arguments})
            );
            let call = table.mcp_call(&request).unwrap();
            assert_eq!(
                table
                    .parse_mcp_call(call["name"].as_str().unwrap(), call["arguments"].clone())
                    .unwrap(),
                request
            );
            let mut explicit = call["arguments"].clone();
            explicit["operation"] = json!("read");
            assert_eq!(
                table.parse_mcp_call("example_read", explicit).unwrap(),
                request
            );
            let text = table.follow_up(Via::Mcp, &request).unwrap();
            let (name, body) = text.split_once(' ').unwrap();
            assert_eq!(name, "example_read");
            assert_eq!(serde_json::from_str::<Value>(body).unwrap(), arguments);
        }
    }
}

#[test]
fn positional_words_flags_and_repeats_fill_the_request() {
    let table = table();
    let parsed = table
        .parse_cli(
            &words("read session-1 why it failed --full -v outline --tag a --tag b --out /tmp/x --budget 900 --json"),
            "r-1",
        )
        .unwrap();
    assert_eq!(parsed.request.format, Format::Json);
    assert_eq!(
        parsed.calling_process,
        json!({"out": "/tmp/x"}).as_object().unwrap().clone()
    );
    assert_eq!(
        parsed.request.budget_tokens,
        command_surface::TokenBudget::Tokens(900)
    );
    assert_eq!(
        parsed.request.arguments,
        json!({"target": "session-1", "words": "why it failed", "full": true, "view": "outline", "tag": ["a", "b"]})
    );
    let rest = table
        .parse_cli(&words("exec git log --oneline -n 3"), "r-1")
        .unwrap();
    assert_eq!(
        rest.request.arguments,
        json!({"command": ["git", "log", "--oneline", "-n", "3"]})
    );
    let after_marker = table
        .parse_cli(&words("read -- --target-like -x"), "r-1")
        .unwrap();
    assert_eq!(
        after_marker.request.arguments,
        json!({"target": "--target-like", "words": "-x"})
    );
    let inline = table
        .parse_cli(
            &words("read s --full=false --limit=5 --fresh --cursor c-9"),
            "r-1",
        )
        .unwrap();
    assert_eq!(
        inline.request.arguments,
        json!({"target": "s", "full": false, "limit": 5})
    );
    assert!(inline.request.fresh);
    assert_eq!(inline.request.cursor.as_deref(), Some("c-9"));
}

#[test]
fn the_default_operation_reads_a_line_with_no_command() {
    let table = table();
    let parsed = table.parse_cli(&words("session-9 errors"), "r-1").unwrap();
    assert_eq!(parsed.request.operation, "read");
    assert_eq!(
        parsed.request.arguments,
        json!({"target": "session-9", "words": "errors"})
    );
}

#[test]
fn malformed_command_lines_name_what_is_wrong() {
    let table = table();
    for (line, message) in [
        ("read", "missing TARGET"),
        ("read s --unknown 1", "unknown argument --unknown"),
        ("read s --full=maybe", "--full is true or false"),
        ("read s --view", "--view needs a value"),
        ("read s --view a --view b", "duplicate --view"),
        ("read s --limit many", "--limit"),
        ("read s --request {}", "--request cannot be mixed"),
    ] {
        let error = table.parse_cli(&words(line), "r-1").unwrap_err();
        assert!(error.message.contains(message), "{line}: {}", error.message);
        assert_eq!(error.kind, FailureKind::BadRequest);
    }
}

#[test]
fn declarations_the_request_cannot_hold_are_refused() {
    let mut spec = read_spec();
    for (argument, message) in [
        (
            ArgumentSpec::new("missing", ArgumentForm::Named, ""),
            "not a field",
        ),
        (
            ArgumentSpec::new("view", ArgumentForm::Flag, ""),
            "cannot be written",
        ),
        (
            ArgumentSpec::new("target", ArgumentForm::Repeated, ""),
            "cannot be written",
        ),
        (
            ArgumentSpec::new("json", ArgumentForm::Flag, ""),
            "flag of every row",
        ),
    ] {
        spec.arguments = vec![argument];

        let mut table = OperationDeclarations::builder("example", Deadlines::default());
        table
            .declare_tool(ToolFamily {
                name: "read".into(),
                description: "read".into(),
                open_world: false,
                idempotent: true,
            })
            .unwrap();
        let error = table
            .declare::<ReadRequest, Value>(spec.clone(), |_, _style| String::new())
            .unwrap_err();
        assert!(error.message.contains(message), "{}", error.message);
    }
}

#[test]
fn help_shows_each_rows_usage() {
    let table = table();
    assert_eq!(
        table.help(),
        concat!(
            "example read TARGET [WORDS...] [--view VIEW] [--full] [--tag TAG...] [--out OUT] [--limit LIMIT]  Read a target\n",
            "example exec -- COMMAND...  Run a command\n",
        )
    );
    assert!(
        table
            .help_for("read")
            .unwrap()
            .contains("  --view  the view\n")
    );
}

fn request_strategy() -> impl Strategy<Value = Value> {
    let text = "[ -~λ]{0,12}";
    (
        text,
        proptest::option::of(text),
        proptest::option::of(text),
        proptest::option::of(any::<bool>()),
        proptest::collection::vec(text, 0..3),
        proptest::option::of(any::<u64>()),
    )
        .prop_map(|(target, words, view, full, tag, limit)| {
            let mut object = serde_json::Map::new();
            object.insert("target".into(), target.into());
            if let Some(words) = words {
                object.insert("words".into(), words.into());
            }
            if let Some(view) = view {
                object.insert("view".into(), view.into());
            }
            if let Some(full) = full {
                object.insert("full".into(), full.into());
            }
            if !tag.is_empty() {
                object.insert("tag".into(), tag.into());
            }
            if let Some(limit) = limit {
                object.insert("limit".into(), limit.into());
            }
            Value::Object(object)
        })
}

/// Split a command line the way `sh` does for the quoting `quote_argument` writes.
fn shell_words(command: &str) -> Vec<String> {
    let output = std::process::Command::new("sh")
        .args(["-c", &format!("printf '%s\\0' {command}")])
        .output()
        .unwrap();
    assert!(output.status.success());
    output
        .stdout
        .strip_suffix(&[0])
        .unwrap()
        .split(|byte| *byte == 0)
        .map(|word| String::from_utf8(word.to_vec()).unwrap())
        .collect()
}

fn deadline_strategy() -> impl Strategy<Value = Deadline> {
    prop_oneof![
        Just(Deadline::Default),
        Just(Deadline::None),
        (0u64..10_000).prop_map(Deadline::Milliseconds),
    ]
}

/// One request of `operation` with every envelope option varied.
fn envelope_strategy(
    operation: &'static str,
    arguments: impl Strategy<Value = Value>,
) -> impl Strategy<Value = RunnableRequest> {
    (
        arguments,
        "[ -~λ]{1,10}",
        deadline_strategy(),
        prop_oneof![
            Just(TokenBudget::Default),
            Just(TokenBudget::Unlimited),
            (0u32..100_000).prop_map(TokenBudget::Tokens),
        ],
        proptest::option::of("[ -~]{0,8}"),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            move |(arguments, request_id, deadline, budget_tokens, cursor, fresh, json)| {
                RunnableRequest {
                    operation: operation.into(),
                    arguments,
                    request_id,
                    deadline,
                    budget_tokens,
                    cursor,
                    fresh,
                    format: if json { Format::Json } else { Format::Text },
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn cli_command_round_trips(
        request in prop_oneof![
            envelope_strategy("read", request_strategy()),
            // A row whose last positional takes the rest of the line.
            envelope_strategy(
                "exec",
                proptest::collection::vec("[ -~λ]{0,8}", 1..4)
                    .prop_map(|command| json!({ "command": command })),
            ),
        ],
    ) {
        let table = table();
        let command = table.cli_command(&request).unwrap();
        let words = shell_words(&command);
        prop_assert_eq!(&words[0], "example");
        let parsed = table.parse_cli(&words[1..], "unused").unwrap();
        prop_assert_eq!(parsed.request, request, "{}", command);
    }
}

#[test]
fn cli_command_uses_the_rows_own_forms() {
    let table = table();
    let request = RunnableRequest::new(
        "read",
        json!({"target": "session-1", "words": "why it failed", "full": true, "tag": ["a", "b"]}),
        "r-1",
    );
    assert_eq!(
        table.cli_command(&request).unwrap(),
        "example read --full --tag a --tag b --request-id r-1 session-1 'why it failed'"
    );
    let exec = RunnableRequest::new("exec", json!({"command": ["ls", "-la"]}), "r-1");
    assert_eq!(
        table.cli_command(&exec).unwrap(),
        "example exec --request-id r-1 -- ls -la"
    );
}

#[test]
fn request_fields_cannot_take_a_flag_of_every_row() {
    // An undeclared field is written `--name`, so a field named like an
    // envelope option would be read as that option.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Paged {
        cursor: Option<String>,
    }
    fn clash_spec() -> OperationSpec {
        OperationSpec::request("page", &["page"], "read", "Page", "Pages")
    }

    let mut table = OperationDeclarations::builder("example", Deadlines::default());
    table
        .declare_tool(ToolFamily {
            name: "read".into(),
            description: "read".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    let error = table
        .declare::<Paged, Value>(clash_spec(), |_, _style| String::new())
        .unwrap_err();
    assert!(error.message.contains("--cursor"), "{}", error.message);
}

#[test]
fn a_positional_field_may_share_a_flags_name() {
    // Declared positional, the field is never written as `--cursor`.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Paged {
        cursor: String,
    }
    fn positional_spec() -> OperationSpec {
        OperationSpec {
            arguments: vec![ArgumentSpec::new(
                "cursor",
                ArgumentForm::Positional,
                "where",
            )],
            ..OperationSpec::request("page", &["page"], "read", "Page", "Pages")
        }
    }

    let mut table = OperationDeclarations::builder("example", Deadlines::default());
    table
        .declare_tool(ToolFamily {
            name: "read".into(),
            description: "read".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    table
        .declare::<Paged, Value>(positional_spec(), |value, _style| format!("{value}\n"))
        .unwrap();
    let parsed = table
        .parse_cli(&["page".into(), "c-7".into()], "r-1")
        .unwrap();
    assert_eq!(parsed.request.arguments, json!({"cursor": "c-7"}));
}

#[test]
fn a_positional_array_is_one_json_value_including_empty_and_structured_lists() {
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct BatchRequest {
        entries: Vec<Value>,
    }
    fn batch_spec() -> OperationSpec {
        OperationSpec {
            arguments: vec![ArgumentSpec::new(
                "entries",
                ArgumentForm::Positional,
                "records",
            )],
            ..OperationSpec::request(
                "batch",
                &["batch"],
                "act",
                "Write records",
                "Writes records",
            )
        }
    }

    let mut table = table();
    table
        .declare::<BatchRequest, Value>(batch_spec(), |value, _style| value.to_string())
        .unwrap();
    for entries in [
        json!([]),
        json!(["two words", {"note":"quote ' and λ"}, [1, 2], false]),
    ] {
        let parsed = table
            .parse_cli(&["batch".into(), entries.to_string()], "r-array")
            .unwrap();
        assert_eq!(parsed.request.arguments, json!({"entries": entries}));
        let rendered = table.cli_command(&parsed.request).unwrap();
        let words = shell_words(&rendered);
        let restored = table.parse_cli(&words[1..], "unused").unwrap();
        assert_eq!(restored.request, parsed.request);
    }
    for invalid in ["not-json", "[broken"] {
        assert!(
            table
                .parse_cli(&["batch".into(), invalid.into()], "r-invalid")
                .is_err()
        );
    }
}

#[test]
fn mcp_parser_selects_only_declared_family_members_and_checks_the_envelope() {
    let mut table = table();
    let mut peek = read_spec();
    peek.name = "peek".into();
    peek.command = vec!["peek".into()];
    peek.http_route = Some("/v1/peek".into());
    table
        .declare::<ReadRequest, Value>(peek, |value, _| format!("{value}\n"))
        .unwrap();
    let mut hidden = read_spec();
    hidden.name = "private".into();
    hidden.command.clear();
    hidden.http_route = None;
    hidden.kind = OperationKind::Request { tool: None };
    table
        .declare::<ReadRequest, Value>(hidden, |value, _| format!("{value}\n"))
        .unwrap();
    let original =
        json!({"operation":"peek","arguments":{"target":"synthetic"},"request_id":"same retry"});
    assert_eq!(
        table
            .parse_mcp_call("example_read", original.clone())
            .unwrap(),
        RunnableRequest::new("peek", json!({"target":"synthetic"}), "same retry")
    );
    for (name, call, reason) in [
        (
            "example_read",
            json!({"arguments":{},"request_id":"r"}),
            "name the operation",
        ),
        (
            "example_read",
            json!({"operation":"exec","arguments":{},"request_id":"r"}),
            "not an operation",
        ),
        (
            "example_read",
            json!({"operation":"private","arguments":{},"request_id":"r"}),
            "not an operation",
        ),
        ("other_read", original.clone(), "unknown MCP tool"),
        ("example_private", original.clone(), "unknown MCP tool"),
        (
            "example_read",
            json!({"operation":"read","request_id":"r"}),
            "arguments",
        ),
        (
            "example_read",
            json!({"operation":"read","arguments":{},"request_id":42}),
            "string",
        ),
        (
            "example_read",
            json!({"operation":"read","arguments":{},"request_id":"r","author":"forged"}),
            "author",
        ),
        (
            "example_read",
            json!({"operation":"read","arguments":{},"request_id":"r","deadline":-1}),
            "Deadline",
        ),
        (
            "example_read",
            json!({"operation":"read","arguments":{},"request_id":"r","budget_tokens":false}),
            "TokenBudget",
        ),
        (
            "example_read",
            json!({"operation":"read","arguments":{},"request_id":"r","cursor":1}),
            "string",
        ),
    ] {
        let error = table.parse_mcp_call(name, call).unwrap_err();
        assert_eq!(error.kind, FailureKind::BadRequest);
        assert!(error.message.contains(reason), "{name}: {error:?}");
    }
}

#[test]
fn declared_null_word_round_trips_cli_and_flat_http_without_changing_numeric_values() {
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Limit {
        #[serde(default)]
        #[schemars(extend("cli_null" = "none"))]
        limit: Option<u64>,
    }
    let mut builder = OperationDeclarations::builder("example", Deadlines::default());
    builder
        .declare::<Limit, Value>(
            OperationSpec {
                kind: OperationKind::Request { tool: None },
                http_route: Some("/inspect".into()),
                ..OperationSpec::request(
                    "inspect",
                    &["inspect"],
                    "",
                    "Inspect",
                    "Inspect with a limit",
                )
            },
            |value, _| value.to_string(),
        )
        .unwrap();
    let table = builder.finish();
    for (word, value) in [
        ("none", Value::Null),
        ("0", json!(0)),
        ("18446744073709551615", json!(u64::MAX)),
    ] {
        let parsed = table
            .parse_cli(
                &["inspect".into(), "--limit".into(), word.into()],
                "limit-id",
            )
            .unwrap()
            .request;
        assert_eq!(parsed.arguments, json!({"limit":value}));
        assert_eq!(
            table.cli_command(&parsed).unwrap(),
            format!("example inspect --limit {word} --request-id limit-id")
        );
        let http = table
            .parse_http_query("inspect", &[("limit".into(), word.into())], "limit-id")
            .unwrap();
        assert_eq!(http.arguments, parsed.arguments);
    }
    assert_eq!(
        table
            .parse_cli(&["inspect".into()], "limit-id")
            .unwrap()
            .request
            .arguments,
        json!({})
    );
    assert!(
        table
            .parse_http_query(
                "inspect",
                &[("limit".into(), "unlimited".into())],
                "limit-id"
            )
            .is_err()
    );
}
