use authored::{
    Arrival, Currency, History, OnStale, Placed, Plan, Refusal, Replay, RequestDigest, Revision,
    Target, Write, replay, write_id,
};
use serde_json::json;
use std::collections::BTreeSet;

fn arrival(on_stale: OnStale) -> Arrival {
    Arrival {
        author: "agent:claude-code:s1".into(),
        via: "mcp".into(),
        at: 1_700_000_000_000,
        on_stale,
    }
}

fn note(text: &str) -> Write<()> {
    Write {
        kind: "note".into(),
        key: None,
        value: Some(json!(text)),
        body: (),
        revises: Vec::new(),
        withdraws: Vec::new(),
        unique: None,
    }
}

fn revise(base: &Revision, text: &str) -> Write<()> {
    Write {
        revises: vec![base.clone()],
        ..note(text)
    }
}

fn withdrawal(target: Target) -> Write<()> {
    Write {
        kind: "withdrawal".into(),
        value: None,
        withdraws: vec![target],
        ..note("")
    }
}

/// Plans and commits one request; returns where each write went.
fn commit(
    history: &mut History,
    request: &str,
    on_stale: OnStale,
    writes: Vec<Write<()>>,
) -> Vec<Placed> {
    let mut plan = Plan::new(request, writes.len(), arrival(on_stale), history);
    for write in writes {
        plan.add(write).unwrap();
    }
    let planned = plan.finish().unwrap();
    let placed = planned.placed.clone();
    history.commit(planned);
    placed
}

fn written(placed: &Placed) -> Revision {
    match placed {
        Placed::Written { revision, .. } => revision.clone(),
        Placed::Already { .. } => panic!("expected a written record"),
    }
}

#[test]
fn request_atomic() {
    let mut history = History::new();
    let first = written(&commit(&mut history, "a", OnStale::Refuse, vec![note("one")])[0]);
    let second = written(
        &commit(
            &mut history,
            "b",
            OnStale::Refuse,
            vec![revise(&first, "two")],
        )[0],
    );
    let before = history.records().len();

    // The third write revises a base that is no longer current.
    let mut plan = Plan::new("c", 3, arrival(OnStale::Refuse), &mut history);
    plan.add(note("x")).unwrap();
    plan.add(revise(&second, "three")).unwrap();
    let refused = plan.add(revise(&first, "stale")).unwrap_err();
    assert_eq!(refused.write, 3);
    assert_eq!(
        refused.refusal,
        Refusal::Stale {
            base: first.clone(),
            heads: vec![Revision::new("a", "c.2")],
        }
    );
    // Nothing of the request can be committed.
    let finished = plan.finish().unwrap_err();
    assert_eq!(finished.refusal, Refusal::Abandoned(3));
    assert_eq!(history.records().len(), before);

    // A request with fewer writes than it declared is refused at finish.
    let mut plan: Plan<'_, _, ()> = Plan::new("d", 2, arrival(OnStale::Refuse), &mut history);
    plan.add(note("only")).unwrap();
    assert!(matches!(
        plan.finish().unwrap_err().refusal,
        Refusal::Count {
            declared: 2,
            written: 1
        }
    ));
    // And a write past the declared count is refused.
    let mut plan = Plan::new("e", 1, arrival(OnStale::Refuse), &mut history);
    plan.add(note("one")).unwrap();
    assert!(matches!(
        plan.add(note("two")).unwrap_err().refusal,
        Refusal::Count {
            declared: 1,
            written: 2
        }
    ));
}

#[test]
fn request_numbering() {
    assert_eq!(write_id("019a", 1, 1), "019a");
    assert_eq!(write_id("019a", 2, 3), "019a.2");

    // Later writes see earlier ones: replace = withdraw A, assert B, and B
    // revised again in the same request.
    let mut history = History::new();
    let a = written(&commit(&mut history, "a", OnStale::Refuse, vec![note("a")])[0]);
    let mut plan = Plan::new("r", 3, arrival(OnStale::Refuse), &mut history);
    let w = written(&plan.add(withdrawal(Target::Revision(a.clone()))).unwrap());
    let b = written(&plan.add(note("b")).unwrap());
    let b2 = written(&plan.add(revise(&b, "b, revised")).unwrap());
    assert_eq!(w, Revision::new("r.1", "r.1"));
    assert_eq!(b, Revision::new("r.2", "r.2"));
    assert_eq!(b2, Revision::new("r.2", "r.3"));
    let planned = plan.finish().unwrap();
    assert_eq!(
        planned.affected_roots,
        BTreeSet::from(["a", "r.1", "r.2"].map(String::from))
    );
    // Authorship comes from the arrival, the same on every record.
    for record in &planned.records {
        assert_eq!(record.header.author, "agent:claude-code:s1");
        assert_eq!(record.header.via, "mcp");
        assert_eq!(record.header.at, 1_700_000_000_000);
    }
    history.commit(planned);
    // A is withdrawn and B's latest revision is current, beside the
    // withdrawal record itself.
    assert_eq!(history.current().unwrap(), BTreeSet::from([w, b2]));
}

