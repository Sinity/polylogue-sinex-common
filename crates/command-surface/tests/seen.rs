use command_surface::*;
use serde_json::json;

fn session() -> AgentSession {
    AgentSession::parse("claude-code:worker").unwrap()
}

#[test]
fn seen_mark_moves_forward() {
    let mut mark = SeenMark {
        consumer: Consumer::Session(session()),
        target: "session:abc".into(),
        position: 10,
        anchor: Some("m-10".into()),
    };
    assert!(!mark.advance(7, Some("m-7".into())));
    assert!(!mark.advance(10, Some("m-10b".into())));
    assert_eq!((mark.position, mark.anchor.as_deref()), (10, Some("m-10")));
    assert!(mark.advance(12, Some("m-12".into())));
    assert_eq!((mark.position, mark.anchor.as_deref()), (12, Some("m-12")));
    let wire = serde_json::to_value(&mark).unwrap();
    assert_eq!(
        wire["consumer"],
        json!({"consumer": "session", "session": "claude-code:worker"})
    );
    assert_eq!(serde_json::from_value::<SeenMark>(wire).unwrap(), mark);
}

#[test]
fn only_showing_moves_seen_mark() {
    let moving: Vec<Showing> = [
        Showing::Displayed,
        Showing::ReadSinceSeen,
        Showing::Prerendered,
        Showing::Hidden,
        Showing::Polled,
        Showing::Preview,
        Showing::Read,
    ]
    .into_iter()
    .filter(|showing| showing.moves_seen_mark())
    .collect();
    assert_eq!(moving, [Showing::Displayed, Showing::ReadSinceSeen]);
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
    let mark = SeenMark {
        consumer: Consumer::Operator,
        target: "page:home".into(),
        position: 40,
        anchor: None,
    };
    assert_eq!(Since::parse("seen"), Some(Since::Seen));
    assert_eq!(Since::parse("12"), Some(Since::Position(12)));
    assert_eq!(Since::parse("yesterday"), None);
    assert_eq!(Since::Seen.after(Some(&mark)), 40);
    assert_eq!(Since::Seen.after(None), 0);
    assert_eq!(Since::Position(12).after(Some(&mark)), 12);
    assert_eq!(serde_json::to_value(Since::Seen).unwrap(), "seen");
    assert_eq!(
        serde_json::from_value::<Since>(json!("12")).unwrap(),
        Since::Position(12)
    );
    assert!(serde_json::from_value::<Since>(json!("later")).is_err());
}
