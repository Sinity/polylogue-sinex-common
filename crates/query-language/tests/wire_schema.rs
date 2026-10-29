use query_language::{
    ast::Span,
    complete::{Completion, CompletionKind},
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct CompletionAnswer {
    candidates: Vec<Completion>,
}

#[test]
fn typed_completion_answer_has_the_actual_closed_wire_schema() {
    let schema = schema_for!(CompletionAnswer).to_value();
    assert_eq!(
        schema["properties"]["candidates"]["items"]["$ref"],
        "#/$defs/Completion"
    );
    let completion = &schema["$defs"]["Completion"];
    assert_eq!(completion["type"], "object");
    assert_eq!(completion["properties"]["value"]["type"], "string");
    assert_eq!(completion["properties"]["help"]["type"], "string");
    assert_eq!(completion["properties"]["replace"]["$ref"], "#/$defs/Span");
    assert_eq!(
        completion["properties"]["kind"]["$ref"],
        "#/$defs/CompletionKind"
    );
    assert_eq!(
        schema["$defs"]["CompletionKind"]["enum"],
        json!([
            "unit", "field", "value", "stage", "operator", "keyword", "command"
        ])
    );
    for field in ["start", "end"] {
        assert_eq!(
            schema["$defs"]["Span"]["properties"][field]["type"],
            "integer"
        );
        assert_eq!(schema["$defs"]["Span"]["properties"][field]["minimum"], 0);
    }
    let answer = CompletionAnswer {
        candidates: vec![Completion {
            value: "\"two words\"".into(),
            replace: Span { start: 2, end: 8 },
            kind: CompletionKind::Value,
            help: "declared value".into(),
        }],
    };
    let wire = serde_json::to_value(&answer).unwrap();
    assert_eq!(wire["candidates"][0]["kind"], "value");
    assert_eq!(wire["candidates"][0]["replace"], json!({"start":2,"end":8}));
    assert_eq!(
        serde_json::from_value::<CompletionAnswer>(wire).unwrap(),
        answer
    );
}
