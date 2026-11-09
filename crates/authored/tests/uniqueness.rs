use authored::{
    Arrival, Currency, History, OnStale, Part, Placed, Plan, Refusal, Revision, SlotValue,
    UniqueSlot, Uniqueness, WhenHeld, Write,
};
use serde_json::json;

const TITLE: Uniqueness = Uniqueness::new(&["about"], WhenHeld::Revise);
const TAG: Uniqueness = Uniqueness::new(&["about", "key", "value"], WhenHeld::Already);
const DUPLICATE: Uniqueness = Uniqueness::new(&["pair", "author"], WhenHeld::Revise);

fn arrival(on_stale: OnStale) -> Arrival {
    Arrival {
        author: "operator".into(),
        via: "cli".into(),
        at: 0,
        on_stale,
    }
}

fn title(about: &[&str], text: &str) -> Write<()> {
    let about: Vec<String> = about.iter().map(|a| a.to_string()).collect();
    Write {
        kind: "title".into(),
        key: None,
        value: Some(json!(text)),
        body: (),
        revises: Vec::new(),
        withdraws: Vec::new(),
        unique: Some(TITLE.unique("title", |_| Part::Each(about.clone()))),
    }
}

fn tag(about: &str, name: &str) -> Write<()> {
    Write {
        kind: "tag".into(),
        key: Some(name.into()),
        value: None,
        body: (),
        revises: Vec::new(),
        withdraws: Vec::new(),
        unique: Some(TAG.unique("tag", |component| match component {
            "about" => Part::One(about.into()),
            "key" => Part::One(name.into()),
            _ => Part::Absent,
        })),
    }
}

fn place(history: &mut History, request: &str, on_stale: OnStale, write: Write<()>) -> Placed {
    let mut plan = Plan::new(request, 1, arrival(on_stale), history);
    let placed = plan.add(write).unwrap();
    let planned = plan.finish().unwrap();
    history.commit(planned);
    placed
}

#[test]
fn uniqueness_slots() {
    // Each value of an `Each` part is its own slot; a `Set` is one unordered
    // value; the kind is always part of the key.
    let unique = DUPLICATE.unique("duplicate", |component| match component {
        "pair" => Part::Set(vec!["item:b".into(), "item:a".into()]),
        _ => Part::One("operator".into()),
    });
    let swapped = DUPLICATE.unique("duplicate", |component| match component {
        "pair" => Part::Set(vec!["item:a".into(), "item:b".into()]),
        _ => Part::One("operator".into()),
    });
    assert_eq!(unique, swapped);
    assert_eq!(
        unique.slots,
        vec![UniqueSlot {
            kind: "duplicate".into(),
            parts: vec![
                (
                    "pair".into(),
                    SlotValue::Set(vec!["item:a".into(), "item:b".into()])
                ),
                ("author".into(), SlotValue::One("operator".into())),
            ],
        }]
    );
    let two = TITLE.unique("title", |_| Part::Each(vec!["s:2".into(), "s:1".into()]));
    assert_eq!(two.slots.len(), 2);
    let none = TITLE.unique("title", |_| Part::Each(Vec::new()));
    assert_eq!(none.slots[0].parts[0].1, SlotValue::Absent);
    // Another kind with the same values is another slot.
    assert_ne!(
        TITLE.unique("title", |_| Part::One("s:1".into())).slots,
        TITLE.unique("outcome", |_| Part::One("s:1".into())).slots
    );

    // Digests are length-prefixed: regrouping the same characters differs.
    let digest = |values: &[&str]| {
        UniqueSlot {
            kind: "k".into(),
            parts: values
                .iter()
                .map(|v| ("c".to_string(), SlotValue::One(v.to_string())))
                .collect(),
        }
        .digest()
    };
    assert_ne!(digest(&["ab", "c"]), digest(&["a", "bc"]));
    assert_eq!(digest(&["ab", "c"]), digest(&["ab", "c"]));
}

