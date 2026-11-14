use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    text: String,
}

fn declarations() -> Arc<OperationDeclarations> {
    let mut deadlines = Deadlines::default();
    deadlines.set(LatencyClass::Read, Some(Duration::from_secs(2)));
    let mut declarations = OperationDeclarations::builder("example", deadlines);
    declarations
        .declare_tool(ToolFamily {
            name: "query".into(),
            description: "Read synthetic text".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    declarations
        .declare::<Input, String>(
            OperationSpec::request(
                "read",
                &["read"],
                "query",
                "Read text",
                "Read the requested synthetic value",
            ),
            |answer, _style| format!("{answer}\n"),
        )
        .unwrap();
    declarations.finish()
}

#[test]
fn declarations_parse_describe_and_render_without_runtime_handlers() {
    let declarations = declarations();
    let parsed = declarations
        .parse_cli(&["read".into(), "--text".into(), "hello".into()], "request")
        .unwrap();
    assert_eq!(parsed.request.arguments, json!({"text": "hello"}));
    assert_eq!(declarations.complete("re"), vec!["read"]);
    assert!(declarations.help_for("read").unwrap().contains("Read text"));
    assert_eq!(declarations.mcp_tools()[0]["name"], "example_query");
    assert_eq!(
        declarations.deadline(&parsed.request),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        declarations
            .render_text("read", &Answer::ok(json!("hello")), TextStyle::Plain)
            .unwrap(),
        "hello\n"
    );
    checks::tools_generated(&declarations).unwrap();
}

struct Read;
impl Operation for Read {
    type Request = Input;
    type Response = String;
    fn execute(&self, request: Input, _: RequestContext) -> OperationFuture<'_, Answer<String>> {
        Box::pin(async move { Answer::ok(request.text) })
    }
}
struct WrongInput;
impl Operation for WrongInput {
    type Request = Value;
    type Response = String;
    fn execute(&self, _: Value, _: RequestContext) -> OperationFuture<'_, Answer<String>> {
        Box::pin(async { Answer::ok(String::new()) })
    }
}

#[test]
fn binding_uses_existing_declarations_and_rejects_schema_drift_and_duplicates() {
    let declarations = declarations();
    let mut table = OperationTable::new(declarations.clone());
    assert!(Arc::ptr_eq(table.declarations(), &declarations));
    assert!(table.bind("read", WrongInput).is_err());
    table.bind("read", Read).unwrap();
    assert!(table.bind("read", Read).is_err());
    assert!(std::ptr::eq(
        table.descriptions(),
        declarations.descriptions()
    ));
}

#[test]
fn rejected_rows_leave_existing_schemas_cli_tools_and_renderers_unchanged() {
    let mut builder = OperationDeclarations::builder("example", Deadlines::default());
    builder
        .declare_tool(ToolFamily {
            name: "query".into(),
            description: "Read synthetic values".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    let spec = || {
        OperationSpec::request(
            "first",
            &["first"],
            "query",
            "Read first",
            "Read a synthetic value",
        )
    };
    builder
        .declare::<Input, String>(spec(), |value, _| format!("first:{value}"))
        .unwrap();
    let before = serde_json::to_value(builder.descriptions()).unwrap();
    let tools = builder.mcp_tools();
    let mut invalid = spec();
    invalid.name = "second".into();
    invalid.command = vec!["second".into()];
    invalid.arguments = vec![ArgumentSpec::new(
        "text",
        ArgumentForm::Flag,
        "Invalid string flag",
    )];
    let error = builder
        .declare::<Input, String>(invalid, |value, _| format!("rejected:{value}"))
        .unwrap_err();
    assert_eq!(error.kind, FailureKind::BadRequest);
    assert_eq!(
        serde_json::to_value(builder.descriptions()).unwrap(),
        before
    );
    assert_eq!(builder.mcp_tools(), tools);
    assert!(builder.parse_cli(&["second".into()], "missing").is_err());
    let second = OperationSpec::request(
        "second",
        &["second"],
        "query",
        "Read second",
        "Read another synthetic value",
    );
    builder
        .declare::<Input, String>(second, |value, _| format!("second:{value}"))
        .unwrap();
    let declarations = builder.finish();
    assert_eq!(
        declarations
            .render_text("first", &Answer::ok(json!("one")), TextStyle::Plain)
            .unwrap(),
        "first:one"
    );
    assert_eq!(
        declarations
            .render_text("second", &Answer::ok(json!("two")), TextStyle::Plain)
            .unwrap(),
        "second:two"
    );
    assert_eq!(
        declarations.mcp_tools()[0]["inputSchema"]["properties"]["operation"]["enum"],
        json!(["first", "second"])
    );
}
