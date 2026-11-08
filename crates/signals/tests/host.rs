//! Host policy tests: inhibition, away handling, on-screen suppression,
//! snooze, producers, forwarded staleness and deferral, with expected
//! handoffs chosen by hand.
use signals::notifications::Outlets;
use signals::*;

const MINUTE: i64 = 60_000;
const DAY: i64 = 24 * HOUR;
struct Utc;
impl Calendar for Utc {
    fn logical_day(&self, at: i64) -> String {
        at.div_euclid(DAY).to_string()
    }
    fn minute(&self, at: i64) -> u16 {
        (at.rem_euclid(DAY) / MINUTE) as u16
    }
    fn next_at(&self, after: i64, minute: u16) -> i64 {
        let candidate = after.div_euclid(DAY) * DAY + i64::from(minute) * MINUTE;
        if candidate > after {
            candidate
        } else {
            candidate + DAY
        }
    }
}
fn at(hour: i64, minute: i64) -> i64 {
    hour * HOUR + minute * MINUTE
}
fn rule(product: &str, name: &str, author: Author, live: bool) -> ActiveRule {
    ActiveRule {
        version: RuleVersion {
            product: product.into(),
            id: name.into(),
            revision: 1,
        },
        rule: serde_json::from_value(
            serde_json::json!({"name": name, "query": name, "interrupt": true}),
        )
        .unwrap(),
        author,
        via: "test".into(),
        live,
        shipped: Shipped::default(),
    }
}
fn row(id: &str, start: i64, end: Option<i64>) -> Row {
    Row {
        occurrence: Occurrence {
            id: id.into(),
            period: "1".into(),
        },
        reference: format!("record:{id}"),
        observed: start,
        start,
        end,
        origin: Origin::Source,
        evidence: serde_json::json!({}),
    }
}
fn observe(e: &mut Evaluator, rule: &ActiveRule, time: i64, rows: Vec<Row>) {
    e.observe(Evaluation {
        rule: rule.version.clone(),
        at: time,
        complete: true,
        rows,
        value: None,
    })
    .unwrap();
}
fn quiet_host() -> HostPolicy {
    HostPolicy::new(
        HostSettings {
            product: "sinex".into(),
            quiet_hours: Some(QuietHours {
                from: 22 * 60,
                to: 8 * 60,
            }),
            ..HostSettings::default()
        },
        InterruptBudget::default(),
    )
}
fn passive(reason: PassiveReason) -> Decision {
    Decision::Passive { reason }
}
const SHOW: Decision = Decision::Show { send_away: false };

/// §1.1, 07:31–08:03: Atuin's capture stalls in quiet hours, the daemon
/// stalls after it, and the morning has one notification.
#[test]
fn morning_outage_is_one_notification_and_sig_inhibit_folds() {
    let mut daemon = rule("sinex", "daemon stalled", Author::Shipped, true);
    daemon.shipped.inhibits = vec!["capture stalled".into()];
    let capture = rule("sinex", "capture stalled", Author::Shipped, true);
    let mut e = Evaluator::new(at(7, 0), vec![daemon.clone(), capture.clone()], &Utc).unwrap();
    observe(&mut e, &daemon, at(7, 0), vec![]);
    observe(&mut e, &capture, at(7, 0), vec![]);
    observe(
        &mut e,
        &capture,
        at(7, 31),
        vec![row("atuin", at(7, 31), None)],
    );
    observe(
        &mut e,
        &daemon,
        at(7, 52),
        vec![row("host", at(7, 52), None)],
    );
    let mut host = quiet_host();
    let morning = e.advance(at(8, 3), &Utc, &mut host).unwrap();
    assert_eq!(morning.deliveries.len(), 2);
    let shown: Vec<_> = morning
        .deliveries
        .iter()
        .filter(|d| d.decision.interrupts())
        .collect();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].fired.rule_name, "daemon stalled");
    assert_eq!(shown[0].at, at(8, 0));
    let root = shown[0].id().to_string();
    let folded = morning
        .deliveries
        .iter()
        .find(|d| d.fired.rule_name == "capture stalled")
        .unwrap();
    assert_eq!(
        folded.decision,
        passive(PassiveReason::CausedBy(root.clone()))
    );
    assert_eq!(folded.fired.basis[0].observed, at(7, 31));
    assert!(folded.why().starts_with("held 29m (quiet)"));
    assert!(morning.summaries.is_empty());

    // The root clears while Atuin is still stalled: the consequence is shown
    // at once, as one grouped notification.
    observe(&mut e, &daemon, at(9, 0), vec![]);
    let cleared = e.advance(at(9, 0), &Utc, &mut host).unwrap();
    assert!(cleared.deliveries.is_empty());
    assert_eq!(cleared.summaries.len(), 1);
    let summary = &cleared.summaries[0];
    assert_eq!(
        summary.kind,
        SummaryKind::Consequences {
            root: root.clone(),
            deliveries: vec![folded.id().to_string()],
        }
    );
    assert!(cleared.outcomes.contains(&OutcomeRecord {
        delivery: root,
        at: at(9, 0),
        outcome: Outcome::Cleared,
    }));
    assert!(cleared.outcomes.contains(&OutcomeRecord {
        delivery: folded.id().to_string(),
        at: at(9, 0),
        outcome: Outcome::Released {
            decision: passive(PassiveReason::Summarised(summary.id.clone())),
        },
    }));
}

