use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum View {
    Dialogue,
    Transcript,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Read {
    view: Option<View>,
    #[serde(default)]
    show: Vec<String>,
    #[serde(default)]
    include_tools: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Note {
    text: String,
    r#as: Option<String>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Resume {
    r#in: Option<String>,
    #[serde(default)]
    stop_source: bool,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Number {
    value: u64,
}

fn verbs() -> StepDeclarations {
    let mut steps = StepDeclarations::default();
    steps
        .declare::<Read>(
            "read",
            vec![
                ArgumentSpec::new("view", ArgumentForm::Named, "Reading view"),
                ArgumentSpec::new("show", ArgumentForm::Repeated, "Message parts").delimiter(','),
                ArgumentSpec::new("include_tools", ArgumentForm::Flag, "Show tools").short('t'),
            ],
        )
        .unwrap();
    steps
        .declare::<Note>(
            "note",
            vec![
                ArgumentSpec::new("text", ArgumentForm::Positional, "Note text"),
                ArgumentSpec::new("as", ArgumentForm::Named, "Note kind"),
            ],
        )
        .unwrap();
    steps
        .declare::<Resume>(
            "resume",
            vec![
                ArgumentSpec::new("in", ArgumentForm::Named, "Harness"),
                ArgumentSpec::new("stop_source", ArgumentForm::Flag, "Stop source"),
            ],
        )
        .unwrap();
    steps
        .declare::<Number>(
            "number",
            vec![ArgumentSpec::new(
                "value",
                ArgumentForm::Positional,
                "A number",
            )],
        )
        .unwrap();
    steps
}
fn words(words: &[&str]) -> Vec<String> {
    words.iter().map(|word| (*word).to_owned()).collect()
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Rows {
    selection: Option<String>,
    #[serde(default)]
    steps: Vec<Value>,
}
fn builder() -> DeclarationsBuilder {
    let mut builder = OperationDeclarations::builder("example", Deadlines::default());
    builder
        .declare_tool(ToolFamily {
            name: "read".into(),
            description: "Read synthetic rows".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    builder
        .declare::<Rows, Value>(
            OperationSpec::request("rows", &["rows"], "read", "List rows", "List selected rows"),
            |value, _| value.to_string(),
        )
        .unwrap();
    builder.declare_steps("rows", "steps", verbs()).unwrap();
    builder
}

#[test]
fn step_parser_enforces_arity() {
    let steps = verbs();
    assert_eq!(
        steps.parse_step("note", &words(&["hello world"])).unwrap(),
        json!({"verb":"note","text":"hello world"})
    );
    assert_eq!(
        steps.parse_step("note", &[]).unwrap_err().message,
        "missing TEXT"
    );
    assert_eq!(
        steps
            .parse_step("note", &words(&["one", "two"]))
            .unwrap_err()
            .message,
        "unexpected word two"
    );
    assert_eq!(
        steps
            .parse_step("read", &words(&["dialogue"]))
            .unwrap_err()
            .message,
        "unexpected word dialogue"
    );
    assert_eq!(
        steps.parse_step("note", &words(&[""])).unwrap(),
        json!({"verb":"note","text":""})
    );
}

#[test]
fn step_options_belong_to_their_step() {
    let steps = verbs();
    assert_eq!(
        steps
            .parse_step("read", &words(&["--view", "dialogue"]))
            .unwrap(),
        json!({"verb":"read","view":"dialogue"})
    );
    assert_eq!(
        steps
            .parse_step("note", &words(&["hello", "--as", "decision"]))
            .unwrap(),
        json!({"verb":"note","text":"hello","as":"decision"})
    );
    assert_eq!(
        steps
            .parse_step("read", &words(&["--as", "decision"]))
            .unwrap_err()
            .message,
        "unknown argument --as"
    );
    assert_eq!(
        steps
            .parse_step("note", &words(&["hello", "--view", "dialogue"]))
            .unwrap_err()
            .message,
        "unknown argument --view"
    );
    assert_eq!(
        steps
            .parse_step("note", &words(&["--text", "hello"]))
            .unwrap_err()
            .message,
        "unknown argument --text"
    );
    assert_eq!(
        steps
            .parse_step("read", &words(&["--json"]))
            .unwrap_err()
            .message,
        "unknown argument --json"
    );
}

#[test]
fn step_signature_renders_for_help() {
    let table = builder().finish();
    let steps = table.steps("rows", "steps").unwrap();
    assert_eq!(
        steps
            .verbs()
            .iter()
            .map(StepDeclaration::signature)
            .collect::<Vec<_>>(),
        [
            "read(view?, show?, include_tools?)",
            "note(text, as?)",
            "resume(in?, stop_source?)",
            "number(value)",
        ]
    );
    let help = table.help_for("rows").unwrap();
    assert_eq!(
        help.lines()
            .filter(|line| line.starts_with("    "))
            .collect::<Vec<_>>(),
        [
            "    read(view?, show?, include_tools?)",
            "    note(text, as?)",
            "    resume(in?, stop_source?)",
            "    number(value)",
        ]
    );
}

#[test]
fn step_names_use_cli_hyphens_and_json_underscores() {
    let steps = verbs();
    assert_eq!(
        steps
            .parse_step("resume", &words(&["--in=codex", "--stop-source"]))
            .unwrap(),
        json!({"verb":"resume","in":"codex","stop_source":true})
    );
    assert_eq!(
        steps.validate_step(&json!({"verb":"resume","in":"codex","stop_source":true})),
        Ok(())
    );
    assert_eq!(
        steps
            .validate_step(&json!({"verb":"resume","stop-source":true}))
            .unwrap_err()
            .message,
        "resume: unknown argument stop-source"
    );
    assert_eq!(
        steps
            .parse_step(
                "read",
                &words(&["-t", "--show", "text,thinking", "--show=tools"])
            )
            .unwrap(),
        json!({"verb":"read","include_tools":true,"show":["text","thinking","tools"]})
    );
}

#[test]
fn step_values_follow_the_typed_declaration() {
    let steps = verbs();
    assert_eq!(
        steps.parse_step("number", &words(&["7"])).unwrap(),
        json!({"verb":"number","value":7})
    );
    assert_eq!(
        steps
            .parse_step("number", &words(&["\"seven\""]))
            .unwrap_err()
            .kind,
        FailureKind::BadRequest
    );
    assert_eq!(
        steps
            .parse_step("read", &words(&["--view", "typo"]))
            .unwrap_err()
            .kind,
        FailureKind::BadRequest
    );
    assert_eq!(
        steps
            .validate_step(&json!({"verb":"note"}))
            .unwrap_err()
            .kind,
        FailureKind::BadRequest
    );
    assert_eq!(
        steps
            .validate_step(&json!({"verb":"read","show":[1]}))
            .unwrap_err()
            .kind,
        FailureKind::BadRequest
    );
    assert_eq!(
        steps
            .parse_step("resume", &words(&["--stop-source=false"]))
            .unwrap(),
        json!({"verb":"resume","stop_source":false})
    );
    assert_eq!(
        steps
            .parse_step(
                "read",
                &words(&["--view", "dialogue", "--view", "transcript"])
            )
            .unwrap_err()
            .message,
        "duplicate --view"
    );
}

#[test]
fn double_dash_preserves_positional_words() {
    let steps = verbs();
    assert_eq!(
        steps
            .parse_step("note", &words(&["--as", "decision", "--", "--view"]))
            .unwrap(),
        json!({"verb":"note","text":"--view","as":"decision"})
    );
    let text = "a".repeat(100_000);
    assert_eq!(
        steps.parse_step("note", &[text.clone()]).unwrap(),
        json!({"verb":"note","text":text})
    );
}

#[test]
fn steps_schema_lists_verbs_with_loose_fields() {
    let table = builder().finish();
    let schema =
        &table.mcp_tools()[0]["inputSchema"]["properties"]["arguments"]["properties"]["steps"];
    assert_eq!(
        schema,
        &json!({"type":"array","items":{"type":"object","properties":{"verb":{"enum":["read","note","resume","number"]}},"required":["verb"]}})
    );
    assert_eq!(
        table.steps("rows", "steps").unwrap().verbs()[1].input_schema()["required"],
        json!(["text"])
    );
    checks::tools_generated(&table).unwrap();
}

#[test]
fn json_steps_validate_each_verb() {
    let table = builder().finish();
    for step in [
        json!({"verb":"note"}),
        json!({"verb":"read","as":"note"}),
        json!({"verb":"note","text":1}),
        json!({"verb":"read","view":"typo"}),
        json!({"verb":"unknown"}),
        json!({}),
        json!(null),
    ] {
        let call = json!({"operation":"rows","arguments":{"steps":[step]}});
        let failure = table
            .parse_mcp_call("example_read", call.clone())
            .unwrap_err();
        assert_eq!(failure.kind, FailureKind::BadRequest);
        assert!(failure.message.starts_with("steps[0]: "));
        assert_eq!(
            table.parse_http_call(call, &Author::Operator).unwrap_err(),
            failure
        );
    }
    let call = json!({"arguments":{"steps":[{"verb":"read","view":"dialogue"},{"verb":"note","text":"hello","as":"decision"}]}});
    assert_eq!(
        table
            .parse_mcp_call("example_read", call.clone())
            .unwrap()
            .arguments,
        call["arguments"]
    );
    assert_eq!(
        table
            .parse_mcp_call("example_read", json!({"arguments":{"steps":"read"}}))
            .unwrap_err()
            .message,
        "steps must be an array"
    );
    assert_eq!(
        table
            .parse_mcp_call("example_read", json!({"arguments":{"steps":[]}}))
            .unwrap()
            .arguments,
        json!({"steps":[]})
    );
}

#[test]
fn cli_and_http_query_validate_steps() {
    let table = builder().finish();
    for steps in [
        json!([{"verb":"note","text":"hello"}]),
        json!([{"verb":"read","text":"hello"}]),
    ] {
        let cli = table.parse_cli(&words(&["rows", "--steps", &steps.to_string()]), "request");
        let http =
            table.parse_http_query("rows", &[("steps".into(), steps.to_string())], "request");
        if steps[0]["verb"] == "note" {
            assert_eq!(cli.unwrap().request.arguments, json!({"steps":steps}));
            assert_eq!(http.unwrap().arguments, json!({"steps":steps}));
        } else {
            assert_eq!(
                cli.unwrap_err().message,
                "steps[0]: read: unknown argument text"
            );
            assert_eq!(
                http.unwrap_err().message,
                "steps[0]: read: unknown argument text"
            );
        }
    }
}

struct Count(Arc<AtomicUsize>);
impl Operation for Count {
    type Request = Rows;
    type Response = Value;
    fn execute(&self, _: Rows, _: RequestContext) -> OperationFuture<'_, Answer<Value>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Answer::ok(json!({"count":1})) })
    }
}

#[tokio::test]
async fn transports_refuse_invalid_steps_before_execution() {
    let count = Arc::new(AtomicUsize::new(0));
    let mut table = OperationTable::new(builder().finish());
    table.bind("rows", Count(count.clone())).unwrap();
    let invalid = json!({"operation":"rows","arguments":{"steps":[{"verb":"read","as":"decision"}]},"request_id":"r"});
    let request = http::Request::post("/v1/rows")
        .header("content-type", "application/json")
        .body(serde_json::to_vec(&invalid).unwrap())
        .unwrap();
    let response = table.http(request, CancellationToken::new()).await;
    assert_eq!(response.status(), 400);
    let answer: Answer<Value> = serde_json::from_slice(response.body()).unwrap();
    let Outcome::Error(failure) = answer.outcome else {
        panic!("expected a failure")
    };
    assert_eq!(failure.message, "steps[0]: read: unknown argument as");
    let mcp = table.mcp(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"example_read","arguments":invalid.clone()}}), CancellationToken::new()).await;
    assert_eq!(mcp["result"]["isError"], true);
    let request: RunnableRequest = serde_json::from_value(invalid).unwrap();
    let answer = table
        .call_as(
            request,
            Via::Cli,
            Caller::default(),
            CancellationToken::new(),
        )
        .await;
    assert!(answer.is_error());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[test]
fn step_declarations_refuse_ambiguous_words() {
    let mut steps = StepDeclarations::default();
    for form in [ArgumentForm::Words, ArgumentForm::Rest] {
        assert_eq!(
            steps
                .declare::<Note>("note", vec![ArgumentSpec::new("text", form, "text")])
                .unwrap_err()
                .message,
            "step positionals take one word; options belong to the step"
        );
    }
    #[derive(Serialize, Deserialize, JsonSchema)]
    struct Bad {
        verb: String,
    }
    assert_eq!(
        steps.declare::<Bad>("bad", vec![]).unwrap_err().message,
        "step fields use underscore names and reserve verb"
    );
    steps
        .declare::<Note>(
            "note",
            vec![ArgumentSpec::new("text", ArgumentForm::Positional, "text")],
        )
        .unwrap();
    assert_eq!(
        steps.declare::<Note>("note", vec![]).unwrap_err().message,
        "step verb \"note\" is not a free name"
    );
    assert_eq!(
        steps
            .declare::<Note>("bad_name", vec![])
            .unwrap_err()
            .message,
        "step verb \"bad_name\" is not a free name"
    );
}

#[test]
fn step_catalog_belongs_to_an_array_argument() {
    let mut builder = builder();
    assert_eq!(
        builder
            .declare_steps("rows", "selection", verbs())
            .unwrap_err()
            .message,
        "steps need an array argument and at least one verb"
    );
    assert_eq!(
        builder
            .declare_steps("rows", "missing", verbs())
            .unwrap_err()
            .message,
        "rows has no argument missing"
    );
    assert_eq!(
        builder
            .declare_steps("rows", "steps", verbs())
            .unwrap_err()
            .message,
        "rows.steps has step declarations already"
    );
}
