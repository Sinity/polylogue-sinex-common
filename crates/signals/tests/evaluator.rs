use signals::*;
use std::collections::BTreeMap;
const MIN: i64 = 60_000;
const DAY: i64 = 24 * HOUR;
struct CalendarUtc;
impl Calendar for CalendarUtc {
    fn logical_day(&self, at: i64) -> String {
        at.div_euclid(DAY).to_string()
    }
    fn minute(&self, at: i64) -> u16 {
        (at.rem_euclid(DAY) / MIN) as u16
    }
    fn next_at(&self, after: i64, minute: u16) -> i64 {
        let candidate = after.div_euclid(DAY) * DAY + i64::from(minute) * MIN;
        if candidate > after {
            candidate
        } else {
            candidate + DAY
        }
    }
    fn quiet_end(&self, at: i64, hours: QuietHours) -> i64 {
        self.next_at(at, hours.to)
    }
}
fn at(hour: i64) -> i64 {
    hour * HOUR
}
fn minute(hour: i64, minute: i64) -> i64 {
    at(hour) + minute * MIN
}
fn rule() -> ActiveRule {
    ActiveRule {
        version: RuleVersion {
            product: "product".into(),
            id: "waiting".into(),
            revision: 1,
        },
        rule: serde_json::from_value(serde_json::json!({"name":"waiting", "query":"waiting"}))
            .unwrap(),
        author: Author::Operator,
        via: "test".into(),
        live: false,
        shipped: Shipped::default(),
    }
}
fn row(id: &str, start: i64, observed: i64, end: Option<i64>) -> Row {
    Row {
        occurrence: Occurrence {
            id: id.into(),
            period: "1".into(),
        },
        reference: format!("record:{id}"),
        observed,
        start,
        end,
        origin: Origin::Source,
        evidence: serde_json::json!({"seen":observed}),
    }
}
fn evaluation(rule: &ActiveRule, at: i64, rows: Vec<Row>) -> Evaluation {
    Evaluation {
        rule: rule.version.clone(),
        at,
        complete: true,
        rows,
        value: None,
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
struct TestEvaluator {
    evaluator: Evaluator,
    host: HostPolicy,
}
impl std::ops::Deref for TestEvaluator {
    type Target = Evaluator;
    fn deref(&self) -> &Evaluator {
        &self.evaluator
    }
}
impl std::ops::DerefMut for TestEvaluator {
    fn deref_mut(&mut self) -> &mut Evaluator {
        &mut self.evaluator
    }
}
impl TestEvaluator {
    fn next_due(&self) -> Option<i64> {
        self.evaluator
            .next_due()
            .into_iter()
            .chain(self.host.next_due())
            .min()
    }
    fn pending_count(&self) -> usize {
        self.evaluator.pending_count() + self.host.pending_count()
    }
}
fn evaluator(rule: ActiveRule, rows: Vec<Row>, start: i64) -> TestEvaluator {
    let mut evaluator = Evaluator::new(start, vec![rule.clone()], &CalendarUtc).unwrap();
    let mut revisions: BTreeMap<i64, Vec<Row>> = BTreeMap::new();
    for row in rows {
        revisions
            .entry(row.observed.max(start))
            .or_default()
            .push(row);
    }
    let mut latest = BTreeMap::new();
    for (time, rows) in revisions {
        for row in rows {
            latest.insert(row.occurrence.id.clone(), row);
        }
        evaluator
            .observe(evaluation(&rule, time, latest.values().cloned().collect()))
            .unwrap();
    }
    TestEvaluator {
        evaluator,
        host: HostPolicy::default(),
    }
}
fn advance(e: &mut TestEvaluator, stop: i64) -> Vec<Delivery> {
    e.evaluator
        .advance(stop, &CalendarUtc, &mut e.host)
        .unwrap()
        .deliveries
}
fn resume(e: &mut TestEvaluator, stop: i64) -> Vec<Delivery> {
    e.evaluator
        .resume(stop, &CalendarUtc, &mut e.host)
        .unwrap()
        .deliveries
}
fn interrupted(d: &Delivery) -> bool {
    d.decision.interrupts()
}
fn passive(reason: PassiveReason) -> Decision {
    Decision::Passive { reason }
}
fn quiet() -> QuietHours {
    QuietHours {
        from: 22 * 60,
        to: 8 * 60,
    }
}
fn roundtrip(e: &TestEvaluator) -> TestEvaluator {
    serde_json::from_str(&serde_json::to_string(e).unwrap()).unwrap()
}

#[test]
fn initial_baseline_and_repeated_advance_baseline_never_fires() {
    let mut initial = evaluator(rule(), vec![row("old", at(10), at(10), None)], at(11));
    assert!(advance(&mut initial, at(12)).is_empty());
    let mut live = evaluator(rule(), vec![row("new", at(12), at(12), None)], at(11));
    assert_eq!(advance(&mut live, at(12)).len(), 1);
    assert!(advance(&mut live, at(12)).is_empty());
}
#[test]
fn observed_future_effective_time_is_a_valid_wakeup_only_observed_rows_wake() {
    let mut active = rule();
    active.live = true;
    let mut e = evaluator(active, vec![row("a", at(12), at(10), None)], at(9));
    assert!(advance(&mut e, at(11)).is_empty());
    assert_eq!(e.next_due(), Some(at(12)));
    assert_eq!(advance(&mut e, at(12))[0].at, at(12));
}
#[test]
fn relative_result_key_counterexample_and_implicit_expiry() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    db.execute_batch(
        "CREATE TABLE note (at INTEGER, valid_until INTEGER); INSERT INTO note VALUES (570,630)",
    )
    .unwrap();
    let query = |now: i64| {
        db.query_row(
            "SELECT count(*) FROM note WHERE at >= ?1 AND at < ?2 AND valid_until > ?2",
            [now - 60, now],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
    };
    assert_ne!(query(601), query(659));
    assert_eq!(601 / 60, 659 / 60);
    assert_eq!((query(600), query(660)), (1, 0));
}
#[test]
fn for_deadline_without_arrival_and_early_end_duration_from_observation() {
    let mut r = rule();
    r.rule.duration = Duration(2 * HOUR);
    let mut e = evaluator(
        r,
        vec![
            row("long", at(10), at(10), Some(at(13))),
            row("short", at(14), at(14), Some(at(15))),
        ],
        at(9),
    );
    let got = advance(&mut e, at(18));
    assert_eq!(
        got.iter()
            .map(|d| (&*d.fired.basis[0].occurrence.id, d.at))
            .collect::<Vec<_>>(),
        vec![("long", at(12))]
    );
}
#[test]
fn prefix_future_metadata_cannot_wake_a_match_only_observed_rows_wake() {
    let a = row("a", minute(13, 30), minute(13, 30), None);
    let mut e = evaluator(rule(), vec![a.clone()], at(9));
    let mut augmented = evaluator(
        rule(),
        vec![a, row("future", minute(14, 30), at(17), None)],
        at(9),
    );
    let expected = advance(&mut e, at(15));
    assert_eq!(expected.len(), 1);
    assert_eq!(advance(&mut augmented, at(15)), expected);
}
#[test]
fn window_excludes_reconstructed_past_baseline_never_fires() {
    let mut e = evaluator(rule(), vec![row("a", at(10), at(10), None)], at(12));
    assert!(advance(&mut e, at(13)).is_empty());
}
#[test]
fn pending_is_not_future_handoff_quiet_hours_hold_interrupts_only() {
    let mut r = rule();
    r.rule.interrupt = true;
    r.rule.quiet_hours = Some(quiet());
    let mut e = evaluator(r, vec![row("a", at(23), at(23), None)], at(22));
    assert!(advance(&mut e, minute(23, 30)).is_empty());
    assert_eq!(e.next_due(), Some(at(32)));
    let got = advance(&mut e, at(32));
    assert_eq!(got[0].at, at(32));
    assert_eq!(got[0].fired.basis[0].observed, at(23));
}
#[test]
fn passive_quiet_exemption_and_immutable_handoff_basis_quiet_hours_hold_interrupts_only() {
    let mut r = rule();
    r.rule.quiet_hours = Some(quiet());
    let mut e = evaluator(
        r,
        vec![
            row("a", at(23), at(23), None),
            row("a", at(23), at(25), Some(at(25))),
        ],
        at(22),
    );
    let got = advance(&mut e, at(23));
    let basis = got[0].clone();
    assert!(advance(&mut e, at(33)).is_empty());
    assert_eq!(basis.fired.basis[0].observed, at(23));
    assert_eq!(basis.at, at(23));
}
#[test]
fn ended_condition_at_release_drops_pending_without_catchup() {
    let mut r = rule();
    r.rule.interrupt = true;
    r.rule.quiet_hours = Some(quiet());
    r.live = true;
    let mut e = evaluator(
        r,
        vec![
            row("a", at(23), at(23), None),
            row("a", at(23), at(31), Some(at(31))),
        ],
        at(22),
    );
    assert!(advance(&mut e, at(34)).is_empty());
    assert_eq!(e.pending_count(), 0);
}
#[test]
fn quiet_release_counts_actual_handoffs() {
    let mut r = rule();
    r.rule.interrupt = true;
    r.rule.quiet_hours = Some(quiet());
    r.rule.rate_limit = Some(RateLimit {
        count: 1,
        window: Duration(HOUR),
    });
    let mut e = evaluator(
        r,
        vec![
            row("a", at(23), at(23), None),
            row("b", at(25), at(25), None),
            row("c", at(27), at(27), None),
        ],
        at(22),
    );
    let got = advance(&mut e, at(33));
    assert_eq!(got.len(), 3);
    assert_eq!(got.iter().filter(|d| interrupted(d)).count(), 1);
    assert_eq!(
        got.iter()
            .filter(|d| d.decision == passive(PassiveReason::RateLimit))
            .count(),
        2
    );
    assert!(got.iter().all(|d| d.at == at(32)));
}
#[test]
fn current_rechecks_but_historical_survives_end_current_state_rechecks_at_handoff() {
    let mut r = rule();
    r.rule.interrupt = true;
    r.rule.quiet_hours = Some(quiet());
    let rows = vec![
        row("a", at(23), at(23), None),
        row("a", at(23), at(25), Some(at(25))),
    ];
    let mut historical = evaluator(r.clone(), rows.clone(), at(22));
    r.live = true;
    let mut current = evaluator(r, rows, at(22));
    assert_eq!(advance(&mut historical, at(33)).len(), 1);
    assert!(advance(&mut current, at(33)).is_empty());
}
#[test]
fn rule_withdrawal_and_revision_cancel_old_work_revision_takes_new_baseline() {
    for enabled in [true, false] {
        let mut r = rule();
        r.rule.interrupt = true;
        r.rule.quiet_hours = Some(quiet());
        let mut e = evaluator(r.clone(), vec![row("a", at(23), at(23), None)], at(22));
        assert!(advance(&mut e, minute(23, 30)).is_empty());
        r.version.revision = 2;
        r.rule.enabled = enabled;
        e.replace_rule(r, minute(23, 30), &CalendarUtc).unwrap();
        assert!(advance(&mut e, at(33)).is_empty());
    }
    let r = rule();
    let mut e = evaluator(r.clone(), vec![], at(9));
    e.withdraw(&r.version);
    assert!(advance(&mut e, at(10)).is_empty());
}
#[test]
fn replay_uses_logical_occurrence_and_period_rearms_trigger_fires_once() {
    let a = row("a", at(10), at(10), None);
    let mut replay = a.clone();
    replay.observed = at(11);
    let mut period = row("a", at(12), at(12), None);
    period.occurrence.period = "2".into();
    let mut e = evaluator(rule(), vec![a, replay, period], at(9));
    let got = advance(&mut e, at(13));
    assert_eq!(got.len(), 2);
    assert_ne!(got[0].id(), got[1].id());
}
#[test]
fn late_current_period_starts_duration_at_observation_duration_from_observation() {
    let mut r = rule();
    r.rule.duration = Duration(20 * MIN);
    let mut e = evaluator(r.clone(), vec![row("a", at(10), at(12), None)], at(9));
    assert_eq!(advance(&mut e, at(13))[0].at, minute(12, 20));
    let mut e = evaluator(r, vec![row("a", at(10), at(12), Some(at(11)))], at(9));
    assert!(advance(&mut e, at(13)).is_empty());
}
#[test]
fn budget_shared_and_operator_exempt_at_handoff_budget_spares_operator_rules() {
    let mut budget = HostPolicy::default();
    for n in 0..6 {
        let mut r = rule();
        r.version.product = format!("product{n}");
        r.rule.interrupt = true;
        r.author = Author::Agent(format!("agent{n}"));
        let mut e = evaluator(r, vec![row("a", at(10), at(10), None)], at(9));
        assert_eq!(
            e.advance(at(11), &CalendarUtc, &mut budget)
                .unwrap()
                .deliveries[0]
                .decision,
            if n < 4 {
                Decision::Show { send_away: false }
            } else {
                passive(PassiveReason::BudgetSpent)
            }
        );
    }
    for (index, author) in [Author::Operator, Author::Shipped].into_iter().enumerate() {
        let mut r = rule();
        r.version.id = format!("exempt-{index}");
        r.rule.interrupt = true;
        r.author = author;
        let mut e = evaluator(r, vec![row("a", at(10), at(10), None)], at(9));
        assert_eq!(
            e.advance(at(11), &CalendarUtc, &mut budget)
                .unwrap()
                .deliveries[0]
                .decision,
            Decision::Show { send_away: false }
        );
    }
}
#[test]
fn resume_uses_actual_clock_and_unknown_does_not_auto_resend_no_retroactive_handoff() {
    let mut r = rule();
    r.rule.duration = Duration(20 * MIN);
    let mut e = evaluator(r, vec![row("a", at(10), at(10), None)], at(9));
    assert!(advance(&mut e, at(10)).is_empty());
    let got = resume(&mut e, at(12));
    assert_eq!(got[0].at, at(12));
    let mut restored = roundtrip(&e);
    assert!(advance(&mut restored, at(13)).is_empty());
    assert!(
        restored
            .advance(at(11), &CalendarUtc, &mut HostPolicy::default())
            .is_err()
    );
}
#[test]
fn resume_consumes_ended_arrivals_without_catchup() {
    let mut live = rule();
    live.live = true;
    let rows = vec![
        row("a", minute(13, 30), minute(13, 30), None),
        row("a", minute(13, 30), at(14), Some(at(14))),
    ];
    for checkpoint in [at(9), at(12), at(13)] {
        let mut e = evaluator(live.clone(), rows.clone(), at(9));
        assert!(advance(&mut e, checkpoint).is_empty());
        let mut restored = roundtrip(&e);
        assert!(resume(&mut restored, at(15)).is_empty());
    }
    let mut control = evaluator(rule(), rows, at(9));
    assert!(advance(&mut control, at(12)).is_empty());
    assert_eq!(resume(&mut control, at(15))[0].at, at(15));
}
#[test]
fn resume_drops_quiet_pending_whose_condition_ended() {
    for checkpoint in [
        at(22),
        minute(23, 30),
        at(30),
        at(31),
        at(32),
        at(33),
        at(34),
    ] {
        let mut r = rule();
        r.rule.interrupt = true;
        r.rule.quiet_hours = Some(quiet());
        r.live = true;
        let mut e = evaluator(
            r,
            vec![
                row("a", at(23), at(23), None),
                row("a", at(23), at(31), Some(at(31))),
            ],
            at(22),
        );
        assert!(advance(&mut e, checkpoint).is_empty());
        let mut restored = roundtrip(&e);
        assert!(resume(&mut restored, at(34)).is_empty());
        assert_eq!(restored.pending_count(), 0);
    }
}
#[test]
fn generated_prefix_partition_and_abstract_checkpoint_partition_invariance() {
    for end in [None, Some(at(11)), Some(at(13))] {
        for seen in [at(10), at(12)] {
            for live in [false, true] {
                let mut r = rule();
                r.rule.duration = Duration(20 * MIN);
                r.live = live;
                let rows = vec![row("a", at(10), seen, end)];
                let mut whole = evaluator(r.clone(), rows.clone(), at(9));
                let expected = advance(&mut whole, at(14));
                for checkpoint in 9..15 {
                    let mut prefix = evaluator(r.clone(), rows.clone(), at(9));
                    let mut got = advance(&mut prefix, at(checkpoint));
                    let mut restored = roundtrip(&prefix);
                    got.extend(advance(&mut restored, at(14)));
                    assert_eq!(
                        got, expected,
                        "end={end:?} seen={seen} live={live} checkpoint={checkpoint}"
                    );
                    let mut augmented = rows.clone();
                    augmented.push(row("unseen", minute(9, 30), at(17), None));
                    let mut augmented = evaluator(r.clone(), augmented, at(9));
                    assert_eq!(
                        advance(&mut augmented, at(checkpoint)),
                        advance(
                            &mut evaluator(r.clone(), rows.clone(), at(9)),
                            at(checkpoint)
                        )
                    );
                }
            }
        }
    }
}
#[test]
fn change_and_threshold_baselines_are_explicit_change_kinds_stay_apart_threshold_crossing_once() {
    let orders = [
        vec!["a", "b"],
        vec!["a", "b"],
        vec!["b", "a"],
        vec!["a", "b"],
        vec!["c", "a", "b"],
    ];
    for (what, expected) in [
        (ChangeKind::Membership, vec![at(13)]),
        (ChangeKind::Ranking, vec![at(11), at(12), at(13)]),
        (ChangeKind::TopK, vec![at(11), at(12), at(13)]),
    ] {
        let mut r = rule();
        r.rule.fires = Fires::OnChange {
            what,
            k: (what == ChangeKind::TopK).then_some(1),
        };
        let mut e = evaluator(r.clone(), vec![], at(9));
        for (n, order) in orders.iter().enumerate() {
            let time = at(9 + n as i64);
            e.observe(evaluation(
                &r,
                time,
                order.iter().map(|id| row(id, time, time, None)).collect(),
            ))
            .unwrap();
        }
        assert_eq!(
            advance(&mut e, at(14))
                .iter()
                .map(|d| d.at)
                .collect::<Vec<_>>(),
            expected
        );
    }
    let mut r = rule();
    r.rule.fires = Fires::OnThreshold {
        value: 10.,
        compare: Compare::Less,
    };
    let mut e = evaluator(r.clone(), vec![], at(9));
    for (n, value) in [5., 4., 20., 3.].into_iter().enumerate() {
        let mut input = evaluation(&r, at(9 + n as i64), vec![]);
        input.value = Some(value);
        e.observe(input).unwrap();
    }
    assert_eq!(
        advance(&mut e, at(13))
            .iter()
            .map(|d| d.at)
            .collect::<Vec<_>>(),
        vec![at(12)]
    );
}

#[test]
fn no_self_amplification_and_incomplete_never_fires() {
    let r = rule();
    let mut e = evaluator(r.clone(), vec![], at(9));
    let mut a = row("delivery", at(10), at(10), None);
    a.origin = Origin::SignalDelivery;
    let mut b = a.clone();
    b.occurrence.id = "injected".into();
    b.origin = Origin::InjectedContext;
    e.observe(evaluation(&r, at(10), vec![a, b])).unwrap();
    assert!(advance(&mut e, at(10)).is_empty());
    let mut input = evaluation(&r, at(11), vec![row("source", at(11), at(11), None)]);
    input.complete = false;
    e.observe(input).unwrap();
    assert!(advance(&mut e, at(12)).is_empty());
    assert_eq!(e.incomplete(), vec![r.version]);
}
#[test]
fn min_change_and_cooldown_net_changes_survive_checkpoint() {
    let mut r = rule();
    r.rule.fires = Fires::OnChange {
        what: ChangeKind::Membership,
        k: None,
    };
    r.rule.min_change = 2;
    let mut e = evaluator(r.clone(), vec![], at(9));
    for (time, ids) in [
        (at(9), vec!["a"]),
        (minute(9, 1), vec!["a", "b"]),
        (minute(9, 2), vec!["a", "b", "c"]),
        (minute(9, 3), vec!["a", "b", "c", "d", "e"]),
    ] {
        e.observe(evaluation(
            &r,
            time,
            ids.iter().map(|id| row(id, time, time, None)).collect(),
        ))
        .unwrap();
    }
    let first = advance(&mut e, minute(9, 4));
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].at, minute(9, 2));
    let mut restored = roundtrip(&e);
    let second = advance(&mut restored, minute(10, 2));
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].fired.change.as_ref().unwrap().joined.len(), 2);
}
#[test]
fn at_once_per_logical_day_and_shipped_override() {
    let mut r = rule();
    r.rule.fires = Fires::At { minute: 10 * 60 };
    let mut e = evaluator(r.clone(), vec![], at(9));
    e.observe(evaluation(&r, at(9), vec![])).unwrap();
    assert_eq!(advance(&mut e, at(34)).len(), 2);
    assert!(advance(&mut e, at(34)).is_empty());
    let mut shipped = r.clone();
    shipped.author = Author::Shipped;
    let mut authored = r;
    authored.rule.enabled = false;
    assert!(
        !effective_rules(&[shipped.clone()], &[authored]).unwrap()[0]
            .rule
            .enabled
    );
    assert_eq!(
        effective_rules(&[shipped.clone()], &[]).unwrap(),
        vec![shipped]
    );
}
#[test]
fn duration_cancelled_by_complete_snapshot_removal_never_rearms_same_period() {
    let mut r = rule();
    r.rule.duration = Duration(HOUR);
    let mut e = evaluator(r.clone(), vec![], at(9));
    e.observe(evaluation(&r, at(10), vec![row("a", at(10), at(10), None)]))
        .unwrap();
    e.observe(evaluation(&r, minute(10, 10), vec![])).unwrap();
    e.observe(evaluation(
        &r,
        minute(10, 20),
        vec![row("a", at(10), minute(10, 20), None)],
    ))
    .unwrap();
    assert!(advance(&mut e, at(12)).is_empty());
}
#[test]
fn rule_schema_defaults_configuration_and_authorship() {
    let r = rule().rule;
    assert!(!r.interrupt);
    assert_eq!(r.fires, Fires::OnNew);
    let configuration = Configuration {
        value_targets: ["temperature".into()].into(),
        action_commands: ["record".into()].into(),
    };
    assert!(r.validate(&configuration).is_ok());
    assert!(
        serde_json::from_value::<Rule>(
            serde_json::json!({"name":"x","query":"x","author":"operator"})
        )
        .is_err()
    );
    let mut action = r.clone();
    action.deliver = Deliver::Action;
    action.command = Some("unknown".into());
    assert!(action.validate(&configuration).is_err());
    action.command = Some("record".into());
    assert!(action.validate(&configuration).is_ok());
    let mut value = r;
    value.deliver = Deliver::Value;
    value.target = Some("temperature".into());
    assert!(value.validate(&configuration).is_ok());
    assert_eq!(
        serde_json::from_str::<Duration>("\"20m\"").unwrap(),
        Duration(20 * MIN)
    );
    assert_eq!(
        serde_json::from_str::<Duration>("\" 1.5s \"").unwrap(),
        Duration(1500)
    );
    assert_eq!(
        serde_json::from_str::<Duration>("\"1w\"").unwrap(),
        Duration(7 * 24 * HOUR)
    );
    assert!(serde_json::from_str::<Duration>("-1").is_err());
    assert!(serde_json::from_str::<Duration>("\"999999999999999999999d\"").is_err());
}
#[test]
fn revision_takes_new_baseline_from_revised_query() {
    let mut r = rule();
    r.rule.fires = Fires::OnChange {
        what: ChangeKind::Membership,
        k: None,
    };
    let mut e = evaluator(r.clone(), vec![], at(9));
    e.observe(evaluation(
        &r,
        at(9),
        vec![row("old-query", at(8), at(8), None)],
    ))
    .unwrap();
    assert!(advance(&mut e, at(10)).is_empty());
    r.version.revision = 2;
    r.rule.query = "other".into();
    e.replace_rule(r.clone(), at(10), &CalendarUtc).unwrap();
    e.observe(evaluation(
        &r,
        at(10),
        vec![row("new-query", at(8), at(8), None)],
    ))
    .unwrap();
    assert!(advance(&mut e, at(11)).is_empty());
    e.observe(evaluation(
        &r,
        at(12),
        vec![
            row("new-query", at(8), at(8), None),
            row("new", at(12), at(12), None),
        ],
    ))
    .unwrap();
    assert_eq!(advance(&mut e, at(12)).len(), 1);
}
#[test]
fn value_and_action_handoffs_preserve_configured_delivery_and_payload() {
    for (deliver, channel) in [
        (Deliver::Value, Channel::Value),
        (Deliver::Action, Channel::Action),
    ] {
        let mut r = rule();
        r.rule.deliver = deliver;
        let mut e = evaluator(r.clone(), vec![], at(9));
        let mut input = evaluation(&r, at(10), vec![row("a", at(10), at(10), None)]);
        input.value = Some(42.);
        e.observe(input).unwrap();
        let got = advance(&mut e, at(11));
        assert_eq!(got[0].fired.channel, channel);
        assert_eq!(got[0].decision, Decision::Run);
        assert_eq!(got[0].fired.value, Some(42.));
        assert_eq!(got[0].fired.basis[0].reference, "record:a");
    }
}
#[test]
fn incomplete_never_fires_scheduled_without_initial_freshness() {
    let mut r = rule();
    r.rule.fires = Fires::At { minute: 10 * 60 };
    let mut e = evaluator(r.clone(), vec![], at(9));
    assert!(advance(&mut e, at(10)).is_empty());
    assert_eq!(e.incomplete(), vec![r.version.clone()]);
    e.observe(evaluation(&r, at(11), vec![])).unwrap();
    assert_eq!(advance(&mut e, at(11))[0].at, at(11));
}
#[test]
fn partition_invariance_with_restored_shared_interrupt_budget() {
    let mut r = rule();
    r.rule.interrupt = true;
    r.author = Author::Agent("agent".into());
    let rows = (0..6)
        .map(|n| row(&n.to_string(), minute(10, n), minute(10, n), None))
        .collect::<Vec<_>>();
    let mut whole = evaluator(r.clone(), rows.clone(), at(9));
    let mut whole_budget = HostPolicy::new(
        HostSettings::default(),
        InterruptBudget {
            per_hour: 2,
            handoffs: vec![],
        },
    );
    let expected = whole
        .advance(at(11), &CalendarUtc, &mut whole_budget)
        .unwrap();
    assert_eq!(
        expected
            .deliveries
            .iter()
            .filter(|d| interrupted(d))
            .count(),
        2
    );
    assert_eq!(
        expected.summaries.last().map(|summary| &summary.kind),
        Some(&SummaryKind::Budget { held_passive: 4 })
    );
    for checkpoint in [at(9), at(10), minute(10, 2), minute(10, 5), at(11)] {
        let mut partial = evaluator(r.clone(), rows.clone(), at(9));
        let mut budget = HostPolicy::new(
            HostSettings::default(),
            InterruptBudget {
                per_hour: 2,
                handoffs: vec![],
            },
        );
        let mut got = partial
            .advance(checkpoint, &CalendarUtc, &mut budget)
            .unwrap();
        let mut restored = roundtrip(&partial);
        let mut restored_budget: HostPolicy =
            serde_json::from_str(&serde_json::to_string(&budget).unwrap()).unwrap();
        got.extend(
            restored
                .advance(at(11), &CalendarUtc, &mut restored_budget)
                .unwrap(),
        );
        assert_eq!(got, expected);
        assert_eq!(restored_budget, whole_budget);
    }
}
#[test]
fn current_state_rechecks_at_handoff_threshold_recovery_during_quiet() {
    let mut r = rule();
    r.rule.fires = Fires::OnThreshold {
        value: 10.,
        compare: Compare::Greater,
    };
    r.live = true;
    r.rule.interrupt = true;
    r.rule.quiet_hours = Some(quiet());
    let mut e = evaluator(r.clone(), vec![], at(21));
    for (time, value) in [(at(21), 1.), (at(23), 20.), (at(25), 1.)] {
        let mut input = evaluation(&r, time, vec![]);
        input.value = Some(value);
        e.observe(input).unwrap();
    }
    assert!(advance(&mut e, at(33)).is_empty());
}
#[test]
fn only_observed_rows_wake_live_change_effective_boundaries() {
    let mut r = rule();
    r.rule.fires = Fires::OnChange {
        what: ChangeKind::Membership,
        k: None,
    };
    r.live = true;
    let mut e = evaluator(r.clone(), vec![], at(9));
    e.observe(evaluation(
        &r,
        at(9),
        vec![row("a", at(10), at(9), Some(at(12)))],
    ))
    .unwrap();
    let got = advance(&mut e, at(13));
    assert_eq!(
        got.iter().map(|d| d.at).collect::<Vec<_>>(),
        vec![at(10), at(12)]
    );
    assert_eq!(got[0].fired.change.as_ref().unwrap().joined[0].id, "a");
    assert_eq!(got[1].fired.change.as_ref().unwrap().left[0].id, "a");
}
fn fired(reports: &[Report]) -> Vec<Fired> {
    reports
        .iter()
        .filter_map(|report| match report {
            Report::Fired(fired) => Some((**fired).clone()),
            _ => None,
        })
        .collect()
}
#[test]
fn one_deliverer_per_host_forwarded_reports_use_one_quiet_policy_and_budget() {
    let mut pl_rule = rule();
    pl_rule.version.product = "pl".into();
    pl_rule.rule.interrupt = true;
    pl_rule.author = Author::Agent("pl-agent".into());
    let mut sx_rule = pl_rule.clone();
    sx_rule.version.product = "sx".into();
    sx_rule.author = Author::Agent("sx-agent".into());
    let mut pl = evaluator(pl_rule, vec![row("pl-row", at(23), at(23), None)], at(22));
    let mut sx = evaluator(sx_rule, vec![row("sx-row", at(23), at(23), None)], at(22));
    // Polylogue in primary mode has no host policy: it only reports.
    let forwarded = pl.evaluator.reports(at(23), &CalendarUtc).unwrap();
    let original = fired(&forwarded).remove(0);
    let mut host = HostPolicy::new(
        HostSettings {
            quiet_hours: Some(quiet()),
            ..HostSettings::default()
        },
        InterruptBudget {
            per_hour: 1,
            handoffs: vec![],
        },
    );
    host.absorb(forwarded);
    let mut copied = original.clone();
    copied.basis[0].observed = at(24);
    host.absorb([Report::Fired(Box::new(copied))]);
    assert_eq!(
        host.held()
            .find(|held| held.fired.id == original.id)
            .map(|held| &held.fired),
        Some(&original)
    );
    let local = sx
        .evaluator
        .advance(at(23), &CalendarUtc, &mut host)
        .unwrap();
    assert!(local.deliveries.is_empty());
    assert!(host.budget.handoffs.is_empty());
    assert_eq!(host.pending_count(), 2);
    let mut restored: HostPolicy =
        serde_json::from_str(&serde_json::to_string(&host).unwrap()).unwrap();
    assert_eq!(restored, host);
    let released = sx
        .evaluator
        .advance(at(32), &CalendarUtc, &mut restored)
        .unwrap();
    assert_eq!(released.deliveries.len(), 2);
    assert!(released.deliveries.iter().all(|d| d.at == at(32)));
    assert_eq!(restored.budget.handoffs, vec![at(32)]);
    assert_eq!(
        released
            .deliveries
            .iter()
            .filter(|d| interrupted(d))
            .count(),
        1
    );
    assert_eq!(
        released
            .deliveries
            .iter()
            .filter(|d| d.decision == passive(PassiveReason::BudgetSpent))
            .count(),
        1
    );
    assert_eq!(released.deliveries[0].id(), original.id);
    assert_eq!(released.deliveries[0].fired.basis[0].observed, at(23));
    restored.absorb([Report::Fired(Box::new(original))]);
    assert!(restored.advance(at(33), &CalendarUtc).deliveries.is_empty());
    assert_eq!(restored.budget.handoffs, vec![at(32)]);
}
#[test]
fn budget_spares_operator_rules_after_forwarding_and_host_rechecks_current_conditions() {
    let mut r = rule();
    r.version.product = "pl".into();
    r.rule.interrupt = true;
    r.author = Author::Operator;
    r.live = true;
    let mut pl = evaluator(r, vec![row("operator", at(10), at(10), None)], at(9));
    let reports = pl.evaluator.reports(at(10), &CalendarUtc).unwrap();
    let mut host = HostPolicy::new(
        HostSettings::default(),
        InterruptBudget {
            per_hour: 0,
            handoffs: vec![],
        },
    );
    // The firing arrives before the producer confirmed it is up to date.
    let original = fired(&reports).remove(0);
    host.absorb([Report::Fired(Box::new(original.clone()))]);
    assert!(host.advance(at(10), &CalendarUtc).deliveries.is_empty());
    assert_eq!(host.pending_count(), 1);
    host.absorb([Report::Progress {
        product: "pl".into(),
        at: at(11),
        incomplete: Default::default(),
    }]);
    let got = host.advance(at(11), &CalendarUtc).deliveries;
    assert_eq!(got[0].decision, Decision::Show { send_away: false });
    assert_eq!(got[0].at, at(11));
    assert!(host.budget.handoffs.is_empty());
    let mut ended = original;
    ended.trigger.occurrence.period = "2".into();
    ended.id = ended.trigger.delivery_id();
    host.absorb([
        Report::Fired(Box::new(ended.clone())),
        Report::Cleared {
            delivery: ended.id.clone(),
            at: at(12),
        },
    ]);
    assert!(host.advance(at(12), &CalendarUtc).deliveries.is_empty());
    host.absorb([Report::Fired(Box::new(ended))]);
    assert!(host.advance(at(13), &CalendarUtc).deliveries.is_empty());
}
#[test]
fn rate_limit_counts_actual_handoffs_when_a_revision_lengthens_its_window() {
    let mut r = rule();
    r.rule.rate_limit = Some(RateLimit {
        count: 100,
        window: Duration(HOUR),
    });
    let mut source = evaluator(
        r.clone(),
        vec![
            row("a", at(10), at(10), None),
            row("b", at(12), at(12), None),
        ],
        at(9),
    );
    let reports = source.evaluator.reports(at(12), &CalendarUtc).unwrap();
    let intents = fired(&reports);
    let mut host = HostPolicy::default();
    host.absorb(
        reports
            .iter()
            .filter(|report| !matches!(report, Report::Fired(_)))
            .cloned(),
    );
    for intent in &intents {
        host.absorb([Report::Fired(Box::new(intent.clone()))]);
        assert_eq!(host.advance(intent.at, &CalendarUtc).deliveries.len(), 1);
    }
    let mut revised = intents[0].clone();
    revised.trigger.rule.revision = 2;
    revised.trigger.occurrence.id = "c".into();
    revised.id = revised.trigger.delivery_id();
    revised.rate_limit = Some(RateLimit {
        count: 2,
        window: Duration(24 * HOUR),
    });
    r.version.revision = 2;
    host.absorb([
        Report::Rule {
            version: r.version,
            enabled: true,
        },
        Report::Fired(Box::new(revised)),
    ]);
    let got = host.advance(at(13), &CalendarUtc).deliveries;
    assert_eq!(got[0].decision, passive(PassiveReason::RateLimit));
}
#[test]
fn retracted_current_state_firing_fires_again_on_the_next_tick_while_true() {
    let mut r = rule();
    r.live = true;
    let mut e = evaluator(r.clone(), vec![row("a", at(10), at(10), None)], at(9));
    let first = advance(&mut e, at(10));
    assert_eq!(first.len(), 1);
    let id = first[0].id().to_string();
    assert!(advance(&mut e, minute(10, 1)).is_empty());

    assert_eq!(
        e.evaluator.retract(&id, at(10)),
        Err(EvaluationError("retraction precedes checkpoint".into()))
    );

    // The retraction applies at its own instant, not at the checkpoint's.
    e.evaluator.retract(&id, minute(10, 2)).unwrap();
    let again = advance(&mut e, minute(10, 3));
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id(), id);
    assert_eq!(again[0].fired.at, minute(10, 2));
    assert_eq!(again[0].at, minute(10, 2));
    assert!(advance(&mut e, minute(10, 4)).is_empty());

    // Once the condition has ended, a retraction fires nothing.
    e.observe(evaluation(
        &r,
        minute(10, 5),
        vec![row("a", at(10), minute(10, 5), Some(minute(10, 5)))],
    ))
    .unwrap();
    advance(&mut e, minute(10, 5));
    e.evaluator.retract(&id, minute(10, 6)).unwrap();
    assert!(advance(&mut e, minute(10, 7)).is_empty());
    assert_eq!(e.pending_count(), 0);
}

