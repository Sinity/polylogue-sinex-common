//! Values that may not be plainly measured, in one vocabulary for both
//! products and the web shell: numbers in answers and their cells.
//! Unknown is never zero: only a state that holds a value has one.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A value with how much of it is known.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum ValueState<T> {
    /// Measured over everything it covers.
    Complete { value: T },
    /// A lower bound: `unknown_count` of the things it covers had no value.
    Partial {
        value: T,
        unknown_count: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Estimated rather than measured, from the stated basis.
    Inferred { value: T, basis: String },
    /// Not known, for the stated reason (`timed out`, `zero denominator` …).
    Unknown { reason: String },
    /// The source never records this.
    NotRecorded,
    /// The value does not apply here.
    NotApplicable,
    /// Being computed; shown when it is ready.
    Pending,
}

impl<T> ValueState<T> {
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Complete { value }
            | Self::Partial { value, .. }
            | Self::Inferred { value, .. } => Some(value),
            _ => None,
        }
    }
    /// The state's name on the wire and in markup (`not_recorded`).
    pub fn name(&self) -> &'static str {
        match self {
            Self::Complete { .. } => "complete",
            Self::Partial { .. } => "partial",
            Self::Inferred { .. } => "inferred",
            Self::Unknown { .. } => "unknown",
            Self::NotRecorded => "not_recorded",
            Self::NotApplicable => "not_applicable",
            Self::Pending => "pending",
        }
    }
    /// The reason a value is missing or partial, when one was given.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Partial { reason, .. } => reason.as_deref(),
            Self::Inferred { basis, .. } => Some(basis),
            Self::Unknown { reason } => Some(reason),
            _ => None,
        }
    }
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> ValueState<U> {
        match self {
            Self::Complete { value } => ValueState::Complete { value: map(value) },
            Self::Partial {
                value,
                unknown_count,
                reason,
            } => ValueState::Partial {
                value: map(value),
                unknown_count,
                reason,
            },
            Self::Inferred { value, basis } => ValueState::Inferred {
                value: map(value),
                basis,
            },
            Self::Unknown { reason } => ValueState::Unknown { reason },
            Self::NotRecorded => ValueState::NotRecorded,
            Self::NotApplicable => ValueState::NotApplicable,
            Self::Pending => ValueState::Pending,
        }
    }
}

impl<T: std::fmt::Display> ValueState<T> {
    /// The plain-text form: `0`, `≥ 3 (2 unknown)`, `≈ 4 (inferred)`,
    /// `— unknown (timed out)`, `— not recorded`, `∅ n/a`, `◷ pending`. No
    /// missing state contains a digit of its own.
    pub fn text(&self) -> String {
        match self {
            Self::Complete { value } => value.to_string(),
            Self::Partial {
                value,
                unknown_count,
                ..
            } => format!("≥ {value} ({unknown_count} unknown)"),
            Self::Inferred { value, .. } => format!("≈ {value} (inferred)"),
            Self::Unknown { reason } => format!("— unknown ({reason})"),
            Self::NotRecorded => "— not recorded".into(),
            Self::NotApplicable => "∅ n/a".into(),
            Self::Pending => "◷ pending".into(),
        }
    }
}

/// A number an answer reports, with what an agent needs to quote it: its
/// unit, its kind, its state, what it is out of, and the query behind it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Number {
    #[serde(flatten)]
    pub value: ValueState<f64>,
    /// `tokens`, `usd`, `ms`, `sessions` …
    pub unit: String,
    /// The product's kind of the number, such as a money kind
    /// (`api_equivalent`, `billed`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denominator: Option<Denominator>,
    /// The query that reproduces the number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
}

/// What a number is out of: the population it was computed over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Denominator {
    pub count: u64,
    pub unit: String,
    /// The query that lists the population.
    pub query: String,
}

impl Number {
    /// `numerator / denominator`; a zero denominator is unknown, never zero.
    pub fn ratio(numerator: f64, denominator: Denominator, unit: impl Into<String>) -> Self {
        let value = if denominator.count == 0 {
            ValueState::Unknown {
                reason: "zero denominator".into(),
            }
        } else {
            ValueState::Complete {
                value: numerator / denominator.count as f64,
            }
        };
        Self {
            value,
            unit: unit.into(),
            kind: None,
            denominator: Some(denominator),
            query: None,
        }
    }
}
