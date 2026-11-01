//! What happens to a delivery after it is recorded. A delivery never changes;
//! attempts and acts append outcomes, and its state is their fold.
use crate::{Decision, DeliveryOutcome, Instant};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A notification closed by the operator this soon after it was shown counts
/// as dismissed at once.
pub const DISMISSED_FAST: i64 = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Desktop,
    Web,
    Cli,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CloseReason {
    Expired,
    Dismissed,
    /// The condition ended or the deliverer closed it.
    Cleared,
    /// A later notification about the same occurrence took its place.
    Replaced,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    /// One outlet's attempt: unknown, delivered or not sent.
    Attempt {
        result: DeliveryOutcome,
    },
    Shown,
    Updated,
    Closed {
        reason: CloseReason,
    },
    Opened {
        via: Via,
    },
    Acknowledged,
    Snoozed {
        until: Instant,
    },
    /// A snoozed or folded notice routed again.
    Released {
        decision: Decision,
    },
    /// The current-state condition ended.
    Cleared,
    /// Displayed successfully to an explicit consumer. Previews, on-screen
    /// suppression and agent reads never count.
    Seen,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutcomeRecord {
    pub delivery: String,
    pub at: Instant,
    pub outcome: Outcome,
}

/// The fold of one delivery's outcomes, in the order they were appended.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryState {
    pub shown_at: Option<Instant>,
    pub opened: bool,
    pub acknowledged: bool,
    pub seen: bool,
    pub snoozed_until: Option<Instant>,
    pub cleared: bool,
    pub closed: Option<CloseReason>,
    pub dismissed_fast: bool,
    pub last_attempt: Option<DeliveryOutcome>,
}
impl DeliveryState {
    pub fn fold<'a>(outcomes: impl IntoIterator<Item = &'a OutcomeRecord>) -> Self {
        let mut state = Self::default();
        for record in outcomes {
            match &record.outcome {
                Outcome::Attempt { result } => state.last_attempt = Some(result.clone()),
                Outcome::Shown => {
                    state.shown_at = Some(record.at);
                    state.closed = None;
                }
                Outcome::Updated => {}
                Outcome::Closed { reason } => {
                    state.closed = Some(*reason);
                    state.dismissed_fast = *reason == CloseReason::Dismissed
                        && state
                            .shown_at
                            .is_some_and(|shown| record.at.saturating_sub(shown) < DISMISSED_FAST);
                }
                Outcome::Opened { .. } => {
                    state.opened = true;
                    state.seen = true;
                }
                Outcome::Acknowledged => {
                    state.acknowledged = true;
                    state.seen = true;
                }
                Outcome::Snoozed { until } => state.snoozed_until = Some(*until),
                Outcome::Released { .. } => state.snoozed_until = None,
                Outcome::Cleared => state.cleared = true,
                Outcome::Seen => state.seen = true,
            }
        }
        state
    }
}

/// How a rule's notifications fared: the evidence behind "make it passive?".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Usefulness {
    pub shown: usize,
    pub opened: usize,
    pub dismissed_fast: usize,
    pub expired: usize,
}
impl Usefulness {
    pub fn of<'a>(states: impl IntoIterator<Item = &'a DeliveryState>) -> Self {
        let mut figures = Self::default();
        for state in states {
            figures.shown += usize::from(state.shown_at.is_some());
            figures.opened += usize::from(state.opened);
            figures.dismissed_fast += usize::from(state.dismissed_fast);
            figures.expired += usize::from(state.closed == Some(CloseReason::Expired));
        }
        figures
    }
}
