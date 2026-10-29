//! Desktop and configured command I/O over the pure signals checkpoint.
use futures_util::StreamExt;
use signals::DeliveryOutcome;
use signals::notifications::*;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;
use zbus::zvariant::Value;
const DESTINATION: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

pub struct Deliverer {
    connection: Option<zbus::Connection>,
    config: NotificationConfig,
    pub state: NotificationState,
    ready: BTreeMap<String, BTreeSet<Route>>,
    owner_position: Option<zbus::message::Sequence>,
}

impl Deliverer {
    /// The caller supplies a session-bus connection, or None when unavailable.
    /// Missing-bus attempts are recorded while command routes still run. Tests
    /// supply a private bus.
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
            .server_owner()
            .map(str::to_owned)
            .ok_or_else(|| zbus::Error::Failure("no_notification_server".into()))?;
        zbus::Proxy::new_owned(connection, owner, PATH, DESTINATION).await
    }

    /// Returns an initial unknown receipt before any effect. Persist it before
    /// calling `send`, so an interrupted attempt is never sent twice after restart.
    pub fn begin(&mut self, notice: Notice) -> Vec<NotificationReceipt> {
        let receipts = self.state.begin(notice, self.config.route);
        for r in &receipts {
            self.ready
                .entry(r.delivery_id.clone())
                .or_default()
                .insert(r.route);
        }
        receipts
    }
    /// All effects require a fresh policy decision, including historical
    /// occurrences. Restored unknown routes are never automatically selected.
    pub async fn send(
        &mut self,
        id: &str,
        can_send: &(dyn Fn(&Notice) -> bool + Send + Sync),
    ) -> Vec<NotificationReceipt> {
        let Some(routes) = self.ready.remove(id) else {
            return Vec::new();
        };
        let Some(a) = self.state.attempt(id).cloned() else {
            return Vec::new();
        };
        if !a.active {
            return Vec::new();
        }
        let mut receipts = Vec::new();
        if let Some(denied) = self
            .state
            .group_members(&a.notice)
            .into_iter()
            .find(|n| !can_send(n))
        {
            receipts.extend(self.cancel(&denied.delivery_id, can_send).await);
        }
        if !self.state.attempt(id).is_some_and(|a| a.active) {
            return receipts;
        }

        if routes.contains(&Route::Dbus) && a.dbus == Some(DeliveryOutcome::Unknown) {
            receipts.push(self.update_notice(&a.notice).await);
        }
        if routes.contains(&Route::Command) && a.command == Some(DeliveryOutcome::Unknown) {
            if !can_send(&a.notice) {
                receipts.extend(self.cancel(id, can_send).await);
                return receipts;
            }
            let outcome = self.command_send(&a.notice).await;
            receipts.push(self.state.completed(
                id,
                Route::Command,
                Completion {
                    outcome,
                    notification_id: None,
                    server_owner: None,
                    slot: None,
                },
            ));
        }
        receipts
    }
    async fn dbus_send(
        &mut self,
        notice: &Notice,
    ) -> (DeliveryOutcome, Option<u32>, Option<String>, Option<String>) {
        let mut used_owner = None;
        let result = tokio::time::timeout(self.config.deadline, async {
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
            let proxy = zbus::Proxy::new_owned(connection, owner, PATH, DESTINATION).await?;
            let capabilities: Vec<String> = proxy.call("GetCapabilities", &()).await?;
            let actions = if capabilities.iter().any(|c| c == "actions") {
                vec!["default", "Open", "acknowledge", "Acknowledge"]
            } else {
                Vec::new()
            };
            let view = self.state.presentation(notice, !actions.is_empty());
            let mut hints = HashMap::new();
            hints.insert("urgency", Value::U8(if view.data_loss { 2 } else { 1 }));
            let category = format!("{}.{}", notice.product, notice.rule);
            hints.insert("category", Value::from(category.as_str()));
            let id: u32 = proxy
                .call(
                    "Notify",
                    &(
                        notice.product.as_str(),
                        view.replaces,
                        notice.icon.as_str(),
                        view.summary.as_str(),
                        view.body.as_str(),
                        actions,
                        hints,
                        if view.data_loss { 0i32 } else { -1i32 },
                    ),
                )
                .await?;
            Ok::<_, zbus::Error>((id, view.slot))
        })
        .await;
        match result {
            Ok(Ok((id, slot))) => (DeliveryOutcome::Delivered, Some(id), used_owner, Some(slot)),
            Ok(Err(e)) => (
                DeliveryOutcome::NotSent(dbus_reason(&e)),
                None,
                used_owner,
                None,
            ),
            Err(_) => (DeliveryOutcome::Unknown, None, used_owner, None),
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

    /// Called on service appearance, never on a timer. Any explicit DBus error
    /// may recover under a new server. Only one retry is allowed under the
    /// original identity, and current state is checked immediately.
    pub fn server_appeared(&mut self) -> Vec<NotificationReceipt> {
        let receipts = self.state.server_appeared();
        for r in &receipts {
            self.ready
                .entry(r.delivery_id.clone())
                .or_default()
                .insert(Route::Dbus);
        }
        receipts
    }
    async fn close(&self, ids: Vec<u32>) {
        for id in ids {
            let _ = tokio::time::timeout(self.config.deadline, async {
                self.close_proxy()
                    .await?
                    .call::<_, _, ()>("CloseNotification", &(id,))
                    .await
            })
            .await;
        }
    }
    pub async fn cancel(
        &mut self,
        id: &str,
        can_send: &(dyn Fn(&Notice) -> bool + Send + Sync),
    ) -> Vec<NotificationReceipt> {
        let Some(notice) = self.state.attempt(id).map(|a| a.notice.clone()) else {
            return Vec::new();
        };
        // Remove every unauthorized contributor before computing a summary.
        let mut ids: Vec<_> = self
            .state
            .group_members(&notice)
            .into_iter()
            .filter(|n| !can_send(n))
            .map(|n| n.delivery_id)
            .collect();
        ids.push(id.to_string());
        let mut receipts = Vec::new();
        let mut close = Vec::new();
        for id in ids {
            let cancelled = self.state.deactivate(&id);
            for id in cancelled.deactivated {
                self.ready.remove(&id);
            }
            receipts.extend(cancelled.receipts);
            close.extend(cancelled.close);
        }
        self.close(close).await;
        loop {
            let update = self.state.group_update(&notice);
            self.close(update.close).await;
            let mut rejected = false;
            for next in update.notices {
                if can_send(&next) {
                    receipts.push(self.update_notice(&next).await);
                } else {
                    let cancelled = self.state.deactivate(&next.delivery_id);
                    for id in cancelled.deactivated {
                        self.ready.remove(&id);
                    }
                    self.close(cancelled.close).await;
                    receipts.extend(cancelled.receipts);
                    rejected = true;
                }
            }
            if !rejected {
                break;
            }
        }
        receipts
    }
    async fn update_notice(&mut self, notice: &Notice) -> NotificationReceipt {
        let (outcome, id, owner, slot) = self.dbus_send(notice).await;
        self.state.completed(
            &notice.delivery_id,
            Route::Dbus,
            Completion {
                outcome,
                notification_id: id,
                server_owner: owner,
                slot,
            },
        )
    }
    pub fn desktop_event(&mut self, id: u32, event: DesktopEvent) -> Vec<NotificationReceipt> {
        self.state.desktop_event(id, event)
    }
    async fn probe_owner(&mut self) -> Option<String> {
        let connection = self.connection.as_ref()?;
        let result = tokio::time::timeout(
            self.config.deadline,
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
    Import {
        attempt: Box<Attempt>,
        imported: tokio::sync::oneshot::Sender<Result<(), ImportError>>,
    },
    Release {
        delivery_id: String,
        released: tokio::sync::oneshot::Sender<Option<Attempt>>,
    },
}

/// The product persists each update before allowing the worker to continue.
/// This handshake makes a crash between sending and receipt safe: it leaves an
/// unknown attempt rather than a candidate for automatic replay.
pub struct NotificationUpdate {
    pub receipts: Vec<NotificationReceipt>,
    pub state: NotificationState,
    /// Persist this handoff in the same transaction as the inactive checkpoint.
    pub released: Option<Attempt>,
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
    if deliverer.config.route == Route::Command || deliverer.connection.is_none() {
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
    let owner = deliverer.probe_owner().await;
    let retry_waiting = deliverer.state.attempts().values().any(|a| {
        a.active
            && !a.retried
            && a.dbus == Some(DeliveryOutcome::NotSent("no_notification_server".into()))
    });
    if let Some(owner) = owner {
        if deliverer.state.server_owner() != Some(owner.as_str()) || retry_waiting {
            deliverer.state.set_server_owner(Some(owner));
            if !appearance(&mut deliverer, &updates, still_matches.as_ref()).await {
                return Ok(());
            }
        }
    } else if deliverer.state.server_owner().is_some() {
        deliverer.state.set_server_owner(None);
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
                if deliverer.owner_position.is_some_and(|position| signal.recv_position() <= position) { continue; }
                if let Ok((name, _, new)) = signal.body().deserialize::<(String, String, String)>()
                    && name == DESTINATION
                {
                    if new.is_empty() {
                        deliverer.state.set_server_owner(None);
                        if !persist(&deliverer, Vec::new(), &updates).await { return Ok(()); }
                    } else {
                        deliverer.state.set_server_owner(Some(new));
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
            let receipts = deliverer.cancel(&id, still_matches).await;
            persist(deliverer, receipts, updates).await
        }
        NotificationRequest::Import { attempt, imported } => {
            let id = attempt.notice.delivery_id.clone();
            let result = deliverer.state.import_attempt(*attempt);
            match result {
                Err(error) => {
                    let _ = imported.send(Err(error));
                    true
                }
                Ok(ImportStatus::Known) => {
                    let _ = imported.send(Ok(()));
                    true
                }
                Ok(ImportStatus::Imported) => {
                    if !persist(deliverer, Vec::new(), updates).await {
                        return false;
                    }
                    let _ = imported.send(Ok(()));
                    let owner = deliverer.probe_owner().await;
                    let a = deliverer.state.attempt(&id).expect("imported attempt");
                    let eligible = a.active
                        && !a.retried
                        && matches!(a.dbus, Some(DeliveryOutcome::NotSent(_)))
                        && owner.as_ref().is_some_and(|o| {
                            a.server_owner.as_ref() != Some(o)
                                || a.dbus
                                    == Some(DeliveryOutcome::NotSent(
                                        "no_notification_server".into(),
                                    ))
                        });
                    if eligible {
                        deliverer.state.set_server_owner(owner);
                        let receipts = deliverer.state.prepare_retry(&id);
                        deliverer
                            .ready
                            .entry(id.clone())
                            .or_default()
                            .insert(Route::Dbus);
                        if !persist(deliverer, receipts, updates).await {
                            return false;
                        }
                        let receipts = deliverer.send(&id, still_matches).await;
                        if !persist(deliverer, receipts, updates).await {
                            return false;
                        }
                    }
                    true
                }
            }
        }
        NotificationRequest::Release {
            delivery_id,
            released,
        } => {
            deliverer.ready.remove(&delivery_id);
            let attempt = deliverer.state.release(&delivery_id);
            if !persist_release(deliverer, Vec::new(), attempt.clone(), updates).await {
                return false;
            }
            let _ = released.send(attempt);
            true
        }
    }
}

async fn persist(
    deliverer: &Deliverer,
    receipts: Vec<NotificationReceipt>,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
) -> bool {
    persist_release(deliverer, receipts, None, updates).await
}
async fn persist_release(
    deliverer: &Deliverer,
    receipts: Vec<NotificationReceipt>,
    released: Option<Attempt>,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
) -> bool {
    let (persisted, wait) = tokio::sync::oneshot::channel();
    if updates
        .send(NotificationUpdate {
            receipts,
            state: deliverer.state.clone(),
            released,
            persisted,
        })
        .is_err()
    {
        return false;
    }
    wait.await.is_ok()
}
