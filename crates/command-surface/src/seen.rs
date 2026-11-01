//! Seen marks: per consumer and target, the latest position displayed to
//! that consumer, so `--since seen` reads only what is new to them.
//!
//! A mark is written only when the target was shown: displayed to the
//! operator, or read by an agent that asked for `since: seen`. A prerendered
//! page, a hidden tab, a poll or a preview never moves it. Each product
//! stores marks with its authored state, so they survive a rebuild; only the
//! latest mark is kept, never a history.
use crate::{AgentSession, Author};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Who a mark belongs to: the operator, or one agent session.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(tag = "consumer", content = "session", rename_all = "snake_case")]
pub enum Consumer {
    Operator,
    Session(AgentSession),
}
impl Consumer {
    /// The consumer an author reads as. Only the operator and an identified
    /// session have marks; any other author reads without one.
    pub fn of(author: &Author) -> Option<Self> {
        match author {
            Author::Operator => Some(Self::Operator),
            Author::Agent(session) => Some(Self::Session(session.clone())),
            _ => None,
        }
    }
}

/// The latest place a consumer was shown in a target: the product's position
/// at that read, and for a session the anchor of the last item shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeenMark {
    pub consumer: Consumer,
    /// The target's reference.
    pub target: String,
    pub position: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}
impl SeenMark {
    /// Record a later showing. A mark only moves forward: a showing at an
    /// earlier position (an old page reread) leaves it as it is. Returns
    /// whether the mark moved.
    pub fn advance(&mut self, position: u64, anchor: Option<String>) -> bool {
        if position <= self.position {
            return false;
        }
        self.position = position;
        self.anchor = anchor;
        true
    }
}

/// How a target came to be in front of a consumer. Only a showing moves a
/// seen mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Showing {
    /// Displayed in a visible view for its display time.
    Displayed,
    /// Read by an agent that asked for `since: seen`.
    ReadSinceSeen,
    /// Rendered ahead of being looked at.
    Prerendered,
    /// Rendered in a tab or pane that was not visible.
    Hidden,
    /// Fetched by a poll or a live update nobody looked at.
    Polled,
    /// Shown as a preview or hover card.
    Preview,
    /// Read by an agent that did not ask for `since: seen`.
    Read,
}
impl Showing {
    pub const fn moves_seen_mark(self) -> bool {
        matches!(self, Self::Displayed | Self::ReadSinceSeen)
    }
}

/// Where a read starts: after the consumer's seen mark, or after an explicit
/// position. Written `seen` or a number on every surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Since {
    Seen,
    Position(u64),
}
impl Since {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "seen" => Some(Self::Seen),
            number => number.parse().ok().map(Self::Position),
        }
    }
    /// The position to read after, given the consumer's mark if any. With no
    /// mark, `seen` reads from the start.
    pub fn after(self, mark: Option<&SeenMark>) -> u64 {
        match self {
            Self::Seen => mark.map_or(0, |mark| mark.position),
            Self::Position(position) => position,
        }
    }
}
impl std::fmt::Display for Since {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seen => f.write_str("seen"),
            Self::Position(position) => write!(f, "{position}"),
        }
    }
}
impl Serialize for Since {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}
impl<'de> Deserialize<'de> for Since {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).ok_or_else(|| serde::de::Error::custom("since is `seen` or a position"))
    }
}
impl JsonSchema for Since {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Since".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^(seen|[0-9]+)$",
            "description": "`seen` (after the caller's seen mark) or a position"
        })
    }
}
