use signal_delivery::{
    AttemptContext, Authorization, CancellationToken, Deliverer, NotificationRequest,
    NotificationUpdate, Policy, PolicyRequest, run,
};
use signals::DeliveryOutcome;
use signals::notifications::*;
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;
use zbus::zvariant::OwnedValue;
#[path = "notifications/attempt_deadlines.rs"]
mod attempt_deadlines;

struct PrivateBus {
    process: std::process::Child,
    address: String,
    _configuration: tempfile::TempDir,
}
impl PrivateBus {
    fn start() -> Self {
        let configuration = tempfile::tempdir().unwrap();
        let path = configuration.path().join("session.conf");
        // This private bus needs no host session configuration or activation.
        std::fs::write(
            &path,
            r#"<busconfig>
<type>session</type>
<listen>unix:tmpdir=/tmp</listen>
<auth>EXTERNAL</auth>
<policy context="default">
  <allow send_destination="*"/>
  <allow receive_sender="*"/>
  <allow own="*"/>
</policy>
</busconfig>"#,
        )
        .unwrap();
        let mut process = std::process::Command::new("dbus-daemon")
            .arg("--config-file")
            .arg(path)
            .args(["--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("private dbus-daemon");
        let mut address = String::new();
        let bytes = std::io::BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        if bytes == 0 {
            let status = process.wait().unwrap();
            panic!("private dbus-daemon returned no address: {status}");
        }
        Self {
            process,
            address: address.trim().to_string(),
            _configuration: configuration,
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
    release: Arc<tokio::sync::Notify>,
    capabilities_entered: Arc<tokio::sync::Notify>,
    stall_capabilities: Arc<std::sync::atomic::AtomicBool>,
    stall: Arc<std::sync::atomic::AtomicBool>,
    reject: Arc<std::sync::atomic::AtomicBool>,
}
struct FakeServer {
    records: Records,
    actions: bool,
    stall: bool,
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeServer {
    async fn get_capabilities(&self) -> Vec<String> {
        self.records.capabilities_entered.notify_one();
        if self
            .records
            .stall_capabilities
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            std::future::pending::<()>().await;
        }
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
    ) -> zbus::fdo::Result<u32> {
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
        if self
            .records
            .reject
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(zbus::fdo::Error::Failed(
                "synthetic notification refusal".into(),
            ));
        }
        if self.stall || self.records.stall.load(std::sync::atomic::Ordering::SeqCst) {
            self.records.release.notified().await;
        }
        Ok(id)
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
fn attempt() -> AttemptContext {
    AttemptContext::new(config().deadline, None, CancellationToken::new())
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
    let context = attempt();
    let id = notice.delivery_id.clone();
    let initial = deliverer.begin(notice);
    assert_eq!(initial[0].outcome, DeliveryOutcome::Unknown);
    deliverer.send(&id, &permit, &context).await
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
    clear(&mut deliverer, "b", &permit).await;
    assert_eq!(
        *records.closed.lock().unwrap(),
        [a[0].notification_id.unwrap()]
    );
    assert!(deliverer.begin(notice("a", "source-a")).is_empty());
    assert!(deliverer.send("a", &permit, &attempt()).await.is_empty());
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
    let budget1 = send(&mut deliverer, budget).await;
    let mut budget_update = notice("budget-2", "source-a");
    budget_update.interrupt = false;
    budget_update.budget_passive_count = Some(2);
    let budget2 = send(&mut deliverer, budget_update).await;
    assert_eq!(budget1[0].notification_id, budget2[0].notification_id);
    assert!(deliverer.state.attempts()["a"].active);
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls[1].summary, "Source unreadable: 2 open occurrences");
    assert_eq!(calls[2].summary, "Source unreadable: 3 open occurrences");
    assert!(calls[0].body.ends_with("example read notice:a"));
    assert!(calls[0].actions.is_empty());
    assert_eq!(
        calls[4].summary,
        "2 notices held passive (interrupt budget spent)"
    );
    assert!(calls[4].body.ends_with("example read notice:budget-2"));
    assert_eq!(calls[2].urgency, 2);
    assert_eq!(calls[2].expire, 0);
    let shrink = clear(&mut deliverer, "b", &permit).await;
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 6);
    assert_eq!(calls[5].replaces, second[0].notification_id.unwrap());
    assert_eq!(calls[5].summary, "Source unreadable: 2 open occurrences");
    assert_eq!(calls[5].urgency, 2);
    assert!(calls[5].body.ends_with("example read notice:c"));
    assert_eq!(shrink.len(), 2);
    assert_eq!(shrink[0].delivery_id, "c");
    assert_eq!(shrink[0].outcome, DeliveryOutcome::Unknown);
    assert_eq!(shrink[1].outcome, DeliveryOutcome::Delivered);
    assert_eq!(
        deliverer.state.attempts()["budget-2"].notification_id,
        budget2[0].notification_id
    );
    let summary_open = deliverer.desktop_event(
        second[0].notification_id.unwrap(),
        DesktopEvent::OpenReference {
            reference: String::new(),
        },
    );
    assert_eq!(summary_open.len(), 1);
    assert_eq!(summary_open[0].delivery_id, "c");
    assert_eq!(
        summary_open[0].event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/c".into()
        })
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
    clear(&mut deliverer, "c", &permit).await;
    assert!(
        records
            .closed
            .lock()
            .unwrap()
            .contains(&second[0].notification_id.unwrap())
    );
    clear(&mut deliverer, "budget-2", &permit).await;
    assert!(deliverer.state.attempts()["a"].active);
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
    let delivered = deliverer.send("a", &permit, &attempt()).await;
    assert_eq!(delivered[0].outcome, DeliveryOutcome::Delivered);
    assert!(deliverer.server_appeared().is_empty());
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    let state = serde_json::from_str(&serde_json::to_string(&deliverer.state).unwrap()).unwrap();
    let mut restored = Deliverer::new(bus.connect().await, config(), state);
    assert!(restored.begin(notice("a", "source-a")).is_empty());
    assert!(restored.send("a", &permit, &attempt()).await.is_empty());
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
    assert!(slow.send("slow", &permit, &attempt()).await.is_empty());
    assert!(slow.server_appeared().is_empty());
    let mut restored = Deliverer::new(slow_bus.connect().await, short, slow.state);
    assert!(restored.begin(notice("slow", "source-a")).is_empty());
    assert!(restored.send("slow", &permit, &attempt()).await.is_empty());
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
    let cancelled = deliverer.send("a", &cancelled, &attempt()).await;
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
    let receipts = deliverer.send("a", &permit, &attempt()).await;
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
    let retried = deliverer.send("a", &permit, &attempt()).await;
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
    let worker = tokio::spawn(run(
        deliverer,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
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
    let worker = tokio::spawn(run(
        deliverer,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
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
    let worker = tokio::spawn(run(
        deliverer,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
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
    let worker = tokio::spawn(run(
        deliverer,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
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
    let worker = tokio::spawn(run(
        restored,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
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
    let worker = tokio::spawn(run(
        restored,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
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
    clear(&mut deliverer, "a", &permit).await;
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls[3].replaces, second[0].notification_id.unwrap());
    assert_eq!(calls[3].summary, "Source unreadable");
    assert!(calls[3].body.contains("source needs attention"));
    assert_eq!(calls[4].summary, "Source unreadable: 2 open occurrences");
    let b = deliverer.state.attempts()["b"].notification_id.unwrap();
    let c = deliverer.state.attempts()["c"].notification_id.unwrap();
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
    clear(&mut deliverer, "b", &permit).await;
    assert!(!deliverer.state.attempts()["a"].active);
    assert!(!deliverer.state.attempts()["b"].active);
    assert_eq!(
        *records.closed.lock().unwrap(),
        [first[0].notification_id.unwrap()]
    );
    assert!(deliverer.server_appeared().is_empty());
}

#[tokio::test]
async fn explicit_dbus_error_retries_on_new_owner_but_not_same_server_restart() {
    let bus = PrivateBus::start();
    let refused = Records::default();
    refused
        .reject
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let old_server = bus
        .server(FakeServer {
            records: refused.clone(),
            actions: true,
            stall: false,
        })
        .await;
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
    let mut initial = Deliverer::new(
        bus.connect().await,
        configured.clone(),
        NotificationState::default(),
    );
    let sent = send(&mut initial, notice("a", "source-a")).await;
    assert!(
        matches!(&sent[0].outcome, DeliveryOutcome::NotSent(reason) if reason.contains("synthetic notification refusal"))
    );
    assert_eq!(sent[1].outcome, DeliveryOutcome::Delivered);
    let command_bytes = std::fs::read(&path).unwrap();
    let checkpoint = serde_json::from_slice(&serde_json::to_vec(&initial.state).unwrap()).unwrap();
    let restored = Deliverer::new(bus.connect().await, configured, checkpoint);
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        restored,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
    // This empty update is a FIFO barrier after worker startup. The same owner
    // must retain the explicit error until the service actually reappears.
    requests
        .send(NotificationRequest::Clear("absent".into()))
        .unwrap();
    let barrier = tokio::time::timeout(Duration::from_secs(5), updated.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(barrier.receipts.is_empty());
    assert!(!barrier.state.attempts()["a"].retried);
    assert_eq!(
        barrier.state.attempts()["a"].dbus,
        Some(sent[0].outcome.clone())
    );
    barrier.persisted.send(()).unwrap();
    assert_eq!(refused.calls.lock().unwrap().len(), 1);
    old_server
        .release_name("org.freedesktop.Notifications")
        .await
        .unwrap();
    let accepted = Records::default();
    let _new_server = bus
        .server(FakeServer {
            records: accepted.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let retry = update(&mut updated).await;
    assert_eq!(retry.receipts.len(), 1);
    assert_eq!(retry.receipts[0].delivery_id, "a");
    assert_eq!(retry.receipts[0].route, Route::Dbus);
    assert_eq!(retry.receipts[0].outcome, DeliveryOutcome::Unknown);
    retry.persisted.send(()).unwrap();
    let delivered = update(&mut updated).await;
    assert_eq!(delivered.receipts.len(), 1);
    assert_eq!(delivered.receipts[0].delivery_id, "a");
    assert_eq!(delivered.receipts[0].outcome, DeliveryOutcome::Delivered);
    assert!(delivered.state.attempts()["a"].retried);
    assert_eq!(
        delivered.state.attempts()["a"].command,
        Some(DeliveryOutcome::Delivered)
    );
    delivered.persisted.send(()).unwrap();
    assert_eq!(accepted.calls.lock().unwrap().len(), 1);
    assert_eq!(std::fs::read(path).unwrap(), command_bytes);
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn missing_bus_records_dbus_failure_and_still_runs_both_route_command() {
    for route in [Route::Dbus, Route::Both] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("notice.json");
        let mut configured = config();
        configured.route = route;
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
        let worker = tokio::spawn(run(
            deliverer,
            received,
            updates,
            Arc::new(permit),
            Arc::new(|| 1000),
            CancellationToken::new(),
            None,
        ));
        requests
            .send(NotificationRequest::Deliver(Box::new(notice(
                "headless", "source-a",
            ))))
            .unwrap();
        let initial = update(&mut updated).await;
        let expected_routes = if route == Route::Both { 2 } else { 1 };
        assert_eq!(initial.receipts.len(), expected_routes);
        assert!(
            initial
                .receipts
                .iter()
                .all(|r| r.outcome == DeliveryOutcome::Unknown)
        );
        assert!(!path.exists());
        initial.persisted.send(()).unwrap();
        let delivered = update(&mut updated).await;
        assert_eq!(delivered.receipts.len(), expected_routes);
        assert_eq!(delivered.receipts[0].delivery_id, "headless");
        assert_eq!(delivered.receipts[0].route, Route::Dbus);
        assert_eq!(
            delivered.receipts[0].outcome,
            DeliveryOutcome::NotSent("no_notification_server".into())
        );
        assert!(!delivered.state.attempts()["headless"].retried);
        if route == Route::Both {
            assert_eq!(delivered.receipts[1].route, Route::Command);
            assert_eq!(delivered.receipts[1].outcome, DeliveryOutcome::Delivered);
            let got: Notice = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            assert_eq!(got, notice("headless", "source-a"));
        } else {
            assert!(!path.exists());
        }
        delivered.persisted.send(()).unwrap();
        drop(requests);
        worker.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn historical_occurrence_still_requires_current_policy() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut d = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let mut n = notice("historical", "once");
    n.current = false;
    d.begin(n);
    let result = d.send("historical", &cancelled, &attempt()).await;
    assert_eq!(result[0].outcome, DeliveryOutcome::Cancelled);
    assert!(records.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn grouped_replacement_checks_policy_before_notify() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut d = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    for id in ["a", "b", "c"] {
        send(&mut d, notice(id, id)).await;
    }
    let before = records.calls.lock().unwrap().len();
    clear(&mut d, "a", &cancelled).await;
    assert_eq!(records.calls.lock().unwrap().len(), before);
    assert!(d.state.attempts().values().all(|a| !a.active));
}

#[tokio::test]
async fn unknown_command_is_not_retried_when_dbus_appears() {
    let bus = PrivateBus::start();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("count");
    let mut configured = config();
    configured.route = Route::Both;
    configured.deadline = Duration::from_millis(100);
    configured.command = Some(RouteCommand {
        program: "sh".into(),
        arguments: vec![
            "-c".into(),
            "cat >/dev/null; printf x >> \"$1\"; exec sleep 10".into(),
            "route".into(),
            path.to_string_lossy().into_owned(),
        ],
    });
    let mut d = Deliverer::new(
        bus.connect().await,
        configured,
        NotificationState::default(),
    );
    d.begin(notice("a", "a"));
    let context = AttemptContext::new(Duration::from_millis(100), None, CancellationToken::new());
    let first = d.send("a", &permit, &context).await;
    assert_eq!(
        first[0].outcome,
        DeliveryOutcome::NotSent("no_notification_server".into())
    );
    assert_eq!(first[1].outcome, DeliveryOutcome::Unknown);
    assert_eq!(std::fs::read(&path).unwrap(), b"x");
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    d.server_appeared();
    let retry = d.send("a", &permit, &attempt()).await;
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].route, Route::Dbus);
    assert_eq!(retry[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(
        d.state.attempt("a").unwrap().command,
        Some(DeliveryOutcome::Unknown)
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"x");
}

#[tokio::test]
async fn live_import_retries_absent_server_once_after_persisting_unknown() {
    let bus = PrivateBus::start();
    let mut source = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut source, notice("a", "a")).await;
    let original = source.state.attempt("a").unwrap().clone();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let receiver = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        receiver,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
    // Consume startup owner checkpoint before inserting the transferred attempt.
    let startup = raw_update(&mut updated).await;
    startup.persisted.send(()).unwrap();
    let (ack, wait) = tokio::sync::oneshot::channel();
    requests
        .send(NotificationRequest::Import {
            attempt: Box::new(original.clone()),
            imported: ack,
        })
        .unwrap();
    let imported = raw_update(&mut updated).await;
    assert_eq!(imported.state.attempt("a").unwrap().dbus, original.dbus);
    assert!(records.calls.lock().unwrap().is_empty());
    imported.persisted.send(()).unwrap();
    assert_eq!(wait.await.unwrap(), Ok(()));
    let prepared = raw_update(&mut updated).await;
    assert!(prepared.state.attempt("a").unwrap().retried);
    assert_eq!(prepared.receipts[0].outcome, DeliveryOutcome::Unknown);
    assert!(records.calls.lock().unwrap().is_empty());
    prepared.persisted.send(()).unwrap();
    let completed = raw_update(&mut updated).await;
    assert_eq!(completed.receipts[0].outcome, DeliveryOutcome::Delivered);
    completed.persisted.send(()).unwrap();
    let (ack, wait) = tokio::sync::oneshot::channel();
    requests
        .send(NotificationRequest::Import {
            attempt: Box::new(original),
            imported: ack,
        })
        .unwrap();
    assert_eq!(wait.await.unwrap(), Ok(()));
    requests
        .send(NotificationRequest::Clear("barrier".into()))
        .unwrap();
    let barrier = raw_update(&mut updated).await;
    assert_eq!(
        barrier.state.attempt("a").unwrap().dbus,
        Some(DeliveryOutcome::Delivered)
    );
    assert!(barrier.state.attempt("a").unwrap().retried);
    barrier.persisted.send(()).unwrap();
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn release_persists_original_handoff_and_inactive_checkpoint_before_ack() {
    let bus = PrivateBus::start();
    let receiver = Deliverer::new(None, config(), NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        receiver,
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1000),
        CancellationToken::new(),
        None,
    ));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice("a", "a"))))
        .unwrap();
    let began = raw_update(&mut updated).await;
    began.persisted.send(()).unwrap();
    let sent = raw_update(&mut updated).await;
    let original = sent.state.attempt("a").unwrap().clone();
    sent.persisted.send(()).unwrap();
    let (ack, mut wait) = tokio::sync::oneshot::channel();
    requests
        .send(NotificationRequest::Release {
            delivery_id: "a".into(),
            released: ack,
        })
        .unwrap();
    let released = raw_update(&mut updated).await;
    assert_eq!(released.released, Some(original.clone()));
    assert!(!released.state.attempt("a").unwrap().active);
    assert_eq!(
        wait.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    );
    released.persisted.send(()).unwrap();
    assert_eq!(wait.await.unwrap(), Some(original));
    drop(requests);
    worker.await.unwrap().unwrap();
    drop(bus);
}

async fn raw_update(
    updates: &mut mpsc::UnboundedReceiver<NotificationUpdate>,
) -> NotificationUpdate {
    tokio::time::timeout(Duration::from_secs(5), updates.recv())
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn importing_grouped_attempts_in_reverse_order_keeps_the_visible_action_owner() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let mut source = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    for id in ["a", "b", "c"] {
        send(&mut source, notice(id, id)).await;
    }
    let mut state = NotificationState::default();
    state.set_server_owner(source.state.server_owner().map(str::to_owned));
    for id in ["c", "b", "a"] {
        state
            .import_attempt(source.state.attempt(id).unwrap().clone())
            .unwrap();
    }
    let mut receiver = Deliverer::new(bus.connect().await, config(), state);
    let id = source.state.attempt("c").unwrap().notification_id.unwrap();
    let actions = receiver.desktop_event(
        id,
        DesktopEvent::OpenReference {
            reference: String::new(),
        },
    );
    assert_eq!(
        actions
            .iter()
            .filter(|r| matches!(r.event, Some(DesktopEvent::OpenReference { .. })))
            .count(),
        1
    );
    assert_eq!(
        actions.iter().find(|r| r.delivery_id == "c").unwrap().event,
        Some(DesktopEvent::OpenReference {
            reference: notice("c", "c").reference
        })
    );
    assert_eq!(
        actions.iter().find(|r| r.delivery_id == "b").unwrap().event,
        Some(DesktopEvent::Seen)
    );
    let before = records.calls.lock().unwrap().len();
    assert!(receiver.send("c", &permit, &attempt()).await.is_empty());
    assert_eq!(records.calls.lock().unwrap().len(), before);
}

fn authorize(
    notices: &[Notice],
    decision: SendDecision,
) -> Result<Authorization, signal_delivery::PolicyFailure> {
    Ok(Authorization {
        decisions: notices
            .iter()
            .map(|n| (n.delivery_id.clone(), decision.clone()))
            .collect(),
    })
}
fn permit(request: PolicyRequest) -> signal_delivery::PolicyFuture {
    Box::pin(async move { authorize(&request.notices, SendDecision::Permit) })
}
fn cancelled(request: PolicyRequest) -> signal_delivery::PolicyFuture {
    Box::pin(async move { authorize(&request.notices, SendDecision::Cancel) })
}
async fn clear(deliverer: &mut Deliverer, id: &str, check: &Policy) -> Vec<NotificationReceipt> {
    let context = attempt();
    let mut receipts = deliverer.cancel(id, check, &context).await.unwrap();
    while let Some(id) = deliverer.ready_ids().into_iter().next() {
        receipts.extend(deliverer.send(&id, check, &context).await);
    }
    receipts
}

#[tokio::test]
async fn imported_retry_rechecks_changed_quiet_policy_after_restart_without_another_retry() {
    use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
    let bus = PrivateBus::start();
    let mut source = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut source, notice("held", "one")).await;
    let original = source.state.attempt("held").unwrap().clone();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let now = Arc::new(AtomicI64::new(1_000));
    let quiet_until = Arc::new(AtomicI64::new(2_000));
    let revision = Arc::new(AtomicU64::new(1));
    let check: signal_delivery::PolicyCheck = {
        let now = Arc::clone(&now);
        let quiet = Arc::clone(&quiet_until);
        let revision = Arc::clone(&revision);
        Arc::new(move |request| {
            let now = Arc::clone(&now);
            let quiet = Arc::clone(&quiet);
            let revision = Arc::clone(&revision);
            Box::pin(async move {
                let decision = if now.load(Ordering::SeqCst) < quiet.load(Ordering::SeqCst) {
                    SendDecision::HoldUntil {
                        at: quiet.load(Ordering::SeqCst),
                        policy: format!("owner-policy:{}", revision.load(Ordering::SeqCst)),
                    }
                } else {
                    SendDecision::Permit
                };
                authorize(&request.notices, decision)
            })
        })
    };
    let clock: signal_delivery::DeliveryClock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        Deliverer::new(bus.connect().await, config(), NotificationState::default()),
        received,
        updates,
        Arc::clone(&check),
        Arc::clone(&clock),
        CancellationToken::new(),
        None,
    ));
    raw_update(&mut updated).await.persisted.send(()).unwrap();
    let (ack, wait) = tokio::sync::oneshot::channel();
    requests
        .send(NotificationRequest::Import {
            attempt: Box::new(original.clone()),
            imported: ack,
        })
        .unwrap();
    raw_update(&mut updated).await.persisted.send(()).unwrap();
    assert_eq!(wait.await.unwrap(), Ok(()));
    let prepared = raw_update(&mut updated).await;
    assert!(prepared.state.attempt("held").unwrap().retried);
    prepared.persisted.send(()).unwrap();
    let held = raw_update(&mut updated).await;
    assert_eq!(
        held.state.held_until("held", Route::Dbus),
        Some(&HeldRoute {
            at: 2_000,
            policy: "owner-policy:1".into()
        })
    );
    assert_eq!(
        held.state.attempt("held").unwrap().dbus,
        Some(DeliveryOutcome::Unknown)
    );
    assert!(records.calls.lock().unwrap().is_empty());
    let checkpoint: NotificationState =
        serde_json::from_slice(&serde_json::to_vec(&held.state).unwrap()).unwrap();
    held.persisted.send(()).unwrap();
    let (ack, wait) = tokio::sync::oneshot::channel();
    requests
        .send(NotificationRequest::Import {
            attempt: Box::new(original),
            imported: ack,
        })
        .unwrap();
    assert_eq!(wait.await.unwrap(), Ok(()));
    requests
        .send(NotificationRequest::Clear("barrier".into()))
        .unwrap();
    let barrier = raw_update(&mut updated).await;
    assert_eq!(
        barrier.state.held_until("held", Route::Dbus),
        checkpoint.held_until("held", Route::Dbus)
    );
    barrier.persisted.send(()).unwrap();
    drop(requests);
    worker.await.unwrap().unwrap();
    now.store(2_000, Ordering::SeqCst);
    quiet_until.store(3_000, Ordering::SeqCst);
    revision.store(2, Ordering::SeqCst);
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        Deliverer::new(bus.connect().await, config(), checkpoint),
        received,
        updates,
        check,
        clock,
        CancellationToken::new(),
        None,
    ));
    let prepared = raw_update(&mut updated).await;
    assert!(prepared.state.held_until("held", Route::Dbus).is_none());
    assert!(prepared.state.attempt("held").unwrap().retried);
    prepared.persisted.send(()).unwrap();
    let held = raw_update(&mut updated).await;
    assert_eq!(
        held.state.held_until("held", Route::Dbus),
        Some(&HeldRoute {
            at: 3_000,
            policy: "owner-policy:2".into()
        })
    );
    assert!(records.calls.lock().unwrap().is_empty());
    held.persisted.send(()).unwrap();
    now.store(3_000, Ordering::SeqCst);
    requests.send(NotificationRequest::Recheck).unwrap();
    let prepared = raw_update(&mut updated).await;
    assert_eq!(prepared.receipts[0].outcome, DeliveryOutcome::Unknown);
    assert!(records.calls.lock().unwrap().is_empty());
    prepared.persisted.send(()).unwrap();
    let delivered = raw_update(&mut updated).await;
    assert_eq!(delivered.receipts[0].outcome, DeliveryOutcome::Delivered);
    assert!(delivered.state.attempt("held").unwrap().retried);
    assert!(delivered.state.held_until("held", Route::Dbus).is_none());
    delivered.persisted.send(()).unwrap();
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn policy_batch_uses_one_short_fence_for_all_members_without_retaining_it_for_receipts() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    struct Lease(Arc<AtomicBool>);
    impl Drop for Lease {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }
    let active = Arc::new(AtomicBool::new(false));
    let batches = Arc::new(AtomicUsize::new(0));
    let members = Arc::new(AtomicUsize::new(0));
    let check: signal_delivery::PolicyCheck = {
        let active = Arc::clone(&active);
        let batches = Arc::clone(&batches);
        let members = Arc::clone(&members);
        Arc::new(move |request| {
            let active = Arc::clone(&active);
            let batches = Arc::clone(&batches);
            let members = Arc::clone(&members);
            Box::pin(async move {
                assert!(
                    !active.swap(true, Ordering::SeqCst),
                    "previous admission fence was not released"
                );
                batches.fetch_add(1, Ordering::SeqCst);
                members.store(request.notices.len(), Ordering::SeqCst);
                let fence = Lease(Arc::clone(&active));
                let authorization = authorize(
                    &request.notices,
                    SendDecision::HoldUntil {
                        at: 10_000,
                        policy: "policy:current".into(),
                    },
                );
                drop(fence);
                authorization
            })
        })
    };
    let mut state = NotificationState::default();
    for n in 0..32 {
        state.begin(
            notice(&format!("member-{n}"), &format!("occurrence-{n}")),
            Route::Command,
        );
    }
    let mut configured = config();
    configured.route = Route::Command;
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        Deliverer::new(None, configured, state),
        received,
        updates,
        check,
        Arc::new(|| 1_000),
        CancellationToken::new(),
        None,
    ));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice("new", "new"))))
        .unwrap();
    raw_update(&mut updated).await.persisted.send(()).unwrap();
    let held = raw_update(&mut updated).await;
    assert!(held.receipts.is_empty());
    assert_eq!(batches.load(Ordering::SeqCst), 1);
    assert_eq!(members.load(Ordering::SeqCst), 33);
    assert!(
        !active.load(Ordering::SeqCst),
        "the policy fence ended before any receipt write"
    );
    held.persisted.send(()).unwrap();
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn crash_after_due_hold_unknown_checkpoint_never_resends_on_restart() {
    let bus = PrivateBus::start();
    let mut source = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    send(&mut source, notice("held", "one")).await;
    source.server_appeared();
    source
        .state
        .hold("held", [Route::Dbus], 1_000, "policy:one");
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        Deliverer::new(bus.connect().await, config(), source.state),
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1_000),
        CancellationToken::new(),
        None,
    ));
    let prepared = raw_update(&mut updated).await;
    assert!(prepared.state.attempt("held").unwrap().retried);
    assert!(prepared.state.held_until("held", Route::Dbus).is_none());
    let checkpoint = prepared.state.clone();
    assert!(records.calls.lock().unwrap().is_empty());
    // The committed unknown body survives, but the worker never got its ack.
    drop(prepared);
    worker.await.unwrap().unwrap();
    drop(requests);
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        Deliverer::new(bus.connect().await, config(), checkpoint),
        received,
        updates,
        Arc::new(permit),
        Arc::new(|| 1_000),
        CancellationToken::new(),
        None,
    ));
    raw_update(&mut updated).await.persisted.send(()).unwrap();
    requests
        .send(NotificationRequest::Clear("barrier".into()))
        .unwrap();
    let barrier = raw_update(&mut updated).await;
    assert_eq!(
        barrier.state.attempt("held").unwrap().dbus,
        Some(DeliveryOutcome::Unknown)
    );
    barrier.persisted.send(()).unwrap();
    assert!(records.calls.lock().unwrap().is_empty());
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn stalled_server_does_not_hold_policy_lock_and_the_next_route_rechecks_policy() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    let policy = Arc::new(tokio::sync::RwLock::new(true));
    let active = Arc::new(AtomicBool::new(false));
    let calls = Arc::new(AtomicUsize::new(0));
    let bus = PrivateBus::start();
    let records = Records::default();
    records.stall.store(true, Ordering::SeqCst);
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    struct Lease {
        active: Arc<AtomicBool>,
        _guard: tokio::sync::OwnedRwLockReadGuard<bool>,
    }
    impl Drop for Lease {
        fn drop(&mut self) {
            self.active.store(false, Ordering::SeqCst);
        }
    }
    let check: signal_delivery::PolicyCheck = {
        let active = Arc::clone(&active);
        let calls = Arc::clone(&calls);
        let policy = Arc::clone(&policy);
        Arc::new(move |request| {
            let active = Arc::clone(&active);
            let calls = Arc::clone(&calls);
            let policy = Arc::clone(&policy);
            Box::pin(async move {
                calls.fetch_add(1, Ordering::SeqCst);
                let guard = Arc::clone(&policy).try_read_owned().unwrap();
                let decision = if *guard {
                    SendDecision::Permit
                } else {
                    SendDecision::Cancel
                };
                assert!(!active.swap(true, Ordering::SeqCst));
                let fence = Lease {
                    active: Arc::clone(&active),
                    _guard: guard,
                };
                let authorization = authorize(&request.notices, decision);
                drop(fence);
                authorization
            })
        })
    };
    let directory = tempfile::tempdir().unwrap();
    let command_output = directory.path().join("command-output");
    let mut configured = config();
    configured.route = Route::Both;
    configured.command = Some(RouteCommand {
        program: "sh".into(),
        arguments: vec![
            "-c".into(),
            "cat > \"$1\"".into(),
            "route".into(),
            command_output.to_string_lossy().into_owned(),
        ],
    });
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        Deliverer::new(
            bus.connect().await,
            configured,
            NotificationState::default(),
        ),
        received,
        updates,
        check,
        Arc::new(|| 1_000),
        CancellationToken::new(),
        None,
    ));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "first", "one",
        ))))
        .unwrap();
    let initial = update(&mut updated).await;
    initial.persisted.send(()).unwrap();
    records.entered.notified().await;
    assert!(
        !active.load(Ordering::SeqCst),
        "no policy fence is retained during Notify"
    );
    *policy
        .try_write()
        .expect("policy changes proceed while Notify is stalled") = false;
    records.release.notify_one();
    let completed = update(&mut updated).await;
    assert_eq!(completed.receipts[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(completed.receipts[1].outcome, DeliveryOutcome::Cancelled);
    assert!(
        !command_output.exists(),
        "the independently admitted command obeys the changed policy"
    );
    completed.persisted.send(()).unwrap();
    records.stall.store(false, Ordering::SeqCst);
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "next", "two",
        ))))
        .unwrap();
    let initial = update(&mut updated).await;
    initial.persisted.send(()).unwrap();
    let cancelled = update(&mut updated).await;
    assert_eq!(cancelled.receipts[0].outcome, DeliveryOutcome::Cancelled);
    cancelled.persisted.send(()).unwrap();
    assert!(calls.load(Ordering::SeqCst) >= 2);
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn platform_effect_waits_for_asynchronous_owner_admission_completion() {
    let bus = PrivateBus::start();
    let records = Records::default();
    let _server = bus
        .server(FakeServer {
            records: records.clone(),
            actions: true,
            stall: false,
        })
        .await;
    let entered = Arc::new(tokio::sync::Notify::new());
    let complete = Arc::new(tokio::sync::Notify::new());
    let check: signal_delivery::PolicyCheck = {
        let entered = Arc::clone(&entered);
        let complete = Arc::clone(&complete);
        Arc::new(move |request| {
            let entered = Arc::clone(&entered);
            let complete = Arc::clone(&complete);
            Box::pin(async move {
                entered.notify_one();
                complete.notified().await;
                authorize(&request.notices, SendDecision::Permit)
            })
        })
    };
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(
        Deliverer::new(bus.connect().await, config(), NotificationState::default()),
        received,
        updates,
        check,
        Arc::new(|| 1_000),
        CancellationToken::new(),
        None,
    ));
    requests
        .send(NotificationRequest::Deliver(Box::new(notice(
            "async", "one",
        ))))
        .unwrap();
    let initial = update(&mut updated).await;
    initial.persisted.send(()).unwrap();
    entered.notified().await;
    assert!(records.calls.lock().unwrap().is_empty());
    complete.notify_one();
    let delivered = update(&mut updated).await;
    assert_eq!(delivered.receipts[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    delivered.persisted.send(()).unwrap();
    drop(requests);
    worker.await.unwrap().unwrap();
}
