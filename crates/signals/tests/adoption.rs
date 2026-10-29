//! Adoption laws use independently chosen civil instants and expected handoffs.
use signals::*;

struct Utc;
const MINUTE: i64 = 60_000;
const DAY: i64 = 24 * HOUR;
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
fn hour(hour: i64) -> i64 {
    hour * HOUR
}
fn rule() -> ActiveRule {
    ActiveRule {
        version: RuleVersion {
            product: "product".into(),
            id: "rule".into(),
            revision: 1,
        },
        rule: serde_json::from_value(serde_json::json!({"name":"rule", "query":"records"}))
            .unwrap(),
        author: Author::Operator,
        via: "test".into(),
        live: false,
    }
}
fn observe(e: &mut Evaluator, r: &ActiveRule, at: i64, ids: &[&str]) {
    e.observe(Evaluation {
        rule: r.version.clone(),
        at,
        complete: true,
        value: None,
        rows: ids
            .iter()
            .map(|id| Row {
                occurrence: Occurrence {
                    id: (*id).into(),
                    episode: "one".into(),
                },
                reference: format!("record:{id}"),
                observed: at,
                start: at,
                end: None,
                origin: Origin::Source,
                evidence: serde_json::json!({"observed":at}),
            })
            .collect(),
    })
    .unwrap();
}
fn advance(e: &mut Evaluator, at: i64) -> Vec<FiredIntent> {
    e.advance_in_mode(at, &Utc, EvaluationMode::Forward)
        .unwrap()
        .intents
}

#[test]
fn late_scheduled_adoption_cannot_fire_at_a_pre_adoption_deadline() {
    let mut r = rule();
    r.rule.fires = Fires::At { minute: 10 * 60 };
    let mut e = Evaluator::new(hour(9), vec![], Policy::default(), &Utc).unwrap();
    e.replace_rule(r.clone(), hour(12), &Utc).unwrap();
    assert_eq!(e.now(), hour(9));
    assert_eq!(e.next_due(), Some(hour(34)));
    observe(&mut e, &r, hour(12), &["present"]);
    assert!(advance(&mut e, hour(13)).is_empty());
    let got = advance(&mut e, hour(34));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].delivery.at, hour(34));
    assert_eq!(got[0].delivery.trigger.rule, r.version);
}

#[test]
fn scheduled_adoption_at_the_exact_boundary_is_due_once() {
    let mut r = rule();
    r.rule.fires = Fires::At { minute: 10 * 60 };
    let mut e = Evaluator::new(hour(9), vec![], Policy::default(), &Utc).unwrap();
    e.replace_rule(r.clone(), hour(10), &Utc).unwrap();
    observe(&mut e, &r, hour(10), &["present"]);
    assert_eq!(e.next_due(), Some(hour(10)));
    let got = advance(&mut e, hour(10));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].delivery.at, hour(10));
    assert!(advance(&mut e, hour(10)).is_empty());
}

#[test]
fn delayed_revision_cancels_old_work_without_ticking_the_old_revision() {
    let mut r = rule();
    r.rule.duration = Duration(HOUR);
    let mut e = Evaluator::new(hour(9), vec![r.clone()], Policy::default(), &Utc).unwrap();
    observe(&mut e, &r, hour(9), &["before-edit"]);
    assert!(advance(&mut e, hour(9)).is_empty());
    assert_eq!(e.pending_count(), 1);
    r.version.revision = 2;
    e.replace_rule(r.clone(), hour(12), &Utc).unwrap();
    assert_eq!(e.now(), hour(9));
    assert_eq!(e.pending_count(), 0);
    observe(&mut e, &r, hour(12), &["before-edit"]);
    assert!(advance(&mut e, hour(12)).is_empty());
    observe(&mut e, &r, hour(13), &["after-edit"]);
    assert!(advance(&mut e, hour(13)).is_empty());
    let got = advance(&mut e, hour(14));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].delivery.trigger.rule.revision, 2);
    assert_eq!(got[0].delivery.trigger.occurrence.id, "after-edit");
}

#[test]
fn identical_revision_retry_preserves_the_change_baseline() {
    let mut r = rule();
    r.rule.fires = Fires::OnChange {
        what: ChangeKind::Membership,
        k: None,
    };
    let mut e = Evaluator::new(hour(9), vec![], Policy::default(), &Utc).unwrap();
    e.replace_rule(r.clone(), hour(12), &Utc).unwrap();
    observe(&mut e, &r, hour(12), &["baseline"]);
    assert!(advance(&mut e, hour(12)).is_empty());
    observe(&mut e, &r, hour(13), &["changed"]);
    let mut expected = e.clone();
    let before = serde_json::to_vec(&e).unwrap();
    e.replace_rule(r, hour(13), &Utc).unwrap();
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    let got = advance(&mut e, hour(13));
    assert_eq!(got.len(), 1);
    assert_eq!(got, advance(&mut expected, hour(13)));
}

