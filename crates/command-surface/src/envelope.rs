//! The request and answer envelopes every surface of both products carries.
//!
//! An answer is an outcome (a value, or a failure and no value) plus the
//! envelope: how many rows there are and how many are shown, what was left
//! out and how to read it, the follow-ups, how the query was read, the
//! positions it read at, the coverage of the sources behind it, who asked,
//! and the tokens its text used.
use crate::{Authorship, Failure, FailureKind, Position};
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

/// A token count, an explicitly lifted budget (JSON null), or the operation's
/// default when absent. Unlimited is distinct from requesting the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TokenBudget {
    Tokens(u32),
    Unlimited,
    #[default]
    #[serde(skip)]
    Default,
}
impl TokenBudget {
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Default)
    }
    pub fn resolve(self, default: Option<u32>) -> Option<u32> {
        match self {
            Self::Default => default,
            Self::Unlimited => None,
            Self::Tokens(n) => Some(n),
        }
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
    /// The answer's token budget; null lifts it and absence uses the default.
    /// The caller's harness cap still applies (see `Budget`).
    #[serde(default, skip_serializing_if = "TokenBudget::is_default")]
    pub budget_tokens: TokenBudget,
    /// Continues an earlier answer from where its page ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    /// Render in full what the caller already holds, instead of one line
    /// naming where it was delivered.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fresh: bool,
    #[serde(default, skip_serializing_if = "Format::is_text")]
    pub format: Format,
}
fn is_default(value: &Deadline) -> bool {
    matches!(value, Deadline::Default)
}

/// A request option a product's table may accept beyond `request_id` and
/// `deadline`, which every table takes. A table names the ones it serves;
/// its tools offer only those, and dispatch refuses the others.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RequestOption {
    BudgetTokens,
    Cursor,
    Fresh,
    Format,
}
impl RequestOption {
    pub const ALL: [Self; 4] = [Self::BudgetTokens, Self::Cursor, Self::Fresh, Self::Format];
    /// The option's field name on the wire.
    pub fn name(self) -> &'static str {
        match self {
            Self::BudgetTokens => "budget_tokens",
            Self::Cursor => "cursor",
            Self::Fresh => "fresh",
            Self::Format => "format",
        }
    }
    /// Whether `request` uses the option.
    pub fn used_by<T>(self, request: &Request<T>) -> bool {
        match self {
            Self::BudgetTokens => !request.budget_tokens.is_default(),
            Self::Cursor => request.cursor.is_some(),
            Self::Fresh => request.fresh,
            Self::Format => request.format != Format::Text,
        }
    }
}

/// How the answer comes back: as text (rows rendered once, the envelope
/// beside them), or with the rows as JSON too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    #[default]
    Text,
    Json,
}
impl Format {
    pub fn is_text(&self) -> bool {
        *self == Self::Text
    }
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
            budget_tokens: TokenBudget::Default,
            cursor: None,
            fresh: false,
            format: Format::Text,
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

/// The position one product's read stood at when the answer was read, and
/// how many commits it was behind the product's latest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadAt {
    pub product: String,
    pub position: Position,
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
    /// The counts are lower bounds: the answer is degraded or a source
    /// behind it was not complete.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lower_bound: bool,
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
    pub positions: Vec<ReadAt>,
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
    /// The value is left out because the answer's text part holds it (an
    /// MCP answer in text format). Read such an answer as `Answer<Option<T>>`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub value_in_text: bool,
}

impl Envelope {
    /// Counts over sources that were not complete, or in an answer that is
    /// degraded, are lower bounds: the count and every facet. Dispatch
    /// applies this to every answer, so no surface can report them as exact.
    pub fn mark_lower_bounds(&mut self, degraded: bool) {
        if !degraded && self.coverage.iter().all(SourceCoverage::is_complete) {
            return;
        }
        if let Some(count) = &mut self.count {
            count.lower_bound = true;
        }
        for facet in &mut self.facets {
            facet.lower_bound = true;
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
        // A value the text part holds is read as `None` by an
        // `Answer<Option<T>>`; any other value type refuses it.
        let value = match wire.value {
            None if wire.envelope.value_in_text => {
                Some(T::deserialize(ValueInText).map_err(D::Error::custom)?)
            }
            value => value,
        };
        let outcome = match (wire.outcome, value, wire.error) {
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
/// Stands for a value an answer's text part holds: an `Option` reads it as
/// `None`, and every other type refuses it.
struct ValueInText;
impl<'de> serde::Deserializer<'de> for ValueInText {
    type Error = serde::de::value::Error;
    fn deserialize_any<V: serde::de::Visitor<'de>>(self, _: V) -> Result<V::Value, Self::Error> {
        Err(serde::de::Error::custom(
            "the value is in the answer's text part; read it as Answer<Option<T>>",
        ))
    }
    fn deserialize_option<V: serde::de::Visitor<'de>>(
        self,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_none()
    }
    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct newtype_struct seq tuple
        tuple_struct map struct enum identifier ignored_any
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
