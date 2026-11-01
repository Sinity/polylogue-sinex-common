//! Failures: what went wrong, where in the input, and what the caller can run
//! instead. A failure is never an outcome: an empty or degraded answer is a
//! read that succeeded, and carries its value (see `Outcome`).
use crate::RunnableRequest;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// The closed shared classes. Products attach their precise code in `Failure::code`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    InvalidQuery,
    UnknownField,
    BadValue,
    NotAQuery,
    BadRequest,
    Ambiguous,
    StaleCursor,
    /// The index or derived product the request needs has not been built.
    NotBuilt,
    /// The request needs configuration the product does not have (an
    /// endpoint, a credential, a source).
    NotConfigured,
    /// The daemon is still opening; the message carries its progress.
    NotYetAvailable,
    Refused,
    NotFound,
    Conflict,
    Io,
    /// An operation that did not finish; never reported as an answer.
    StoppedEarly,
    NotRunning,
    Timeout,
    Cancelled,
}

impl FailureKind {
    pub const ALL: [Self; 18] = [
        Self::InvalidQuery,
        Self::UnknownField,
        Self::BadValue,
        Self::NotAQuery,
        Self::BadRequest,
        Self::Ambiguous,
        Self::StaleCursor,
        Self::NotBuilt,
        Self::NotConfigured,
        Self::NotYetAvailable,
        Self::Refused,
        Self::NotFound,
        Self::Conflict,
        Self::Io,
        Self::StoppedEarly,
        Self::NotRunning,
        Self::Timeout,
        Self::Cancelled,
    ];

