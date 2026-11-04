//! The server half of one connection per browser: a union stream of
//! subscriptions, each resumed from its own position.
use command_surface::{CommittedEvent, EventLog, Position, SseDecoder};
use web_shell::FragmentUpdate;
use web_shell::feed::{self, FeedEvent, FeedReload, Subscription};

fn position(cursor: &str) -> Position {
    Position::new("g1", cursor.parse().unwrap())
}
/// Each committed value names the reference it changes.
fn log() -> EventLog<String> {
    let mut log = EventLog::new("g1");
    for (cursor, reference) in [
        ("1", "session:a"),
        ("2", "session:b"),
        ("3", "session:a"),
        ("4", "waiting"),
        ("5", "session:b"),
    ] {
        log.append(CommittedEvent {
            position: position(cursor),
            event: "fragment".into(),
            value: reference.into(),
        })
        .unwrap();
    }
    log
}
fn updates(subscription: &Subscription, changed: &String) -> Vec<FragmentUpdate> {
    if subscription.reference == *changed {
        vec![FragmentUpdate::Ref {
            target: changed.replace(':', "-"),
            url: format!("/fragment?of={changed}"),
        }]
    } else {
        vec![]
    }
}
fn subscription(id: &str, reference: &str, since: &str) -> Subscription {
    Subscription {
        id: id.into(),
        reference: reference.into(),
        since: position(since).event_id(),
    }
}
fn decode(stream: &str) -> Vec<command_surface::SseEvent> {
    SseDecoder::new().push(stream.as_bytes()).unwrap()
}

#[test]
fn union_stream_resumes_each_subscription_from_its_own_position() {
    let subscriptions = [
        subscription("0.s0", "session:a", "1"),
        subscription("1.s0", "session:b", "3"),
        subscription("2.s0", "waiting", "1"),
    ];
    let events = decode(&feed::catch_up(&log(), &subscriptions, updates).unwrap());
    let delivered: Vec<(String, String)> = events
        .iter()
        .map(|event| {
            assert_eq!(event.event, feed::FRAGMENT_EVENT);
            let value: FeedEvent = serde_json::from_str(&event.data).unwrap();
            (value.subscription, event.id.clone().unwrap())
        })
        .collect();
    // Commit order; session:b's event at 2 precedes its position and is not resent.
    assert_eq!(
        delivered,
        vec![
            ("0.s0".to_owned(), position("3").event_id()),
            ("2.s0".to_owned(), position("4").event_id()),
            ("1.s0".to_owned(), position("5").event_id()),
        ]
    );
}

#[test]
fn a_lost_position_reloads_only_its_subscription() {
    let mut log = log();
    log.retain_after(&position("2")).unwrap();
    let subscriptions = [
        subscription("0.s0", "session:a", "1"),
        subscription("0.s1", "session:a", "3"),
    ];
    let events = decode(&feed::catch_up(&log, &subscriptions, updates).unwrap());
    assert_eq!(events[0].event, command_surface::RELOAD_EVENT);
    let reload: FeedReload = serde_json::from_str(&events[0].data).unwrap();
    assert_eq!(reload.subscription.as_deref(), Some("0.s0"));
    assert_eq!(reload.reason, "position no longer retained");
    assert_eq!(
        events.len(),
        1,
        "0.s1 is already past every retained event for it"
    );
}

#[test]
fn live_events_go_to_every_subscription_they_concern() {
    let subscriptions = [
        subscription("0.s0", "session:a", "5"),
        subscription("1.s0", "session:a", "5"),
        subscription("1.s1", "waiting", "5"),
    ];
    let event = CommittedEvent {
        position: position("6"),
        event: "fragment".into(),
        value: "session:a".to_owned(),
    };
    let events = decode(&feed::live(&event, &subscriptions, updates).unwrap());
    let targets: Vec<String> = events
        .iter()
        .map(|event| {
            serde_json::from_str::<FeedEvent>(&event.data)
                .unwrap()
                .subscription
        })
        .collect();
    assert_eq!(targets, ["0.s0", "1.s0"]);
}

#[test]
fn subscriptions_parameter_is_checked() {
    let list = vec![subscription("0.s0", "session:a", "1")];
    let text = serde_json::to_string(&list).unwrap();
    assert_eq!(feed::subscriptions(&text).unwrap(), list);
    let twice = serde_json::to_string(&[list[0].clone(), list[0].clone()]).unwrap();
    assert!(feed::subscriptions(&twice).is_err());
    assert!(feed::subscriptions(r#"[{"id":"","reference":"x","since":"1"}]"#).is_err());
    assert!(feed::subscriptions("not json").is_err());
}

#[test]
fn a_refused_subscription_is_a_named_reload() {
    let refused = FeedReload::refuse(
        &subscription("3.s2", "page:missing", "1"),
        "cannot follow page:missing",
    )
    .encode()
    .unwrap();
    let events = decode(&refused);
    assert_eq!(events[0].event, "reload");
    assert_eq!(
        events[0].data,
        r#"{"subscription":"3.s2","reason":"cannot follow page:missing"}"#
    );
}

#[test]
fn feed_element_carries_its_subscription() {
    let markup = web_shell::Feed {
        route: "/events",
        reference: "session:a&b",
        since: "[\"g1\",\"4\"]",
        refresh: Some("/refresh?of=session:a"),
    }
    .render()
    .into_string();
    assert_eq!(
        markup,
        "<shell-feed data-route=\"/events\" data-reference=\"session:a&amp;b\" data-since=\"[&quot;g1&quot;,&quot;4&quot;]\" data-refresh=\"/refresh?of=session:a\"></shell-feed>"
    );
}
