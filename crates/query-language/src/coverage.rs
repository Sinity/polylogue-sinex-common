//! Coverage: whether a source observed a period, one vocabulary in both
//! products. `fill` labels every time bucket with one of these states, so a
//! period a source did not watch reads as missing data, never as zero.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Coverage {
    /// The source observed the whole period; an empty bucket holds nothing.
    Complete,
    /// The source should have observed the period and did not.
    Gap { reason: String },
    /// Nothing says whether the source observed the period.
    Unknown,
    /// The period lies outside what the source can cover (before an import
    /// span began, for example).
    OutsideSpan,
    /// The period has not been delivered yet.
    NotYet,
    /// The operator excluded the period or the source.
    Excluded,
    /// The source exists but could not be read.
    Unavailable { cause: String },
}
impl Coverage {
    /// The closed set of state names, as `fill` writes them to `coverage`.
    pub const STATES: [&'static str; 7] = [
        "complete",
        "gap",
        "unknown",
        "outside_span",
        "not_yet",
        "excluded",
        "unavailable",
    ];
    pub fn state(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Gap { .. } => "gap",
            Self::Unknown => "unknown",
            Self::OutsideSpan => "outside_span",
            Self::NotYet => "not_yet",
            Self::Excluded => "excluded",
            Self::Unavailable { .. } => "unavailable",
        }
    }
    /// The reason of a gap or the cause of an unavailable source.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Gap { reason } => Some(reason),
            Self::Unavailable { cause } => Some(cause),
            _ => None,
        }
    }
    /// Only a completely observed period makes an empty bucket a known zero.
    pub fn observed(&self) -> bool {
        matches!(self, Self::Complete)
    }
}
/// The field `fill` adds with the bucket's state name.
pub const STATE_FIELD: &str = "coverage";
/// The field `fill` adds with a gap's reason or an unavailable source's cause.
pub const DETAIL_FIELD: &str = "coverage_detail";