    /// The single exit/HTTP table. `None` means a client-side outcome.
    pub const fn mapping(self) -> (u8, Option<u16>) {
        match self {
            Self::InvalidQuery
            | Self::UnknownField
            | Self::BadValue
            | Self::NotAQuery
            | Self::BadRequest
            | Self::Ambiguous
            | Self::StaleCursor => (2, Some(400)),
            Self::NotBuilt | Self::NotConfigured | Self::NotYetAvailable => (2, Some(501)),
            Self::Refused => (3, Some(403)),
            Self::NotFound => (4, Some(404)),
            Self::Conflict => (4, Some(409)),
            Self::Io => (4, Some(500)),
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
    /// The kind's name on the wire (`unknown_field`).
    pub fn name(self) -> &'static str {
        match self {
            Self::InvalidQuery => "invalid_query",
            Self::UnknownField => "unknown_field",
            Self::BadValue => "bad_value",
            Self::NotAQuery => "not_a_query",
            Self::BadRequest => "bad_request",
            Self::Ambiguous => "ambiguous",
            Self::StaleCursor => "stale_cursor",
            Self::NotBuilt => "not_built",
            Self::NotConfigured => "not_configured",
            Self::NotYetAvailable => "not_yet_available",
            Self::Refused => "refused",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::Io => "io",
            Self::StoppedEarly => "stopped_early",
            Self::NotRunning => "not_running",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Where in the submitted input a failure is, in UTF-8 bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InputSpan {
    pub offset: usize,
    pub length: usize,
}

/// What the caller can run instead: the corrected query text, or a whole
/// request. Either one runs as given.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Suggestion {
    Query(String),
    Request(RunnableRequest),
}

/// A typed failure. The structured parts a caller acts on (the suggestion, the
/// candidates, the caret diagnostic) sit behind one box, so a `Result`
/// carrying a failure stays small; on the wire they are ordinary fields.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    pub message: String,
    /// A product's more precise code, mapped to `kind` at its operation boundary.
    pub code: Option<String>,
    pub position: Option<InputSpan>,
    pub hint: Option<String>,
    pub retryable: bool,
    detail: Option<Box<Detail>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Detail {
    suggestion: Option<Suggestion>,
    candidates: Vec<String>,
    caret: Option<String>,
}

impl Failure {
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            code: None,
            position: None,
            hint: None,
            retryable: matches!(
                kind,
                FailureKind::Timeout | FailureKind::NotRunning | FailureKind::NotYetAvailable
            ),
            detail: None,
        }
    }
    fn detail_mut(&mut self) -> &mut Detail {
        self.detail.get_or_insert_with(Box::default)
    }
    fn tidy(mut self) -> Self {
        if self.detail.as_deref() == Some(&Detail::default()) {
            self.detail = None;
        }
        self
    }
    /// A corrected query or request that runs as given.
    pub fn suggestion(&self) -> Option<&Suggestion> {
        self.detail.as_ref()?.suggestion.as_ref()
    }
    pub fn with_suggestion(mut self, suggestion: Suggestion) -> Self {
        self.detail_mut().suggestion = Some(suggestion);
        self
    }
    /// What the input could have meant, each runnable as given: the matches
    /// of an ambiguous reference, or known names close to an unknown one.
    pub fn candidates(&self) -> &[String] {
        self.detail
            .as_ref()
            .map_or(&[], |detail| detail.candidates.as_slice())
    }
    pub fn with_candidates(mut self, candidates: Vec<String>) -> Self {
        self.detail_mut().candidates = candidates;
        self.tidy()
    }
    /// The input line with a marker under the failing span.
    pub fn caret(&self) -> Option<&str> {
        self.detail.as_ref()?.caret.as_deref()
    }
    /// Place the failure at `span` of `input`: sets the position and the caret.
    pub fn at(mut self, input: &str, span: InputSpan) -> Self {
        self.position = Some(span);
        self.detail_mut().caret = Some(caret(input, span));
        self
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }
    /// The one conversion of a query-language refusal: the same kind, its
    /// message, hint, suggestion, candidates and retryability, and its place
    /// in `query` as a span (to the end of the word there) with its caret.
    pub fn from_query_error(error: query_language::Error, query: &str) -> Self {
        use query_language::ErrorKind as Q;
        let kind = match error.kind {
            Q::InvalidQuery => FailureKind::InvalidQuery,
            Q::UnknownField => FailureKind::UnknownField,
            Q::BadValue => FailureKind::BadValue,
            Q::NotAQuery => FailureKind::NotAQuery,
            Q::Ambiguous => FailureKind::Ambiguous,
            Q::StaleCursor => FailureKind::StaleCursor,
        };
        let span = InputSpan {
            offset: error.position,
            length: token_length(query, error.position),
        };
        let mut failure = Self::new(kind, error.message)
            .at(query, span)
            .with_candidates(error.candidates);
        failure.hint = error.hint;
        failure.retryable = error.retryable;
        match error.suggestion {
            Some(corrected) => failure.with_suggestion(Suggestion::Query(corrected)),
            None => failure,
        }
    }
}

/// The length of the word starting at `offset`, up to the next whitespace.
fn token_length(input: &str, offset: usize) -> usize {
    input.get(offset..).map_or(0, |rest| {
        rest.find(char::is_whitespace).unwrap_or(rest.len())
    })
}

/// The line of `input` that holds `span`, and under it `^` for each of the
/// span's characters on that line (at least one).
pub fn caret(input: &str, span: InputSpan) -> String {
    let offset = floor_char_boundary(input, span.offset.min(input.len()));
    let start = input[..offset].rfind('\n').map_or(0, |n| n + 1);
    let end = input[offset..]
        .find('\n')
        .map_or(input.len(), |n| offset + n);
    let line = &input[start..end];
    let column = input[start..offset].chars().count();
    let span_end = floor_char_boundary(input, (offset + span.length).min(end));
    let width = input[offset..span_end].chars().count().max(1);
    format!("{line}\n{}{}", " ".repeat(column), "^".repeat(width))
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Failure {}

/// The wire form: the QL shape `{kind, message, position, caret, suggestion,
/// candidates, hint, retryable}`, plus the product's code.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Failure")]
struct FailureWire {
    kind: FailureKind,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    position: Option<InputSpan>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    caret: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    suggestion: Option<Suggestion>,
    /// Never empty when present.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    candidates: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    hint: Option<String>,
    retryable: bool,
}

impl Serialize for Failure {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let detail = self.detail.as_deref().cloned().unwrap_or_default();
        FailureWire {
            kind: self.kind,
            message: self.message.clone(),
            code: self.code.clone(),
            position: self.position,
            caret: detail.caret,
            suggestion: detail.suggestion,
            candidates: detail.candidates,
            hint: self.hint.clone(),
            retryable: self.retryable,
        }
        .serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Failure {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = FailureWire::deserialize(deserializer)?;
        Ok(Self {
            kind: wire.kind,
            message: wire.message,
            code: wire.code,
            position: wire.position,
            hint: wire.hint,
            retryable: wire.retryable,
            detail: Some(Box::new(Detail {
                suggestion: wire.suggestion,
                candidates: wire.candidates,
                caret: wire.caret,
            })),
        }
        .tidy())
    }
}
impl JsonSchema for Failure {
    fn schema_name() -> Cow<'static, str> {
        "Failure".into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        FailureWire::json_schema(generator)
    }
}
