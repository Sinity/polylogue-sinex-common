use signals::DeliveryOutcome;
use signals::notifications::*;
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use zbus::zvariant::OwnedValue;

struct PrivateBus {
    process: std::process::Child,
    address: String,
}
impl PrivateBus {
    fn start() -> Self {
        let mut process = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("private dbus-daemon");
        let mut address = String::new();
        std::io::BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        Self {
            process,
            address: address.trim().to_string(),
        }
    }
    async fn connect(&self) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap()
    }
    async fn server(&self, fake: FakeServer) -> zbus::Connection {
        zbus::connection::Builder::address(self.address.as_str())
            .unwrap()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .serve_at("/org/freedesktop/Notifications", fake)
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}
impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[derive(Clone, Debug)]
struct Call {
    app: String,
    replaces: u32,
    icon: String,
    summary: String,
    body: String,
    actions: Vec<String>,
    urgency: u8,
    category: String,
    expire: i32,
}
#[derive(Clone, Default)]
struct Records {
    calls: Arc<Mutex<Vec<Call>>>,
    closed: Arc<Mutex<Vec<u32>>>,
    entered: Arc<tokio::sync::Notify>,
    stall: Arc<std::sync::atomic::AtomicBool>,
}
struct FakeServer {
    records: Records,
    actions: bool,
    stall: bool,
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeServer {
    fn get_capabilities(&self) -> Vec<String> {
        if self.actions {
            vec!["actions".into()]
        } else {
            vec![]
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app: String,
        replaces: u32,
        icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire: i32,
    ) -> u32 {
        let id = {
            let mut calls = self.records.calls.lock().unwrap();
            calls.push(Call {
                app,
                replaces,
                icon,
                summary,
                body,
                actions,
                urgency: u8::try_from(hints.get("urgency").unwrap()).unwrap(),
                category: <&str>::try_from(hints.get("category").unwrap())
                    .unwrap()
                    .to_string(),
                expire,
            });
            if replaces == 0 {
                calls.len() as u32
            } else {
                replaces
            }
        };
        self.records.entered.notify_one();
        if self.stall || self.records.stall.load(std::sync::atomic::Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        id
    }
    fn close_notification(&self, id: u32) {
        self.records.closed.lock().unwrap().push(id);
    }
    #[zbus(signal)]
    async fn action_invoked(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        action: &str,
    ) -> zbus::Result<()>;
    #[zbus(signal)]
    async fn notification_closed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}
fn config() -> NotificationConfig {
    NotificationConfig {
        route: Route::Dbus,
        command: None,
        deadline: Duration::from_secs(2),
    }
}
fn notice(id: &str, occurrence: &str) -> Notice {
    Notice {
        delivery_id: id.into(),
        product: "Example".into(),
        icon: "example".into(),
        rule: "source-unreadable".into(),
        occurrence: occurrence.into(),
        summary: "Source unreadable".into(),
        body: "A source needs attention".into(),
        reference: format!("https://example.invalid/notices/{id}"),
        command_hint: format!("example read notice:{id}"),
        interrupt: true,
        current: true,
        data_loss: false,
        budget_passive_count: None,
    }
}
async fn send(deliverer: &mut Deliverer, notice: Notice) -> Vec<NotificationReceipt> {
    let id = notice.delivery_id.clone();
    let initial = deliverer.begin(notice);
    assert_eq!(initial[0].outcome, DeliveryOutcome::Unknown);
    deliverer.send(&id, &|_| true).await
}

#[tokio::test]
async fn notification_replaced_in_place_and_current_clear_closes() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let a = send(&mut deliverer, notice("a", "source-a")).await;
    let mut b = notice("b", "source-a");
    b.data_loss = true;
    let b = send(&mut deliverer, b).await;
    assert_eq!(a[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(b[0].notification_id, a[0].notification_id);
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls[0].replaces, 0);
    assert_eq!(calls[1].replaces, a[0].notification_id.unwrap());
    assert_eq!(calls[0].app, "Example");
    assert_eq!(calls[0].icon, "example");
    assert_eq!(calls[0].category, "Example.source-unreadable");
    assert_eq!(calls[0].urgency, 1);
    assert_eq!(calls[0].expire, -1);
    assert_eq!(calls[1].urgency, 2);
    assert_eq!(calls[1].expire, 0);
    assert_eq!(
        calls[0].actions,
        ["default", "Open", "acknowledge", "Acknowledge"]
    );
    deliverer.cancel("b").await;
    assert_eq!(
        *records.closed.lock().unwrap(),
        [a[0].notification_id.unwrap()]
    );
    assert!(deliverer.begin(notice("a", "source-a")).is_empty());
    assert!(deliverer.send("a", &|_| true).await.is_empty());
}

#[tokio::test]
async fn grouping_replaces_rule_summary_and_budget_summary() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: false,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let mut losing_data = notice("a", "source-a");
    losing_data.data_loss = true;
    let first = send(&mut deliverer, losing_data).await;
    let second = send(&mut deliverer, notice("b", "source-b")).await;
    let third = send(&mut deliverer, notice("c", "source-c")).await;
    assert_ne!(first[0].notification_id, second[0].notification_id);
    assert_eq!(second[0].notification_id, third[0].notification_id);
    let opened = deliverer.desktop_event(
        second[0].notification_id.unwrap(),
        DesktopEvent::OpenReference {
            reference: String::new(),
        },
    );
    assert_eq!(
        opened
            .iter()
            .filter(|r| matches!(r.event, Some(DesktopEvent::OpenReference { .. })))
            .count(),
        1
    );
    assert_eq!(
        opened.iter().find(|r| r.delivery_id == "b").unwrap().event,
        Some(DesktopEvent::Seen)
    );
    assert_eq!(
        opened.iter().find(|r| r.delivery_id == "c").unwrap().event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/c".into()
        })
    );
    let mut budget = notice("budget-1", "source-a");
    budget.interrupt = false;
    budget.budget_passive_count = Some(1);
    let budget1 = send(&mut deliverer, budget.clone()).await;
    budget.delivery_id = "budget-2".into();
    budget.budget_passive_count = Some(2);
    let budget2 = send(&mut deliverer, budget).await;
    assert_eq!(budget1[0].notification_id, budget2[0].notification_id);
    assert!(deliverer.state.attempts["a"].active);
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls[1].summary, "Source unreadable: 2 open occurrences");
    assert_eq!(calls[2].summary, "Source unreadable: 3 open occurrences");
    assert!(calls[0].body.ends_with("example read notice:a"));
    assert!(calls[0].actions.is_empty());
    assert_eq!(
        calls[4].summary,
        "2 notices held passive (interrupt budget spent)"
    );
    assert_eq!(calls[2].urgency, 2);
    assert_eq!(calls[2].expire, 0);
    let shrink = deliverer.cancel("b").await;
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 6);
    assert_eq!(calls[5].replaces, second[0].notification_id.unwrap());
    assert_eq!(calls[5].summary, "Source unreadable: 2 open occurrences");
    assert_eq!(calls[5].urgency, 2);
    assert!(calls[5].body.ends_with("example read notice:c"));
    assert_eq!(shrink.len(), 1);
    assert_eq!(shrink[0].delivery_id, "c");
    assert_eq!(
        deliverer.state.attempts["budget-2"].notification_id,
        budget2[0].notification_id
    );
    let budget_open = deliverer.desktop_event(
        budget2[0].notification_id.unwrap(),
        DesktopEvent::OpenReference {
            reference: String::new(),
        },
    );
    assert_eq!(budget_open.len(), 1);
    assert_eq!(budget_open[0].delivery_id, "budget-2");
    assert_eq!(
        budget_open[0].event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/budget-2".into()
        })
    );
    deliverer.cancel("c").await;
    assert!(
        records
            .closed
            .lock()
            .unwrap()
            .contains(&second[0].notification_id.unwrap())
    );
    deliverer.cancel("budget-2").await;
    assert!(deliverer.state.attempts["a"].active);
    assert!(
        !records
            .closed
            .lock()
            .unwrap()
            .contains(&first[0].notification_id.unwrap())
    );
    assert_eq!(
        records.closed.lock().unwrap().last().copied(),
        budget2[0].notification_id
    );
    assert_eq!(records.calls.lock().unwrap().len(), 6);
}

