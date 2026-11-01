//! The worker against a private D-Bus daemon and a fake notification server
//! that implements capabilities, `replaces_id`, actions, close reasons and
//! `ActivationToken`.
use signal_delivery::{Deliverer, NotificationRequest, NotificationUpdate, run};
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
    hints: HashMap<String, String>,
    expire: i32,
}
impl Call {
    fn hint(&self, name: &str) -> Option<&str> {
        self.hints.get(name).map(String::as_str)
    }
}
#[derive(Clone, Default)]
struct Records {
    calls: Arc<Mutex<Vec<Call>>>,
    closed: Arc<Mutex<Vec<u32>>>,
    entered: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}
struct FakeServer {
    records: Records,
    actions: bool,
    stall: bool,
}
fn text(value: &OwnedValue) -> String {
    if let Ok(text) = <&str>::try_from(value) {
        text.to_string()
    } else if let Ok(number) = u8::try_from(value) {
        number.to_string()
    } else if let Ok(flag) = bool::try_from(value) {
        flag.to_string()
    } else {
        format!("{value:?}")
    }
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeServer {
    async fn get_capabilities(&self) -> Vec<String> {
        if self.actions {
            vec!["actions".into(), "body".into()]
        } else {
            vec!["body".into()]
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
                hints: hints.iter().map(|(k, v)| (k.clone(), text(v))).collect(),
                expire,
            });
            if replaces == 0 {
                calls.len() as u32
            } else {
                replaces
            }
        };
        self.records.entered.notify_one();
        if self.stall {
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
    async fn activation_token(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        id: u32,
        token: &str,
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
        away_command: None,
        deadline: Duration::from_secs(2),
    }
}
fn desktop() -> Outlets {
    Outlets {
        desktop: true,
        command: false,
    }
}
fn notice(id: &str, occurrence: &str) -> Notice {
    Notice {
        delivery_id: id.into(),
        product: "Example".into(),
        icon: "example".into(),
        desktop_entry: Some("example-web".into()),
        rule: "source-unreadable".into(),
        occurrence: occurrence.into(),
        summary: "Source unreadable".into(),
        body: "A source needs attention".into(),
        lines: vec![],
        reference: format!("https://example.invalid/notices/{id}"),
        command_hint: format!("example read notice:{id}"),
        current: true,
        transient: false,
        critical: false,
        outlets: desktop(),
        actions: vec![],
        showing: 0,
    }
}
async fn send(deliverer: &mut Deliverer, notice: Notice) -> Vec<NotificationReceipt> {
    let id = notice.delivery_id.clone();
    let initial = deliverer.begin(notice);
    assert!(
        initial
            .iter()
            .all(|receipt| receipt.outcome == DeliveryOutcome::Unknown)
    );
    deliverer.send(&id).await
}
async fn clear(deliverer: &mut Deliverer, id: &str) -> Vec<NotificationReceipt> {
    let mut receipts = deliverer.cancel(id).await;
    for id in deliverer.ready_ids() {
        receipts.extend(deliverer.send(&id).await);
    }
    receipts
}
async fn fake(bus: &PrivateBus, actions: bool, stall: bool) -> (Records, zbus::Connection) {
    let records = Records::default();
    let connection = bus
        .server(FakeServer {
            records: records.clone(),
            actions,
            stall,
        })
        .await;
    (records, connection)
}

#[tokio::test]
async fn notification_replaced_in_place_and_current_clear_closes() {
    let bus = PrivateBus::start();
    let (records, _server) = fake(&bus, true, false).await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let a = send(&mut deliverer, notice("a", "source-a")).await;
    let mut b = notice("b", "source-a");
    b.critical = true;
    b.current = false;
    b.transient = true;
    let b = send(&mut deliverer, b).await;
    assert_eq!(a[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(b[0].notification_id, a[0].notification_id);
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls[0].replaces, 0);
    assert_eq!(calls[1].replaces, a[0].notification_id.unwrap());
    assert_eq!(calls[0].app, "Example");
    assert_eq!(calls[0].icon, "example");
    assert_eq!(calls[0].hint("category"), Some("Example.source-unreadable"));
    assert_eq!(calls[0].hint("urgency"), Some("1"));
    assert_eq!(calls[0].hint("x-example-delivery"), Some("a"));
    assert_eq!(calls[0].hint("desktop-entry"), Some("example-web"));
    assert_eq!(calls[0].hint("resident"), Some("true"));
    assert_eq!(calls[0].hint("transient"), None);
    assert_eq!(calls[0].expire, -1);
    assert_eq!(calls[1].hint("urgency"), Some("2"));
    assert_eq!(calls[1].hint("transient"), Some("true"));
    assert_eq!(calls[1].hint("resident"), None);
    assert_eq!(calls[1].expire, 0);
    assert_eq!(
        calls[0].actions,
        [
            "default",
            "Open",
            "snooze",
            "Snooze 1h",
            "acknowledge",
            "Acknowledge"
        ]
    );
    clear(&mut deliverer, "b").await;
    assert_eq!(
        *records.closed.lock().unwrap(),
        [a[0].notification_id.unwrap()]
    );
    assert!(deliverer.begin(notice("a", "source-a")).is_empty());
    assert!(deliverer.send("a").await.is_empty());
}

#[tokio::test]
async fn grouping_folds_open_occurrences_and_summaries_replace_in_place() {
    let bus = PrivateBus::start();
    let (records, _server) = fake(&bus, false, false).await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let mut losing_data = notice("a", "source-a");
    losing_data.critical = true;
    let first = send(&mut deliverer, losing_data).await;
    let second = send(&mut deliverer, notice("b", "source-b")).await;
    let third = send(&mut deliverer, notice("c", "source-c")).await;
    assert_ne!(first[0].notification_id, second[0].notification_id);
    assert_eq!(second[0].notification_id, third[0].notification_id);
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls[1].summary, "Source unreadable: 2 open occurrences");
    assert_eq!(calls[2].summary, "Source unreadable: 3 open occurrences");
    assert_eq!(calls[2].hint("urgency"), Some("2"));
    // Without the actions capability the body names the command.
    assert!(calls[0].body.ends_with("example read notice:a"));
    assert!(calls[0].actions.is_empty());
    let opened = deliverer.desktop_event(
        second[0].notification_id.unwrap(),
        DesktopEvent::OpenReference {
            reference: String::new(),
            activation_token: None,
        },
    );
    assert_eq!(
        opened.iter().find(|r| r.delivery_id == "b").unwrap().event,
        Some(DesktopEvent::Seen)
    );
    assert_eq!(
        opened.iter().find(|r| r.delivery_id == "c").unwrap().event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/c".into(),
            activation_token: None,
        })
    );
    let shrink = clear(&mut deliverer, "b").await;
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[3].replaces, second[0].notification_id.unwrap());
    assert_eq!(calls[3].summary, "Source unreadable: 2 open occurrences");
    assert_eq!(shrink[0].delivery_id, "c");
    assert_eq!(shrink[0].outcome, DeliveryOutcome::Unknown);
    assert_eq!(shrink[1].outcome, DeliveryOutcome::Delivered);
    clear(&mut deliverer, "c").await;
    assert!(
        records
            .closed
            .lock()
            .unwrap()
            .contains(&second[0].notification_id.unwrap())
    );

    // A summary notification: each newer count replaces it in place.
    let budget =
        |held_passive, at| signals::Summary::new(at, signals::SummaryKind::Budget { held_passive });
    let one = send(
        &mut deliverer,
        Notice::for_summary(&budget(1, 10), "Example", "example", "/notices"),
    )
    .await;
    let two = send(
        &mut deliverer,
        Notice::for_summary(&budget(2, 20), "Example", "example", "/notices"),
    )
    .await;
    assert_eq!(one[0].notification_id, two[0].notification_id);
    let calls = records.calls.lock().unwrap().clone();
    assert_eq!(
        calls.last().unwrap().summary,
        "2 notices held passive (interrupt budget spent)"
    );
    assert!(deliverer.state.attempts()["a"].active);
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
    let (records, _server) = fake(&bus, true, false).await;
    let retry = deliverer.server_appeared();
    assert_eq!(retry[0].delivery_id, "a");
    assert_eq!(retry[0].outcome, DeliveryOutcome::Unknown);
    let delivered = deliverer.send("a").await;
    assert_eq!(delivered[0].outcome, DeliveryOutcome::Delivered);
    assert!(deliverer.server_appeared().is_empty());
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    let state = serde_json::from_str(&serde_json::to_string(&deliverer.state).unwrap()).unwrap();
    let mut restored = Deliverer::new(bus.connect().await, config(), state);
    assert!(restored.begin(notice("a", "source-a")).is_empty());
    assert!(restored.send("a").await.is_empty());
    assert!(restored.server_appeared().is_empty());

    let slow_bus = PrivateBus::start();
    let (slow_records, _slow_server) = fake(&slow_bus, true, true).await;
    let mut short = config();
    short.deadline = Duration::from_millis(500);
    let mut slow = Deliverer::new(
        slow_bus.connect().await,
        short.clone(),
        NotificationState::default(),
    );
    let unknown = send(&mut slow, notice("slow", "source-a")).await;
    assert_eq!(unknown[0].outcome, DeliveryOutcome::Unknown);
    assert!(slow.send("slow").await.is_empty());
    assert!(slow.server_appeared().is_empty());
    let mut restored = Deliverer::new(slow_bus.connect().await, short, slow.state);
    assert!(restored.begin(notice("slow", "source-a")).is_empty());
    assert!(restored.send("slow").await.is_empty());
    assert_eq!(slow_records.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn a_later_showing_is_shown_again_under_the_same_identity() {
    let bus = PrivateBus::start();
    let (records, _server) = fake(&bus, true, false).await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let first = send(&mut deliverer, notice("a", "source-a")).await;
    let id = first[0].notification_id.unwrap();
    deliverer.desktop_event(id, DesktopEvent::Dismissed);
    let mut again = notice("a", "source-a");
    assert!(deliverer.begin(again.clone()).is_empty());
    again.showing = 1;
    let shown = send(&mut deliverer, again).await;
    assert_eq!(shown[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(records.calls.lock().unwrap().len(), 2);
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
async fn worker_records_actions_as_desktop_operator_acts_with_activation_token() {
    let bus = PrivateBus::start();
    let (records, server) = fake(&bus, true, false).await;
    let deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates));
    let mut waiting = notice("a", "session-a");
    waiting.actions = vec![NoticeAction {
        key: "approve".into(),
        label: "Approve".into(),
    }];
    requests
        .send(NotificationRequest::Deliver(Box::new(waiting)))
        .unwrap();
    let initial = update(&mut updated).await;
    assert_eq!(initial.receipts[0].outcome, DeliveryOutcome::Unknown);
    assert!(records.calls.lock().unwrap().is_empty());
    initial.persisted.send(()).unwrap();
    let delivered = update(&mut updated).await;
    let id = delivered.receipts[0].notification_id.unwrap();
    assert_eq!(
        delivered.receipts[0]
            .outcomes(5)
            .iter()
            .map(|record| record.outcome.clone())
            .collect::<Vec<_>>(),
        [
            signals::Outcome::Attempt {
                result: DeliveryOutcome::Delivered
            },
            signals::Outcome::Shown
        ]
    );
    delivered.persisted.send(()).unwrap();
    assert_eq!(
        records.calls.lock().unwrap()[0].actions[6..],
        ["approve", "Approve"]
    );
    let interface = server
        .object_server()
        .interface::<_, FakeServer>("/org/freedesktop/Notifications")
        .await
        .unwrap();
    let emitter = interface.signal_emitter();
    FakeServer::activation_token(emitter, id, "token-1")
        .await
        .unwrap();
    FakeServer::action_invoked(emitter, id, "default")
        .await
        .unwrap();
    let opened = update(&mut updated).await;
    assert_eq!(
        opened.receipts[0].event,
        Some(DesktopEvent::OpenReference {
            reference: "https://example.invalid/notices/a".into(),
            activation_token: Some("token-1".into()),
        })
    );
    assert_eq!(opened.receipts[0].via.as_deref(), Some("desktop"));
    opened.persisted.send(()).unwrap();
    for (action, event) in [
        ("snooze", DesktopEvent::Snooze),
        ("acknowledge", DesktopEvent::Acknowledged),
        (
            "approve",
            DesktopEvent::Action {
                key: "approve".into(),
            },
        ),
    ] {
        FakeServer::action_invoked(emitter, id, action)
            .await
            .unwrap();
        let acted = update(&mut updated).await;
        assert_eq!(acted.receipts[0].event, Some(event));
        acted.persisted.send(()).unwrap();
    }
    FakeServer::notification_closed(emitter, id, 2)
        .await
        .unwrap();
    let dismissed = update(&mut updated).await;
    assert_eq!(dismissed.receipts[0].event, Some(DesktopEvent::Dismissed));
    assert_eq!(
        dismissed.receipts[0].outcomes(9)[0].outcome,
        signals::Outcome::Closed {
            reason: signals::CloseReason::Dismissed
        }
    );
    dismissed.persisted.send(()).unwrap();
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn away_command_receives_json_and_both_outlets_have_separate_receipts() {
    let bus = PrivateBus::start();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notice.json");
    let mut configured = config();
    configured.away_command = Some(RouteCommand {
        program: "sh".into(),
        arguments: vec![
            "-c".into(),
            "cat >> \"$1\"".into(),
            "away".into(),
            path.to_string_lossy().into_owned(),
        ],
    });
    let mut deliverer = Deliverer::new(
        bus.connect().await,
        configured,
        NotificationState::default(),
    );
    // A critical notice while the operator is away goes to both outlets.
    let mut critical = notice("a", "source-a");
    critical.critical = true;
    critical.outlets = Outlets::of(&signals::Decision::Show { send_away: true });
    let initial = deliverer.begin(critical.clone());
    assert_eq!(initial.len(), 2);
    let receipts = deliverer.send("a").await;
    assert_eq!(
        receipts[0].outcome,
        DeliveryOutcome::NotSent("no_notification_server".into())
    );
    assert_eq!(receipts[1].outlet, Outlet::Command);
    assert_eq!(receipts[1].outcome, DeliveryOutcome::Delivered);
    let first_bytes = std::fs::read(&path).unwrap();
    let got: Notice = serde_json::from_slice(&first_bytes).unwrap();
    assert_eq!(got, critical);
    let (_records, _server) = fake(&bus, true, false).await;
    let retry = deliverer.server_appeared();
    assert_eq!(retry.len(), 1);
    assert_eq!(retry[0].outlet, Outlet::Desktop);
    let retried = deliverer.send("a").await;
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].outcome, DeliveryOutcome::Delivered);
    assert_eq!(std::fs::read(&path).unwrap(), first_bytes);

    // A notice sent away never touches the desktop.
    let mut away = notice("b", "source-b");
    away.outlets = Outlets::of(&signals::Decision::SendAway);
    let receipts = send(&mut deliverer, away).await;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].outlet, Outlet::Command);
}