#[test]
fn identical_retry_preserves_a_quiet_held_occurrence_and_its_original_basis() {
    let mut r = rule();
    r.rule.interrupt = true;
    let mut e = Evaluator::new(hour(21), vec![r.clone()], Policy::default(), &Utc).unwrap();
    let mut host = HostPolicy::new(
        InterruptBudget::default(),
        Some(QuietHours {
            from: 22 * 60,
            to: 8 * 60,
        }),
    );
    observe(&mut e, &r, hour(23), &["held"]);
    assert!(e.advance(hour(24), &Utc, &mut host).unwrap().is_empty());
    let held = host.pending_intents().next().unwrap().clone();
    let host_before = serde_json::to_vec(&host).unwrap();
    let evaluator_before = serde_json::to_vec(&e).unwrap();
    e.replace_rule(r, hour(21), &Utc).unwrap();
    assert_eq!(serde_json::to_vec(&e).unwrap(), evaluator_before);
    assert_eq!(serde_json::to_vec(&host).unwrap(), host_before);
    let delivered = e.advance(hour(32), &Utc, &mut host).unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].id, held.delivery.id);
    assert_eq!(delivered[0].basis, held.delivery.basis);
    assert_eq!(delivered[0].matched, hour(23));
}

#[test]
fn withdrawal_before_catchup_cancels_the_adopted_schedule() {
    let mut r = rule();
    r.rule.fires = Fires::At { minute: 10 * 60 };
    let mut e = Evaluator::new(hour(9), vec![], Policy::default(), &Utc).unwrap();
    e.replace_rule(r.clone(), hour(12), &Utc).unwrap();
    observe(&mut e, &r, hour(12), &["present"]);
    e.withdraw(&r.version);
    assert!(advance(&mut e, hour(34)).is_empty());
    assert_eq!(e.pending_count(), 0);
}

#[test]
fn invalid_adoption_or_pre_adoption_observation_has_no_partial_mutation() {
    let mut r = rule();
    let mut e = Evaluator::new(hour(12), vec![], Policy::default(), &Utc).unwrap();
    let before = serde_json::to_vec(&e).unwrap();
    assert!(e.replace_rule(r.clone(), hour(11), &Utc).is_err());
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    e.replace_rule(r.clone(), hour(14), &Utc).unwrap();
    let before = serde_json::to_vec(&e).unwrap();
    assert!(
        e.observe(Evaluation {
            rule: r.version.clone(),
            at: hour(13),
            complete: true,
            rows: vec![],
            value: None
        })
        .is_err()
    );
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    r.rule.query = "different".into();
    assert!(e.replace_rule(r, hour(14), &Utc).is_err());
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
}

#[test]
fn queued_pre_adoption_observation_refuses_adoption_without_mutation() {
    let r = rule();
    let mut e = Evaluator::new(hour(9), vec![], Policy::default(), &Utc).unwrap();
    observe(&mut e, &r, hour(10), &["before-adoption"]);
    let before = serde_json::to_vec(&e).unwrap();
    assert!(e.replace_rule(r, hour(12), &Utc).is_err());
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
}

#[test]
fn frozen_retry_after_checkpoint_advance_retains_the_original_baseline() {
    let mut r = rule();
    r.rule.fires = Fires::OnChange {
        what: ChangeKind::Membership,
        k: None,
    };
    let mut e = Evaluator::new(hour(9), vec![], Policy::default(), &Utc).unwrap();
    e.replace_rule(r.clone(), hour(12), &Utc).unwrap();
    observe(&mut e, &r, hour(12), &["baseline"]);
    assert!(advance(&mut e, hour(12)).is_empty());
    observe(&mut e, &r, hour(13), &["changed"]);
    assert_eq!(advance(&mut e, hour(13)).len(), 1);
    let before = serde_json::to_vec(&e).unwrap();
    // This is the original frozen adoption, retried after a newer checkpoint.
    e.replace_rule(r.clone(), hour(12), &Utc).unwrap();
    assert_eq!(serde_json::to_vec(&e).unwrap(), before);
    observe(&mut e, &r, hour(14), &["again"]);
    let got = advance(&mut e, hour(14));
    assert_eq!(got.len(), 1);
    let change = got[0].delivery.change.as_ref().unwrap();
    assert_eq!(
        change
            .before
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["changed"]
    );
    assert_eq!(
        change
            .after
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["again"]
    );
}
