//! Typed envelopes. An error has no value field, so it cannot carry partial rows.
use crate::{Failure, FailureKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
}
fn is_default(value: &Deadline) -> bool {
    matches!(value, Deadline::Default)
}
/// Dynamic dispatch keeps arguments typed until the table's serialization boundary.
pub type RunnableRequest = Request<Value>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Next {
    pub label: String,
    pub request: RunnableRequest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum Answer<T> {
    Ok {
        value: T,
        notes: Vec<String>,
        next: Vec<Next>,
    },
    Empty {
        value: T,
        notes: Vec<String>,
        next: Vec<Next>,
    },
    Degraded {
        value: T,
        notes: Vec<String>,
        next: Vec<Next>,
    },
    Error {
        error: Failure,
        notes: Vec<String>,
        next: Vec<Next>,
    },
}
impl<T> Answer<T> {
    pub fn ok(value: T) -> Self {
        Self::Ok {
            value,
            notes: vec![],
            next: vec![],
        }
    }
    pub fn error(error: Failure) -> Self {
        Self::Error {
            error,
            notes: vec![],
            next: vec![],
        }
    }
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Ok { .. } => 0,
            Self::Empty { .. } => 1,
            Self::Degraded { .. } => 5,
            Self::Error { error, .. } => error.kind.exit_code(),
        }
    }
    pub fn http_status(&self) -> Option<u16> {
        match self {
            Self::Error { error, .. } => error.kind.http_status(),
            _ => Some(200),
        }
    }
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Ok { value, .. } | Self::Empty { value, .. } | Self::Degraded { value, .. } => {
                Some(value)
            }
            Self::Error { .. } => None,
        }
    }
    pub fn map<U>(self, map: impl FnOnce(T) -> U) -> Answer<U> {
        match self {
            Self::Ok { value, notes, next } => Answer::Ok {
                value: map(value),
                notes,
                next,
            },
            Self::Empty { value, notes, next } => Answer::Empty {
                value: map(value),
                notes,
                next,
            },
            Self::Degraded { value, notes, next } => Answer::Degraded {
                value: map(value),
                notes,
                next,
            },
            Self::Error { error, notes, next } => Answer::Error { error, notes, next },
        }
    }
    pub fn next_mut(&mut self) -> &mut Vec<Next> {
        match self {
            Self::Ok { next, .. }
            | Self::Empty { next, .. }
            | Self::Degraded { next, .. }
            | Self::Error { next, .. } => next,
        }
    }
    pub fn notes(&self) -> &[String] {
        match self {
            Self::Ok { notes, .. }
            | Self::Empty { notes, .. }
            | Self::Degraded { notes, .. }
            | Self::Error { notes, .. } => notes,
        }
    }
    pub fn next(&self) -> &[Next] {
        match self {
            Self::Ok { next, .. }
            | Self::Empty { next, .. }
            | Self::Degraded { next, .. }
            | Self::Error { next, .. } => next,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
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
