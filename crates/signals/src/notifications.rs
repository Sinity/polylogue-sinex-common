//! Desktop delivery runs after a product commits its outbox, outside its writer.
//! The product persists returned receipts and checkpoints, and supplies the current
//! condition check and the operations invoked by desktop actions.

use crate::DeliveryOutcome;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use zbus::zvariant::Value;

const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

/// The evaluator has already applied quiet hours and the shared host budget.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
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

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DesktopEvent {
    /// The product opens this reference through its ordinary read operation.
    OpenReference {
        reference: String,
    },
    Acknowledged,
    Dismissed,
    Expired,
    Closed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationReceipt {
    pub delivery_id: String,
    pub route: Route,
    pub outcome: DeliveryOutcome,
    pub notification_id: Option<u32>,
    pub event: Option<DesktopEvent>,
    /// Desktop actions are operator acts, never acts by a rule's author.
    pub via: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attempt {
    pub notice: Notice,
    pub dbus: Option<DeliveryOutcome>,
    pub command: Option<DeliveryOutcome>,
    pub notification_id: Option<u32>,
    pub retried: bool,
    pub active: bool,
}

/// Persist this beside the product's delivery records. An unknown attempt is
/// deliberately terminal: after a crash there is no safe automatic resend.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotificationState {
    pub attempts: BTreeMap<String, Attempt>,
    slots: BTreeMap<String, u32>,
    first_occurrence: BTreeMap<String, String>,
    server_owner: Option<String>,
}

pub struct Deliverer {
    connection: Option<zbus::Connection>,
    config: NotificationConfig,
    pub state: NotificationState,
    ready: BTreeSet<String>,
}

impl Deliverer {
    /// The caller supplies a session-bus connection, or None for a headless
    /// command-only route. Tests supply a private bus.
    pub fn new(
        connection: impl Into<Option<zbus::Connection>>,
        config: NotificationConfig,
        state: NotificationState,
    ) -> Self {
        Self {
            connection: connection.into(),
            config,
            state,
            ready: BTreeSet::new(),
        }
    }

    async fn proxy(&self) -> zbus::Result<zbus::Proxy<'static>> {
        let connection = self
            .connection
            .clone()
            .ok_or_else(|| zbus::Error::Failure("no_notification_server".into()))?;
        zbus::Proxy::new_owned(connection, DESTINATION, PATH, DESTINATION).await
    }

    async fn close_proxy(&self) -> zbus::Result<zbus::Proxy<'static>> {
        let connection = self
            .connection
            .clone()
            .ok_or_else(|| zbus::Error::Failure("no_notification_server".into()))?;
        let owner = self
            .state
            .server_owner
            .clone()
            .ok_or_else(|| zbus::Error::Failure("no_notification_server".into()))?;
        zbus::Proxy::new_owned(connection, owner, PATH, DESTINATION).await
    }

    /// Returns an initial unknown receipt before any effect. Persist it before
    /// calling `send`, so an interrupted attempt is never sent twice after restart.
    pub fn begin(&mut self, notice: Notice) -> Vec<NotificationReceipt> {
        if self.state.attempts.contains_key(&notice.delivery_id)
            || (!notice.interrupt && notice.budget_passive_count.is_none())
        {
            return Vec::new();
        }
        for prior in self.state.attempts.values_mut().filter(|a| {
            a.notice.product == notice.product
                && a.notice.rule == notice.rule
                && a.notice.occurrence == notice.occurrence
        }) {
            prior.active = false;
        }
        self.ready.insert(notice.delivery_id.clone());
        let mut attempt = Attempt {
            notice,
            dbus: None,
            command: None,
            notification_id: None,
            retried: false,
            active: true,
        };
        let mut receipts = Vec::new();
        if matches!(self.config.route, Route::Dbus | Route::Both) {
            attempt.dbus = Some(DeliveryOutcome::Unknown);
            receipts.push(receipt(&attempt, Route::Dbus));
        }
        if matches!(self.config.route, Route::Command | Route::Both) {
            attempt.command = Some(DeliveryOutcome::Unknown);
            receipts.push(receipt(&attempt, Route::Command));
        }
        self.state
            .attempts
            .insert(attempt.notice.delivery_id.clone(), attempt);
        receipts
    }

    /// Send only immediately after persisting `begin`'s unknown receipts. The
    /// owned worker calls this once; restored unknown attempts are never selected.
    pub async fn send(
        &mut self,
        delivery_id: &str,
        still_matches: &(dyn Fn(&Notice) -> bool + Send + Sync),
    ) -> Vec<NotificationReceipt> {
        if !self.ready.remove(delivery_id) {
            return Vec::new();
        }
        let Some(attempt) = self.state.attempts.get(delivery_id).cloned() else {
            return Vec::new();
        };
        if !attempt.active || (attempt.notice.current && !still_matches(&attempt.notice)) {
            return self.cancel(delivery_id).await;
        }
        let mut receipts = Vec::new();
        if attempt.dbus == Some(DeliveryOutcome::Unknown) {
            let (outcome, id) = self.dbus_send(&attempt.notice).await;
            let updated = self
                .state
                .attempts
                .get_mut(delivery_id)
                .expect("existing attempt");
            updated.dbus = Some(outcome);
            updated.notification_id = id;
            receipts.push(receipt(updated, Route::Dbus));
        }
        if attempt.command == Some(DeliveryOutcome::Unknown) {
            let outcome = self.command_send(&attempt.notice).await;
            let updated = self
                .state
                .attempts
                .get_mut(delivery_id)
                .expect("existing attempt");
            updated.command = Some(outcome);
            receipts.push(receipt(updated, Route::Command));
        }
        receipts
    }

    async fn dbus_send(&mut self, notice: &Notice) -> (DeliveryOutcome, Option<u32>) {
        let deadline = self.config.deadline;
        let result = tokio::time::timeout(deadline, async {
            let connection = self
                .connection
                .clone()
                .ok_or_else(|| zbus::Error::Failure("no_notification_server".into()))?;
            let owner = connection
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "GetNameOwner",
                    &(DESTINATION,),
                )
                .await?
                .body()
                .deserialize::<String>()?;
            if self.state.server_owner.as_ref() != Some(&owner) {
                self.invalidate_server();
                self.state.server_owner = Some(owner.clone());
            }
            let slot = self.slot(notice);
            let replaces = self.state.slots.get(&slot).copied().unwrap_or(0);
            // Pin the destination to this owner so a restarted service can never
            // receive an ID belonging to its predecessor.
            let proxy = zbus::Proxy::new_owned(connection, owner, PATH, DESTINATION).await?;
            let capabilities: Vec<String> = proxy.call("GetCapabilities", &()).await?;
            let actions = if capabilities.iter().any(|s| s == "actions") {
                vec!["default", "Open", "acknowledge", "Acknowledge"]
            } else {
                Vec::new()
            };
            let mut body = notice.body.clone();
            if actions.is_empty() && !notice.command_hint.is_empty() {
                body.push('\n');
                body.push_str(&notice.command_hint);
            }
            let summary = if let Some(count) = notice.budget_passive_count {
                format!("{count} notices held passive (interrupt budget spent)")
            } else if slot.starts_with("summary:") {
                format!(
                    "{}: {} open occurrences",
                    notice.summary,
                    self.open_occurrences(notice).len()
                )
            } else {
                notice.summary.clone()
            };
            let mut hints = HashMap::new();
            let data_loss = notice.data_loss
                || (slot.starts_with("summary:")
                    && self.state.attempts.values().any(|a| {
                        a.active
                            && a.notice.data_loss
                            && a.notice.product == notice.product
                            && a.notice.rule == notice.rule
                    }));
            hints.insert("urgency", Value::U8(if data_loss { 2 } else { 1 }));
            let category = format!("{}.{}", notice.product, notice.rule);
            hints.insert("category", Value::from(category.as_str()));
            let id: u32 = proxy
                .call(
                    "Notify",
                    &(
                        notice.product.as_str(),
                        replaces,
                        notice.icon.as_str(),
                        summary.as_str(),
                        body.as_str(),
                        actions,
                        hints,
                        if data_loss { 0i32 } else { -1i32 },
                    ),
                )
                .await?;
            Ok::<_, zbus::Error>((id, slot))
        })
        .await;
        match result {
            Ok(Ok((id, slot))) => {
                self.state.slots.insert(slot, id);
                (DeliveryOutcome::Delivered, Some(id))
            }
            Ok(Err(error)) => (DeliveryOutcome::NotSent(dbus_reason(&error)), None),
            Err(_) => (DeliveryOutcome::Unknown, None),
        }
    }

    fn group(notice: &Notice) -> String {
        // JSON tuples keep product/rule/occurrence separators unambiguous.
        serde_json::to_string(&(&notice.product, &notice.rule)).expect("string tuple")
    }

    fn open_occurrences(&self, notice: &Notice) -> BTreeSet<String> {
        self.state
            .attempts
            .values()
            .filter(|a| {
                a.active
                    && a.notice.budget_passive_count.is_none()
                    && a.notice.product == notice.product
                    && a.notice.rule == notice.rule
            })
            .map(|a| a.notice.occurrence.clone())
            .collect()
    }

    fn slot(&mut self, notice: &Notice) -> String {
        if notice.budget_passive_count.is_some() {
            return "budget".into();
        }
        let group = Self::group(notice);
        let first = self
            .state
            .first_occurrence
            .entry(group.clone())
            .or_insert_with(|| notice.occurrence.clone());
        if *first == notice.occurrence {
            format!("occurrence:{group}")
        } else {
            format!("summary:{group}")
        }
    }

    async fn command_send(&self, notice: &Notice) -> DeliveryOutcome {
        let Some(command) = &self.config.command else {
            return DeliveryOutcome::NotSent("notification_command_not_configured".into());
        };
        let input = match serde_json::to_vec(notice) {
            Ok(input) => input,
            Err(error) => return DeliveryOutcome::NotSent(error.to_string()),
        };
        let mut child = match Command::new(&command.program)
            .args(&command.arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => return DeliveryOutcome::NotSent(format!("command_spawn:{error}")),
        };
        let result = tokio::time::timeout(self.config.deadline, async {
            let mut stdin = child.stdin.take().expect("piped stdin");
            stdin.write_all(&input).await?;
            stdin.shutdown().await?;
            drop(stdin);
            child.wait().await
        })
        .await;
        match result {
            Ok(Ok(status)) if status.success() => DeliveryOutcome::Delivered,
            Ok(Ok(status)) => DeliveryOutcome::NotSent(format!("command_exit:{status}")),
            Ok(Err(error)) => DeliveryOutcome::NotSent(format!("command_io:{error}")),
            Err(_) => {
                let _ = child.kill().await;
                let _ = child.wait().await;
                DeliveryOutcome::Unknown
            }
        }
    }

    /// Called on service appearance, never on a timer. Only one retry is allowed,
    /// under the original identity, and current state is checked immediately.
    pub fn server_appeared(&mut self) -> Vec<NotificationReceipt> {
        self.invalidate_server();
        let mut receipts = Vec::new();
        for (id, attempt) in &mut self.state.attempts {
            if !attempt.retried
                && attempt.dbus == Some(DeliveryOutcome::NotSent("no_notification_server".into()))
            {
                attempt.retried = true;
                attempt.dbus = Some(DeliveryOutcome::Unknown);
                self.ready.insert(id.clone());
                receipts.push(receipt(attempt, Route::Dbus));
            }
        }
        receipts
    }

    fn invalidate_server(&mut self) {
        self.state.slots.clear();
        for attempt in self.state.attempts.values_mut() {
            attempt.notification_id = None;
        }
    }

    /// A product sends this when a current condition clears, even during quiet
    /// time or when the notification server is absent.
    pub async fn cancel(&mut self, delivery_id: &str) -> Vec<NotificationReceipt> {
        let Some(attempt) = self.state.attempts.get_mut(delivery_id) else {
            return Vec::new();
        };
        attempt.active = false;
        let notice = attempt.notice.clone();
        let id = attempt.notification_id;
        let mut receipts = Vec::new();
        for route in [Route::Dbus, Route::Command] {
            let outcome = if route == Route::Dbus {
                &mut attempt.dbus
            } else {
                &mut attempt.command
            };
            if outcome.is_some() && *outcome != Some(DeliveryOutcome::Delivered) {
                *outcome = Some(DeliveryOutcome::Cancelled);
                receipts.push(receipt(attempt, route));
            }
        }
        // A grouped notification belongs to all of its live occurrences.
        let shared = id.is_some()
            && self
                .state
                .attempts
                .values()
                .any(|other| other.active && other.notification_id == id);
        if !shared && let Some(id) = id {
            let _ = tokio::time::timeout(self.config.deadline, async {
                self.close_proxy()
                    .await?
                    .call::<_, _, ()>("CloseNotification", &(id,))
                    .await
            })
            .await;
            self.state.slots.retain(|_, value| *value != id);
        }
        let group = Self::group(&notice);
        let open = self.open_occurrences(&notice);
        let first = self.state.first_occurrence.get(&group).cloned();
        if let Some(first) = first
            && !open.contains(&first)
        {
            self.state.first_occurrence.remove(&group);
            self.state.slots.remove(&format!("occurrence:{group}"));
            if let Some(next) = open.first() {
                self.state
                    .first_occurrence
                    .insert(group.clone(), next.clone());
                if let Some(id) = self.state.slots.remove(&format!("summary:{group}")) {
                    self.state.slots.insert(format!("occurrence:{group}"), id);
                }
            }
        }
        if open.len() <= 1 {
            if let Some(id) = self.state.slots.remove(&format!("summary:{group}")) {
                let _ = tokio::time::timeout(self.config.deadline, async {
                    self.close_proxy()
                        .await?
                        .call::<_, _, ()>("CloseNotification", &(id,))
                        .await
                })
                .await;
            }
        } else if matches!(self.config.route, Route::Dbus | Route::Both) {
            let first = self.state.first_occurrence.get(&group);
            let representative = self
                .state
                .attempts
                .values()
                .find(|a| {
                    a.active
                        && a.notice.product == notice.product
                        && a.notice.rule == notice.rule
                        && Some(&a.notice.occurrence) != first
                })
                .map(|a| a.notice.clone());
            if let Some(representative) = representative {
                let (outcome, notification_id) = self.dbus_send(&representative).await;
                let attempt = self
                    .state
                    .attempts
                    .get_mut(&representative.delivery_id)
                    .expect("active occurrence");
                attempt.dbus = Some(outcome);
                attempt.notification_id = notification_id;
                receipts.push(receipt(attempt, Route::Dbus));
            }
        }
        receipts
    }

    /// Converts protocol signals into delivery records. The product executes the
    /// returned OpenReference through its operation table as an operator act.
    pub fn desktop_event(
        &mut self,
        notification_id: u32,
        event: DesktopEvent,
    ) -> Vec<NotificationReceipt> {
        let mut receipts = Vec::new();
        for attempt in self
            .state
            .attempts
            .values_mut()
            .filter(|a| a.active && a.notification_id == Some(notification_id))
        {
            let event = match &event {
                DesktopEvent::OpenReference { .. } => DesktopEvent::OpenReference {
                    reference: attempt.notice.reference.clone(),
                },
                event => event.clone(),
            };
            if matches!(
                event,
                DesktopEvent::Dismissed | DesktopEvent::Expired | DesktopEvent::Closed
            ) {
                attempt.active = false;
            }
            let mut receipt = receipt(attempt, Route::Dbus);
            receipt.event = Some(event);
            receipt.via = Some("desktop".into());
            receipts.push(receipt);
        }
        if matches!(
            event,
            DesktopEvent::Dismissed | DesktopEvent::Expired | DesktopEvent::Closed
        ) {
            self.state.slots.retain(|_, id| *id != notification_id);
            self.state.first_occurrence.retain(|group, occurrence| {
                self.state.attempts.values().any(|a| {
                    a.active
                        && Self::group(&a.notice) == *group
                        && a.notice.occurrence == *occurrence
                })
            });
        }
        receipts
    }
}

