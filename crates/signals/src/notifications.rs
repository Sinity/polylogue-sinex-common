//! Pure desktop notification values and checkpoint transitions. Platform I/O
//! lives in signal-delivery; both products serialize these exact values.
//! Routing has already decided that each notice interrupts and where it goes.
use crate::{Decision, DeliveryOutcome, Instant, Summary};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
mod state;
pub use state::{Cancellation, Completion, GroupUpdate, Presentation};

/// Where an interrupting notice goes: the desktop, the away command, or both
/// (a critical notice while the operator is away).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Outlets {
    pub desktop: bool,
    pub command: bool,
}
impl Outlets {
    pub fn of(decision: &Decision) -> Self {
        match decision {
            Decision::Show { send_away } => Self {
                desktop: true,
                command: *send_away,
            },
            Decision::SendAway => Self {
                desktop: false,
                command: true,
            },
            _ => Self::default(),
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Outlet {
    Desktop,
    Command,
}

/// A button a rule supplies besides Open, Snooze and Acknowledge (Live
/// sessions: Approve, Deny).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoticeAction {
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Notice {
    pub delivery_id: String,
    pub product: String,
    pub icon: String,
    /// The product's web app, for the `desktop-entry` hint.
    pub desktop_entry: Option<String>,
    pub rule: String,
    pub occurrence: String,
    /// `<subject> · <what>`, one line.
    pub summary: String,
    pub body: String,
    /// Lines of consequences folded into this root cause.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    pub reference: String,
    /// Shown when the server cannot offer actions.
    pub command_hint: String,
    /// A current-state notice stays until its condition clears (`resident`).
    pub current: bool,
    /// Kept out of the server's history (`transient`).
    pub transient: bool,
    /// Only where captured data is being lost now.
    pub critical: bool,
    pub outlets: Outlets,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<NoticeAction>,
    /// Each showing of the same delivery counts up; a snoozed notice shown
    /// again is a new showing under the same identity.
    #[serde(default)]
    pub showing: u32,
}
impl Notice {
    /// The notice for a summary notification, owned by the deliverer's product.
    pub fn for_summary(summary: &Summary, product: &str, icon: &str, reference: &str) -> Self {
        Self {
            delivery_id: summary.id.clone(),
            product: product.into(),
            icon: icon.into(),
            desktop_entry: None,
            rule: "summary".into(),
            occurrence: summary.occurrence(),
            summary: summary.title(),
            body: String::new(),
            lines: Vec::new(),
            reference: reference.into(),
            command_hint: String::new(),
            current: false,
            transient: false,
            critical: false,
            outlets: Outlets {
                desktop: true,
                command: false,
            },
            actions: Vec::new(),
            showing: 0,
        }
    }
}

/// The hint carrying a delivery ID, `x-<product>-delivery`. Sinex's D-Bus
/// source marks a captured notification carrying it as a signal origin, so no
/// rule reads the products' own notifications.
pub fn delivery_hint(product: &str) -> String {
    format!("x-{}-delivery", product.to_lowercase())
}
/// The product and delivery a captured notification was made for, if any.
pub fn signal_origin<'a>(
    hints: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<(String, String)> {
    hints.into_iter().find_map(|(key, value)| {
        let product = key.strip_prefix("x-")?.strip_suffix("-delivery")?;
        (!product.is_empty()).then(|| (product.to_string(), value.to_string()))
    })
}

#[derive(Clone, Debug)]
pub struct RouteCommand {
    pub program: std::path::PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct NotificationConfig {
    /// The deployment's away command: receives each sent-away notice as JSON
    /// on standard input.
    pub away_command: Option<RouteCommand>,
    /// How long one platform call may take before its outcome is unknown.
    pub deadline: std::time::Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DesktopEvent {
    /// The default action. The product opens the reference, passing the
    /// activation token to the browser launch so focus moves under Wayland.
    OpenReference {
        reference: String,
        activation_token: Option<String>,
    },
    Snooze,
    Acknowledged,
    /// A rule-supplied action.
    Action {
        key: String,
    },
    /// Another notice of a summary was opened.
    Seen,
    Dismissed,
    Expired,
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct NotificationReceipt {
    pub delivery_id: String,
    pub outlet: Outlet,
    pub outcome: DeliveryOutcome,
    pub notification_id: Option<u32>,
    pub event: Option<DesktopEvent>,
    /// Desktop actions are operator acts, never acts by a rule's author.
    pub via: Option<String>,
}
impl NotificationReceipt {
    /// The outcomes this receipt appends to its delivery.
    pub fn outcomes(&self, at: Instant) -> Vec<crate::OutcomeRecord> {
        use crate::{CloseReason, Outcome, Via};
        let mut outcomes = Vec::new();
        match &self.event {
            None => {
                outcomes.push(Outcome::Attempt {
                    result: self.outcome.clone(),
                });
                if self.outlet == Outlet::Desktop && self.outcome == DeliveryOutcome::Delivered {
                    outcomes.push(Outcome::Shown);
                }
            }
            Some(DesktopEvent::OpenReference { .. }) => {
                outcomes.push(Outcome::Opened { via: Via::Desktop })
            }
            Some(DesktopEvent::Acknowledged) => outcomes.push(Outcome::Acknowledged),
            Some(DesktopEvent::Seen) => outcomes.push(Outcome::Seen),
            Some(DesktopEvent::Dismissed) => outcomes.push(Outcome::Closed {
                reason: CloseReason::Dismissed,
            }),
            Some(DesktopEvent::Expired) => outcomes.push(Outcome::Closed {
                reason: CloseReason::Expired,
            }),
            Some(DesktopEvent::Closed) => outcomes.push(Outcome::Closed {
                reason: CloseReason::Cleared,
            }),
            // The product applies these acts through the host policy.
            Some(DesktopEvent::Snooze | DesktopEvent::Action { .. }) => {}
        }
        outcomes
            .into_iter()
            .map(|outcome| crate::OutcomeRecord {
                delivery: self.delivery_id.clone(),
                at,
                outcome,
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Attempt {
    pub notice: Notice,
    pub desktop: Option<DeliveryOutcome>,
    pub command: Option<DeliveryOutcome>,
    pub notification_id: Option<u32>,
    pub notification_slot: Option<String>,
    pub shown: bool,
    pub retried: bool,
    pub active: bool,
    pub server_owner: Option<String>,
}

/// Persist this beside the product's delivery records. An unknown attempt is
/// deliberately terminal: after a crash there is no safe automatic resend.
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct NotificationState {
    attempts: BTreeMap<String, Attempt>,
    slots: BTreeMap<String, u32>,
    first_occurrence: BTreeMap<String, String>,
    server_owner: Option<String>,
    shown_delivery: BTreeMap<u32, String>,
}
