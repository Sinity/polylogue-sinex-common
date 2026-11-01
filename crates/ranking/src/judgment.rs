//! Judgments, judge sets and the errors that refuse malformed input.

use std::collections::BTreeSet;
use std::fmt;

/// One recorded comparison by one judge: targets listed best first.
///
/// Each element of `order` is a tie group. Items in one group are equally
/// placed; a pair is the case of two groups of one item, and a k-ordering is
/// k groups of one item each. A judgment needs at least two distinct items,
/// and no item may appear twice.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Judgment {
    pub judge: String,
    pub order: Vec<Vec<String>>,
}

impl Judgment {
    /// A strict ordering: each item is its own group, best first.
    pub fn ordering<I, S>(judge: impl Into<String>, items: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            judge: judge.into(),
            order: items.into_iter().map(|item| vec![item.into()]).collect(),
        }
    }

    /// Every target in the judgment, in order of its tie groups.
    pub fn items(&self) -> impl Iterator<Item = &str> {
        self.order.iter().flatten().map(String::as_str)
    }
}

/// Which judges' judgments count toward a fit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JudgeSet {
    All,
    Named(BTreeSet<String>),
}

impl JudgeSet {
    /// The set naming exactly these judges.
    pub fn named<I, S>(judges: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Named(judges.into_iter().map(Into::into).collect())
    }

    pub fn counts(&self, judge: &str) -> bool {
        match self {
            Self::All => true,
            Self::Named(judges) => judges.contains(judge),
        }
    }
}

/// Why a fit or a selection refused its input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RankError {
    /// A judgment with fewer than two distinct targets.
    TooFewTargets { judge: String },
    /// A judgment with an empty tie group.
    EmptyTieGroup { judge: String },
    /// A target named twice in one judgment.
    RepeatedTarget { judge: String, item: String },
    /// A tie group larger than the uniform tie-break model's cost bound.
    TieGroupTooLarge { judge: String, size: usize },
    /// A selection asked about an item the fit does not contain.
    UnknownItem { item: String },
    /// A selection needs `wanted` distinct candidates and has `available`.
    TooFewCandidates { wanted: usize, available: usize },
    /// A selection of fewer than two items compares nothing.
    SetBelowTwo { size: usize },
    /// The fit's information matrix was not positive definite, which the prior
    /// rules out for finite input; the numbers overflowed.
    Numerical { reason: &'static str },
}

impl fmt::Display for RankError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooFewTargets { judge } => {
                write!(f, "judgment by {judge} has fewer than two distinct targets")
            }
            Self::EmptyTieGroup { judge } => {
                write!(f, "judgment by {judge} has an empty tie group")
            }
            Self::RepeatedTarget { judge, item } => {
                write!(f, "judgment by {judge} names {item} more than once")
            }
            Self::TieGroupTooLarge { judge, size } => write!(
                f,
                "judgment by {judge} has a tie group of {size}, above the cost bound of {}",
                crate::model::MAX_TIE_GROUP
            ),
            Self::UnknownItem { item } => write!(f, "{item} is not in the fit"),
            Self::TooFewCandidates { wanted, available } => {
                write!(
                    f,
                    "a set of {wanted} needs {wanted} candidates, {available} given"
                )
            }
            Self::SetBelowTwo { size } => write!(f, "a set of {size} compares nothing"),
            Self::Numerical { reason } => write!(f, "numerical failure: {reason}"),
        }
    }
}

impl std::error::Error for RankError {}

/// Refuses any judgment that cannot be a comparison.
pub(crate) fn validate(judgments: &[Judgment]) -> Result<(), RankError> {
    for judgment in judgments {
        if judgment.order.iter().any(Vec::is_empty) {
            return Err(RankError::EmptyTieGroup {
                judge: judgment.judge.clone(),
            });
        }
        let mut seen = BTreeSet::new();
        for item in judgment.items() {
            if !seen.insert(item) {
                return Err(RankError::RepeatedTarget {
                    judge: judgment.judge.clone(),
                    item: item.to_owned(),
                });
            }
        }
        if seen.len() < 2 {
            return Err(RankError::TooFewTargets {
                judge: judgment.judge.clone(),
            });
        }
    }
    Ok(())
}