fn receipt(attempt: &Attempt, route: Route) -> NotificationReceipt {
    NotificationReceipt {
        delivery_id: attempt.notice.delivery_id.clone(),
        route,
        outcome: if route == Route::Dbus {
            attempt.dbus.clone()
        } else {
            attempt.command.clone()
        }
        .unwrap_or(DeliveryOutcome::Unknown),
        notification_id: attempt.notification_id,
        event: None,
        via: None,
    }
}

fn dbus_reason(error: &zbus::Error) -> String {
    match error {
        zbus::Error::Failure(reason) if reason == "no_notification_server" => reason.clone(),
        zbus::Error::MethodError(name, _, _)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.ServiceUnknown"
                    | "org.freedesktop.DBus.Error.NameHasNoOwner"
            ) =>
        {
            "no_notification_server".into()
        }
        _ => format!("dbus:{error}"),
    }
}

pub enum NotificationRequest {
    Deliver(Box<Notice>),
    Clear(String),
}

/// The product persists each update before allowing the worker to continue.
/// This handshake makes a crash between sending and receipt safe: it leaves an
/// unknown attempt rather than a candidate for automatic replay.
pub struct NotificationUpdate {
    pub receipts: Vec<NotificationReceipt>,
    pub state: NotificationState,
    pub persisted: tokio::sync::oneshot::Sender<()>,
}

