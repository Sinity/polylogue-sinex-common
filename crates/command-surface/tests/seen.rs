use command_surface::*;
use serde_json::json;
use std::time::Duration;

fn session() -> AgentSession {
    AgentSession::parse("claude-code:worker").unwrap()
}
fn at(generation: &str, counter: u64) -> Position {
    Position::new(generation, counter)
}
fn mark() -> SeenMark {
    SeenMark {
        consumer: Consumer::Session(session()),
        target: "session:abc".into(),
        position: at("g1", 10),
        anchor: Some("m-10".into()),
    }
}

#[test]
fn seen_mark_moves_forward() {
    let mut mark = mark();
    assert!(!mark.show(Showing::Displayed, at("g1", 7), Some("m-7".into())));
    assert!(!mark.show(Showing::Displayed, at("g1", 10), Some("m-10b".into())));
    assert_eq!(mark.position, at("g1", 10));
    assert_eq!(mark.anchor.as_deref(), Some("m-10"));
    assert!(mark.show(Showing::Displayed, at("g1", 12), Some("m-12".into())));
    assert_eq!(mark.position, at("g1", 12));
    assert_eq!(mark.anchor.as_deref(), Some("m-12"));
    // A rebuilt index's counters do not compare with the old ones: the later
    // showing replaces the mark even with a smaller counter.
    assert!(mark.show(Showing::Displayed, at("g2", 3), None));
    assert_eq!(mark.position, at("g2", 3));
    let wire = serde_json::to_value(&mark).unwrap();
    assert_eq!(
        wire["consumer"],
        json!({"consumer": "session", "session": "claude-code:worker"})
    );
    assert_eq!(wire["position"], json!({"generation": "g2", "counter": 3}));
    assert_eq!(serde_json::from_value::<SeenMark>(wire).unwrap(), mark);
}

#[test]
fn only_showing_moves_seen_mark() {
    for showing in [
        Showing::Glimpsed,
        Showing::Prerendered,
        Showing::Hidden,
        Showing::Polled,
        Showing::Preview,
        Showing::Read,
    ] {
        let mut unmoved = mark();
        assert!(
            !unmoved.show(showing, at("g1", 20), Some("m-20".into())),
            "{showing:?}"
        );
        assert_eq!(unmoved, mark(), "{showing:?}");
    }
    for showing in [Showing::Displayed, Showing::ReadSinceSeen] {
        let mut moved = mark();
        assert!(
            moved.show(showing, at("g1", 20), Some("m-20".into())),
            "{showing:?}"
        );
        assert_eq!(moved.position, at("g1", 20));
    }
}

#[test]
fn display_time_decides_a_visible_view() {
    assert_eq!(
        Showing::of_visible_view(Duration::from_millis(999)),
        Showing::Glimpsed
    );
    assert_eq!(
        Showing::of_visible_view(Showing::DISPLAY_TIME),
        Showing::Displayed
    );
    let mut glimpsed = mark();
    assert!(!glimpsed.show(
        Showing::of_visible_view(Duration::from_millis(200)),
        at("g1", 30),
        None
    ));
    let mut read = mark();
    assert!(read.show(
        Showing::of_visible_view(Duration::from_secs(3)),
        at("g1", 30),
        None
    ));
    // The module path the web shell uses.
    let _: command_surface::seen::Showing = Showing::Displayed;
}

#[test]
fn consumers_are_the_operator_and_identified_sessions() {
    assert_eq!(Consumer::of(&Author::Operator), Some(Consumer::Operator));
    assert_eq!(
        Consumer::of(&Author::Agent(session())),
        Some(Consumer::Session(session()))
    );
    for author in [
        Author::AgentOfHarness(Harness::new("codex").unwrap()),
        Author::UnidentifiedAgent,
        Author::Script("backup.service".into()),
        Author::Unattributed,
    ] {
        assert_eq!(Consumer::of(&author), None);
    }
}

#[test]
fn since_reads_after_the_mark_or_a_position() {
    let mark = mark();
    assert_eq!(Since::parse("seen"), Some(Since::Seen));
    assert_eq!(Since::parse("g:1:12"), Some(Since::Position(at("g:1", 12))));
    assert_eq!(Since::parse("12"), None);
    assert_eq!(Since::parse("yesterday"), None);
    assert_eq!(Since::Seen.after(Some(&mark)), Some(&at("g1", 10)));
    assert_eq!(Since::Seen.after(None), None);
    let explicit = Since::Position(at("g1", 12));
    assert_eq!(explicit.after(Some(&mark)), Some(&at("g1", 12)));
    assert_eq!(serde_json::to_value(Since::Seen).unwrap(), "seen");
    assert_eq!(
        serde_json::from_value::<Since>(json!("g1:12")).unwrap(),
        Since::Position(at("g1", 12))
    );
    assert!(serde_json::from_value::<Since>(json!("later")).is_err());
}
