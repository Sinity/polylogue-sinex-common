//! The request and answer envelopes every surface of both products carries.
//!
//! An answer is an outcome (a value, or a failure and no value) plus the
//! envelope: how many rows there are and how many are shown, what was left
//! out and how to read it, the follow-ups, how the query was read, the
//! positions it read at, the coverage of the sources behind it, who asked,
//! and the tokens its text used.
use crate::{Authorship, Failure, FailureKind};
use query_language::coverage::CoverageState;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::borrow::Cow;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LatencyClass {
    Local,
    Status,
    Completion,
    Read,
    Search,
    Aggregate,
}

impl LatencyClass {
    pub const ALL: [Self; 6] = [
        Self::Local,
        Self::Status,
        Self::Completion,
        Self::Read,
        Self::Search,
        Self::Aggregate,
    ];
    pub(crate) const fn index(self) -> usize {
        self as usize
    }
}

/// Milliseconds from admission. A JSON null lifts the deadline; absence uses
/// the operation's class default. All surfaces use this same representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Deadline {
    Milliseconds(u64),
    None,
    #[default]
    #[serde(skip)]
    Default,
}
impl Deadline {
    pub fn duration(self, default: Option<Duration>) -> Option<Duration> {
        match self {
            Self::Default => default,
            Self::None => None,
            Self::Milliseconds(ms) => Some(Duration::from_millis(ms)),
        }
    }
    /// The client waits 50 ms beyond the server deadline for its typed answer.
    pub fn socket_timeout(self, default: Option<Duration>) -> Option<Duration> {
        self.duration(default)
            .and_then(|d| d.checked_add(Duration::from_millis(50)))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request<T> {
    pub operation: String,
    pub arguments: T,
    pub request_id: String,
    #[serde(default, skip_serializing_if = "is_default")]
    pub deadline: Deadline,
    /// How many tokens the answer's text may use; absent uses the operation's
    /// default. The caller's harness cap clamps it (see `Budget`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_tokens: Option<u32>,
}
fn is_default(value: &Deadline) -> bool {
    matches!(value, Deadline::Default)
}
/// Dynamic dispatch keeps arguments typed until the table's serialization boundary.
pub type RunnableRequest = Request<Value>;

impl RunnableRequest {
    pub fn new(
        operation: impl Into<String>,
        arguments: Value,
        request_id: impl Into<String>,
    ) -> Self {
        Self {
            operation: operation.into(),
            arguments,
            request_id: request_id.into(),
            deadline: Deadline::Default,
            budget_tokens: None,
        }
    }
}

/// A ready follow-up: the next page, a narrower query, the same request with
/// its deadline lifted.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Next {
    pub label: String,
    pub request: RunnableRequest,
}

/// What came back: a value with how complete it is, or a failure and no value.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome<T> {
    Ok(T),
    /// The read succeeded and matched nothing. Only a successful read is
    /// empty; a failure never is.
    Empty(T),
    /// Rows came back, and part of what the answer needed is missing; `missing`
    /// says which part, and is never empty.
    Degraded {
        value: T,
        missing: Vec<String>,
    },
    Error(Failure),
}

impl<T> Outcome<T> {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Ok(_) => "ok",
            Self::Empty(_) => "empty",
            Self::Degraded { .. } => "degraded",
            Self::Error(_) => "error",
        }
    }
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ok(value) | Self::Empty(value) | Self::Degraded { value, .. } => Some(value),
            Self::Error(_) => None,
        }
    }
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> Outcome<U> {
        match self {
            Self::Ok(value) => Outcome::Ok(map(value)),
            Self::Empty(value) => Outcome::Empty(map(value)),
            Self::Degraded { value, missing } => Outcome::Degraded {
                value: map(value),
                missing,
            },
            Self::Error(error) => Outcome::Error(error),
        }
    }
}

/// A count of rows. A count over an interval in which a source was not
/// complete is a lower bound, and says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Count {
    pub value: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lower_bound: bool,
}
impl Count {
    pub const fn exact(value: u64) -> Self {
        Self {
            value,
            lower_bound: false,
        }
    }
}