#[test]
fn request_idempotent() {
    #[derive(serde::Serialize)]
    struct Assert {
        kind: &'static str,
        value: &'static str,
    }
    let typed = RequestDigest::of(&[Assert {
        kind: "note",
        value: "x",
    }])
    .unwrap();
    // The same content built with the other key order hashes the same.
    let mut map = serde_json::Map::new();
    map.insert("value".into(), json!("x"));
    map.insert("kind".into(), json!("note"));
    let built = RequestDigest::of(&json!([map])).unwrap();
    assert_eq!(typed, built);
    let other = RequestDigest::of(&json!([{"kind": "note", "value": "y"}])).unwrap();
    assert_ne!(typed, other);

    assert_eq!(replay(None, &typed), Replay::New);
    assert_eq!(replay(Some(&typed), &built), Replay::Repeat);
    assert_eq!(replay(Some(&typed), &other), Replay::Conflict);
}

#[test]
fn base_stale_refused() {
    let mut history = History::new();
    let first = written(&commit(&mut history, "a", OnStale::Refuse, vec![note("one")])[0]);
    commit(
        &mut history,
        "b",
        OnStale::Refuse,
        vec![revise(&first, "two")],
    );

    // A command on the old base is refused, naming the current head.
    let mut plan = Plan::new("c", 1, arrival(OnStale::Refuse), &mut history);
    let refused = plan.add(revise(&first, "late")).unwrap_err();
    assert_eq!(
        refused.refusal,
        Refusal::Stale {
            base: first.clone(),
            heads: vec![Revision::new("a", "b")],
        }
    );

    // Markup or an import forks instead: two heads, a visible conflict.
    commit(
        &mut history,
        "d",
        OnStale::Fork,
        vec![revise(&first, "late")],
    );
    let state = Currency::new(&mut history).root_state("a").unwrap();
    assert!(state.conflict());
    assert_eq!(
        state.heads(),
        vec![Revision::new("a", "b"), Revision::new("a", "d")]
    );

    // One revision of both heads resolves it.
    let resolve = Write {
        revises: state.heads(),
        ..note("both")
    };
    commit(&mut history, "e", OnStale::Refuse, vec![resolve]);
    assert_eq!(
        Currency::new(&mut history).heads("a").unwrap(),
        vec![Revision::new("a", "e")]
    );
}

#[test]
fn bases_and_targets_must_exist_and_share_a_root() {
    let mut history = History::new();
    let a = written(&commit(&mut history, "a", OnStale::Refuse, vec![note("a")])[0]);
    let b = written(&commit(&mut history, "b", OnStale::Refuse, vec![note("b")])[0]);

    let mut plan = Plan::new("c", 1, arrival(OnStale::Refuse), &mut history);
    let both = Write {
        revises: vec![a.clone(), b.clone()],
        ..note("x")
    };
    assert_eq!(
        plan.add(both).unwrap_err().refusal,
        Refusal::BasesSpanRoots(vec![a.clone(), b.clone()])
    );

    let missing = Revision::new("a", "nope");
    let mut plan = Plan::new("d", 1, arrival(OnStale::Fork), &mut history);
    assert_eq!(
        plan.add(revise(&missing, "x")).unwrap_err().refusal,
        Refusal::UnknownBase(missing)
    );

    let mut plan = Plan::new("e", 1, arrival(OnStale::Refuse), &mut history);
    assert_eq!(
        plan.add(withdrawal(Target::Root("nope".into())))
            .unwrap_err()
            .refusal,
        Refusal::UnknownTarget(Target::Root("nope".into()))
    );
}
