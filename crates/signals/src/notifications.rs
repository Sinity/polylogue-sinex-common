//! Pure desktop delivery values and checkpoint transitions. Platform I/O lives
//! in signal-delivery; both products serialize these exact shared values.
use crate::DeliveryOutcome;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
mod state;
pub use state::{Cancellation, Completion, GroupUpdate, ImportError, ImportStatus, Presentation};

/// The evaluator has already applied quiet hours and the shared host budget.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct Notice {
    pub delivery_id: String,
    pub product: String,
    pub icon: String,
    pub rule: String,
    pub occurrence: String,
    pub summary: String,
    pub body: String,
    pub reference: String,
    /// Shown when the server cannot offer actions.
    pub command_hint: String,
    pub interrupt: bool,
    pub current: bool,
    /// Critical urgency is reserved for capture losing data now.
    pub data_loss: bool,
    /// The evaluator supplies a cumulative count of budget-downgraded notices.
    /// Such notices update one host summary rather than showing individually.
    pub budget_passive_count: Option<u64>,
}

#[derive(
    Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    #[default]
    Dbus,
    Command,
    Both,
}

#[derive(Clone, Debug)]
pub struct RouteCommand {
    pub program: std::path::PathBuf,
    pub arguments: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct NotificationConfig {
    pub route: Route,
    pub command: Option<RouteCommand>,
    pub deadline: Duration,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DesktopEvent {
    /// The product opens this reference through its ordinary read operation.
    OpenReference {
        reference: String,
    },
    Seen,
    Acknowledged,
    Dismissed,
    Expired,
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
pub struct NotificationReceipt {
    pub delivery_id: String,
    pub route: Route,
    pub outcome: DeliveryOutcome,
    pub notification_id: Option<u32>,
    pub event: Option<DesktopEvent>,
    /// Desktop actions are operator acts, never acts by a rule's author.
    pub via: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Attempt {
    pub notice: Notice,
    pub dbus: Option<DeliveryOutcome>,
    pub command: Option<DeliveryOutcome>,
    pub notification_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notification_slot: Option<String>,
    #[serde(default)]
    pub shown: bool,
    pub retried: bool,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handoff: Option<String>,
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
