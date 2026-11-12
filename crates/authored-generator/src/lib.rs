//! The shared history generator: random histories of revisions, forks,
//! conflict resolutions, withdrawals of revisions and roots, and withdrawals
//! of withdrawals. Both products replay them into their stores and check
//! their stored currency against [`History::states`].
//!
//! Records are produced in an order in which every link names an earlier
//! record, as a write path produces them; a test that wants another arrival
//! order shuffles [`History::records`]. Withdrawals never form a cycle.
use authored::{Header, History, Links, Revision, Target};
use proptest::prelude::*;
use std::collections::BTreeSet;

/// The kind the generator gives withdrawal records; other records are
/// `note`.
pub const WITHDRAWAL: &str = "withdrawal";

#[derive(Debug, Clone)]
enum Step {
    New,
    /// A revision of the picked record; with a second pick, a revision of two
    /// revisions of one record (a conflict resolution).
    Revise(u16, Option<u16>),
    /// A withdrawal of the picked records; `true` names the root.
    Withdraw(Vec<(u16, bool)>),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        3 => Just(Step::New),
        4 => (any::<u16>(), proptest::option::of(any::<u16>()))
            .prop_map(|(first, second)| Step::Revise(first, second)),
        3 => proptest::collection::vec((any::<u16>(), any::<bool>()), 1..3)
            .prop_map(Step::Withdraw),
    ]
}

/// Histories of up to `max_records` records.
pub fn history(max_records: usize) -> impl Strategy<Value = History> {
    proptest::collection::vec(step(), 0..=max_records).prop_map(build)
}

fn pick<T: Clone>(items: &[T], choice: u16) -> Option<T> {
    (!items.is_empty()).then(|| items[choice as usize % items.len()].clone())
}

fn build(steps: Vec<Step>) -> History {
    let mut history = History::new();
    let mut statements: Vec<Revision> = Vec::new();
    let mut all: Vec<Revision> = Vec::new();
    for (index, step) in steps.into_iter().enumerate() {
        let rev = format!("r{index}");
        let (root, kind, revises, withdraws) = match step {
            Step::New => (rev.clone(), "note", BTreeSet::new(), BTreeSet::new()),
            Step::Revise(first, second) => {
                let Some(base) = pick(&statements, first) else {
                    continue;
                };
                let mut revises = BTreeSet::from([base.clone()]);
                if let Some(second) = second {
                    let siblings = history.revisions(&base.root).unwrap_or_default();
                    if let Some(other) = pick(&siblings, second) {
                        revises.insert(other);
                    }
                }
                (base.root.clone(), "note", revises, BTreeSet::new())
            }
            Step::Withdraw(picks) => {
                let withdraws: BTreeSet<Target> = picks
                    .into_iter()
                    .filter_map(|(choice, whole)| {
                        pick(&all, choice).map(|target| {
                            if whole {
                                Target::Root(target.root)
                            } else {
                                Target::Revision(target)
                            }
                        })
                    })
                    .collect();
                if withdraws.is_empty() {
                    continue;
                }
                (rev.clone(), WITHDRAWAL, BTreeSet::new(), withdraws)
            }
        };
        let header = Header {
            root,
            rev,
            kind: kind.to_owned(),
            key: None,
            value: None,
            author: "operator".to_owned(),
            via: "generator".to_owned(),
            at: Some(index as i64),
            revises,
            withdraws,
        };
        let revision = header.revision();
        if kind != WITHDRAWAL {
            statements.push(revision.clone());
        }
        all.push(revision);
        history.push(header, Vec::new());
    }
    history
}
