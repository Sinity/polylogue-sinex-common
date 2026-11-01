//! Coverage: for one source and interval, how much of what happened there the
//! product holds. Both products' `fill` and every answer's coverage facet use
//! this one vocabulary.
use serde::{Deserialize, Serialize};

/// The state of one source over one interval.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CoverageState {
    /// Everything the source produced in the interval is held.
    Complete,
    /// The source may have missed data in the interval, for the stated reason
    /// (`overflow`, `disconnected`, `stalled` …).
    Gap { reason: String },
    /// Nothing says whether the interval is held or not.
    Unknown,
    /// The interval lies outside what the source's import or export spans.
    OutsideSpan,
    /// The source's next opportunity to deliver the interval has not come.
    NotYet,
    /// The operator excluded the source or interval from capture.
    Excluded,
    /// The source could not be reached, for the stated cause.
    Unavailable { cause: String },
}

impl CoverageState {
    /// Whether an answer over this interval can be taken as whole. Every other
    /// state makes a count over the interval a lower bound.
    pub const fn is_complete(&self) -> bool {
        matches!(self, Self::Complete)
    }
}
