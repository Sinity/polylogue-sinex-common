//! `wait` against `follow`, `backtest` against the live evaluator, and
//! rebuilding a lost checkpoint.
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
fn rule(name: &str) -> ActiveRule {
    ActiveRule {
        version: RuleVersion {
            product: "polylogue".into(),
            id: name.into(),
            revision: 1,
        },
        rule: serde_json::from_value(serde_json::json!({"name": name, "query": name})).unwrap(),
        author: Author::Agent("codex".into()),
        via: "test".into(),
        live: false,
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
fn evaluation(rule: &ActiveRule, time: i64, rows: Vec<Row>) -> Evaluation {
    Evaluation {
        rule: rule.version.clone(),
        at: time,
        complete: true,
        rows,
        value: None,
    }
}

#[test]
fn sig_wait_is_follow() {
    let mut watched = rule("index up to date");
    watched.rule.duration = Duration(5 * MINUTE);
    let history = [
        evaluation(&watched, at(10, 0), vec![]),
        evaluation(
            &watched,
            at(10, 3),
            vec![row("a", at(10, 3), Some(at(10, 6)))],
        ),
        evaluation(&watched, at(10, 10), vec![row("b", at(10, 10), None)]),
        evaluation(&watched, at(10, 20), vec![]),
    ];
    let mut follow = Follow::new(watched.clone(), at(10, 0), &Utc).unwrap();
    for evaluation in &history {
        follow.observe(evaluation.clone()).unwrap();
    }
    let messages = follow.advance(at(11, 0), &Utc).unwrap();
    // `a` stopped matching before its five minutes; `b` held them.
    assert_eq!(messages[0].trigger.occurrence.id, "b");
    assert_eq!(messages[0].at, at(10, 15));
    for stop in [at(10, 5), at(10, 14), at(10, 15), at(10, 30)] {
        let mut wait = Wait::new(
            watched.clone(),
            WaitUntil::Rows,
            at(10, 0),
            Duration(HOUR),
            &Utc,
        )
        .unwrap();
        for evaluation in &history {
            wait.observe(evaluation.clone()).unwrap();
        }
        let first = wait.advance(stop, &Utc).unwrap();
        if stop < messages[0].at {
            assert_eq!(first, None);
            let due = if stop == at(10, 5) {
                at(10, 6)
            } else {
                at(10, 15)
            };
            assert_eq!(wait.next_due(), due);
        } else {
            assert_eq!(
                first,
                Some(WaitEnd::Fired {
                    fired: Box::new(messages[0].clone())
                })
            );
        }
    }
    // A condition already true when the wait starts returns at once.
    let mut now = Wait::new(
        rule("ready"),
        WaitUntil::Rows,
        at(10, 0),
        Duration(MINUTE),
        &Utc,
    )
    .unwrap();
    now.observe(evaluation(
        &rule("ready"),
        at(10, 0),
        vec![row("x", at(9, 0), None)],
    ))
    .unwrap();
    assert!(matches!(
        now.advance(at(10, 0), &Utc).unwrap(),
        Some(WaitEnd::Fired { .. })
    ));
    let mut empty = Wait::new(
        watched.clone(),
        WaitUntil::Empty,
        at(10, 0),
        Duration(HOUR),
        &Utc,
    )
    .unwrap();
    for evaluation in history.iter().skip(1) {
        empty.observe(evaluation.clone()).unwrap();
    }
    assert_eq!(
        empty.advance(at(11, 0), &Utc).unwrap(),
        Some(WaitEnd::Empty { at: at(10, 20) })
    );
    let mut timeout = Wait::new(
        watched.clone(),
        WaitUntil::Rows,
        at(10, 0),
        Duration(MINUTE),
        &Utc,
    )
    .unwrap();
    timeout.observe(history[0].clone()).unwrap();
    assert_eq!(
        timeout.advance(at(11, 0), &Utc).unwrap(),
        Some(WaitEnd::Timeout {
            at: at(10, 1),
            last: Some(Box::new(history[0].clone())),
        })
    );
}

#[test]
fn sig_backtest_equals_live() {
    let mut green = rule("build green");
    green.rule.interrupt = true;
    let mut summaries = rule("summaries");
    summaries.rule.fires = Fires::OnChange {
        what: ChangeKind::Membership,
        k: None,
    };
    let rules = vec![green.clone(), summaries.clone()];
    let mut history = History::default();
    for day in 0..7 {
        let base = day * DAY;
        history
            .observations
            .push(evaluation(&green, base + at(8, 0), vec![]));
        history
            .observations
            .push(evaluation(&summaries, base + at(8, 0), vec![]));
        for (n, minute) in [(0, 15), (1, 40), (2, 55)] {
            let rows: Vec<_> = (0..=n)
                .map(|k| row(&format!("{day}-{k}"), base + at(9, k * 10), None))
                .collect();
            history
                .observations
                .push(evaluation(&green, base + at(9, minute), rows.clone()));
            history
                .observations
                .push(evaluation(&summaries, base + at(9, minute), rows));
        }
        history
            .attention
            .push((base + at(9, 0), AttentionInput::DeepWork { active: true }));
        history
            .attention
            .push((base + at(9, 50), AttentionInput::Breakpoint));
        history.attention.push((
            base + at(12, 0),
            AttentionInput::Presence {
                presence: Presence::Away,
            },
        ));
        history.attention.push((
            base + at(13, 0),
            AttentionInput::Presence {
                presence: Presence::Present,
            },
        ));
    }
    history.observations.sort_by_key(|evaluation| evaluation.at);
    let settings = HostSettings {
        quiet_hours: Some(QuietHours {
            from: 22 * 60,
            to: 8 * 60,
        }),
        ..HostSettings::default()
    };
    let stop = 7 * DAY;
    let replayed = backtest(rules.clone(), &history, 0, stop, settings.clone(), 2, &Utc).unwrap();
    assert!(
        replayed
            .deliveries
            .iter()
            .any(|d| d.decision == Decision::Show { send_away: false })
    );
    assert!(replayed.deliveries.iter().any(|d| !d.held.is_empty()));

    // Live: observations arrive as they happen, and the daemon advances at
    // its own irregular instants.
    let mut evaluator = Evaluator::new(0, rules, &Utc).unwrap();
    let mut host = HostPolicy::new(
        settings,
        InterruptBudget {
            per_hour: 2,
            handoffs: vec![],
        },
    );
    let mut live = Handoffs::default();
    let mut observations = history.observations.iter().peekable();
    let mut facts = history.attention.iter().peekable();
    let mut clock = 0;
    while clock < stop {
        clock = (clock + 37 * MINUTE).min(stop);
        while let Some(evaluation) = observations.next_if(|e| e.at <= clock) {
            evaluator.observe(evaluation.clone()).unwrap();
        }
        while let Some((time, input)) = facts.next_if(|(time, _)| *time <= clock) {
            host.observe(*time, input.clone()).unwrap();
        }
        // An observation arriving at `clock` must not precede the checkpoint.
        let checkpoint = observations.peek().map_or(clock, |next| next.at.min(clock));
        let checkpoint = facts
            .peek()
            .map_or(checkpoint, |(time, _)| (*time).min(checkpoint));
        live.extend(evaluator.advance(checkpoint, &Utc, &mut host).unwrap());
    }
    live.extend(evaluator.advance(stop, &Utc, &mut host).unwrap());
    assert_eq!(live, replayed);
}

#[test]
fn sig_checkpoint_loss_never_refires() {
    let mut watched = rule("failures");
    watched.rule.fires = Fires::OnChange {
        what: ChangeKind::Membership,
        k: None,
    };
    let new_rows = rule("new rows");
    let mut threshold = rule("load");
    threshold.rule.fires = Fires::OnThreshold {
        value: 10.,
        compare: Compare::Greater,
    };
    let rules = vec![watched.clone(), new_rows.clone(), threshold.clone()];
    let mut observations = Vec::new();
    for (n, minute) in [0, 10, 20, 30].into_iter().enumerate() {
        let rows: Vec<_> = (0..n)
            .map(|k| row(&k.to_string(), at(9, 0), None))
            .collect();
        observations.push(evaluation(&watched, at(9, minute), rows.clone()));
        observations.push(evaluation(&new_rows, at(9, minute), rows));
        let mut sample = evaluation(&threshold, at(9, minute), vec![]);
        sample.value = Some([1., 20., 2., 30.][n]);
        observations.push(sample);
    }
    let mut live = Evaluator::new(at(9, 0), rules.clone(), &Utc).unwrap();
    for evaluation in &observations {
        live.observe(evaluation.clone()).unwrap();
    }
    let delivered = live
        .advance(at(9, 15), &Utc, &mut HostPolicy::default())
        .unwrap()
        .deliveries;
    assert!(!delivered.is_empty());
    let triggers: Vec<_> = delivered.iter().map(|d| d.fired.trigger.clone()).collect();
    let ids: Vec<_> = delivered.iter().map(|d| d.id().to_string()).collect();

    // The checkpoint is lost; rebuild from the rules, the stored deliveries
    // and the stored observations.
    let mut rebuilt = Evaluator::rebuild(at(9, 0), rules.clone(), triggers.clone(), &Utc).unwrap();
    for evaluation in &observations {
        rebuilt.observe(evaluation.clone()).unwrap();
    }
    let again = rebuilt
        .advance(at(10, 0), &Utc, &mut HostPolicy::default())
        .unwrap();
    assert!(
        again
            .deliveries
            .iter()
            .all(|d| !ids.contains(&d.id().to_string()))
    );
    assert!(!again.deliveries.is_empty());

    // Rebuilt from a later point, a threshold already above its value cannot
    // tell a missed crossing from its baseline, and says so.
    let mut late = Evaluator::rebuild(at(9, 20), rules, triggers, &Utc).unwrap();
    for evaluation in observations.iter().filter(|e| e.at >= at(9, 30)) {
        late.observe(evaluation.clone()).unwrap();
    }
    let reported = late
        .advance(at(10, 0), &Utc, &mut HostPolicy::default())
        .unwrap();
    assert_eq!(reported.rearmed_late, vec![threshold.version]);
    assert!(
        reported
            .deliveries
            .iter()
            .all(|d| !ids.contains(&d.id().to_string()))
    );
}