#[tokio::test]
async fn command_only_worker_needs_no_session_bus() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("notice.json");
    let mut configured = config();
    configured.away_command = Some(RouteCommand {
        program: "sh".into(),
        arguments: vec![
            "-c".into(),
            "cat > \"$1\"".into(),
            "away".into(),
            path.to_string_lossy().into_owned(),
        ],
    });
    let deliverer = Deliverer::new(None, configured, NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates));
    let mut headless = notice("headless", "source-a");
    headless.outlets = Outlets::of(&signals::Decision::SendAway);
    requests
        .send(NotificationRequest::Deliver(Box::new(headless)))
        .unwrap();
    let initial = update(&mut updated).await;
    assert_eq!(initial.receipts[0].outlet, Outlet::Command);
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
    let (records, _server) = fake(&bus, true, true).await;
    let mut configured = config();
    configured.deadline = Duration::from_secs(1);
    let deliverer = Deliverer::new(
        bus.connect().await,
        configured,
        NotificationState::default(),
    );
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates));
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
    // An unknown attempt is never resent.
    assert_eq!(records.calls.lock().unwrap().len(), 1);
    drop(requests);
    worker.await.unwrap().unwrap();
}

#[tokio::test]
async fn service_appearance_retries_automatically() {
    let bus = PrivateBus::start();
    let deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let (requests, received) = mpsc::unbounded_channel();
    let (updates, mut updated) = mpsc::unbounded_channel();
    let worker = tokio::spawn(run(deliverer, received, updates));
    for (id, occurrence) in [("a", "source-a"), ("b", "source-b")] {
        requests
            .send(NotificationRequest::Deliver(Box::new(notice(
                id, occurrence,
            ))))
            .unwrap();
        let initial = update(&mut updated).await;
        initial.persisted.send(()).unwrap();
        let missing = update(&mut updated).await;
        assert_eq!(
            missing.receipts[0].outcome,
            DeliveryOutcome::NotSent("no_notification_server".into())
        );
        missing.persisted.send(()).unwrap();
    }
    let (records, _server) = fake(&bus, true, false).await;
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
async fn a_producer_key_replaces_its_notification_in_place() {
    let bus = PrivateBus::start();
    let (records, _server) = fake(&bus, true, false).await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    // The host deliverer's product shows producer notices under the
    // implicit rule, with the producer's key as the occurrence.
    let producer = |id: &str| Notice {
        rule: "producer:pueue".into(),
        current: false,
        ..notice(id, "job-418")
    };
    let first = send(&mut deliverer, producer("p1")).await;
    let second = send(&mut deliverer, producer("p2")).await;
    assert_eq!(first[0].notification_id, second[0].notification_id);
    assert_eq!(
        records.calls.lock().unwrap()[1].replaces,
        first[0].notification_id.unwrap()
    );
}

#[tokio::test]
async fn consequence_lines_are_part_of_the_root_notification() {
    let bus = PrivateBus::start();
    let (records, _server) = fake(&bus, true, false).await;
    let mut deliverer = Deliverer::new(bus.connect().await, config(), NotificationState::default());
    let mut root = notice("root", "host");
    root.summary = "Sinex stalled since 07:52".into();
    root.body = "1 source affected".into();
    root.lines = vec!["capture stalled · Atuin (inhibited)".into()];
    send(&mut deliverer, root).await;
    assert_eq!(
        records.calls.lock().unwrap()[0].body,
        "1 source affected\n└ capture stalled · Atuin (inhibited)"
    );
}
