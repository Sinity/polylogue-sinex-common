//! The comparison of two answers of one relation. `| diff` and the signals
//! watch evaluator both use it, so a watch report and a diff always agree.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// In the after answer only.
    Joined,
    /// In the before answer only.
    Left,
    /// In both, at a different rank.
    Moved,
}
impl ChangeKind {
    /// The closed set `| diff` writes to its `change` field.
    pub const NAMES: [&'static str; 3] = ["joined", "left", "moved"];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Joined => "joined",
            Self::Left => "left",
            Self::Moved => "moved",
        }
    }
}
/// One changed row. Ranks are 1-based positions in each answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change<K> {
    pub key: K,
    pub kind: ChangeKind,
    pub old_rank: Option<usize>,
    pub new_rank: Option<usize>,
}
/// Compare two answers given as row identities in rank order, each listed
/// once. Joined and moved rows come in the after answer's order, then the
/// rows that left, in the before answer's order. Unchanged rows are omitted.
pub fn changes<K: Ord + Clone>(before: &[K], after: &[K]) -> Vec<Change<K>> {
    let old: BTreeMap<&K, usize> = before.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let new: BTreeMap<&K, usize> = after.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let mut out = Vec::new();
    for (rank, key) in after.iter().enumerate() {
        match old.get(key) {
            None => out.push(Change {
                key: key.clone(),
                kind: ChangeKind::Joined,
                old_rank: None,
                new_rank: Some(rank + 1),
            }),
            Some(&previous) if previous != rank => out.push(Change {
                key: key.clone(),
                kind: ChangeKind::Moved,
                old_rank: Some(previous + 1),
                new_rank: Some(rank + 1),
            }),
            Some(_) => {}
        }
    }
    for (rank, key) in before.iter().enumerate() {
        if !new.contains_key(key) {
            out.push(Change {
                key: key.clone(),
                kind: ChangeKind::Left,
                old_rank: Some(rank + 1),
                new_rank: None,
            });
        }
    }
    out
}
/// The first `k` rows of an answer, for top-k comparisons.
pub fn top<K>(answer: &[K], k: usize) -> &[K] {
    &answer[..answer.len().min(k)]
}
/// Whether any row joined or left.
pub fn membership_changed<K>(changes: &[Change<K>]) -> bool {
    changes
        .iter()
        .any(|change| change.kind != ChangeKind::Moved)
}
