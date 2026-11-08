use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    text: String,
}

fn catalog() -> Arc<OperationCatalog> {
    let mut deadlines = Deadlines::default();
    deadlines.set(LatencyClass::Read, Some(Duration::from_secs(2)));
    let mut catalog = OperationCatalog::builder("example", deadlines);
    catalog
        .declare_tool(ToolFamily {
            name: "query".into(),
            description: "Read synthetic text".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    catalog
        .declare::<Input, String>(
            OperationSpec::request(
                "read",
                &["read"],
                "query",
                "Read text",
                "Read the requested synthetic value",
            ),
            |answer| format!("{answer}\n"),
        )
        .unwrap();
    catalog.finish()
}

#[test]
fn catalog_parses_describes_and_renders_without_runtime_handlers() {
    let catalog = catalog();
    let parsed = catalog
        .parse_cli(&["read".into(), "--text".into(), "hello".into()], "request")
        .unwrap();
    assert_eq!(parsed.request.arguments, json!({"text": "hello"}));
    assert_eq!(catalog.complete("re"), vec!["read"]);
    assert!(catalog.help_for("read").unwrap().contains("Read text"));
    assert_eq!(catalog.mcp_tools()[0]["name"], "example_query");
    assert_eq!(
        catalog.deadline(&parsed.request),
        Some(Duration::from_secs(2))
    );
    assert_eq!(
        catalog
            .render_text("read", &Answer::ok(json!("hello")))
            .unwrap(),
        "hello\n"
    );
    laws::tools_generated(&catalog).unwrap();
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
fn binding_uses_existing_catalog_and_rejects_schema_drift_and_duplicates() {
    let catalog = catalog();
    let mut table = OperationTable::new(catalog.clone());
    assert!(Arc::ptr_eq(table.catalog(), &catalog));
    assert!(table.bind("read", WrongInput).is_err());
    table.bind("read", Read).unwrap();
    assert!(table.bind("read", Read).is_err());
    assert!(std::ptr::eq(table.descriptions(), catalog.descriptions()));
}
