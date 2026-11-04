//! Laws of ranking, `covered` and `diff`, each against an independent oracle.
use proptest::prelude::*;
use query_language::diff::{ChangeKind, changes};
use query_language::fusion::{Entry, Fusion, K, fuse};
use query_language::semantics::covered;
use std::collections::{BTreeMap, BTreeSet};

/// Ranked lists over a small universe, each key at most once per list, with
/// a recency per key shared by every list.
fn lists() -> impl Strategy<Value = Vec<Vec<Entry<u8>>>> {
    (
        prop::collection::vec(prop::option::of(0i64..4), 24),
        prop::collection::vec(prop::collection::vec(0u8..24, 0..20), 0..5),
    )
        .prop_map(|(recency, lists)| {
            lists
                .into_iter()
                .map(|keys| {
                    let mut seen = BTreeSet::new();
                    keys.into_iter()
                        .filter(|key| seen.insert(*key))
                        .map(|key| Entry {
                            key,
                            recency: recency[key as usize],
                        })
                        .collect()
                })
                .collect()
        })
}
/// An independent n-ary reciprocal-rank fusion over complete lists, by the
/// definition: per row, 1/(K + rank) summed largest first in `f64`; then
/// recency, then identity.
fn oracle(lists: &[Vec<Entry<u8>>]) -> Vec<u8> {
    let mut rows: BTreeMap<u8, (Vec<usize>, Option<i64>)> = BTreeMap::new();
    for list in lists {
        for (index, entry) in list.iter().enumerate() {
            let row = rows.entry(entry.key).or_insert((Vec::new(), None));
            row.0.push(index + 1);
            row.1 = row.1.max(entry.recency);
        }
    }
    let mut scored: Vec<(f64, Option<i64>, u8)> = rows
        .into_iter()
        .map(|(key, (mut ranks, recency))| {
            ranks.sort();
            let mut total = 0.0;
            for rank in ranks {
                total += 1.0 / (K + rank) as f64;
            }
            (total, recency, key)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| match (a.1, b.1) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.2.cmp(&b.2))
    });
    scored.into_iter().map(|(_, _, key)| key).collect()
}

proptest! {
    /// Law `ql.rank.fusion-oracle`: the fused order equals an independent
    /// n-ary reciprocal-rank fusion.
    #[test]
    fn rank_fusion_oracle(lists in lists()) {
        let fused: Vec<u8> = fuse(lists.clone()).into_iter().map(|row| row.key).collect();
        prop_assert_eq!(fused, oracle(&lists));
    }

    /// Law `ql.rank.early-stop-exact`: reading the lists only as deep as each
    /// page needs returns exactly the full fusion, for every page size.
    #[test]
    fn rank_early_stop_exact(lists in lists(), size in 1usize..7) {
        let full = fuse(lists.clone());
        let mut fusion = Fusion::new(lists.iter().map(|list| list.clone().into_iter()).collect());
        let mut paged = Vec::new();
        loop {
            let page = fusion.next_page(size);
            prop_assert!(page.len() <= size);
            if page.is_empty() {
                break;
            }
            paged.extend(page);
        }
        prop_assert_eq!(paged.len(), full.len());
        for (a, b) in paged.iter().zip(&full) {
            prop_assert_eq!(a.key, b.key);
            prop_assert_eq!(&a.contributions, &b.contributions);
        }
    }

    /// Law `ql.covered`: the union length equals a count of covered ticks.
    #[test]
    fn covered_equals_the_ticks_of_the_union(intervals in prop::collection::vec((-40i64..40, -40i64..40), 0..12)) {
        let ticks: BTreeSet<i64> = intervals
            .iter()
            .flat_map(|(start, end)| *start..*end)
            .collect();
        prop_assert_eq!(covered(&intervals), ticks.len() as i64);
    }

    /// `diff` reports every row that joined, left or moved, with its ranks,
    /// and nothing for rows at the same rank.
    #[test]
    fn diff_reports_joined_left_and_moved_with_ranks(
        before in prop::collection::btree_set(0u8..12, 0..10),
        after in prop::collection::btree_set(0u8..12, 0..10),
        rotate in 0usize..10,
    ) {
        let before: Vec<u8> = before.into_iter().collect();
        let mut after: Vec<u8> = after.into_iter().collect();
        if !after.is_empty() {
            let n = rotate % after.len();
            after.rotate_left(n);
        }
        let reported = changes(&before, &after);
        for change in &reported {
            let old = before.iter().position(|k| *k == change.key).map(|i| i + 1);
            let new = after.iter().position(|k| *k == change.key).map(|i| i + 1);
            prop_assert_eq!(change.old_rank, old);
            prop_assert_eq!(change.new_rank, new);
            prop_assert_eq!(change.kind, match (old, new) {
                (None, Some(_)) => ChangeKind::Joined,
                (Some(_), None) => ChangeKind::Left,
                _ => ChangeKind::Moved,
            });
            prop_assert_ne!(old, new);
        }
        let keys: BTreeSet<u8> = before.iter().chain(&after).copied().collect();
        let changed: BTreeSet<u8> = reported.iter().map(|change| change.key).collect();
        for key in keys {
            let same = before.iter().position(|k| *k == key) == after.iter().position(|k| *k == key);
            prop_assert_eq!(changed.contains(&key), !same);
        }
    }
}

#[test]
fn an_early_page_reads_less_than_the_whole_lists() {
    let long: Vec<Entry<u32>> = (0..10_000)
        .map(|key| Entry { key, recency: None })
        .collect();
    let mut fusion = Fusion::new(vec![long.clone().into_iter(), long.into_iter()]);
    let page = fusion.next_page(10);
    assert_eq!(
        page.iter().map(|row| row.key).collect::<Vec<_>>(),
        (0..10).collect::<Vec<_>>()
    );
    assert!(
        fusion.depths().iter().all(|depth| *depth < 100),
        "{:?}",
        fusion.depths()
    );
}
