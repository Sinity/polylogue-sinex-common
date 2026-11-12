use authored::{Currency, CurrencyError, Header, History, Revision, Target};
use std::collections::BTreeSet;

fn record(root: &str, rev: &str, revises: &[&Revision], withdraws: &[Target]) -> Header {
    Header {
        root: root.to_owned(),
        rev: rev.to_owned(),
        kind: if withdraws.is_empty() {
            "note"
        } else {
            "withdrawal"
        }
        .to_owned(),
        key: None,
        value: None,
        author: "operator".to_owned(),
        via: "cli".to_owned(),
        at: Some(0),
        revises: revises.iter().map(|r| (*r).clone()).collect(),
        withdraws: withdraws.iter().cloned().collect(),
    }
}

fn revision(root: &str, rev: &str) -> Revision {
    Revision::new(root, rev)
}

/// The current revisions of record `x`; withdrawals are records too, and are
/// current themselves.
fn current(history: &mut History) -> BTreeSet<Revision> {
    Currency::new(history)
        .heads("x")
        .unwrap()
        .into_iter()
        .collect()
}

/// R1←R2←R3 of root `x`.
fn chain() -> (History, Revision, Revision, Revision) {
    let (r1, r2, r3) = (revision("x", "x"), revision("x", "r2"), revision("x", "r3"));
    let mut history = History::new();
    history.push(record("x", "x", &[], &[]), vec![]);
    history.push(record("x", "r2", &[&r1], &[]), vec![]);
    history.push(record("x", "r3", &[&r2], &[]), vec![]);
    (history, r1, r2, r3)
}

fn withdraw(history: &mut History, id: &str, target: Target) -> Revision {
    history.push(record(id, id, &[], &[target]), vec![]);
    revision(id, id)
}

#[test]
fn currency_middle_withdrawal() {
    // Withdrawing the middle revision leaves the newest current; the rule as
    // first written ("no non-withdrawn revision revises R") made R1 current
    // again and reported a false conflict.
    let (mut history, r1, r2, r3) = chain();
    withdraw(&mut history, "w2", Target::Revision(r2.clone()));
    let mut currency = Currency::new(&mut history);
    assert!(currency.current(&r3).unwrap());
    assert!(!currency.current(&r1).unwrap());
    assert!(!currency.current(&r2).unwrap());
    let state = currency.root_state("x").unwrap();
    assert_eq!(state.heads(), vec![r3.clone()]);
    assert!(!state.conflict());

    // Withdrawing the newest makes the one below it current.
    let (mut history, r1, r2, r3) = chain();
    withdraw(&mut history, "w3", Target::Revision(r3.clone()));
    assert_eq!(current(&mut history), BTreeSet::from([r2.clone()]));
    // Withdrawing both upper revisions reaches R1.
    withdraw(&mut history, "w2", Target::Revision(r2.clone()));
    assert_eq!(current(&mut history), BTreeSet::from([r1.clone()]));

    // Withdrawing the withdrawal of R2 restores the chain: R3 current.
    let (mut history, _, r2, r3) = chain();
    let w2 = withdraw(&mut history, "w2", Target::Revision(r2.clone()));
    withdraw(&mut history, "unw2", Target::Revision(w2));
    assert_eq!(current(&mut history), BTreeSet::from([r3]));
}

#[test]
fn withdrawing_a_root_withdraws_every_revision() {
    let (mut history, ..) = chain();
    let withdrawal = withdraw(&mut history, "w", Target::Root("x".into()));
    assert!(Currency::new(&mut history).heads("x").unwrap().is_empty());
    // Withdrawing that withdrawal by its root restores the record.
    withdraw(&mut history, "u", Target::Root(withdrawal.root));
    assert_eq!(
        Currency::new(&mut history).heads("x").unwrap(),
        vec![revision("x", "r3")]
    );
}

#[test]
fn two_revisions_of_one_base_conflict_until_one_revises_both() {
    let r1 = revision("x", "x");
    let (left, right) = (revision("x", "left"), revision("x", "right"));
    let mut history = History::new();
    history.push(record("x", "x", &[], &[]), vec![]);
    history.push(record("x", "left", &[&r1], &[]), vec![]);
    history.push(record("x", "right", &[&r1], &[]), vec![]);
    let state = Currency::new(&mut history).root_state("x").unwrap();
    assert!(state.conflict());
    assert_eq!(state.heads(), vec![left.clone(), right.clone()]);
    history.push(record("x", "merged", &[&left, &right], &[]), vec![]);
    assert_eq!(
        Currency::new(&mut history).heads("x").unwrap(),
        vec![revision("x", "merged")]
    );
    // Withdrawing one side of a resolved conflict leaves the resolution.
    withdraw(&mut history, "w", Target::Revision(left));
    assert_eq!(
        Currency::new(&mut history).heads("x").unwrap(),
        vec![revision("x", "merged")]
    );
}

