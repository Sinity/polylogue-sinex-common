use command_surface::*;
use proptest::prelude::*;
use serde_json::{Value, json};

proptest! {
    #[test]
    fn failure_suggestion_preserves_its_wire_value(text in ".*"){
        let request=RunnableRequest::new("read",json!({"text":text}),"r-1");
        for suggestion in [Suggestion::Request(request.clone()),Suggestion::Query(text.clone())]{
            let failure=Failure::new(FailureKind::BadValue,"invalid synthetic value").with_suggestion(suggestion.clone());
            let value=serde_json::to_value(&failure).unwrap();
            prop_assert_eq!(serde_json::from_value::<Suggestion>(value["suggestion"].clone()).unwrap(),suggestion);
            prop_assert_eq!(serde_json::from_value::<Failure>(value).unwrap(),failure);
        }
    }
    #[test]
    fn failure_candidates_survive_the_wire_in_order(candidates in proptest::collection::vec(".*", 0..4)){
        let failure=Failure::new(FailureKind::Ambiguous,"synthetic name is ambiguous").with_candidates(candidates.clone());
        let value=serde_json::to_value(Answer::<Value>::error(failure.clone())).unwrap();
        prop_assert_eq!(value["error"].get("candidates").is_some(),!candidates.is_empty());
        let answer=serde_json::from_value::<Answer<Value>>(value).unwrap();
        let error=answer.failure().expect("error answer");
        prop_assert_eq!(error.candidates(),candidates.as_slice());
        prop_assert_eq!(error,&failure);
    }
    #[test]
    fn envelope_serialization_preserves_unicode_and_identity(text in ".*",id in ".+"){
        let request=RunnableRequest{operation:"read".into(),arguments:json!({"text":text}),request_id:id,deadline:Deadline::None,budget_tokens:Some(4000),cursor:Some("c-1".into()),fresh:true,format:Format::Json};
        let bytes=serde_json::to_vec(&request).unwrap();prop_assert_eq!(serde_json::from_slice::<RunnableRequest>(&bytes).unwrap(),request);
        let answer=Answer::ok(json!({"text":text}));let bytes=serde_json::to_vec(&answer).unwrap();prop_assert_eq!(serde_json::from_slice::<Answer<Value>>(&bytes).unwrap(),answer);
    }
    #[test]
    fn sse_roundtrip_at_every_byte_boundary(text in ".*",chunk in 1usize..32){
        let value=json!({"text":text});let event=SseEvent::json("generation:position","replace",&value).unwrap();let encoded=event.encode().unwrap();
        let mut decoder=SseDecoder::new();let mut decoded=vec![];for bytes in encoded.as_bytes().chunks(chunk){decoded.extend(decoder.push(bytes).unwrap());}
        prop_assert_eq!(decoded.len(),1);prop_assert_eq!(serde_json::from_str::<Value>(&decoded[0].data).unwrap(),value);prop_assert_eq!(decoded[0].id.as_deref(),Some("generation:position"));
    }
}
#[test]
fn null_deadline_lifts_while_missing_uses_class() {
    let base = json!({"operation":"read","arguments":{},"request_id":"r"});
    let missing: RunnableRequest = serde_json::from_value(base.clone()).unwrap();
    assert_eq!(missing.deadline, Deadline::Default);
    let mut null = base;
    null["deadline"] = Value::Null;
    assert_eq!(
        serde_json::from_value::<RunnableRequest>(null)
            .unwrap()
            .deadline,
        Deadline::None
    );
    assert!(
        serde_json::from_value::<RunnableRequest>(
            json!({"operation":"read","arguments":{},"request_id":"r","deadline":-1})
        )
        .is_err()
    );
    assert!(
        serde_json::to_value(missing)
            .unwrap()
            .get("deadline")
            .is_none()
    );
}
#[test]
fn error_cannot_deserialize_beside_partial_value() {
    let failure = Answer::<Value>::error(Failure::new(FailureKind::Timeout, "scan stopped"));
    let mut value = serde_json::to_value(failure).unwrap();
    value["value"] = json!([1]);
    assert!(serde_json::from_value::<Answer<Value>>(value).is_err());
}
#[test]
fn resume_from_last_event_id_or_reload() {
    let mut log = EventLog::new("g:1");
    let positions: Vec<_> = (0..4).map(|n| Position::new("g:1", n)).collect();
    for position in &positions {
        log.append(CommittedEvent {
            position: position.clone(),
            event: "append".into(),
            value: position.counter.to_string(),
        })
        .unwrap();
    }
    let Resume::Events(events) = log.resume(Some(&positions[1].event_id())) else {
        panic!()
    };
    assert_eq!(
        events.iter().map(|e| &e.value).collect::<Vec<_>>(),
        vec!["2", "3"]
    );
    log.retain_after(&positions[2]).unwrap();
    assert!(matches!(
        log.resume(Some(&positions[1].event_id())),
        Resume::Reload { .. }
    ));
    let Resume::Events(events) = log.resume(Some(&positions[2].event_id())) else {
        panic!()
    };
    assert_eq!(events.len(), 1);
    let old = Position::new("old", 3);
    assert_eq!(positions[2].event_id(), "g:1:2");
    assert_eq!(Position::parse("g:1:2").unwrap(), positions[2]);
    assert!(Position::parse("g1").is_err());
    assert_eq!(positions[2].is_after(&positions[1]), Some(true));
    assert_eq!(old.is_after(&positions[1]), None);
    assert!(
        log.resume(Some(&old.event_id()))
            .encode()
            .unwrap()
            .starts_with("event: reload\n")
    );
    assert!(SseEvent::json("id\ninjection", "message", &0).is_err());
}
#[test]
fn sse_comments_multiline_crlf_and_persistent_ids() {
    let mut decoder = SseDecoder::new();
    let events = decoder
        .push(b":keep alive\r\nid: 3\r\ndata: a\r\ndata: b\r\n\r\ndata: c\n\n")
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].data, "a\nb");
    assert_eq!(events[1].id.as_deref(), Some("3"));
}
#[test]
fn unmeasured_never_zero_and_measurement_reuse_needs_same_key() {
    let ratio = Measurement::ratio("coverage", 0, 0);
    assert_eq!(ratio.value(), None);
    assert_eq!(ratio.state(), MeasurementState::NotMeasured);
    assert!(
        Measurement::new(
            "unknown",
            Some(0),
            MeasurementState::NotMeasured,
            Some("missing".into()),
            0
        )
        .is_err()
    );
    let malformed =
        json!({"name":"unknown","value":0,"state":"unavailable","reason":"offline","age":0});
    assert!(serde_json::from_value::<Measurement<u64>>(malformed).is_err());
    let mut cache = MeasurementCache::default();
    cache.insert("position-1", Measurement::measured("rows", 0));
    assert_eq!(cache.get(&"position-1").unwrap().value(), Some(&0));
    assert!(cache.get(&"position-2").is_none());
    assert!(matches!(
        Status::new(vec![ratio]).verdict,
        Verdict::Unknown { .. }
    ));
}
#[tokio::test(start_paused = true)]
async fn measurement_deadlines_are_independent() {
    let missing = measure(
        "slow",
        std::time::Duration::from_millis(10),
        Box::pin(std::future::pending::<Measurement<u64>>()),
    );
    let measured = measure(
        "fast",
        std::time::Duration::from_millis(10),
        Box::pin(async { Measurement::measured("fast", 7) }),
    );
    let (slow, fast) = tokio::join!(missing, measured);
    assert_eq!(slow.reason(), Some("timed out"));
    assert_eq!(fast.value(), Some(&7));
    assert!(matches!(
        Status::new(vec![slow, fast]).verdict,
        Verdict::Degraded { .. }
    ));
}

#[test]
fn cli_quote_is_interpreted_as_one_exact_argument() {
    for text in [
        "",
        "normal",
        "a b",
        "'double\"quote'",
        "$(printf injected); line\nend",
        "λ",
    ] {
        let script = format!("printf '%s' {}", quote_argument(text));
        let output = std::process::Command::new("sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, text.as_bytes());
    }
}