pub type ConditionCheck = Arc<dyn Fn(&Notice) -> bool + Send + Sync>;

/// Runs one host's deliverer. The unbounded channel carries outbox wakeups outside
/// the writer; durable outbox storage remains owned by the product.
pub async fn run(
    mut deliverer: Deliverer,
    mut requests: mpsc::UnboundedReceiver<NotificationRequest>,
    updates: mpsc::UnboundedSender<NotificationUpdate>,
    still_matches: ConditionCheck,
) -> zbus::Result<()> {
    if deliverer.config.route == Route::Command {
        while let Some(request) = requests.recv().await {
            if !handle_request(&mut deliverer, request, &updates, still_matches.as_ref()).await {
                break;
            }
        }
        return Ok(());
    }
    let proxy = deliverer.proxy().await?;
    let mut desktop = proxy.receive_all_signals().await?;
    let connection = deliverer
        .connection
        .clone()
        .ok_or_else(|| zbus::Error::Failure("no_notification_server".into()))?;
    let bus = zbus::Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let mut owners = bus.receive_signal("NameOwnerChanged").await?;
    let owner_reply = connection
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(DESTINATION,),
        )
        .await;
    let owner_position = match &owner_reply {
        Ok(message) | Err(zbus::Error::MethodError(_, _, message)) => Some(message.recv_position()),
        _ => None,
    };
    let owner = owner_reply
        .ok()
        .and_then(|message| message.body().deserialize::<String>().ok());
    let retry_waiting = deliverer.state.attempts.values().any(|a| {
        !a.retried && a.dbus == Some(DeliveryOutcome::NotSent("no_notification_server".into()))
    });
    if let Some(owner) = owner {
        if deliverer.state.server_owner.as_ref() != Some(&owner) || retry_waiting {
            deliverer.state.server_owner = Some(owner);
            if !appearance(&mut deliverer, &updates, still_matches.as_ref()).await {
                return Ok(());
            }
        }
    } else if deliverer.state.server_owner.take().is_some() {
        deliverer.invalidate_server();
        if !persist(&deliverer, Vec::new(), &updates).await {
            return Ok(());
        }
    }
    loop {
        tokio::select! {
            request = requests.recv() => {
                match request {
                    Some(request) => {
                        if !handle_request(&mut deliverer, request, &updates, still_matches.as_ref()).await { break; }
                    }
                    None => break,
                }
            }
            Some(signal) = owners.next() => {
                // The startup probe already includes owner changes preceding its
                // reply. Applying those queued signals again would discard IDs
                // returned by a retry performed after that probe.
                if owner_position.is_some_and(|position| signal.recv_position() <= position) { continue; }
                if let Ok((name, _, new)) = signal.body().deserialize::<(String, String, String)>()
                    && name == DESTINATION
                {
                    if new.is_empty() {
                        deliverer.state.server_owner = None;
                        deliverer.invalidate_server();
                        if !persist(&deliverer, Vec::new(), &updates).await { return Ok(()); }
                    } else {
                        deliverer.state.server_owner = Some(new);
                        if !appearance(&mut deliverer, &updates, still_matches.as_ref()).await { return Ok(()); }
                    }
                }
            }
            Some(signal) = desktop.next() => {
                let event = match signal.header().member().map(|m| m.as_str()) {
                    Some("ActionInvoked") => signal.body().deserialize::<(u32, String)>().ok().and_then(|(id, action)| {
                        match action.as_str() {
                            "default" => Some((id, DesktopEvent::OpenReference { reference: String::new() })),
                            "acknowledge" => Some((id, DesktopEvent::Acknowledged)),
                            _ => None,
                        }
                    }),
                    Some("NotificationClosed") => signal.body().deserialize::<(u32, u32)>().ok().map(|(id, reason)| {
                        (id, match reason { 1 => DesktopEvent::Expired, 2 => DesktopEvent::Dismissed, _ => DesktopEvent::Closed })
                    }),
                    _ => None,
                };
                if let Some((id, event)) = event {
                    let receipts = deliverer.desktop_event(id, event);
                    if !persist(&deliverer, receipts, &updates).await { break; }
                }
            }
        }
    }
    Ok(())
}