#[test]
fn currency_cycle_is_an_error() {
    // Two imported withdrawals naming each other have no meaning.
    let (a, b) = (revision("a", "a"), revision("b", "b"));
    let mut history = History::new();
    history.push(
        record("a", "a", &[], &[Target::Revision(b.clone())]),
        vec![],
    );
    history.push(
        record("b", "b", &[], &[Target::Revision(a.clone())]),
        vec![],
    );
    assert!(matches!(
        Currency::new(&mut history).withdrawn(&a),
        Err(CurrencyError::Cycle(_))
    ));
    assert!(matches!(
        Currency::new(&mut history).current(&b),
        Err(CurrencyError::Cycle(_))
    ));
}

#[test]
fn withdrawal_chains_have_no_depth_limit() {
    // w0 is withdrawn by w1, w1 by w2, … ; the last is active, so they
    // alternate down the chain.
    let mut history = History::new();
    history.push(record("x", "x", &[], &[]), vec![]);
    let mut previous = Target::Revision(revision("x", "x"));
    let length = 20_000;
    for n in 0..length {
        let id = format!("w{n}");
        history.push(record(&id, &id, &[], &[previous]), vec![]);
        previous = Target::Revision(revision(&id, &id));
    }
    let mut currency = Currency::new(&mut history);
    assert!(!currency.withdrawn(&revision("w19999", "w19999")).unwrap());
    assert!(currency.withdrawn(&revision("w19998", "w19998")).unwrap());
    // An even number of withdrawals above x: the one directly on x is
    // withdrawn, so x is current.
    assert_eq!(
        currency.current(&revision("x", "x")).unwrap(),
        length % 2 == 0
    );
}

#[test]
fn affected_roots_follow_withdrawals_of_withdrawals() {
    let mut history = History::new();
    history.push(record("x", "x", &[], &[]), vec![]);
    history.push(record("y", "y", &[], &[]), vec![]);
    history.push(
        record("w", "w", &[], &[Target::Revision(revision("x", "x"))]),
        vec![],
    );
    history.push(record("v", "v", &[], &[Target::Root("w".into())]), vec![]);
    let restore = record("u", "u", &[], &[Target::Revision(revision("v", "v"))]);
    let roots = authored::affected_roots(&mut history, &restore).unwrap();
    assert_eq!(
        roots,
        BTreeSet::from(["u", "v", "w", "x"].map(String::from))
    );
}

#[test]
fn unknown_authored_time_is_distinct_from_a_recorded_epoch_without_changing_identity() {
    let mut known = record("x", "x", &[], &[]);
    let revision = known.revision();
    assert_eq!(known.revision(), Revision::new("x", "x"));
    // This is the complete predecessor encoding with its scalar timestamp.
    assert_eq!(
        serde_json::to_string(&known).unwrap(),
        r#"{"root":"x","rev":"x","kind":"note","key":null,"value":null,"author":"operator","via":"cli","at":0,"revises":[],"withdraws":[]}"#
    );
    let encoded = serde_json::to_value(&known).unwrap();
    assert_eq!(encoded["at"], serde_json::json!(0));
    assert_eq!(
        serde_json::from_value::<Header>(encoded.clone())
            .unwrap()
            .at,
        Some(0)
    );
    known.at = None;
    let mut unknown = serde_json::to_value(&known).unwrap();
    assert!(unknown["at"].is_null());
    let restored: Header = serde_json::from_value(unknown.clone()).unwrap();
    assert_eq!(restored.at, None);
    assert_eq!(restored.revision(), revision);
    unknown.as_object_mut().unwrap().remove("at");
    assert_eq!(serde_json::from_value::<Header>(unknown).unwrap().at, None);
    let mut malformed = encoded;
    malformed["at"] = serde_json::json!("unrecorded");
    assert!(serde_json::from_value::<Header>(malformed).is_err());
}