#[test]
fn retracting_a_held_firing_drops_the_host_copy_and_it_is_held_again() {
    let mut r = rule();
    r.live = true;
    r.rule.interrupt = true;
    r.rule.quiet_hours = Some(quiet());
    let mut e = evaluator(r, vec![row("a", at(23), at(23), None)], at(22));
    assert!(advance(&mut e, at(23)).is_empty());
    let held = e.host.held().next().unwrap().fired.id.clone();

    e.evaluator.retract(&held, minute(23, 1)).unwrap();
    assert!(advance(&mut e, minute(23, 2)).is_empty());
    let again: Vec<_> = e.host.held().map(|held| held.fired.clone()).collect();
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, held);
    assert_eq!(again[0].at, minute(23, 1));
    let got = advance(&mut e, at(32));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id(), held);
    assert_eq!(got[0].at, at(32));
}

#[test]
fn retracted_edge_firing_fires_again_only_on_a_new_edge() {
    // A threshold firing is keyed by its crossing: still true is no new edge.
    let mut r = rule();
    r.rule.fires = Fires::OnThreshold {
        value: 10.,
        compare: Compare::Less,
    };
    let sample = |e: &mut TestEvaluator, time: i64, value: f64| {
        let mut input = evaluation(&r, time, vec![]);
        input.value = Some(value);
        e.observe(input).unwrap();
    };
    let mut e = evaluator(r.clone(), vec![], at(9));
    sample(&mut e, at(9), 20.);
    sample(&mut e, at(10), 5.);
    let crossing = advance(&mut e, at(10));
    assert_eq!(crossing.len(), 1);
    e.evaluator.retract(crossing[0].id(), at(10)).unwrap();
    sample(&mut e, at(11), 4.);
    assert!(advance(&mut e, at(11)).is_empty());
    sample(&mut e, at(12), 20.);
    sample(&mut e, at(13), 3.);
    let next = advance(&mut e, at(13));
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].at, at(13));
    assert_ne!(next[0].id(), crossing[0].id());

    // A new-row firing is keyed by its occurrence, which is its edge: while
    // the occurrence stays in the result it fires again.
    let mut e = evaluator(rule(), vec![row("a", at(10), at(10), None)], at(9));
    let first = advance(&mut e, at(10));
    e.evaluator.retract(first[0].id(), at(10)).unwrap();
    let again = advance(&mut e, at(11));
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id(), first[0].id());
}