async fn appearance(
    deliverer: &mut Deliverer,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
    still_matches: &(dyn Fn(&Notice) -> bool + Send + Sync),
) -> bool {
    let receipts = deliverer.server_appeared();
    let ids: Vec<_> = receipts.iter().map(|r| r.delivery_id.clone()).collect();
    if !persist(deliverer, receipts, updates).await {
        return false;
    }
    for id in ids {
        let receipts = deliverer.send(&id, still_matches).await;
        if !persist(deliverer, receipts, updates).await {
            return false;
        }
    }
    true
}

async fn handle_request(
    deliverer: &mut Deliverer,
    request: NotificationRequest,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
    still_matches: &(dyn Fn(&Notice) -> bool + Send + Sync),
) -> bool {
    match request {
        NotificationRequest::Deliver(notice) => {
            let id = notice.delivery_id.clone();
            let began = deliverer.begin(*notice);
            if began.is_empty() {
                return true;
            }
            if !persist(deliverer, began, updates).await {
                return false;
            }
            let receipts = deliverer.send(&id, still_matches).await;
            persist(deliverer, receipts, updates).await
        }
        NotificationRequest::Clear(id) => {
            let receipts = deliverer.cancel(&id).await;
            persist(deliverer, receipts, updates).await
        }
    }
}

async fn persist(
    deliverer: &Deliverer,
    receipts: Vec<NotificationReceipt>,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
) -> bool {
    let (persisted, wait) = tokio::sync::oneshot::channel();
    if updates
        .send(NotificationUpdate {
            receipts,
            state: deliverer.state.clone(),
            persisted,
        })
        .is_err()
    {
        return false;
    }
    wait.await.is_ok()
}
