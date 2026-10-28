use command_surface::*;
use proptest::prelude::*;
use serde_json::{Value, json};

proptest! {
    #[test]
    fn failure_suggestion_preserves_its_wire_value(text in ".*"){
        let suggestion=json!({"operation":"read","arguments":{"text":text}});
        let failure=Failure::new(FailureKind::BadValue,"invalid synthetic value").with_suggestion(suggestion.clone());
        let value=serde_json::to_value(&failure).unwrap();prop_assert_eq!(&value["suggestion"],&suggestion);
        prop_assert_eq!(serde_json::from_value::<Failure>(value).unwrap(),failure);
    }
    #[test]
    fn envelope_serialization_preserves_unicode_and_identity(text in ".*",id in ".+"){
        let request=RunnableRequest{operation:"read".into(),arguments:json!({"text":text}),request_id:id,deadline:Deadline::None};
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
    let mut log = EventLog::new("g1");
    let positions: Vec<_> = (0..4)
        .map(|n| EventPosition {
            generation: "g1".into(),
            cursor: n.to_string(),
        })
        .collect();
    for position in &positions {
        log.append(CommittedEvent {
            position: position.clone(),
            event: "append".into(),
            value: position.cursor.clone(),
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
    let old = EventPosition {
        generation: "old".into(),
        cursor: "3".into(),
    };
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
fn unmeasured_never_zero_and_figure_reuse_needs_same_key() {
    let ratio = Figure::ratio("coverage", 0, 0);
    assert_eq!(ratio.value(), None);
    assert_eq!(ratio.state(), FigureState::NotMeasured);
    assert!(
        Figure::new(
            "unknown",
            Some(0),
            FigureState::NotMeasured,
            Some("missing".into()),
            0
        )
        .is_err()
    );
    let malformed =
        json!({"name":"unknown","value":0,"state":"unavailable","reason":"offline","age":0});
    assert!(serde_json::from_value::<Figure<u64>>(malformed).is_err());
    let mut cache = FigureCache::default();
    cache.insert("position-1", Figure::measured("rows", 0));
    assert_eq!(cache.get(&"position-1").unwrap().value(), Some(&0));
    assert!(cache.get(&"position-2").is_none());
    assert!(matches!(
        Status::new(vec![ratio]).verdict,
        Verdict::Unknown { .. }
    ));
}
#[tokio::test(start_paused = true)]
async fn figure_deadlines_are_independent() {
    let missing = measure_figure(
        "slow",
        std::time::Duration::from_millis(10),
        Box::pin(std::future::pending::<Figure<u64>>()),
    );
    let measured = measure_figure(
        "fast",
        std::time::Duration::from_millis(10),
        Box::pin(async { Figure::measured("fast", 7) }),
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