#[test]
fn sig_inhibit_shipped_only() {
    let mut authored = rule("sinex", "daemon stalled", Author::Operator, true);
    authored.shipped.inhibits = vec!["capture stalled".into()];
    assert!(Evaluator::new(0, vec![authored.clone()], &Utc).is_err());
    assert!(effective_rules(&[], &[authored.clone()]).is_err());
    let mut critical = rule("sinex", "loss", Author::Agent("a".into()), true);
    critical.shipped.urgency = Urgency::Critical;
    assert!(critical.validate_authorship().is_err());

    // An authored rule is not exempt: an operator's own `capture stalled`
    // is folded under the shipped root like the shipped one.
    let mut daemon = rule("sinex", "daemon stalled", Author::Shipped, true);
    daemon.shipped.inhibits = vec!["capture stalled".into()];
    let mine = rule("sinex", "capture stalled", Author::Operator, true);
    let mut e = Evaluator::new(at(9, 0), vec![daemon.clone(), mine.clone()], &Utc).unwrap();
    observe(&mut e, &daemon, at(9, 0), vec![]);
    observe(&mut e, &mine, at(9, 0), vec![]);
    observe(&mut e, &daemon, at(9, 1), vec![row("host", at(9, 1), None)]);
    observe(&mut e, &mine, at(9, 2), vec![row("atuin", at(9, 2), None)]);
    let got = e
        .advance(at(9, 3), &Utc, &mut HostPolicy::default())
        .unwrap();
    assert_eq!(got.deliveries[0].decision, SHOW);
    assert!(matches!(
        got.deliveries[1].decision,
        Decision::Passive {
            reason: PassiveReason::CausedBy(_)
        }
    ));
}

#[test]
fn sig_away_summary_once() {
    let held = rule("polylogue", "held", Author::Operator, false);
    let acknowledged = rule("polylogue", "acknowledged", Author::Operator, false);
    let cleared = rule("polylogue", "cleared", Author::Operator, true);
    let rules = vec![held.clone(), acknowledged.clone(), cleared.clone()];
    let mut e = Evaluator::new(at(12, 0), rules.clone(), &Utc).unwrap();
    for rule in &rules {
        observe(&mut e, rule, at(12, 0), vec![]);
    }
    observe(&mut e, &held, at(12, 20), vec![row("a", at(12, 20), None)]);
    observe(
        &mut e,
        &acknowledged,
        at(12, 30),
        vec![row("b", at(12, 30), None)],
    );
    observe(
        &mut e,
        &cleared,
        at(12, 40),
        vec![row("c", at(12, 40), None)],
    );
    observe(&mut e, &cleared, at(12, 55), vec![]);
    let mut host = HostPolicy::default();
    host.observe(
        at(12, 15),
        AttentionInput::Presence {
            presence: Presence::Away,
        },
    )
    .unwrap();
    host.observe(
        at(13, 5),
        AttentionInput::Presence {
            presence: Presence::Present,
        },
    )
    .unwrap();
    let away = e.advance(at(12, 50), &Utc, &mut host).unwrap();
    assert!(away.deliveries.is_empty());
    assert_eq!(host.pending_count(), 3);
    let acknowledged_id = host
        .held()
        .find(|held| held.fired.rule_name == "acknowledged")
        .unwrap()
        .fired
        .id
        .clone();
    let act = host.acknowledge(&acknowledged_id, at(12, 50));
    assert_eq!(
        act.deliveries[0].decision,
        passive(PassiveReason::Acknowledged)
    );
    let back = e.advance(at(14, 0), &Utc, &mut host).unwrap();
    assert!(back.deliveries.iter().all(|d| !d.decision.interrupts()));
    assert_eq!(back.summaries.len(), 1);
    let summary = &back.summaries[0];
    assert_eq!(summary.at, at(13, 5));
    let SummaryKind::Away { entries } = &summary.kind else {
        panic!("away summary expected");
    };
    let states: Vec<_> = entries.iter().map(|entry| entry.state).collect();
    assert_eq!(
        states,
        [AwayState::Acknowledged, AwayState::Cleared, AwayState::Held]
    );
    assert_eq!(summary.title(), "3 while you were away · 1 still open");
    assert_eq!(
        back.deliveries[0].decision,
        passive(PassiveReason::Summarised(summary.id.clone()))
    );
    assert!(
        e.advance(at(15, 0), &Utc, &mut host)
            .unwrap()
            .summaries
            .is_empty()
    );

    // With an away command, notices are sent away and the return lists them.
    let mut e = Evaluator::new(at(12, 0), vec![held.clone()], &Utc).unwrap();
    observe(&mut e, &held, at(12, 0), vec![]);
    observe(
        &mut e,
        &held,
        at(12, 20),
        vec![row("a", at(12, 20), None), row("b", at(12, 20), None)],
    );
    let mut host = HostPolicy::new(
        HostSettings {
            away_command: true,
            ..HostSettings::default()
        },
        InterruptBudget::default(),
    );
    host.observe(
        at(12, 15),
        AttentionInput::Presence {
            presence: Presence::Away,
        },
    )
    .unwrap();
    host.observe(
        at(13, 5),
        AttentionInput::Presence {
            presence: Presence::Present,
        },
    )
    .unwrap();
    let sent = e.advance(at(12, 30), &Utc, &mut host).unwrap();
    assert!(
        sent.deliveries
            .iter()
            .all(|d| d.decision == Decision::SendAway)
    );
    host.acknowledge(sent.deliveries[0].id(), at(12, 40));
    let back = e.advance(at(13, 5), &Utc, &mut host).unwrap();
    assert_eq!(
        back.summaries[0].title(),
        "2 while you were away · 1 still open"
    );
}

