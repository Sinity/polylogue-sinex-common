//! Tests of the shared history generator. Products run the same generator
//! against their stored currency.
use authored::{Header, History, Revision, Target, affected_roots};
use authored_generator::history;
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

/// An independent statement of the rule, written from the definition over
/// the whole record list rather than through link lookups.
fn oracle(records: &[Header]) -> BTreeSet<Revision> {
    fn withdrawn(
        records: &[Header],
        revision: &Revision,
        known: &mut BTreeMap<Revision, bool>,
    ) -> bool {
        if let Some(value) = known.get(revision) {
            return *value;
        }
        let value = records.iter().any(|record| {
            record.withdraws.iter().any(|target| match target {
                Target::Root(root) => *root == revision.root,
                Target::Revision(named) => named == revision,
            }) && !withdrawn(records, &record.revision(), known)
        });
        known.insert(revision.clone(), value);
        value
    }
    let mut known = BTreeMap::new();
    let mut current = BTreeSet::new();
    for record in records {
        let revision = record.revision();
        if withdrawn(records, &revision, &mut known) {
            continue;
        }
        // Descendants: the closure of "revises something already in the set".
        let mut descendants: BTreeSet<Revision> = BTreeSet::new();
        loop {
            let before = descendants.len();
            for other in records {
                if other.revises.contains(&revision)
                    || other.revises.iter().any(|base| descendants.contains(base))
                {
                    descendants.insert(other.revision());
                }
            }
            if descendants.len() == before {
                break;
            }
        }
        if descendants
            .iter()
            .all(|descendant| withdrawn(records, descendant, &mut known))
        {
            current.insert(revision);
        }
    }
    current
}

fn rebuilt(records: &[Header]) -> History {
    let mut history = History::new();
    for record in records {
        history.push(record.clone(), Vec::new());
    }
    history
}

proptest! {
    #[test]
    fn currency_pure_function(
        mut generated in history(40),
        order in proptest::collection::vec(any::<u32>(), 40),
        clock in proptest::collection::vec(any::<i64>(), 40),
    ) {
        let expected = oracle(generated.records());
        prop_assert_eq!(generated.current().unwrap(), expected.clone());
        // Arrival order, write counters and clocks never change the answer.
        let mut records = generated.records().to_vec();
        let mut keyed: Vec<(u32, Header)> = records
            .drain(..)
            .enumerate()
            .map(|(index, mut record)| {
                record.at = clock[index % clock.len()];
                (order[index % order.len()], record)
            })
            .collect();
        keyed.sort_by_key(|(key, _)| *key);
        let shuffled: Vec<Header> = keyed.into_iter().map(|(_, record)| record).collect();
        let mut history = rebuilt(&shuffled);
        prop_assert_eq!(history.current().unwrap(), expected);
    }

    #[test]
    fn currency_affected_roots_suffice(generated in history(40)) {
        // Adding a record changes the state of no root outside its affected
        // roots, so a stored index recomputed for those roots equals a full
        // recomputation after every write.
        let records = generated.records();
        let mut history = History::new();
        for record in records {
            let before: BTreeMap<String, _> = history
                .states()
                .unwrap()
                .into_iter()
                .map(|state| (state.root.clone(), state))
                .collect();
            let roots = affected_roots(&mut history, record).unwrap();
            history.push(record.clone(), Vec::new());
            for state in history.states().unwrap() {
                if !roots.contains(&state.root) {
                    prop_assert_eq!(Some(&state), before.get(&state.root));
                }
            }
        }
    }
}