#[test]
fn uniqueness_second_write_revises() {
    let mut history = History::new();
    let first = place(&mut history, "a", OnStale::Refuse, title(&["s:1"], "One"));
    assert_eq!(
        first,
        Placed::Written {
            revision: Revision::first("a"),
            revised: vec![]
        }
    );
    let second = place(&mut history, "b", OnStale::Refuse, title(&["s:1"], "Two"));
    assert_eq!(
        second,
        Placed::Written {
            revision: Revision::new("a", "b"),
            revised: vec![Revision::first("a")]
        }
    );
    // Another target is another slot.
    let other = place(&mut history, "c", OnStale::Refuse, title(&["s:2"], "Other"));
    assert_eq!(
        other,
        Placed::Written {
            revision: Revision::first("c"),
            revised: vec![]
        }
    );
}

#[test]
fn uniqueness_already() {
    let mut history = History::new();
    place(&mut history, "a", OnStale::Refuse, tag("s:1", "star"));
    let again = place(&mut history, "b", OnStale::Refuse, tag("s:1", "star"));
    assert_eq!(
        again,
        Placed::Already {
            heads: vec![Revision::first("a")]
        }
    );
    assert_eq!(history.records().len(), 1);
    // Once the tag is withdrawn, the slot is free again.
    let withdrawal = Write {
        kind: "withdrawal".into(),
        withdraws: vec![authored::Target::Root("a".into())],
        unique: None,
        ..tag("s:1", "star")
    };
    place(&mut history, "c", OnStale::Refuse, withdrawal);
    let fresh = place(&mut history, "d", OnStale::Refuse, tag("s:1", "star"));
    assert_eq!(
        fresh,
        Placed::Written {
            revision: Revision::first("d"),
            revised: vec![]
        }
    );
}

#[test]
fn uniqueness_revises_every_head_of_a_conflict() {
    let mut history = History::new();
    place(&mut history, "a", OnStale::Refuse, title(&["s:1"], "One"));
    // An import forks the title from its first revision twice.
    for id in ["b", "c"] {
        let fork = Write {
            revises: vec![Revision::first("a")],
            ..title(&["s:1"], id)
        };
        place(&mut history, id, OnStale::Fork, fork);
    }
    assert!(
        Currency::new(&mut history)
            .root_state("a")
            .unwrap()
            .conflict()
    );
    let resolved = place(
        &mut history,
        "d",
        OnStale::Refuse,
        title(&["s:1"], "Settled"),
    );
    assert_eq!(
        resolved,
        Placed::Written {
            revision: Revision::new("a", "d"),
            revised: vec![Revision::new("a", "b"), Revision::new("a", "c")]
        }
    );
    assert_eq!(
        Currency::new(&mut history).heads("a").unwrap(),
        vec![Revision::new("a", "d")]
    );
}

#[test]
fn uniqueness_ambiguous_across_records() {
    let mut history = History::new();
    place(&mut history, "a", OnStale::Refuse, title(&["s:1"], "One"));
    place(&mut history, "b", OnStale::Refuse, title(&["s:2"], "Two"));
    // One write about both sessions would revise two records.
    let mut plan = Plan::new("c", 1, arrival(OnStale::Refuse), &mut history);
    assert_eq!(
        plan.add(title(&["s:1", "s:2"], "Both"))
            .unwrap_err()
            .refusal,
        Refusal::Ambiguous {
            holders: vec![Revision::first("a"), Revision::first("b")]
        }
    );
    // A forking channel writes a new record instead.
    let forked = place(
        &mut history,
        "d",
        OnStale::Fork,
        title(&["s:1", "s:2"], "Both"),
    );
    assert_eq!(
        forked,
        Placed::Written {
            revision: Revision::first("d"),
            revised: vec![]
        }
    );
}