#[test]
fn sig_on_screen_is_passive_not_seen() {
    let watched = rule("polylogue", "waiting", Author::Operator, false);
    let mut e = Evaluator::new(at(10, 0), vec![watched.clone()], &Utc).unwrap();
    observe(&mut e, &watched, at(10, 0), vec![]);
    observe(
        &mut e,
        &watched,
        at(10, 30),
        vec![row("a", at(10, 30), None)],
    );
    let mut host = HostPolicy::default();
    host.observe(
        at(10, 10),
        AttentionInput::Focus {
            references: vec!["record:a".into()],
        },
    )
    .unwrap();
    let got = e.advance(at(11, 0), &Utc, &mut host).unwrap();
    assert_eq!(got.deliveries[0].decision, passive(PassiveReason::OnScreen));
    assert_eq!(Outlets::of(&got.deliveries[0].decision), Outlets::default());
    assert!(!DeliveryState::fold(&got.outcomes).seen);
    assert!(
        got.outcomes
            .iter()
            .all(|record| record.outcome != Outcome::Seen)
    );
}

#[test]
fn sig_snooze_rechecks_and_sig_outcomes_append_only() {
    for live in [true, false] {
        let watched = rule("polylogue", "waiting", Author::Operator, live);
        let mut e = Evaluator::new(at(10, 0), vec![watched.clone()], &Utc).unwrap();
        observe(&mut e, &watched, at(10, 0), vec![]);
        observe(&mut e, &watched, at(10, 5), vec![row("a", at(10, 5), None)]);
        observe(&mut e, &watched, at(10, 30), vec![]);
        let mut host = HostPolicy::default();
        let shown = e.advance(at(10, 5), &Utc, &mut host).unwrap().deliveries;
        let delivery = shown[0].clone();
        assert_eq!(delivery.decision, SHOW);
        let mut outcomes = vec![host.snooze(&delivery, at(11, 0), at(10, 10))];
        let later = e.advance(at(12, 0), &Utc, &mut host).unwrap();
        // A delivery is never handed off twice; later news is an outcome.
        assert!(later.deliveries.is_empty());
        outcomes.extend(later.outcomes.clone());
        let released = later.outcomes.iter().find(|record| {
            matches!(record.outcome, Outcome::Released { .. }) && record.delivery == delivery.id()
        });
        if live {
            // The condition ended at 10:30, before the snooze ended.
            assert!(released.is_none());
            assert!(outcomes.iter().any(|r| r.outcome == Outcome::Cleared));
        } else {
            assert_eq!(released.unwrap().at, at(11, 0));
        }
        outcomes.extend(host.acknowledge(delivery.id(), at(12, 0)).outcomes);
        let state = DeliveryState::fold(&outcomes);
        assert!(state.acknowledged && state.seen);
        assert_eq!(
            state.snoozed_until,
            if live { Some(at(11, 0)) } else { None }
        );
        assert_eq!(shown[0], delivery);
    }
}