#[test]
fn retraction_leaves_the_rules_other_deliveries_alone() {
    let mut r = rule();
    r.live = true;
    let mut e = evaluator(
        r.clone(),
        vec![
            row("a", at(10), at(10), None),
            row("b", at(10), at(10), None),
        ],
        at(9),
    );
    let first = advance(&mut e, at(10));
    assert_eq!(first.len(), 2);
    let id = |name: &str| {
        first
            .iter()
            .find(|d| d.fired.trigger.occurrence.id == name)
            .unwrap()
            .id()
            .to_string()
    };
    e.evaluator.retract(&id("a"), at(10)).unwrap();
    let again = advance(&mut e, at(11));
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id(), id("a"));

    // b is still open: its end is reported as cleared, once.
    e.observe(evaluation(
        &r,
        at(12),
        vec![
            row("a", at(10), at(10), None),
            row("b", at(10), at(12), Some(at(12))),
        ],
    ))
    .unwrap();
    let ended = e
        .evaluator
        .advance(at(12), &CalendarUtc, &mut e.host)
        .unwrap();
    assert!(ended.deliveries.is_empty());
    let cleared: Vec<_> = ended
        .outcomes
        .iter()
        .filter(|outcome| outcome.outcome == Outcome::Cleared)
        .map(|outcome| outcome.delivery.clone())
        .collect();
    assert_eq!(cleared, [id("b")]);
}