#[test]
fn uniqueness_revision_into_a_held_slot() {
    let mut history = History::new();
    place(&mut history, "a", OnStale::Refuse, title(&["s:1"], "One"));
    place(&mut history, "b", OnStale::Refuse, title(&["s:2"], "Two"));
    // Revising b to be about s:1 would give s:1 two current titles.
    let moved = Write {
        revises: vec![Revision::first("b")],
        ..title(&["s:1"], "Moved")
    };
    let mut plan = Plan::new("c", 1, arrival(OnStale::Refuse), &mut history);
    assert_eq!(
        plan.add(moved.clone()).unwrap_err().refusal,
        Refusal::SlotTaken {
            holders: vec![Revision::first("a")]
        }
    );
    // Revising a record within its own slot is ordinary.
    let kept = Write {
        revises: vec![Revision::first("a")],
        ..title(&["s:1"], "One, again")
    };
    place(&mut history, "d", OnStale::Refuse, kept);
}

/// Two first titles for one session, each written where the other was not
/// yet seen (two hosts, or an imported bundle), as the products' apply path
/// stores them: no uniqueness check, so the slot has two records.
fn two_first_titles() -> History {
    let mut history = History::new();
    for (root, text) in [("a", "From the laptop"), ("b", "From the desktop")] {
        let write = title(&["s:1"], text);
        history.push(
            authored::Header {
                root: root.into(),
                rev: root.into(),
                kind: write.kind,
                key: None,
                value: write.value,
                author: "operator".into(),
                via: "cli".into(),
                at: 0,
                revises: Default::default(),
                withdraws: Default::default(),
            },
            write.unique.unwrap().slots,
        );
    }
    history
}

#[test]
fn revision_naming_all_slot_heads_resolves_a_multi_family_slot() {
    let mut history = two_first_titles();
    // A revision of one record alone would leave the other holding the slot.
    let one = Write {
        revises: vec![Revision::first("b")],
        ..title(&["s:1"], "Settled")
    };
    let mut plan = Plan::new("c", 1, arrival(OnStale::Refuse), &mut history);
    assert_eq!(
        plan.add(one).unwrap_err().refusal,
        Refusal::SlotTaken {
            holders: vec![Revision::first("a")]
        }
    );
    // One revision naming every head of both records resolves the conflict.
    let both = Write {
        revises: vec![Revision::first("b"), Revision::first("a")],
        ..title(&["s:1"], "Settled")
    };
    let mut plan = Plan::new("d", 1, arrival(OnStale::Refuse), &mut history);
    let placed = plan.add(both).unwrap();
    let planned = plan.finish().unwrap();
    assert_eq!(
        planned.affected_roots,
        ["a".to_owned(), "b".to_owned()].into_iter().collect()
    );
    history.commit(planned);
    assert_eq!(
        placed,
        Placed::Written {
            revision: Revision::new("a", "d"),
            revised: vec![Revision::first("a"), Revision::first("b")]
        }
    );
    let mut currency = Currency::new(&mut history);
    assert_eq!(currency.heads("a").unwrap(), vec![Revision::new("a", "d")]);
    assert!(currency.heads("b").unwrap().is_empty());
}

#[test]
fn a_write_without_bases_into_a_multi_family_slot_revises_every_head() {
    let mut history = two_first_titles();
    let placed = place(
        &mut history,
        "c",
        OnStale::Refuse,
        title(&["s:1"], "Settled"),
    );
    assert_eq!(
        placed,
        Placed::Written {
            revision: Revision::new("a", "c"),
            revised: vec![Revision::first("a"), Revision::first("b")]
        }
    );
    let mut currency = Currency::new(&mut history);
    assert_eq!(currency.heads("a").unwrap(), vec![Revision::new("a", "c")]);
    assert!(currency.heads("b").unwrap().is_empty());
}

#[test]
fn bases_of_records_holding_no_common_slot_stay_refused() {
    let mut history = History::new();
    place(&mut history, "a", OnStale::Refuse, title(&["s:1"], "One"));
    place(&mut history, "b", OnStale::Refuse, title(&["s:2"], "Two"));
    let both = Write {
        revises: vec![Revision::first("a"), Revision::first("b")],
        ..title(&["s:1"], "Merged")
    };
    let mut plan = Plan::new("c", 1, arrival(OnStale::Refuse), &mut history);
    assert_eq!(
        plan.add(both).unwrap_err().refusal,
        Refusal::BasesSpanRoots(vec![Revision::first("a"), Revision::first("b")])
    );
}