#[test]
fn sig_external_notice_budgeted_and_cleared_by_key() {
    let mut host = HostPolicy::new(
        HostSettings::default(),
        InterruptBudget {
            per_hour: 2,
            handoffs: vec![],
        },
    );
    for n in 0..4 {
        host.submit(
            ProducerNotice {
                producer: "pueue".into(),
                id: format!("fact-{n}"),
                key: format!("job-{n}"),
                title: format!("job {n} finished"),
                body: "exit 0".into(),
                reference: None,
                interrupt: true,
                clears: None,
            },
            at(10, n),
        );
        host.advance(at(10, n), &Utc);
    }
    host.submit(
        ProducerNotice {
            producer: "pueue".into(),
            id: "fact-4".into(),
            key: "job-4".into(),
            title: "job 4 failed".into(),
            body: "exit 1".into(),
            reference: None,
            interrupt: true,
            clears: Some("job-0".into()),
        },
        at(10, 4),
    );
    let last = host.advance(at(10, 4), &Utc);
    assert_eq!(host.budget.used(at(10, 4)), 2);
    assert_eq!(
        last.deliveries[0].decision,
        passive(PassiveReason::BudgetSpent)
    );
    assert_eq!(
        last.deliveries[0].fired.author,
        Author::Producer("pueue".into())
    );
    assert_eq!(
        last.summaries[0].kind,
        SummaryKind::Budget { held_passive: 3 }
    );
    assert_eq!(last.outcomes[0].outcome, Outcome::Cleared);
}

#[test]
fn sig_forwarded_stale_holds() {
    let mut forwarded = rule("polylogue", "waiting", Author::Operator, true);
    forwarded.rule.interrupt = true;
    let mut source = Evaluator::new(at(10, 0), vec![forwarded.clone()], &Utc).unwrap();
    observe(&mut source, &forwarded, at(10, 0), vec![]);
    observe(
        &mut source,
        &forwarded,
        at(10, 5),
        vec![row("a", at(10, 5), None)],
    );
    let reports = source.reports(at(10, 5), &Utc).unwrap();
    let mut host = HostPolicy::default();
    host.absorb(reports);
    assert_eq!(host.advance(at(10, 5), &Utc).deliveries.len(), 1);

    // Another firing whose producer has not confirmed since: held, never shown.
    observe(
        &mut source,
        &forwarded,
        at(10, 10),
        vec![row("a", at(10, 5), None), row("b", at(10, 10), None)],
    );
    let reports = source.reports(at(10, 10), &Utc).unwrap();
    let only_firing: Vec<_> = reports
        .iter()
        .filter(|report| matches!(report, Report::Fired(_)))
        .cloned()
        .collect();
    host.absorb(only_firing);
    for minute in 10..20 {
        assert!(host.advance(at(10, minute), &Utc).deliveries.is_empty());
    }
    assert_eq!(
        host.held().next().unwrap().holds[0].reason,
        HoldReason::Incomplete
    );
    // Progress through the firing instant releases it at the later handoff.
    host.absorb(reports);
    let shown = host.advance(at(10, 20), &Utc).deliveries;
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].fired.trigger.occurrence.id, "b");
    assert_eq!(shown[0].at, at(10, 20));
}

/// A current-state notice forwarded from a producer that fired it at
/// `fired_at` and reported progress at that same instant.
fn forwarded_current(fired_at: i64) -> Fired {
    let forwarded = rule("polylogue", "waiting", Author::Operator, true);
    let mut source = Evaluator::new(at(10, 0), vec![forwarded.clone()], &Utc).unwrap();
    observe(&mut source, &forwarded, at(10, 0), vec![]);
    observe(
        &mut source,
        &forwarded,
        fired_at,
        vec![row("a", fired_at, None)],
    );
    let reports = source.reports(fired_at, &Utc).unwrap();
    let fired = reports
        .iter()
        .find_map(|report| match report {
            Report::Fired(fired) => Some((**fired).clone()),
            _ => None,
        })
        .unwrap();
    assert!(fired.current);
    assert_eq!(fired.at, fired_at);
    assert!(reports.iter().any(|report| matches!(
        report,
        Report::Progress { at, .. } if *at == fired_at
    )));
    fired
}

