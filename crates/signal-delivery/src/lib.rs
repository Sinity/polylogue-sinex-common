//! Desktop notifications and the away command over the pure signals
//! notification state. One task takes committed notices from a channel, calls
//! `Notify` or the away command with a deadline, and returns outcomes; holds
//! and rechecks belong to the host policy in `signals`.
use futures_util::StreamExt;
use signals::DeliveryOutcome;
use signals::notifications::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{Instant, timeout_at};
use zbus::zvariant::Value;
const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub struct Deliverer {
    connection: Option<zbus::Connection>,
    config: NotificationConfig,
    pub state: NotificationState,
    ready: BTreeMap<String, BTreeSet<Outlet>>,
    owner_position: Option<zbus::message::Sequence>,
    /// The latest activation token the server sent for each notification.
    tokens: HashMap<u32, String>,
}

struct PreparedDesktop {
    proxy: zbus::Proxy<'static>,
    owner: String,
    actions: bool,
}

impl Deliverer {
    /// The caller supplies a session-bus connection, or None when unavailable.
    /// Missing-bus attempts are recorded while the away command still runs.
    /// Tests supply a private bus.
    pub fn new(
        connection: impl Into<Option<zbus::Connection>>,
        config: NotificationConfig,
        state: NotificationState,
    ) -> Self {
        Self {
            connection: connection.into(),
            config,
            state,
            ready: BTreeMap::new(),
            owner_position: None,
            tokens: HashMap::new(),
        }
    }

    fn deadline(&self) -> Instant {
        Instant::now() + self.config.deadline
    }

    /// Returns the unknown receipts recorded before any effect. Persist them
    /// before calling `send`, so an interrupted attempt is never sent twice
    /// after a restart.
    pub fn begin(&mut self, notice: Notice) -> Vec<NotificationReceipt> {
        let receipts = self.state.begin(notice);
        for r in &receipts {
            self.ready
                .entry(r.delivery_id.clone())
                .or_default()
                .insert(r.outlet);
        }
        receipts
    }
    /// Runs the persisted unknown outlets of one notice. Restored unknown
    /// outlets are never selected automatically.
    pub async fn send(&mut self, id: &str) -> Vec<NotificationReceipt> {
        let Some(outlets) = self.ready.remove(id) else {
            return Vec::new();
        };
        let mut receipts = Vec::new();
        for outlet in outlets {
            let Some(attempt) = self.state.attempt(id).cloned().filter(|a| a.active) else {
                break;
            };
            let outcome = match outlet {
                Outlet::Desktop => &attempt.desktop,
                Outlet::Command => &attempt.command,
            };
            if *outcome != Some(DeliveryOutcome::Unknown) {
                continue;
            }
            let deadline = self.deadline();
            let completion = match outlet {
                Outlet::Desktop => match self.prepare_desktop(deadline).await {
                    Ok(prepared) => self.notify(&attempt.notice, prepared, deadline).await,
                    Err(completion) => completion,
                },
                Outlet::Command => Completion {
                    outcome: self.command_send(&attempt.notice, deadline).await,
                    notification_id: None,
                    server_owner: None,
                    slot: None,
                },
            };
            receipts.push(self.state.completed(id, outlet, completion));
        }
        receipts
    }
    async fn prepare_desktop(&mut self, deadline: Instant) -> Result<PreparedDesktop, Completion> {
        let mut used_owner = None;
        let result = timeout_at(deadline, async {
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
            used_owner = Some(owner.clone());
            self.state.set_server_owner(Some(owner.clone()));
            let proxy =
                zbus::Proxy::new_owned(connection, owner.clone(), PATH, DESTINATION).await?;
            let capabilities: Vec<String> = proxy.call("GetCapabilities", &()).await?;
            Ok::<_, zbus::Error>(PreparedDesktop {
                proxy,
                owner,
                actions: capabilities.iter().any(|c| c == "actions"),
            })
        })
        .await;
        let outcome = match result {
            Ok(Ok(prepared)) => return Ok(prepared),
            Ok(Err(error)) => DeliveryOutcome::NotSent(dbus_reason(&error)),
            // Nothing was shown yet, so the attempt may be retried.
            Err(_) => DeliveryOutcome::NotSent("deadline".into()),
        };
        Err(Completion {
            outcome,
            notification_id: None,
            server_owner: used_owner,
            slot: None,
        })
    }

