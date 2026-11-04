//! Ranking: one reciprocal-rank fusion of every ranked list a query has, and
//! an exact early stop that reads the lists only as deep as a page needs.
//!
//! A row's score is the sum, over the lists that rank it, of 1/(K + rank)
//! (Cormack, Clarke and Büttcher 2009). Ties go to the more recent row, then
//! to the smaller identity. Ranking orders rows and never adds or removes one:
//! rows no list ranks are the caller's, after the ranked rows.
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// The fusion constant, shown in explanations; not a setting.
pub const K: usize = 60;

/// One list entry: a row identity and its recency (larger is more recent).
/// A row's recency is a property of the row, the same in every list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry<T> {
    pub key: T,
    pub recency: Option<i64>,
}
/// Where one list placed a row; ranks are 1-based.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Contribution {
    pub list: usize,
    pub rank: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Fused<T> {
    pub key: T,
    pub score: f64,
    pub recency: Option<i64>,
    pub contributions: Vec<Contribution>,
}
/// A row's score from its contributions, in `f64`, summed largest
/// contribution first. The order is part of the definition: two rows with the
/// same ranks in different lists get bit-identical scores and fall to the
/// tie-breaks, and every caller computes the identical value.
pub fn score(contributions: &[Contribution]) -> f64 {
    let mut ranks: Vec<usize> = contributions.iter().map(|c| c.rank).collect();
    ranks.sort_unstable();
    ranks.iter().map(|rank| 1.0 / (K + rank) as f64).sum()
}
/// The fused order: score descending, then recency descending (unknown
/// last), then identity ascending.
pub fn order<T: Ord>(a: &Fused<T>, b: &Fused<T>) -> Ordering {
    b.score
        .total_cmp(&a.score)
        .then_with(|| match (a.recency, b.recency) {
            (Some(x), Some(y)) => y.cmp(&x),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        })
        .then_with(|| a.key.cmp(&b.key))
}
/// Fuse complete lists: every row any list ranks, best first. A key repeated
/// within one list counts at its first position.
pub fn fuse<T: Ord + Clone>(lists: Vec<Vec<Entry<T>>>) -> Vec<Fused<T>> {
    let mut rows: BTreeMap<T, (Option<i64>, Vec<Contribution>)> = BTreeMap::new();
    for (list, entries) in lists.into_iter().enumerate() {
        let mut seen = BTreeSet::new();
        for (index, entry) in entries.into_iter().enumerate() {
            if !seen.insert(entry.key.clone()) {
                continue;
            }
            let row = rows.entry(entry.key).or_insert((None, Vec::new()));
            row.0 = row.0.max(entry.recency);
            row.1.push(Contribution {
                list,
                rank: index + 1,
            });
        }
    }
    let mut fused: Vec<_> = rows
        .into_iter()
        .map(|(key, (recency, contributions))| Fused {
            score: score(&contributions),
            key,
            recency,
            contributions,
        })
        .collect();
    fused.sort_by(order);
    fused
}

/// Fusion over list cursors that yield entries in rank order. Each page reads
/// the lists round-robin, one depth at a time, until no unread entry could
/// change the page; the pages concatenate to exactly `fuse` of the full lists.
///
/// This is the no-random-access threshold algorithm (Fagin, Lotem and Naor
/// 2001) specialised to reciprocal-rank fusion: a row seen in some lists has
/// a known part of its score and a bound on the rest, and an unseen row's
/// score is bounded by the next contribution of every unfinished list.
pub struct Fusion<T, I> {
    lists: Vec<ListCursor<I>>,
    rows: BTreeMap<T, Seen>,
    emitted: BTreeSet<T>,
}
struct ListCursor<I> {
    entries: I,
    read: usize,
    finished: bool,
}
struct Seen {
    recency: Option<i64>,
    ranks: Vec<Option<usize>>,
}
impl<T, I> Fusion<T, I>
where
    T: Ord + Clone,
    I: Iterator<Item = Entry<T>>,
{
    pub fn new(lists: Vec<I>) -> Self {
        Self {
            lists: lists
                .into_iter()
                .map(|entries| ListCursor {
                    entries,
                    read: 0,
                    finished: false,
                })
                .collect(),
            rows: BTreeMap::new(),
            emitted: BTreeSet::new(),
        }
    }
    /// How many entries each list has been read to, for a continuation and
    /// for explaining the cost of a page.
    pub fn depths(&self) -> Vec<usize> {
        self.lists.iter().map(|list| list.read).collect()
    }
    /// The next `size` rows of the fused order.
    pub fn next_page(&mut self, size: usize) -> Vec<Fused<T>> {
        if size == 0 {
            return Vec::new();
        }
        loop {
            if let Some(page) = self.settled_page(size) {
                for row in &page {
                    self.emitted.insert(row.key.clone());
                }
                return page;
            }
            self.read_one_depth();
        }
    }
    fn next_contribution(&self, list: usize) -> f64 {
        let cursor = &self.lists[list];
        if cursor.finished {
            0.0
        } else {
            1.0 / (K + cursor.read + 1) as f64
        }
    }
    fn settled_page(&self, size: usize) -> Option<Vec<Fused<T>>> {
        let all_finished = self.lists.iter().all(|list| list.finished);
        let mut settled = Vec::new();
        let mut unsettled_bounds = Vec::new();
        for (key, seen) in &self.rows {
            if self.emitted.contains(key) {
                continue;
            }
            let contributions: Vec<_> = seen
                .ranks
                .iter()
                .enumerate()
                .filter_map(|(list, rank)| rank.map(|rank| Contribution { list, rank }))
                .collect();
            let pending: Vec<_> = (0..self.lists.len())
                .filter(|&list| seen.ranks[list].is_none() && !self.lists[list].finished)
                .collect();
            let known = score(&contributions);
            if pending.is_empty() {
                settled.push(Fused {
                    key: key.clone(),
                    score: known,
                    recency: seen.recency,
                    contributions,
                });
            } else {
                let rest: f64 = pending.iter().map(|&l| self.next_contribution(l)).sum();
                unsettled_bounds.push(known + rest);
            }
        }
        settled.sort_by(order);
        if all_finished {
            settled.truncate(size);
            return Some(settled);
        }
        if settled.len() < size {
            return None;
        }
        settled.truncate(size);
        let threshold = settled.last().map_or(0.0, |row| row.score);
        // A row not yet seen anywhere can score at most the next entry of every list.
        let unseen: f64 = (0..self.lists.len())
            .map(|l| self.next_contribution(l))
            .sum();
        // Strictly below, with a margin for summation order: ties and rounding
        // only ever read further, never settle early.
        let below = |bound: f64| bound < threshold * (1.0 - 1e-12);
        (below(unseen) && unsettled_bounds.iter().all(|&bound| below(bound))).then_some(settled)
    }
    fn read_one_depth(&mut self) {
        let count = self.lists.len();
        for list in 0..count {
            let cursor = &mut self.lists[list];
            if cursor.finished {
                continue;
            }
            loop {
                match cursor.entries.next() {
                    None => {
                        cursor.finished = true;
                        break;
                    }
                    Some(entry) => {
                        cursor.read += 1;
                        let rank = cursor.read;
                        let seen = self.rows.entry(entry.key).or_insert_with(|| Seen {
                            recency: None,
                            ranks: vec![None; count],
                        });
                        seen.recency = seen.recency.max(entry.recency);
                        // A repeated key keeps its first rank; read on to a new one.
                        if seen.ranks[list].is_none() {
                            seen.ranks[list] = Some(rank);
                            break;
                        }
                    }
                }
            }
        }
    }
}