#[test]
fn sig_forwarded_progress_at_firing_releases_at_later_handoff() {
    // Real clocks: the producer reports progress at the firing instant T and
    // the host hands off at its own later time T + δ.
    let fired_at = at(10, 5);
    for delta in [1, MINUTE, 10 * MINUTE] {
        let fired = forwarded_current(fired_at);
        let mut host = HostPolicy::default();
        host.absorb([Report::Progress {
            product: "polylogue".into(),
            at: fired_at,
            incomplete: Default::default(),
        }]);
        host.absorb([Report::Fired(Box::new(fired.clone()))]);
        let shown = host.advance(fired_at + delta, &Utc).deliveries;
        assert_eq!(shown.len(), 1, "delta {delta}");
        assert_eq!(shown[0].id(), fired.id);
        assert_eq!(shown[0].at, fired_at + delta);
        assert_eq!(host.pending_count(), 0);
    }
}

#[test]
fn sig_forwarded_progress_behind_firing_holds() {
    let fired_at = at(10, 5);
    let fired = forwarded_current(fired_at);
    let mut host = HostPolicy::default();
    host.absorb([Report::Progress {
        product: "polylogue".into(),
        at: fired_at - 1,
        incomplete: Default::default(),
    }]);
    host.absorb([Report::Fired(Box::new(fired.clone()))]);
    for handoff in [fired_at, fired_at + MINUTE, fired_at + 10 * MINUTE] {
        assert!(host.advance(handoff, &Utc).deliveries.is_empty());
    }
    let held = host.held().next().unwrap();
    assert_eq!(held.fired.id, fired.id);
    assert_eq!(held.holds[0].reason, HoldReason::Incomplete);
    // Progress reaching the firing instant releases it.
    host.absorb([Report::Progress {
        product: "polylogue".into(),
        at: fired_at,
        incomplete: Default::default(),
    }]);
    let shown = host.advance(fired_at + 11 * MINUTE, &Utc).deliveries;
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].id(), fired.id);
}

#[test]
fn sig_deferral_bounded() {
    let mut agent = rule(
        "polylogue",
        "build green",
        Author::Agent("codex".into()),
        false,
    );
    agent.rule.interrupt = true;
    let mut e = Evaluator::new(at(9, 0), vec![agent.clone()], &Utc).unwrap();
    observe(&mut e, &agent, at(9, 0), vec![]);
    observe(&mut e, &agent, at(9, 10), vec![row("a", at(9, 10), None)]);
    observe(
        &mut e,
        &agent,
        at(9, 40),
        vec![row("a", at(9, 10), None), row("b", at(9, 40), None)],
    );
    let mut host = HostPolicy::default();
    host.observe(at(9, 2), AttentionInput::DeepWork { active: true })
        .unwrap();
    host.observe(at(9, 45), AttentionInput::Breakpoint).unwrap();
    let got = e.advance(at(11, 0), &Utc, &mut host).unwrap().deliveries;
    let times: Vec<_> = got
        .iter()
        .map(|d| (d.fired.trigger.occurrence.id.as_str(), d.at))
        .collect();
    assert_eq!(times, [("a", at(9, 35)), ("b", at(9, 45))]);
    assert!(got[0].why().starts_with("held 25m (deep work)"));
    assert!(got.iter().all(|d| d.decision == SHOW));
}

#[test]
fn sig_trigger_never_starts_agent() {
    let launcher = std::path::Path::new("/run/current-system/sw/bin/claude");
    let logger = std::path::Path::new("/run/current-system/sw/bin/logger");
    assert!(refuse_agent_launchers([("summarise", launcher)], &["claude", "codex"]).is_err());
    assert!(refuse_agent_launchers([("record", logger)], &["claude", "codex"]).is_ok());
}

#[test]
fn sig_own_notifications_excluded() {
    use signals::notifications::{delivery_hint, signal_origin};
    let hint = delivery_hint("Sinex");
    assert_eq!(hint, "x-sinex-delivery");
    assert_eq!(
        signal_origin([("urgency", "1"), (hint.as_str(), "d1")]),
        Some(("sinex".into(), "d1".into()))
    );
    assert_eq!(signal_origin([("category", "im.received")]), None);
    // The capture marks such a row as a signal origin, and no rule reads it.
    let watched = rule("sinex", "notifications", Author::Operator, false);
    let mut e = Evaluator::new(at(9, 0), vec![watched.clone()], &Utc).unwrap();
    let mut own = row("own", at(9, 1), None);
    own.origin = Origin::SignalDelivery;
    observe(&mut e, &watched, at(9, 1), vec![own]);
    assert!(
        e.advance(at(10, 0), &Utc, &mut HostPolicy::default())
            .unwrap()
            .deliveries
            .is_empty()
    );
}
