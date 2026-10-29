//! Desktop and configured command I/O over the pure signals checkpoint.
mod attempt;
pub use attempt::{AttemptContext, CancellationToken, Interrupted};
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

struct PreparedDbus {
    proxy: zbus::Proxy<'static>,
    owner: String,
    actions: Vec<&'static str>,
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

    async fn proxy(&mut self) -> zbus::Result<zbus::Proxy<'static>> {
        let connection = self
            .connection
            .clone()
            .ok_or_else(|| zbus::Error::Failure("no_notification_server".into()))?;
        zbus::Proxy::new_owned(connection, DESTINATION, PATH, DESTINATION).await
    }

    async fn close_proxy(&mut self) -> zbus::Result<zbus::Proxy<'static>> {
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
        check: &Policy,
        context: &AttemptContext,
    ) -> Vec<NotificationReceipt> {
        let Some(routes) = self.ready.remove(id) else {
            return Vec::new();
        };
        let mut receipts = Vec::new();
        for route in routes {
            let Some(attempt) = self.state.attempt(id).cloned().filter(|a| a.active) else {
                break;
            };
            let outcome = if route == Route::Dbus {
                &attempt.dbus
            } else {
                &attempt.command
            };
            if *outcome != Some(DeliveryOutcome::Unknown) {
                continue;
            }
            if let Err(interrupted) = context.check() {
                receipts.push(self.not_sent(id, route, interrupted.reason()));
                continue;
            }
            let prepared = if route == Route::Dbus {
                match self.prepare_dbus(context).await {
                    Ok(prepared) => Some(prepared),
                    Err(completion) => {
                        receipts.push(self.state.completed(id, route, completion));
                        continue;
                    }
                }
            } else {
                None
            };
            let (allowed, more) = self.authorize(id, route, check, context).await;
            receipts.extend(more);
            if !allowed {
                continue;
            }
            let completion = if let Some(prepared) = prepared {
                self.notify(&attempt.notice, prepared, context).await
            } else {
                Completion {
                    outcome: self.command_send(&attempt.notice, context).await,
                    notification_id: None,
                    server_owner: None,
                    slot: None,
                }
            };
            receipts.push(self.state.completed(id, route, completion));
        }
        receipts
    }
    fn not_sent(&mut self, id: &str, route: Route, reason: &str) -> NotificationReceipt {
        let mut completion = not_sent_reason(reason);
        if route == Route::Dbus {
            completion.server_owner = self.state.server_owner().map(str::to_owned);
        }
        self.state.completed(id, route, completion)
    }
    /// No owner lock crosses an I/O await. This fence admits exactly one
    /// occurrence/route, with its summary contributors, under current policy.
    async fn authorize(
        &mut self,
        id: &str,
        route: Route,
        check: &Policy,
        context: &AttemptContext,
    ) -> (bool, Vec<NotificationReceipt>) {
        let mut receipts = Vec::new();
        loop {
            let Some(notice) = self
                .state
                .attempt(id)
                .filter(|a| a.active)
                .map(|a| a.notice.clone())
            else {
                return (false, receipts);
            };
            if let Err(interrupted) = context.check() {
                receipts.push(self.not_sent(id, route, interrupted.reason()));
                return (false, receipts);
            }
            let request = PolicyRequest {
                delivery_id: id.to_string(),
                route,
                notices: self.state.group_members(&notice),
                attempt: context.clone(),
            };
            let member_ids: Vec<_> = request
                .notices
                .iter()
                .map(|n| n.delivery_id.clone())
                .collect();
            let decisions = match check(request).await {
                Ok(Authorization { decisions }) => decisions,
                Err(failure) => {
                    receipts.push(self.not_sent(id, route, failure.reason()));
                    return (false, receipts);
                }
            };
            // The callback owns asynchronous transaction cleanup. It must use
            // this same lifetime inside its scope and finish rollback before
            // returning, even when its work is interrupted.
            if member_ids.iter().any(|id| !decisions.contains_key(id)) {
                receipts.push(self.not_sent(id, route, "policy_incomplete"));
                return (false, receipts);
            }
            let denied = member_ids
                .iter()
                .find(|id| decisions.get(*id) == Some(&SendDecision::Cancel))
                .cloned();
            let held = member_ids
                .iter()
                .filter_map(|id| match decisions.get(id) {
                    Some(SendDecision::HoldUntil { at, policy }) => Some((*at, policy.clone())),
                    _ => None,
                })
                .max_by_key(|(at, _)| *at);
            // Decisions are immutable after admission. Policy changes may now
            // proceed while the already-admitted platform call is in flight.
            if let Some(denied) = denied {
                receipts.extend(self.cancel_authorized(&denied, &decisions, context).await);
                continue;
            }
            if let Err(interrupted) = context.check() {
                receipts.push(self.not_sent(id, route, interrupted.reason()));
                return (false, receipts);
            }
            if let Some((at, policy)) = held {
                self.state.hold(id, [route], at, &policy);
                return (false, receipts);
            }
            return (true, receipts);
        }
    }
    async fn prepare_dbus(&mut self, context: &AttemptContext) -> Result<PreparedDbus, Completion> {
        let mut used_owner = None;
        let result = context
            .run(async {
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
                let actions = if capabilities.iter().any(|c| c == "actions") {
                    vec!["default", "Open", "acknowledge", "Acknowledge"]
                } else {
                    Vec::new()
                };
                Ok::<_, zbus::Error>(PreparedDbus {
                    proxy,
                    owner,
                    actions,
                })
            })
            .await;
        let outcome = match result {
            Ok(Ok(prepared)) => return Ok(prepared),
            Ok(Err(error)) => DeliveryOutcome::NotSent(dbus_reason(&error)),
            Err(interrupted) => DeliveryOutcome::NotSent(interrupted.reason().into()),
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
        prepared: PreparedDbus,
        context: &AttemptContext,
    ) -> Completion {
        if let Err(interrupted) = context.check() {
            let mut completion = not_sent_reason(interrupted.reason());
            completion.server_owner = Some(prepared.owner);
            return completion;
        }
        let view = self
            .state
            .presentation(notice, !prepared.actions.is_empty());
        let mut hints = HashMap::new();
        hints.insert("urgency", Value::U8(if view.data_loss { 2 } else { 1 }));
        let category = format!("{}.{}", notice.product, notice.rule);
        hints.insert("category", Value::from(category.as_str()));
        let mut issued = false;
        let result = context
            .run(async {
                issued = true;
                prepared
                    .proxy
                    .call::<_, _, u32>(
                        "Notify",
                        &(
                            notice.product.as_str(),
                            view.replaces,
                            notice.icon.as_str(),
                            view.summary.as_str(),
                            view.body.as_str(),
                            prepared.actions,
                            hints,
                            if view.data_loss { 0i32 } else { -1i32 },
                        ),
                    )
                    .await
            })
            .await;
        let (outcome, id, slot) = match result {
            Ok(Ok(id)) => (DeliveryOutcome::Delivered, Some(id), Some(view.slot)),
            Ok(Err(e)) => (DeliveryOutcome::NotSent(dbus_reason(&e)), None, None),
            Err(interrupted) => (
                if issued {
                    DeliveryOutcome::Unknown
                } else {
                    DeliveryOutcome::NotSent(interrupted.reason().into())
                },
                None,
                None,
            ),
        };
        Completion {
            outcome,
            notification_id: id,
            server_owner: Some(prepared.owner),
            slot,
        }
    }
    async fn command_send(&mut self, notice: &Notice, context: &AttemptContext) -> DeliveryOutcome {
        let Some(command) = &self.config.command else {
            return DeliveryOutcome::NotSent("notification_command_not_configured".into());
        };
        let input = match serde_json::to_vec(notice) {
            Ok(input) => input,
            Err(error) => return DeliveryOutcome::NotSent(error.to_string()),
        };
        if let Err(interrupted) = context.check() {
            return DeliveryOutcome::NotSent(interrupted.reason().into());
        }
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
        let result = context
            .run(async {
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
                // Owned cleanup may finish after the work deadline. It grants
                // no more execution time and cannot authorize another spawn.
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
    async fn close(&mut self, ids: Vec<u32>, context: &AttemptContext) {
        for id in ids {
            let _ = context
                .run(async {
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
        check: &Policy,
        context: &AttemptContext,
    ) -> Result<Vec<NotificationReceipt>, PolicyFailure> {
        let Some(notice) = self.state.attempt(id).map(|a| a.notice.clone()) else {
            return Ok(Vec::new());
        };
        context.check().map_err(PolicyFailure::Interrupted)?;
        let notices = self.state.group_members(&notice);
        let members: Vec<_> = notices
            .iter()
            .map(|notice| notice.delivery_id.clone())
            .collect();
        let Authorization { decisions } = check(PolicyRequest {
            delivery_id: id.to_string(),
            route: Route::Dbus,
            notices,
            attempt: context.clone(),
        })
        .await?;
        if members.iter().any(|id| !decisions.contains_key(id)) {
            return Err(PolicyFailure::Unavailable);
        }
        context.check().map_err(PolicyFailure::Interrupted)?;
        Ok(self.cancel_authorized(id, &decisions, context).await)
    }
    async fn cancel_authorized(
        &mut self,
        id: &str,
        decisions: &BTreeMap<String, SendDecision>,
        context: &AttemptContext,
    ) -> Vec<NotificationReceipt> {
        let Some(notice) = self.state.attempt(id).map(|a| a.notice.clone()) else {
            return Vec::new();
        };
        let mut ids: Vec<_> = self
            .state
            .group_members(&notice)
            .into_iter()
            .filter(|n| decisions.get(&n.delivery_id) == Some(&SendDecision::Cancel))
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
        self.close(close, context).await;
        loop {
            let update = self.state.group_update(&notice);
            self.close(update.close, context).await;
            let mut rejected = false;
            for next in update.notices {
                match decisions.get(&next.delivery_id) {
                    Some(SendDecision::Permit) => {
                        if let Some(prepared) = self.state.prepare_refresh(&next.delivery_id) {
                            self.ready
                                .entry(next.delivery_id)
                                .or_default()
                                .insert(Route::Dbus);
                            receipts.push(prepared);
                        }
                    }
                    Some(SendDecision::HoldUntil { at, policy }) => {
                        if let Some(prepared) = self.state.prepare_refresh(&next.delivery_id) {
                            self.state
                                .hold(&next.delivery_id, [Route::Dbus], *at, policy);
                            receipts.push(prepared);
                        }
                    }
                    Some(SendDecision::Cancel) => {
                        let cancelled = self.state.deactivate(&next.delivery_id);
                        for id in cancelled.deactivated {
                            self.ready.remove(&id);
                        }
                        self.close(cancelled.close, context).await;
                        receipts.extend(cancelled.receipts);
                        rejected = true;
                    }
                    None => {
                        if let Some(prepared) = self.state.prepare_refresh(&next.delivery_id) {
                            receipts.push(prepared);
                            receipts.push(self.not_sent(
                                &next.delivery_id,
                                Route::Dbus,
                                "policy_incomplete",
                            ));
                        }
                    }
                }
            }
            if !rejected {
                break;
            }
        }
        receipts
    }
    pub fn ready_ids(&self) -> Vec<String> {
        self.ready.keys().cloned().collect()
    }
    pub fn resume_holds(&mut self, now: i64) -> Vec<NotificationReceipt> {
        let receipts = self.state.resume_holds(now);
        for receipt in &receipts {
            self.ready
                .entry(receipt.delivery_id.clone())
                .or_default()
                .insert(receipt.route);
        }
        receipts
    }
    pub fn desktop_event(&mut self, id: u32, event: DesktopEvent) -> Vec<NotificationReceipt> {
        self.state.desktop_event(id, event)
    }
    async fn probe_owner(&mut self, context: &AttemptContext) -> Option<String> {
        let connection = self.connection.as_ref()?;
        let result = context
            .run(connection.call_method(
                Some("org.freedesktop.DBus"),
                "/org/freedesktop/DBus",
                Some("org.freedesktop.DBus"),
                "GetNameOwner",
                &(DESTINATION,),
            ))
            .await
            .ok()?;
        self.owner_position = match &result {
            Ok(m) | Err(zbus::Error::MethodError(_, _, m)) => Some(m.recv_position()),
            _ => self.owner_position,
        };
        result.ok()?.body().deserialize().ok()
    }
}

fn not_sent_reason(reason: &str) -> Completion {
    Completion {
        outcome: DeliveryOutcome::NotSent(reason.into()),
        notification_id: None,
        server_owner: None,
        slot: None,
    }
}

struct OwnerLifetime {
    cancellation: CancellationToken,
    parent_deadline: Option<tokio::time::Instant>,
}
impl OwnerLifetime {
    fn dispatch(&self, deliverer: &Deliverer) -> AttemptContext {
        AttemptContext::new(
            deliverer.config.deadline,
            self.parent_deadline,
            self.cancellation.clone(),
        )
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
    Recheck,
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

/// The precise occurrence and route admitted under one current owner snapshot.
pub struct PolicyRequest {
    pub delivery_id: String,
    pub route: Route,
    pub notices: Vec<Notice>,
    pub attempt: AttemptContext,
}
/// Completed owner-policy admission. The callback finishes all database
/// commit/rollback and releases every guard before returning these decisions.
/// Later independently admitted routes obtain a fresh authorization.
pub struct Authorization {
    pub decisions: BTreeMap<String, SendDecision>,
}
/// Operational failure is distinct from an authoritative policy denial.
/// Products retain detailed diagnostics; receipts carry these static reasons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyFailure {
    Unavailable,
    Interrupted(Interrupted),
}
impl PolicyFailure {
    pub fn reason(self) -> &'static str {
        match self {
            Self::Unavailable => "policy_unavailable",
            Self::Interrupted(interrupted) => interrupted.reason(),
        }
    }
}
impl std::fmt::Display for PolicyFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}
impl std::error::Error for PolicyFailure {}
pub type PolicyFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<Authorization, PolicyFailure>> + Send + 'static>,
>;
pub type Policy = dyn Fn(PolicyRequest) -> PolicyFuture + Send + Sync;
pub type PolicyCheck = Arc<Policy>;
pub type DeliveryClock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Runs one host's deliverer. The unbounded channel carries outbox wakeups outside
/// the writer; durable outbox storage remains owned by the product.
pub async fn run(
    mut deliverer: Deliverer,
    mut requests: mpsc::UnboundedReceiver<NotificationRequest>,
    updates: mpsc::UnboundedSender<NotificationUpdate>,
    still_matches: PolicyCheck,
    clock: DeliveryClock,
    cancellation: CancellationToken,
    parent_deadline: Option<tokio::time::Instant>,
) -> zbus::Result<()> {
    let owner = OwnerLifetime {
        cancellation,
        parent_deadline,
    };
    if owner.cancellation.is_cancelled() {
        return Ok(());
    }
    if !resume(
        &mut deliverer,
        (clock)(),
        &updates,
        still_matches.as_ref(),
        &owner,
    )
    .await
    {
        return Ok(());
    }
    if deliverer.config.route == Route::Command || deliverer.connection.is_none() {
        loop {
            tokio::select! {
                _=owner.cancellation.cancelled()=>break,
                request=requests.recv()=>{
                    let Some(request)=request else {break};
                    if !handle_request(&mut deliverer,request,&updates,still_matches.as_ref(),(clock)(), &owner).await.map_err(|failure| zbus::Error::Failure(failure.reason().into()))? {break;}
                }
                _=hold_timer(deliverer.state.next_hold(),(clock)())=>{
                    if !resume(&mut deliverer,(clock)(),&updates,still_matches.as_ref(), &owner).await {break;}
                }
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
    let startup = owner.dispatch(&deliverer);
    let server_owner = deliverer.probe_owner(&startup).await;
    let retry_waiting = deliverer.state.attempts().values().any(|a| {
        a.active
            && !a.retried
            && a.dbus == Some(DeliveryOutcome::NotSent("no_notification_server".into()))
    });
    if let Some(server_owner) = server_owner {
        if deliverer.state.server_owner() != Some(server_owner.as_str()) || retry_waiting {
            deliverer.state.set_server_owner(Some(server_owner));
            let context = owner.dispatch(&deliverer);
            if !appearance(&mut deliverer, &updates, still_matches.as_ref(), &context).await {
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
            _=owner.cancellation.cancelled()=>break,
            _=hold_timer(deliverer.state.next_hold(),(clock)())=>{
                if !resume(&mut deliverer,(clock)(),&updates,still_matches.as_ref(), &owner).await {break;}
            }
            request = requests.recv() => {
                match request {
                    Some(request) => {
                        if !handle_request(&mut deliverer, request, &updates, still_matches.as_ref(),(clock)(), &owner).await.map_err(|failure| zbus::Error::Failure(failure.reason().into()))? { break; }
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
                        if !persist(&mut deliverer, Vec::new(), &updates).await { return Ok(()); }
                    } else {
                        deliverer.state.set_server_owner(Some(new));
                        let attempt = owner.dispatch(&deliverer);
                        if !appearance(&mut deliverer, &updates, still_matches.as_ref(), &attempt).await { return Ok(()); }
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
    still_matches: &Policy,
    context: &AttemptContext,
) -> bool {
    let receipts = deliverer.server_appeared();
    if !persist(deliverer, receipts, updates).await {
        return false;
    }
    flush(deliverer, updates, still_matches, context).await
}
async fn flush(
    deliverer: &mut Deliverer,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
    check: &Policy,
    context: &AttemptContext,
) -> bool {
    while let Some(id) = deliverer.ready_ids().into_iter().next() {
        let receipts = deliverer.send(&id, check, context).await;
        if !persist(deliverer, receipts, updates).await {
            return false;
        }
    }
    true
}
async fn resume(
    deliverer: &mut Deliverer,
    now: i64,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
    check: &Policy,
    owner: &OwnerLifetime,
) -> bool {
    let context = owner.dispatch(deliverer);
    let receipts = deliverer.resume_holds(now);
    if receipts.is_empty() {
        return true;
    }
    if !persist(deliverer, receipts, updates).await {
        return false;
    }
    flush(deliverer, updates, check, &context).await
}
async fn hold_timer(due: Option<i64>, now: i64) {
    match due {
        None => std::future::pending::<()>().await,
        Some(due) => {
            tokio::time::sleep(std::time::Duration::from_millis(
                u64::try_from(due.saturating_sub(now).max(1)).unwrap_or(u64::MAX),
            ))
            .await
        }
    }
}

async fn handle_request(
    deliverer: &mut Deliverer,
    request: NotificationRequest,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
    still_matches: &Policy,
    now: i64,
    owner: &OwnerLifetime,
) -> Result<bool, PolicyFailure> {
    Ok(match request {
        NotificationRequest::Deliver(notice) => {
            let context = owner.dispatch(deliverer);
            let began = deliverer.begin(*notice);
            if began.is_empty() {
                return Ok(true);
            }
            // Once submitted, observe custody acknowledgement even after the
            // work deadline. The same context gates every subsequent effect.
            if !persist(deliverer, began, updates).await {
                return Ok(false);
            }
            flush(deliverer, updates, still_matches, &context).await
        }
        NotificationRequest::Clear(id) => {
            let context = owner.dispatch(deliverer);
            let receipts = deliverer.cancel(&id, still_matches, &context).await?;
            if !persist(deliverer, receipts, updates).await {
                return Ok(false);
            }
            flush(deliverer, updates, still_matches, &context).await
        }
        NotificationRequest::Recheck => resume(deliverer, now, updates, still_matches, owner).await,
        NotificationRequest::Import { attempt, imported } => {
            let context = owner.dispatch(deliverer);
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
                        return Ok(false);
                    }
                    let _ = imported.send(Ok(()));
                    let server_owner = deliverer.probe_owner(&context).await;
                    let a = deliverer.state.attempt(&id).expect("imported attempt");
                    let eligible = a.active
                        && !a.retried
                        && matches!(a.dbus, Some(DeliveryOutcome::NotSent(_)))
                        && server_owner.as_ref().is_some_and(|o| {
                            a.server_owner.as_ref() != Some(o)
                                || a.dbus
                                    == Some(DeliveryOutcome::NotSent(
                                        "no_notification_server".into(),
                                    ))
                        });
                    if eligible {
                        deliverer.state.set_server_owner(server_owner);
                        let receipts = deliverer.state.prepare_retry(&id);
                        deliverer
                            .ready
                            .entry(id.clone())
                            .or_default()
                            .insert(Route::Dbus);
                        if !persist(deliverer, receipts, updates).await {
                            return Ok(false);
                        }
                        if !flush(deliverer, updates, still_matches, &context).await {
                            return Ok(false);
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
                return Ok(false);
            }
            let _ = released.send(attempt);
            true
        }
    })
}

async fn persist(
    deliverer: &mut Deliverer,
    receipts: Vec<NotificationReceipt>,
    updates: &mpsc::UnboundedSender<NotificationUpdate>,
) -> bool {
    persist_release(deliverer, receipts, None, updates).await
}
async fn persist_release(
    deliverer: &mut Deliverer,
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
