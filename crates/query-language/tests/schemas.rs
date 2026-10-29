use query_language::{ast::Query, catalog::SourceOutput};

#[test]
fn canonical_query_products_have_structural_schemas() {
    let query = serde_json::to_value(schemars::schema_for!(Query)).unwrap();
    assert_eq!(query["type"], "object");
    assert!(query["properties"]["source"]["$ref"].is_string());
    for ty in [
        "ConditionKind",
        "Value",
        "StageKind",
        "Operand",
        "ExtensionArgument",
    ] {
        assert!(query["$defs"][ty].is_object(), "missing typed {ty}");
    }
    let output = serde_json::to_value(schemars::schema_for!(SourceOutput)).unwrap();
    assert_eq!(output["type"], "object");
    assert!(output["properties"]["source"]["$ref"].is_string());
    for ty in [
        "OutputSource",
        "OutputSchema",
        "Field",
        "FieldKind",
        "UnknownRule",
    ] {
        assert!(output["$defs"][ty].is_object(), "missing typed {ty}");
    }
}
