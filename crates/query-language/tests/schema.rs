use query_language::{
    cursor::{PageRequest, SealedCursor},
    time::{ClockWire, ResolvedTime},
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
    let clock = serde_json::to_value(schemars::schema_for!(ClockWire)).unwrap();
    assert_eq!(clock["properties"]["now"]["type"], "string");
    assert_eq!(
        clock["$defs"]["DaySettingWire"]["properties"]["zone"]["type"],
        "string"
    );
    let resolved = serde_json::to_value(schemars::schema_for!(ResolvedTime)).unwrap();
    assert_eq!(resolved["properties"]["expression"]["type"], "string");
    assert!(resolved["properties"].get("value").is_some());
}

#[test]
fn direct_operation_wire_types_decode_without_runtime_zone_resolution() {
    fn wire<T: serde::Serialize + serde::de::DeserializeOwned + schemars::JsonSchema>() {}
    wire::<PageRequest>();
    wire::<ClockWire>();
    wire::<ResolvedTime>();
    let clock:ClockWire=serde_json::from_value(serde_json::json!({"now":"2026-01-01T00:00Z","day":{"zone":"Unresolved/Example","start":"04:00:00"}})).unwrap();
    assert_eq!(clock.day.zone, "Unresolved/Example");
}
