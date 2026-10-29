use query_language::{
    cursor::{PageRequest, SealedCursor},
    time::{Clock, ResolvedTime},
};

#[test]
fn operation_request_schema_exposes_opaque_cursor_and_positive_page_size() {
    let cursor = serde_json::to_value(schemars::schema_for!(SealedCursor)).unwrap();
    assert_eq!(cursor["type"], "string");
    let request = serde_json::to_value(schemars::schema_for!(PageRequest)).unwrap();
    assert_eq!(request["type"], "object");
    assert_eq!(request["properties"]["page_size"]["minimum"], 1);
    assert!(request["properties"].get("cursor").is_some());
    assert!(
        serde_json::from_value::<PageRequest>(serde_json::json!({"page_size":0,"cursor":null}))
            .is_err()
    );
}

#[test]
fn resolved_time_response_schema_uses_strings_for_instants_and_named_zones() {
    let clock = serde_json::to_value(schemars::schema_for!(Clock)).unwrap();
    assert_eq!(clock["properties"]["now"]["type"], "string");
    assert_eq!(
        clock["$defs"]["DaySetting"]["properties"]["zone"]["type"],
        "string"
    );
    let resolved = serde_json::to_value(schemars::schema_for!(ResolvedTime)).unwrap();
    assert_eq!(resolved["properties"]["expression"]["type"], "string");
    assert!(resolved["properties"].get("value").is_some());
}