#[tokio::test]
async fn not_sent_retried_once_unknown_never_and_restart_keeps_identity() {
    let bus = PrivateBus::start();
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let first = send(&mut deliverer, notice("a", "source-a")).await;
    assert_eq!(
        first[0].outcome,
        DeliveryOutcome::NotSent("no_notification_server".into())
    );
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let retry = deliverer.server_appeared();
    assert_eq!(retry[0].delivery_id, "a");
    assert_eq!(retry[0].outcome, DeliveryOutcome::Unknown);
    let delivered = deliverer.send("a", &|_| true).await;
    assert_eq!(delivered[0].outcome, DeliveryOutcome::Delivered);
    assert!(deliverer.server_appeared().is_empty());
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    let state = serde_json::from_str(&serde_json::to_string(&deliverer.state).unwrap()).unwrap();
    let mut restored = Deliverer::new(bus.connect().await, config(), state);
    assert!(restored.begin(notice("a", "source-a")).is_empty());
    assert!(restored.send("a", &|_| true).await.is_empty());
    assert!(restored.server_appeared().is_empty());

    let slow_bus = PrivateBus::start();
    let slow_records = Records::default();
    let _slow_server = slow_bus
        .server(FakeServer {
            records: slow_records.clone(),
            actions: true,
            stall: true,
        })
        .await;
    let mut short = config();
    short.deadline = Duration::from_secs(2);
    let mut slow = Deliverer::new(
        slow_bus.connect().await,
        short.clone(),
        NotificationState::default(),
    );
    let unknown = send(&mut slow, notice("slow", "source-a")).await;
    assert_eq!(unknown[0].outcome, DeliveryOutcome::Unknown);
    assert!(slow.send("slow", &|_| true).await.is_empty());
    assert!(slow.server_appeared().is_empty());
    let mut restored = Deliverer::new(slow_bus.connect().await, short, slow.state);
    assert!(restored.begin(notice("slow", "source-a")).is_empty());
    assert!(restored.send("slow", &|_| true).await.is_empty());
    assert_eq!(slow_records.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn current_state_rechecks_before_retry_and_passive_never_notifies() {
    let bus = PrivateBus::start();
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut deliverer, notice("a", "source-a")).await;
    let mut passive = notice("passive", "source-b");
    passive.interrupt = false;
    assert!(deliverer.begin(passive).is_empty());
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    deliverer.server_appeared();
    let cancelled = deliverer.send("a", &|_| false).await;
    assert_eq!(cancelled[0].outcome, DeliveryOutcome::Cancelled);
    assert!(records.calls.lock().unwrap().is_empty());
    assert!(deliverer.server_appeared().is_empty());
}

#[tokio::test]
async fn configured_command_receives_json_and_both_routes_have_separate_receipts() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notice.json");
    let mut configured = config();
    configured.route = Route::Both;
    configured.command = Some(RouteCommand {
        program: "sh".into(),
        arguments: vec![
            "-c".into(),
            "cat >> \"$1\"".into(),
            "notice-route".into(),
            path.to_string_lossy().into_owned(),
        ],
    });
    let mut deliverer = Deliverer::new(
        bus.connect().await,
        configured,
        NotificationState::default(),
    );
    let notice = notice("a", "source-a");
    let initial = deliverer.begin(notice.clone());
    assert_eq!(initial.len(), 2);
    let receipts = deliverer.send("a", &|_| true).await;
    assert_eq!(receipts.len(), 2);
    assert_eq!(
        receipts[0].outcome,
        DeliveryOutcome::NotSent("no_notification_server".into())
    );
    assert_eq!(receipts[1].outcome, DeliveryOutcome::Delivered);
    let first_bytes = std::fs::read(&path).unwrap();
    let got: Notice = serde_json::from_slice(&first_bytes).unwrap();
    assert_eq!(got, notice);
    let _server = bus
        .server(FakeServer {
            records,
            actions: true,
            stall: false,
        })
        .await;
    let retry = deliverer.server_appeared();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].route, Route::Dbus);
    let retried = deliverer.send("a", &|_| true).await;
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].route, Route::Dbus);
    assert_eq!(retried[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(std::fs::read(path).unwrap(), first_bytes);
}

async fn update(updates: &mut mpsc::UnboundedReceiver<NotificationUpdate>) -> NotificationUpdate {
    loop {
        let update = tokio::time::timeout(Duration::from_secs(5), updates.recv())
            .await
            .unwrap()
            .unwrap();
        if update.receipts.is_empty() {
            update.persisted.send(()).unwrap();
        } else {
            return update;
        }
    }
}

#[tokio::test]
async fn worker_records_protocol_actions_as_desktop_operator_acts() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates, Arc::new(|_| true)));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "a", "source-a",
        ))))
        .unwrap();
    let initial = update(&mut updated).await;
    assert_eq!(initial.receipts[0].outcome, DeliveryOutcome::Unknown);
    assert!(records.calls.lock().unwrap().is_empty());
    initial.persisted.send(()).unwrap();
    let delivered = update(&mut updated).await;
    let id = delivered.receipts[0].notification_id.unwrap();
    delivered.persisted.send(()).unwrap();
    let interface = server
        .object_server()
        .interface::<_, FakeServer>("/org/freedesktop/Notifications")
        .await
        .unwrap();
    FakeServer::action_invoked(interface.signal_emitter(), id, "default")
        .await
        .unwrap();
    let opened = update(&mut updated).await;
    assert_eq!(
        opened.receipts[0].event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/a".into()
        })
    );
    assert_eq!(opened.receipts[0].via.as_deref(), Some("desktop"));
    opened.persisted.send(()).unwrap();
    FakeServer::action_invoked(interface.signal_emitter(), id, "acknowledge")
        .await
        .unwrap();
    let ack = update(&mut updated).await;
    assert_eq!(ack.receipts[0].event, Some(DesktopEvent::Acknowledged));
    ack.persisted.send(()).unwrap();
    FakeServer::notification_closed(interface.signal_emitter(), id, 1)
        .await
        .unwrap();
    let expired = update(&mut updated).await;
    assert_eq!(expired.receipts[0].event, Some(DesktopEvent::Expired));
    expired.persisted.send(()).unwrap();
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn service_appearance_retries_automatically() {
    let bus = PrivateBus::start();
    let deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates, Arc::new(|_| true)));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "a", "source-a",
        ))))
        .unwrap();
    let initial = update(&mut updated).await;
    // The caller can commit more outbox rows while the worker awaits persistence
    // or a notification server. No protocol call runs on the caller's path.
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "passive", "source-b",
        ))))
        .unwrap();
    initial.persisted.send(()).unwrap();
    let missing = update(&mut updated).await;
    assert_eq!(
        missing.receipts[0].outcome,
        DeliveryOutcome::NotSent("no_notification_server".into())
    );
    missing.persisted.send(()).unwrap();
    // The second queued item is processed before the service-appearance signal.
    let initial2 = update(&mut updated).await;
    initial2.persisted.send(()).unwrap();
    let missing2 = update(&mut updated).await;
    missing2.persisted.send(()).unwrap();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let retry = update(&mut updated).await;
    assert_eq!(retry.receipts.len(), 2);
    assert!(
        retry
            .receipts
            .iter()
            .all(|r| r.outcome == DeliveryOutcome::Unknown)
    );
    retry.persisted.send(()).unwrap();
    for _ in 0..2 {
        let done = update(&mut updated).await;
        assert_eq!(done.receipts[0].outcome, DeliveryOutcome::Delivered);
        done.persisted.send(()).unwrap();
    }
    assert_eq!(records.calls.lock().unwrap().len(), 2);
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn command_only_worker_needs_no_session_bus() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notice.json");
    let mut configured = config();
    configured.route = Route::Command;
    configured.command = Some(RouteCommand {
        program: "sh".into(),
        arguments: vec![
            "-c".into(),
            "cat > \"$1\"".into(),
            "notice-route".into(),
            path.to_string_lossy().into_owned(),
        ],
    });
    let deliverer = Deliverer::new(None, configured, NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates, Arc::new(|_| true)));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "headless", "source-a",
        ))))
        .unwrap();
    let initial = update(&mut updated).await;
    assert_eq!(initial.receipts[0].route, Route::Command);
    assert!(!path.exists());
    initial.persisted.send(()).unwrap();
    let delivered = update(&mut updated).await;
    assert_eq!(delivered.receipts[0].outcome, DeliveryOutcome::Delivered);
    delivered.persisted.send(()).unwrap();
    let got: Notice = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(got.delivery_id, "headless");
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn slow_server_never_holds_writer() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: true,
        })
        .await;
    let deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates, Arc::new(|_| true)));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "slow", "source-a",
        ))))
        .unwrap();
    let initial = update(&mut updated).await;
    initial.persisted.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), records.entered.notified())
        .await
        .unwrap();
    // Notify is now unanswered. A writer can still commit its next group and
    // submit its outbox wakeup because it shares no protocol future or lock.
    let mut group = tempfile::tempfile().unwrap();
    std::io::Write::write_all(&mut group, b"committed group").unwrap();
    group.sync_all().unwrap();
    requests
        .send(NotificationRequest::Clear("slow".into()))
        .unwrap();
    let unanswered = update(&mut updated).await;
    assert_eq!(unanswered.receipts[0].outcome, DeliveryOutcome::Unknown);
    unanswered.persisted.send(()).unwrap();
    let cleared = update(&mut updated).await;
    assert_eq!(cleared.receipts[0].outcome, DeliveryOutcome::Cancelled);
    cleared.persisted.send(()).unwrap();
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn appearance_during_downtime_retries_and_same_server_restart_keeps_replace_id() {
    let bus = PrivateBus::start();
    let mut initial = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut initial, notice("a", "source-a")).await;
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let restored = Deliverer::new(bus.connect().await, config(), initial.state);
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(restored, received, updates, Arc::new(|_| true)));
    let retry = update(&mut updated).await;
    assert_eq!(retry.receipts[0].delivery_id, "a");
    assert_eq!(retry.receipts[0].outcome, DeliveryOutcome::Unknown);
    retry.persisted.send(()).unwrap();
    let delivered = update(&mut updated).await;
    let notification_id = delivered.receipts[0].notification_id.unwrap();
    let checkpoint = delivered.state.clone();
    delivered.persisted.send(()).unwrap();
    drop(requests);
    worker.await.unwrap().unwrap();
    let restored = Deliverer::new(bus.connect().await, config(), checkpoint);
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(restored, received, updates, Arc::new(|_| true)));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "b", "source-a",
        ))))
        .unwrap();
    let next = update(&mut updated).await;
    assert_eq!(next.receipts[0].delivery_id, "b");
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    next.persisted.send(()).unwrap();
    let delivered = update(&mut updated).await;
    assert_eq!(delivered.receipts[0].notification_id, Some(notification_id));
    delivered.persisted.send(()).unwrap();
    assert_eq!(
        records.calls.lock().unwrap().last().unwrap().replaces,
        notification_id
    );
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn clearing_first_occurrence_promotes_summary_and_keeps_actions_on_visible_notice() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut deliverer, notice("a", "source-a")).await;
    let second = send(&mut deliverer, notice("b", "source-b")).await;
    send(&mut deliverer, notice("c", "source-c")).await;
    deliverer.cancel("a").await;
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls[3].replaces, second[0].notification_id.unwrap());
    assert_eq!(calls[3].summary, "Source unreadable");
    assert!(calls[3].body.contains("source needs attention"));
    assert_eq!(calls[4].summary, "Source unreadable: 2 open occurrences");
    let b = deliverer.state.attempts["b"].notification_id.unwrap();
    let c = deliverer.state.attempts["c"].notification_id.unwrap();
    assert_ne!(b, c);
    let opened = deliverer.desktop_event(
        b,
        DesktopEvent::OpenReference {
            reference: String::new(),
        },
    );
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].delivery_id, "b");
    assert_eq!(
        opened[0].event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/b".into()
        })
    );
    let opened = deliverer.desktop_event(
        c,
        DesktopEvent::OpenReference {
            reference: String::new(),
        },
    );
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].delivery_id, "c");
}

#[tokio::test]
async fn failed_replacement_keeps_prior_notification_actionable_until_condition_clears() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let first = send(&mut deliverer, notice("a", "source-a")).await;
    records
        .stall
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let replacement = send(&mut deliverer, notice("b", "source-a")).await;
    assert_eq!(replacement[0].outcome, DeliveryOutcome::Unknown);
    let opened = deliverer.desktop_event(
        first[0].notification_id.unwrap(),
        DesktopEvent::OpenReference {
            reference: String::new(),
        },
    );
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].delivery_id, "a");
    assert_eq!(
        opened[0].event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/a".into()
        })
    );
    deliverer.cancel("b").await;
    assert!(!deliverer.state.attempts["a"].active);
    assert!(!deliverer.state.attempts["b"].active);
    assert_eq!(
        *records.closed.lock().unwrap(),
        [first[0].notification_id.unwrap()]
    );
    assert!(deliverer.server_appeared().is_empty());
}
