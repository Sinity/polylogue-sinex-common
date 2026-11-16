use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Selection {
    selection: Value,
}

fn declarations(default: bool) -> DeclarationsBuilder {
    let mut builder = OperationDeclarations::builder("example", Deadlines::default());
    builder.accept(RequestOption::Format);
    for tool in ["read", "write"] {
        builder
            .declare_tool(ToolFamily {
                name: tool.into(),
                description: format!("{tool} synthetic rows"),
                open_world: false,
                idempotent: true,
            })
            .unwrap();
    }
    for (name, tool) in [("rows", "read"), ("status", "read"), ("note", "write")] {
        builder
            .declare::<Selection, Value>(
                OperationSpec::request(
                    name,
                    &[name],
                    tool,
                    "Read values",
                    "Exercise request identities",
                ),
                |value, _| value.to_string(),
            )
            .unwrap();
    }
    if default {
        builder.set_tool_default("read", "rows").unwrap();
    }
    builder
}

#[test]
fn call_without_request_id_derives_stable_id() {
    let table = declarations(true).finish();
    let call = json!({"arguments":{"selection":"topic"}});
    let first = table.parse_mcp_call("example_read", call.clone()).unwrap();
    let second = table.parse_mcp_call("example_read", call).unwrap();
    assert_eq!(first.request_id, second.request_id);
    assert_eq!(first.request_id.len(), 64);
    assert!(
        first
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    );
    assert_eq!(
        table.mcp_tools()[0]["inputSchema"]["required"],
        json!(["arguments"])
    );
}

