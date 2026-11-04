//! Seen marks: per consumer and target, the latest position displayed to
//! that consumer, so `--since seen` reads only what is new to them.
//!
//! A mark is written only when the target was shown: displayed to the
//! operator, or read by an agent that asked for `since: seen`. A prerendered
//! page, a hidden tab, a poll or a preview never moves it. Each product
//! stores marks with its authored state, so they survive a rebuild; only the
//! latest mark is kept, never a history.
use crate::{AgentSession, Author, Position};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
/// at that showing, and for a session the anchor of the last item shown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SeenMark<A> {
    pub consumer: Consumer,
    /// The target's reference.
    pub target: String,
    pub position: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<A>,
}
impl<A: Ord> SeenMark<A> {
    /// Record that the target was put before the consumer at `position`.
    /// Only a showing moves the mark, and only forward within the mark's
    /// generation. A position from another generation of the product's
    /// index does not compare, so the later showing replaces the mark.
    /// Within one position, anchors follow the product's item order (`A::Ord`),
    /// so paging an unchanged session advances without moving backwards.
    /// Returns whether the mark moved.
    pub fn show(&mut self, showing: Showing, position: Position, anchor: Option<A>) -> bool {
        if !showing.moves_seen_mark()
            || (position.generation == self.position.generation
                && (position.counter < self.position.counter
                    || (position.counter == self.position.counter && anchor <= self.anchor)))
        {
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
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Showing {
    /// Visible to a person for at least `Showing::DISPLAY_TIME`.
    Displayed,
    /// Visible for less than the display time, as when scrolling past.
    Glimpsed,
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
    /// How long a view stays visible before it counts as displayed: long
    /// enough to read a line, so scrolling past or a flicker never counts.
    pub const DISPLAY_TIME: Duration = Duration::from_secs(1);
    /// What a visible view amounts to after `visible_for`: displayed once it
    /// reaches the display time, a glimpse before.
    pub fn of_visible_view(visible_for: Duration) -> Self {
        if visible_for >= Self::DISPLAY_TIME {
            Self::Displayed
        } else {
            Self::Glimpsed
        }
    }
    pub const fn moves_seen_mark(self) -> bool {
        matches!(self, Self::Displayed | Self::ReadSinceSeen)
    }
}

/// Where a read starts: after the consumer's seen mark, or after an explicit
/// position. Written `seen` or `generation:counter` on every surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Since {
    Seen,
    Position(Position),
}
impl Since {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "seen" => Some(Self::Seen),
            position => Position::parse(position).ok().map(Self::Position),
        }
    }
    /// The position to read after, given the consumer's mark if any; `None`
    /// reads from the start (`seen` with no mark yet).
    pub fn after<'a, A>(&'a self, mark: Option<&'a SeenMark<A>>) -> Option<&'a Position> {
        match self {
            Self::Seen => mark.map(|mark| &mark.position),
            Self::Position(position) => Some(position),
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
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom("since is `seen` or a position, generation:counter")
        })
    }
}
impl JsonSchema for Since {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Since".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^(seen|.+:[0-9]+)$",
            "description": "`seen` (after the caller's seen mark) or a position, generation:counter"
        })
    }
}
