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

struct Read;
impl Operation for Read {
    type Request = ReadRequest;
    type Response = Value;
    fn spec(&self) -> OperationSpec {
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
    fn execute(
        &self,
        request: ReadRequest,
        _: RequestContext,
    ) -> OperationFuture<'_, Answer<Value>> {
        Box::pin(async move { Answer::ok(serde_json::to_value(request).unwrap()) })
    }
    fn render_text(&self, value: &Value) -> String {
        format!("{value}\n")
    }
}
struct Exec;
impl Operation for Exec {
    type Request = ExecRequest;
    type Response = Value;
    fn spec(&self) -> OperationSpec {
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
    fn execute(
        &self,
        request: ExecRequest,
        _: RequestContext,
    ) -> OperationFuture<'_, Answer<Value>> {
        Box::pin(async move { Answer::ok(json!(request.command)) })
    }
    fn render_text(&self, value: &Value) -> String {
        format!("{value}\n")
    }
}

fn table() -> OperationTable {
    let mut table = OperationTable::new("example", Deadlines::default());
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
    table.register(Read).unwrap();
    table.register(Exec).unwrap();
    table.set_default_operation("read").unwrap();
    table
}
fn words(line: &str) -> Vec<String> {
    line.split(' ').map(str::to_owned).collect()
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
    let mut spec = Read.spec();
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
            ArgumentSpec::new("tag", ArgumentForm::Positional, ""),
            "cannot be written",
        ),
        (
            ArgumentSpec::new("json", ArgumentForm::Flag, ""),
            "flag of every row",
        ),
    ] {
        spec.arguments = vec![argument];
        struct Declared(OperationSpec);
        impl Operation for Declared {
            type Request = ReadRequest;
            type Response = Value;
            fn spec(&self) -> OperationSpec {
                self.0.clone()
            }
            fn execute(
                &self,
                _: ReadRequest,
                _: RequestContext,
            ) -> OperationFuture<'_, Answer<Value>> {
                Box::pin(async { Answer::ok(Value::Null) })
            }
            fn render_text(&self, _: &Value) -> String {
                String::new()
            }
        }
        let mut table = OperationTable::new("example", Deadlines::default());
        table
            .declare_tool(ToolFamily {
                name: "read".into(),
                description: "read".into(),
                open_world: false,
                idempotent: true,
            })
            .unwrap();
        let error = table.register(Declared(spec.clone())).unwrap_err();
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
    struct Clash;
    impl Operation for Clash {
        type Request = Paged;
        type Response = Value;
        fn spec(&self) -> OperationSpec {
            OperationSpec::request("page", &["page"], "read", "Page", "Pages")
        }
        fn execute(&self, _: Paged, _: RequestContext) -> OperationFuture<'_, Answer<Value>> {
            Box::pin(async { Answer::ok(Value::Null) })
        }
        fn render_text(&self, _: &Value) -> String {
            String::new()
        }
    }
    let mut table = OperationTable::new("example", Deadlines::default());
    table
        .declare_tool(ToolFamily {
            name: "read".into(),
            description: "read".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    let error = table.register(Clash).unwrap_err();
    assert!(error.message.contains("--cursor"), "{}", error.message);
}

#[test]
fn a_positional_field_may_share_a_flags_name() {
    // Declared positional, the field is never written as `--cursor`.
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Paged {
        cursor: String,
    }
    struct Positional;
    impl Operation for Positional {
        type Request = Paged;
        type Response = Value;
        fn spec(&self) -> OperationSpec {
            OperationSpec {
                arguments: vec![ArgumentSpec::new(
                    "cursor",
                    ArgumentForm::Positional,
                    "where",
                )],
                ..OperationSpec::request("page", &["page"], "read", "Page", "Pages")
            }
        }
        fn execute(&self, request: Paged, _: RequestContext) -> OperationFuture<'_, Answer<Value>> {
            Box::pin(async move { Answer::ok(json!(request.cursor)) })
        }
        fn render_text(&self, value: &Value) -> String {
            format!("{value}\n")
        }
    }
    let mut table = OperationTable::new("example", Deadlines::default());
    table
        .declare_tool(ToolFamily {
            name: "read".into(),
            description: "read".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    table.register(Positional).unwrap();
    let parsed = table
        .parse_cli(&["page".into(), "c-7".into()], "r-1")
        .unwrap();
    assert_eq!(parsed.request.arguments, json!({"cursor": "c-7"}));
}