#[test]
fn identical_retry_derives_same_id() {
    let table = declarations(true).finish();
    let first = table
        .parse_mcp_call(
            "example_read",
            serde_json::from_str(r#"{"arguments":{"selection":{"z":[{"b":2,"a":1}],"a":true}}}"#)
                .unwrap(),
        )
        .unwrap();
    let second = table
        .parse_mcp_call(
            "example_read",
            serde_json::from_str(
                r#"{"operation":"rows","arguments":{"selection":{"a":true,"z":[{"a":1,"b":2}]}}}"#,
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(first.request_id, second.request_id);
}

#[test]
fn changed_arguments_derive_new_id() {
    let table = declarations(true).finish();
    let mut ids = std::collections::BTreeSet::new();
    for selection in [
        json!("topic"),
        json!("other"),
        json!([1, 2]),
        json!([2, 1]),
        json!({"key":false}),
        json!({"key":true}),
    ] {
        let request = table
            .parse_mcp_call(
                "example_read",
                json!({"arguments":{"selection": selection}}),
            )
            .unwrap();
        assert!(ids.insert(request.request_id));
    }
}

#[test]
fn explicit_request_id_is_kept() {
    let table = declarations(true).finish();
    let request = table
        .parse_mcp_call(
            "example_read",
            json!({"arguments":{"selection":"topic"},"request_id":"deliberate-repeat"}),
        )
        .unwrap();
    assert_eq!(request.request_id, "deliberate-repeat");
}

#[test]
fn authors_tools_operations_have_distinct_ids() {
    let table = declarations(true).finish();
    let mut ids = std::collections::BTreeSet::new();
    for (author, tool, operation) in [
        (Author::Operator, "example_read", "rows"),
        (
            Author::parse("agent:codex:synthetic").unwrap(),
            "example_read",
            "rows",
        ),
        (Author::Operator, "example_read", "status"),
        (Author::Operator, "example_write", "note"),
    ] {
        let request = table
            .parse_mcp_call_as(
                tool,
                json!({"operation":operation,"arguments":{"selection":"topic"}}),
                &author,
            )
            .unwrap();
        assert!(ids.insert(request.request_id));
    }
    // The same operation in another product's tool also has its own identity.
    let first = table
        .parse_mcp_call("example_read", json!({"arguments":{"selection":"topic"}}))
        .unwrap();
    let mut builder = OperationDeclarations::builder("other", Deadlines::default());
    builder
        .declare_tool(ToolFamily {
            name: "read".into(),
            description: "Read values".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    builder
        .declare::<Selection, Value>(
            OperationSpec::request(
                "rows",
                &["rows"],
                "read",
                "Read values",
                "Read synthetic values",
            ),
            |value, _| value.to_string(),
        )
        .unwrap();
    let other = builder
        .finish()
        .parse_mcp_call("other_read", json!({"arguments":{"selection":"topic"}}))
        .unwrap();
    assert_ne!(first.request_id, other.request_id);
}

#[test]
fn default_operation_applies_when_absent() {
    let table = declarations(true).finish();
    let request = table
        .parse_mcp_call("example_read", json!({"arguments":{"selection":"topic"}}))
        .unwrap();
    assert_eq!(request.operation, "rows");
    let explicit = table
        .parse_mcp_call(
            "example_read",
            json!({"operation":"status","arguments":{"selection":"topic"}}),
        )
        .unwrap();
    assert_eq!(explicit.operation, "status");
    assert_eq!(
        table.mcp_tools()[0]["inputSchema"]["properties"]["operation"]["enum"],
        json!(["rows", "status"])
    );
}

#[test]
fn multiple_operations_need_opt_in_default() {
    let table = declarations(false).finish();
    let failure = table
        .parse_mcp_call("example_read", json!({"arguments":{"selection":"topic"}}))
        .unwrap_err();
    assert_eq!(failure.message, "name the operation: one of rows, status");
    assert_eq!(
        table.mcp_tools()[0]["inputSchema"]["required"],
        json!(["operation", "arguments"])
    );
}

#[test]
fn tool_default_must_belong_to_the_tool() {
    let mut builder = declarations(false);
    assert_eq!(
        builder
            .set_tool_default("read", "note")
            .unwrap_err()
            .message,
        "note is not an operation of tool read"
    );
    assert_eq!(
        builder
            .set_tool_default("missing", "rows")
            .unwrap_err()
            .message,
        "rows is not an operation of tool missing"
    );
    assert_eq!(
        builder
            .set_tool_default("read", "missing")
            .unwrap_err()
            .message,
        "missing is not an operation of tool read"
    );
}

#[test]
fn http_and_mcp_share_request_identity() {
    let table = declarations(true).finish();
    let author = Author::parse("agent:codex:synthetic").unwrap();
    let call = json!({"operation":"rows","arguments":{"selection":"topic"}});
    let http = table.parse_http_call(call.clone(), &author).unwrap();
    let mcp = table
        .parse_mcp_call_as("example_read", call, &author)
        .unwrap();
    assert_eq!(http, mcp);
    let explicit = table
        .parse_http_call(
            json!({"operation":"rows","arguments":{"selection":"topic"},"request_id":"http-id"}),
            &author,
        )
        .unwrap();
    assert_eq!(explicit.request_id, "http-id");
}

#[test]
fn cli_invocations_keep_the_callers_new_id() {
    let table = declarations(true).finish();
    let words = vec!["rows".into(), "--selection".into(), "\"topic\"".into()];
    let first = table.parse_cli(&words, "invocation-1").unwrap();
    let second = table.parse_cli(&words, "invocation-2").unwrap();
    assert_eq!(first.request.request_id, "invocation-1");
    assert_eq!(second.request.request_id, "invocation-2");
    assert_eq!(first.request.arguments, second.request.arguments);
}

struct Remember(Mutex<BTreeMap<String, Value>>);
impl Operation for Remember {
    type Request = Selection;
    type Response = Value;
    fn execute(&self, _: Selection, context: RequestContext) -> OperationFuture<'_, Answer<Value>> {
        Box::pin(async move {
            let mut runs = self.0.lock().unwrap();
            if let Some(first) = runs.get(&context.request_id) {
                return Answer::ok(json!({"repeat":true,"first_run":first}));
            }
            let first = json!({"id":format!("run:{}", runs.len() + 1),"at":"2026-01-01T00:00:00Z","request_id":context.request_id});
            runs.insert(context.request_id.clone(), first.clone());
            Answer::ok(json!({"repeat":false,"first_run":first}))
        })
    }
}
fn caller(author: Author) -> Caller {
    Caller::default().with_authorship(Authorship {
        author,
        via: Via::Mcp,
    })
}
async fn call(table: &OperationTable, arguments: Value, caller: Caller) -> Value {
    let mut arguments = arguments;
    arguments
        .as_object_mut()
        .unwrap()
        .insert("format".into(), "json".into());
    table.mcp_as(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"example_read","arguments":arguments}}), caller, CancellationToken::new()).await
}

#[tokio::test]
async fn retry_reply_names_the_first_run() {
    let mut table = OperationTable::new(declarations(true).finish());
    table.bind("rows", Remember(Mutex::default())).unwrap();
    let request = json!({"arguments":{"selection":"topic"}});
    let first = call(&table, request.clone(), caller(Author::Operator)).await;
    let second = call(&table, request.clone(), caller(Author::Operator)).await;
    let value = |reply: &Value| reply["result"]["structuredContent"]["value"].clone();
    let first = value(&first);
    assert_eq!(first["repeat"], false);
    assert_eq!(
        value(&second),
        json!({"repeat":true,"first_run":first["first_run"]})
    );
    assert_eq!(first["first_run"]["id"], "run:1");
    assert_eq!(first["first_run"]["at"], "2026-01-01T00:00:00Z");
    let other = call(
        &table,
        request,
        caller(Author::parse("agent:codex:synthetic").unwrap()),
    )
    .await;
    assert_eq!(value(&other)["first_run"]["id"], "run:2");
    let deliberate = call(
        &table,
        json!({"arguments":{"selection":"topic"},"request_id":"again"}),
        caller(Author::Operator),
    )
    .await;
    assert_eq!(value(&deliberate)["first_run"]["id"], "run:3");
}

#[test]
fn anonymous_mcp_uses_the_unidentified_agent() {
    let table = declarations(true).finish();
    let call = json!({"arguments":{"selection":"topic"}});
    let anonymous = table.parse_mcp_call("example_read", call.clone()).unwrap();
    assert_eq!(
        anonymous,
        table
            .parse_mcp_call_as("example_read", call.clone(), &Author::UnidentifiedAgent)
            .unwrap()
    );
    assert_ne!(
        anonymous.request_id,
        table
            .parse_mcp_call_as("example_read", call, &Author::Operator)
            .unwrap()
            .request_id
    );
}
