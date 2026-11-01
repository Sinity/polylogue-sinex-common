//! The shared Inbox, Why and Signals views render this crate's values.
#![cfg(feature = "web")]
use signals::web::*;
use signals::*;

const MINUTE: i64 = 60_000;
struct Utc;
impl Calendar for Utc {
    fn logical_day(&self, at: i64) -> String {
        (at / (24 * HOUR)).to_string()
    }
    fn minute(&self, at: i64) -> u16 {
        (at.rem_euclid(24 * HOUR) / MINUTE) as u16
    }
    fn next_at(&self, after: i64, minute: u16) -> i64 {
        after - after.rem_euclid(24 * HOUR) + i64::from(minute) * MINUTE + 24 * HOUR
    }
}
fn time(at: i64) -> String {
    let minute = at.rem_euclid(24 * HOUR) / MINUTE;
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

#[test]
fn inbox_why_and_signals_views_show_decisions_and_reasons() {
    let rule = ActiveRule {
        version: RuleVersion {
            product: "polylogue".into(),
            id: "build-green".into(),
            revision: 3,
        },
        rule: serde_json::from_value(
            serde_json::json!({"name": "build green", "query": "ci", "interrupt": true}),
        )
        .unwrap(),
        author: Author::Agent("codex:7c1e".into()),
        via: "mcp".into(),
        live: false,
        shipped: Shipped::default(),
    };
    let mut e = Evaluator::new(9 * HOUR, vec![rule.clone()], &Utc).unwrap();
    let row = |id: &str, at: i64| Row {
        occurrence: Occurrence {
            id: id.into(),
            period: "1".into(),
        },
        reference: format!("ci:{id}"),
        observed: at,
        start: at,
        end: None,
        origin: Origin::Source,
        evidence: serde_json::json!({}),
    };
    e.observe(Evaluation {
        rule: rule.version.clone(),
        at: 9 * HOUR + 10 * MINUTE,
        complete: true,
        rows: vec![row("run-1", 9 * HOUR + 10 * MINUTE)],
        value: None,
    })
    .unwrap();
    e.observe(Evaluation {
        rule: rule.version.clone(),
        at: 9 * HOUR + 30 * MINUTE,
        complete: true,
        rows: vec![
            row("run-1", 9 * HOUR + 10 * MINUTE),
            row("run-2", 9 * HOUR + 30 * MINUTE),
        ],
        value: None,
    })
    .unwrap();
    let mut host = HostPolicy::default();
    host.observe(9 * HOUR, AttentionInput::DeepWork { active: true })
        .unwrap();
    host.observe(
        9 * HOUR + 21 * MINUTE,
        AttentionInput::Presence {
            presence: Presence::Present,
        },
    )
    .unwrap();
    host.observe(9 * HOUR + 21 * MINUTE, AttentionInput::Breakpoint)
        .unwrap();
    let delivered = e
        .advance(9 * HOUR + 31 * MINUTE, &Utc, &mut host)
        .unwrap()
        .deliveries;
    let shown = &delivered[0];
    assert_eq!(
        shown.why(),
        "held 11m (deep work) · budget 0 of 4 this hour · shown"
    );
    let outcomes = vec![OutcomeRecord {
        delivery: shown.id().into(),
        at: shown.at,
        outcome: Outcome::Opened { via: Via::Desktop },
    }];
    let opened = DeliveryState::fold(&outcomes);
    let unseen = DeliveryState::default();
    let held: Vec<_> = host.held().collect();
    assert_eq!(held.len(), 1);
    let attention = host.attention();
    let entries = [InboxEntry {
        delivery: shown,
        state: &opened,
        title: "cargo test green on sx-admission",
        href: "/ci/run-1",
        why_href: "/notices/1/why",
        lines: &[],
    }];
    let held_entries = [HeldEntry {
        held: held[0],
        title: "cargo test green on pl-ingest",
        why_href: "/notices/2/why",
    }];
    let page = inbox(&Inbox {
        attention: &attention,
        quiet_hours: Some(QuietHours {
            from: 22 * 60,
            to: 8 * 60,
        }),
        entries: &entries,
        held: &held_entries,
        time: &time,
    })
    .into_string();
    assert!(page.contains("0 unseen · 1 held"), "{page}");
    assert!(page.contains("present · deep work · budget 1 of 4 · quiet 22:00–08:00"));
    assert!(page.contains("Interrupted"));
    assert!(page.contains("held: deep work (≤ 09:55)"));
    assert!(page.contains("09:21"));

    let why_panel = why(shown, "notify when a CI run is green", &outcomes, &time).into_string();
    assert!(why_panel.contains("build green · revision 3"));
    assert!(why_panel.contains("by codex:7c1e"));
    assert!(why_panel.contains("held 11m (deep work)"));
    assert!(why_panel.contains("ci:run-1"));

    let rules = [RuleRow {
        rule: &rule,
        read_as: "notify when a CI run is green",
        last_fired: Some(shown.at),
        deliveries: 2,
        usefulness: Usefulness::of([&opened, &unseen]),
        href: "/signals/build-green",
    }];
    let table = signals(&rules, &time).into_string();
    assert!(table.contains("codex:7c1e"));
    assert!(table.contains("interrupt"));
    assert!(table.contains("opened 1 · dismissed at once 0 · expired 0"));
}