/// Something the answer did not include, and the request that reads it at the
/// answer's read context.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LeftOut {
    pub what: String,
    pub count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<RunnableRequest>,
}

/// Where one product's read stood when the answer was read, and how far it
/// was behind the product's latest position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadPosition {
    pub product: String,
    pub position: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lag: Option<u64>,
}

/// One source behind an answer, and its coverage over the answer's interval:
/// its overall state, and each stretch that is not complete.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceCoverage {
    pub source: String,
    #[serde(flatten)]
    pub state: CoverageState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub not_complete: Vec<CoverageInterval>,
}
impl SourceCoverage {
    /// Whether the source and each of its stretches are complete.
    pub fn is_complete(&self) -> bool {
        self.state.is_complete() && self.not_complete.iter().all(|i| i.state.is_complete())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CoverageInterval {
    #[schemars(with = "String")]
    pub start: jiff::Timestamp,
    #[schemars(with = "String")]
    pub end: jiff::Timestamp,
    #[serde(flatten)]
    pub state: CoverageState,
}

/// Counts of the rows behind a sliced answer by one field's values, so a
/// caller narrows without reading.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Facet {
    pub field: String,
    pub counts: Vec<FacetCount>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FacetCount {
    pub value: String,
    pub count: u64,
}

/// Everything an answer says besides its outcome. Every field is optional on
/// the wire; an absent field means the operation had nothing to say there.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Envelope {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<Count>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub left_out: Vec<LeftOut>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<Next>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// How the product read the request, in words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_as: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub positions: Vec<ReadPosition>,
    /// Present only when a source's state affects the answer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coverage: Vec<SourceCoverage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub facets: Vec<Facet>,
    /// Who asked, as the product computed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<Authorship>,
    /// References rendered as one "already in your context" line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_tokens: Option<u32>,
}

impl Envelope {
    /// A count over sources that were not complete is a lower bound. Dispatch
    /// applies this to every answer, so no surface can report it as exact.
    pub fn mark_lower_bounds(&mut self) {
        if self.coverage.iter().any(|source| !source.is_complete())
            && let Some(count) = &mut self.count
        {
            count.lower_bound = true;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Answer<T> {
    pub outcome: Outcome<T>,
    pub envelope: Envelope,
}

impl<T> Answer<T> {
    pub fn ok(value: T) -> Self {
        Self::from(Outcome::Ok(value))
    }
    pub fn empty(value: T) -> Self {
        Self::from(Outcome::Empty(value))
    }
    /// `missing` names what the answer lacks; an empty list is a product bug
    /// that dispatch reports as one.
    pub fn degraded(value: T, missing: Vec<String>) -> Self {
        Self::from(Outcome::Degraded { value, missing })
    }
    pub fn error(error: Failure) -> Self {
        Self::from(Outcome::Error(error))
    }
    pub fn with_next(mut self, label: impl Into<String>, request: RunnableRequest) -> Self {
        self.envelope.next.push(Next {
            label: label.into(),
            request,
        });
        self
    }
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.envelope.notes.push(note.into());
        self
    }
    pub fn exit_code(&self) -> u8 {
        match &self.outcome {
            Outcome::Ok(_) => 0,
            Outcome::Empty(_) => 1,
            Outcome::Degraded { .. } => 5,
            Outcome::Error(error) => error.kind.exit_code(),
        }
    }
    pub fn http_status(&self) -> Option<u16> {
        match &self.outcome {
            Outcome::Error(error) => error.kind.http_status(),
            _ => Some(200),
        }
    }
    pub fn value(&self) -> Option<&T> {
        self.outcome.value()
    }
    pub fn failure(&self) -> Option<&Failure> {
        match &self.outcome {
            Outcome::Error(error) => Some(error),
            _ => None,
        }
    }
    pub fn is_error(&self) -> bool {
        matches!(self.outcome, Outcome::Error(_))
    }
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> Answer<U> {
        Answer {
            outcome: self.outcome.map(map),
            envelope: self.envelope,
        }
    }
}
impl<T> From<Outcome<T>> for Answer<T> {
    fn from(outcome: Outcome<T>) -> Self {
        Self {
            outcome,
            envelope: Envelope::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum OutcomeName {
    Ok,
    Empty,
    Degraded,
    Error,
}

/// The wire form: `outcome`, then `value` or `error`, then the envelope's
/// fields beside them. A present `value` of JSON null is a value.
#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct AnswerWire<T> {
    outcome: OutcomeName,
    #[serde(default, deserialize_with = "present")]
    value: Option<T>,
    #[serde(default)]
    error: Option<Failure>,
    #[serde(default)]
    missing: Vec<String>,
    #[serde(flatten)]
    envelope: Envelope,
}
fn present<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}
/// The schema of the wire form.
#[derive(JsonSchema)]
#[allow(dead_code)]
struct AnswerSchema<T> {
    outcome: OutcomeName,
    /// Present unless the outcome is `error`; an MCP answer in text form
    /// leaves it out, its rows being in the text part.
    #[serde(default)]
    value: Option<T>,
    /// Present only when the outcome is `error`.
    #[serde(default)]
    error: Option<Failure>,
    /// Present only on a degraded answer, and never empty there.
    #[serde(default)]
    missing: Vec<String>,
    #[serde(flatten)]
    envelope: Envelope,
}
#[derive(Serialize)]
struct AnswerWireRef<'a, T> {
    outcome: OutcomeName,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a Failure>,
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    missing: &'a [String],
    #[serde(flatten)]
    envelope: &'a Envelope,
}

impl<T: Serialize> Serialize for Answer<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (outcome, value, error, missing): (_, _, _, &[String]) = match &self.outcome {
            Outcome::Ok(value) => (OutcomeName::Ok, Some(value), None, &[]),
            Outcome::Empty(value) => (OutcomeName::Empty, Some(value), None, &[]),
            Outcome::Degraded { value, missing } => {
                (OutcomeName::Degraded, Some(value), None, missing)
            }
            Outcome::Error(error) => (OutcomeName::Error, None, Some(error), &[]),
        };
        AnswerWireRef {
            outcome,
            value,
            error,
            missing,
            envelope: &self.envelope,
        }
        .serialize(serializer)
    }
}
impl<'de, T: DeserializeOwned> Deserialize<'de> for Answer<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let wire = AnswerWire::<T>::deserialize(deserializer)?;
        let outcome = match (wire.outcome, wire.value, wire.error) {
            (OutcomeName::Error, None, Some(error)) if wire.missing.is_empty() => {
                Outcome::Error(error)
            }
            (OutcomeName::Error, ..) => {
                return Err(D::Error::custom(
                    "an error answer has an error and no value",
                ));
            }
            (_, _, Some(_)) => return Err(D::Error::custom("only an error answer has an error")),
            (_, None, None) => {
                return Err(D::Error::custom(
                    "an answer that is not an error has a value",
                ));
            }
            (OutcomeName::Degraded, Some(value), None) if !wire.missing.is_empty() => {
                Outcome::Degraded {
                    value,
                    missing: wire.missing,
                }
            }
            (OutcomeName::Degraded, ..) => {
                return Err(D::Error::custom("a degraded answer names what is missing"));
            }
            (_, Some(_), None) if !wire.missing.is_empty() => {
                return Err(D::Error::custom("only a degraded answer has missing parts"));
            }
            (OutcomeName::Ok, Some(value), None) => Outcome::Ok(value),
            (OutcomeName::Empty, Some(value), None) => Outcome::Empty(value),
        };
        Ok(Self {
            outcome,
            envelope: wire.envelope,
        })
    }
}
impl<T: JsonSchema> JsonSchema for Answer<T> {
    fn schema_name() -> Cow<'static, str> {
        format!("Answer_for_{}", T::schema_name()).into()
    }
    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        AnswerSchema::<T>::json_schema(generator)
    }
}

/// Whether an action can be taken now, and why not.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Availability {
    Enabled,
    Disabled { reason: String },
    Partial { reason: String },
    Loading,
    Unavailable { reason: String },
}

pub(crate) fn bad_request(message: impl Into<String>) -> Failure {
    Failure::new(FailureKind::BadRequest, message)
}
