//! Stable failure classification, independent of a transport or product.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The closed shared classes. Products attach their precise code in `Failure::code`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Empty,
    InvalidQuery,
    UnknownField,
    BadValue,
    NotAQuery,
    BadRequest,
    Ambiguous,
    StaleCursor,
    NotYetAvailable,
    Refused,
    NotFound,
    Conflict,
    Io,
    Degraded,
    StoppedEarly,
    NotRunning,
    Timeout,
    Cancelled,
}

impl FailureKind {
    pub const ALL: [Self; 18] = [
        Self::Empty,
        Self::InvalidQuery,
        Self::UnknownField,
        Self::BadValue,
        Self::NotAQuery,
        Self::BadRequest,
        Self::Ambiguous,
        Self::StaleCursor,
        Self::NotYetAvailable,
        Self::Refused,
        Self::NotFound,
        Self::Conflict,
        Self::Io,
        Self::Degraded,
        Self::StoppedEarly,
        Self::NotRunning,
        Self::Timeout,
        Self::Cancelled,
    ];

    /// The single exit/HTTP table. `None` means a client-side outcome.
    pub const fn mapping(self) -> (u8, Option<u16>) {
        match self {
            Self::Empty => (1, Some(200)),
            Self::InvalidQuery
            | Self::UnknownField
            | Self::BadValue
            | Self::NotAQuery
            | Self::BadRequest
            | Self::Ambiguous
            | Self::StaleCursor => (2, Some(400)),
            Self::NotYetAvailable => (2, Some(501)),
            Self::Refused => (3, Some(403)),
            Self::NotFound => (4, Some(404)),
            Self::Conflict => (4, Some(409)),
            Self::Io => (4, Some(500)),
            Self::Degraded => (5, Some(200)),
            Self::StoppedEarly => (5, Some(500)),
            Self::NotRunning => (6, None),
            Self::Timeout => (7, Some(504)),
            Self::Cancelled => (7, None),
        }
    }
    pub const fn exit_code(self) -> u8 {
        self.mapping().0
    }
    pub const fn http_status(self) -> Option<u16> {
        self.mapping().1
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Position {
    /// UTF-8 byte offset in the submitted input.
    pub offset: usize,
    pub length: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    /// A product's more precise code, mapped to `kind` at its operation boundary.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub position: Option<Position>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suggestion: Option<Box<Value>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub hint: Option<String>,
    pub retryable: bool,
}

impl Failure {
    pub fn with_suggestion(mut self, suggestion: Value) -> Self {
        self.suggestion = Some(Box::new(suggestion));
        self
    }
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
            position: None,
            suggestion: None,
            hint: None,
            retryable: matches!(kind, FailureKind::Timeout | FailureKind::NotRunning),
        }
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}