    async fn notify(
        &mut self,
        notice: &Notice,
        prepared: PreparedDesktop,
        deadline: Instant,
    ) -> Completion {
        let view = self.state.presentation(notice, prepared.actions);
        let mut actions: Vec<&str> = Vec::new();
        if prepared.actions {
            actions.extend([
                "default",
                "Open",
                "snooze",
                "Snooze 1h",
                "acknowledge",
                "Acknowledge",
            ]);
            for action in &notice.actions {
                actions.extend([action.key.as_str(), action.label.as_str()]);
            }
        }
        let category = format!("{}.{}", notice.product, notice.rule);
        let mut hints = HashMap::new();
        hints.insert(
            "urgency".to_string(),
            Value::U8(if view.critical { 2 } else { 1 }),
        );
        hints.insert("category".to_string(), Value::from(category.as_str()));
        hints.insert(
            delivery_hint(&notice.product),
            Value::from(notice.delivery_id.as_str()),
        );
        if let Some(entry) = &notice.desktop_entry {
            hints.insert("desktop-entry".to_string(), Value::from(entry.as_str()));
        }
        if notice.transient {
            hints.insert("transient".to_string(), Value::Bool(true));
        }
        if notice.current {
            hints.insert("resident".to_string(), Value::Bool(true));
        }
        let arguments = (
            notice.product.as_str(),
            view.replaces,
            notice.icon.as_str(),
            view.summary.as_str(),
            view.body.as_str(),
            actions,
            hints,
            if view.critical { 0i32 } else { -1i32 },
        );
        let call = prepared.proxy.call::<_, _, u32>("Notify", &arguments);
        // Once Notify is issued, a missing answer leaves the outcome unknown:
        // the server may have shown it, so it is never sent again.
        let (outcome, id, slot) = match timeout_at(deadline, call).await {
            Ok(Ok(id)) => (DeliveryOutcome::Delivered, Some(id), Some(view.slot)),
            Ok(Err(e)) => (DeliveryOutcome::NotSent(dbus_reason(&e)), None, None),
            Err(_) => (DeliveryOutcome::Unknown, None, None),
        };
        Completion {
            outcome,
            notification_id: id,
            server_owner: Some(prepared.owner),
            slot,
        }
    }
    async fn command_send(&mut self, notice: &Notice, deadline: Instant) -> DeliveryOutcome {
        let Some(command) = &self.config.away_command else {
            return DeliveryOutcome::NotSent("away_command_not_configured".into());
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
        let result = timeout_at(deadline, async {
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
            Ok(Err(_)) => DeliveryOutcome::Unknown,
            Err(_) => {
                let _ = child.kill().await;
                DeliveryOutcome::Unknown
            }
        }
    }

    /// Called on service appearance, never on a timer. Each explicit desktop
    /// failure is retried once under its original identity.
    pub fn server_appeared(&mut self) -> Vec<NotificationReceipt> {
        let receipts = self.state.server_appeared();
        for r in &receipts {
            self.ready
                .entry(r.delivery_id.clone())
                .or_default()
                .insert(Outlet::Desktop);
        }
        receipts
    }
    async fn close(&mut self, ids: Vec<u32>) {
        let Some(owner) = self.state.server_owner().map(str::to_owned) else {
            return;
        };
        let Some(connection) = self.connection.clone() else {
            return;
        };
        for id in ids {
            let deadline = self.deadline();
            let _ = timeout_at(deadline, async {
                zbus::Proxy::new_owned(connection.clone(), owner.clone(), PATH, DESTINATION)
                    .await?
                    .call::<_, _, ()>("CloseNotification", &(id,))
                    .await
            })
            .await;
        }
    }
    /// The condition cleared or the operator acted: deactivate the occurrence,
    /// close its notification and refresh the rule's group. Grouped
    /// replacements are recorded unknown and become ready to send.
    pub async fn cancel(&mut self, id: &str) -> Vec<NotificationReceipt> {
        let Some(notice) = self.state.attempt(id).map(|a| a.notice.clone()) else {
            return Vec::new();
        };
        let cancelled = self.state.deactivate(id);
        for id in &cancelled.deactivated {
            self.ready.remove(id);
        }
        let mut receipts = cancelled.receipts;
        self.close(cancelled.close).await;
        let update = self.state.group_update(&notice);
        self.close(update.close).await;
        for next in update.notices {
            if let Some(prepared) = self.state.prepare_refresh(&next.delivery_id) {
                self.ready
                    .entry(next.delivery_id)
                    .or_default()
                    .insert(Outlet::Desktop);
                receipts.push(prepared);
            }
        }
        receipts
    }
    pub fn ready_ids(&self) -> Vec<String> {
        self.ready.keys().cloned().collect()
    }
    pub fn desktop_event(&mut self, id: u32, event: DesktopEvent) -> Vec<NotificationReceipt> {
        let event = match event {
            DesktopEvent::OpenReference {
                reference,
                activation_token: None,
            } => DesktopEvent::OpenReference {
                reference,
                activation_token: self.tokens.remove(&id),
            },
            event => event,
        };
        self.state.desktop_event(id, event)
    }
    async fn probe_owner(&mut self) -> Option<String> {
        let connection = self.connection.clone()?;
        let result = timeout_at(
            self.deadline(),
            connection.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(DESTINATION,),
            ),
        )
        .await
        .ok()?;
        self.owner_position = match &result {
            Ok(m) | Err(zbus::Error::MethodError(_, _, m)) => Some(m.recv_position()),
            _ => self.owner_position,
        };
        result.ok()?.body().deserialize().ok()
    }
    /// The action a server reported, as the event the product records.
    fn action_event(&self, id: u32, action: &str) -> Option<DesktopEvent> {
        match action {
            "default" => Some(DesktopEvent::OpenReference {
                reference: String::new(),
                activation_token: None,
            }),
            "snooze" => Some(DesktopEvent::Snooze),
            "acknowledge" => Some(DesktopEvent::Acknowledged),
            key => self
                .state
                .attempts()
                .values()
                .filter(|a| a.active && a.notification_id == Some(id))
                .any(|a| a.notice.actions.iter().any(|action| action.key == key))
                .then(|| DesktopEvent::Action { key: key.into() }),
        }
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
    /// An interrupting notice the host policy handed off, already committed.
    Deliver(Box<Notice>),
    /// A delivery's condition cleared, or the operator acknowledged it.
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

/// Runs one host's deliverer until the request channel closes or the product
/// stops acknowledging updates. Committed notices arrive through the channel,
/// never through a writer lock, so a slow server never holds a writer.
pub async fn run(
    mut deliverer: Deliverer,
    mut requests: mpsc::UnboundedReceiver<NotificationRequest>,
    updates: mpsc::UnboundedSender<NotificationUpdate>,
) -> zbus::Result<()> {
    let Some(connection) = deliverer.connection.clone() else {
        while let Some(request) = requests.recv().await {
            if !handle_request(&mut deliverer, request, &updates).await {
                break;
            }
        }
        return Ok(());
    };
    let proxy = zbus::Proxy::new_owned(connection.clone(), DESTINATION, PATH, DESTINATION).await?;
    let mut desktop = proxy.receive_all_signals().await?;
    let bus = zbus::Proxy::new(
        &connection,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .await?;
    let mut owners = bus.receive_signal("NameOwnerChanged").await?;
    let server_owner = deliverer.probe_owner().await;
    let retry_waiting = deliverer.state.attempts().values().any(|a| {
        a.active
            && !a.retried
            && a.desktop == Some(DeliveryOutcome::NotSent("no_notification_server".into()))
    });
    if let Some(server_owner) = server_owner {
        if deliverer.state.server_owner() != Some(server_owner.as_str()) || retry_waiting {
            deliverer.state.set_server_owner(Some(server_owner));
            if !appearance(&mut deliverer, &updates).await {
                return Ok(());
            }
        }
    } else if deliverer.state.server_owner().is_some() {
        deliverer.state.set_server_owner(None);
        if !persist(&mut deliverer, Vec::new(), &updates).await {
            return Ok(());
        }
    }
    loop {
        tokio::select! {
            request = requests.recv() => {
                let Some(request) = request else { break };
                if !handle_request(&mut deliverer, request, &updates).await { break; }
            }
            Some(signal) = owners.next() => {
                // The startup probe already includes owner changes preceding its
                // reply. Applying those queued signals again would discard IDs
                // returned by a retry performed after that probe.
                if deliverer.owner_position.is_some_and(|position| signal.recv_position() <= position) { continue; }
                if let Ok((name, _, new)) = signal.body().deserialize::<(String, String, String)>()
                    && name == DESTINATION
                {
                    if new.is_empty() {
                        deliverer.state.set_server_owner(None);
                        if !persist(&mut deliverer, Vec::new(), &updates).await { break; }
                    } else {
                        deliverer.state.set_server_owner(Some(new));
                        if !appearance(&mut deliverer, &updates).await { break; }
                    }
                }
            }
            Some(signal) = desktop.next() => {
                let event = match signal.header().member().map(|m| m.as_str()) {
                    Some("ActivationToken") => {
                        if let Ok((id, token)) = signal.body().deserialize::<(u32, String)>() {
                            deliverer.tokens.insert(id, token);
                        }
                        None
                    }
                    Some("ActionInvoked") => signal.body().deserialize::<(u32, String)>().ok().and_then(|(id, action)| {
                        deliverer.action_event(id, &action).map(|event| (id, event))
                    }),
                    Some("NotificationClosed") => signal.body().deserialize::<(u32, u32)>().ok().map(|(id, reason)| {
                        deliverer.tokens.remove(&id);
                        (id, match reason { 1 => DesktopEvent::Expired, 2 => DesktopEvent::Dismissed, _ => DesktopEvent::Closed })
                    }),
                    _ => None,
                };
                if let Some((id, event)) = event {
                    let receipts = deliverer.desktop_event(id, event);
                    if !persist(&mut deliverer, receipts, &updates).await { break; }
                }
            }
        }
    }
    Ok(())
}

async fn appearance(
    deliverer: &mut Deliverer,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
) -> bool {
    let receipts = deliverer.server_appeared();
    if !persist(deliverer, receipts, updates).await {
        return false;
    }
    flush(deliverer, updates).await
}
async fn flush(
    deliverer: &mut Deliverer,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
) -> bool {
    while let Some(id) = deliverer.ready_ids().into_iter().next() {
        let receipts = deliverer.send(&id).await;
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
) -> bool {
    let receipts = match request {
        NotificationRequest::Deliver(notice) => {
            let began = deliverer.begin(*notice);
            if began.is_empty() {
                return true;
            }
            began
        }
        NotificationRequest::Clear(id) => deliverer.cancel(&id).await,
    };
    persist(deliverer, receipts, updates).await && flush(deliverer, updates).await
}

async fn persist(
    deliverer: &mut Deliverer,
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
