use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    #[serde(default)]
    events: bool,
}
fn spec(name: &str) -> OperationSpec {
    OperationSpec {
        failures: vec![FailureKind::StoppedEarly],
        ..OperationSpec::request(
            name,
            &[name],
            "query",
            "Synthetic read",
            "Exercise typed request lifetime",
        )
    }
}
fn builder() -> CatalogBuilder {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    catalog
        .declare_tool(ToolFamily {
            name: "query".into(),
            description: "Read".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    catalog
}
fn timed(name: &str, arguments: Value) -> RunnableRequest {
    let mut request = RunnableRequest::new(name, arguments, "request");
    request.deadline = Deadline::Milliseconds(1000);
    request
}
fn error(answer: &Answer<Value>) -> &Failure {
    let Outcome::Error(failure) = &answer.outcome else {
        panic!("expected a failure")
    };
    failure
}

struct Handoff {
    entered: mpsc::UnboundedSender<RequestContext>,
    owned: mpsc::UnboundedSender<()>,
    proceed: Arc<Notify>,
    acknowledge: Arc<Notify>,
    effects: Arc<AtomicUsize>,
}
impl Operation for Handoff {
    type Request = Input;
    type Response = bool;
    fn execute(&self, _: Input, context: RequestContext) -> OperationFuture<'_, Answer<bool>> {
        Box::pin(async move {
            self.entered.send(context.clone()).unwrap();
            self.proceed.notified().await;
            if let Err(failure) = context.finish_read_phase() {
                return Answer::error(failure);
            }
            self.effects.fetch_add(1, Ordering::SeqCst);
            self.owned.send(()).unwrap();
            self.acknowledge.notified().await;
            Answer::ok(true)
        })
    }
}
struct Fixture {
    table: Arc<OperationTable>,
    entered: mpsc::UnboundedReceiver<RequestContext>,
    owned: mpsc::UnboundedReceiver<()>,
    proceed: Arc<Notify>,
    acknowledge: Arc<Notify>,
    effects: Arc<AtomicUsize>,
}
fn handoff() -> Fixture {
    let (entered, receive) = mpsc::unbounded_channel();
    let (owned, ownership) = mpsc::unbounded_channel();
    let proceed = Arc::new(Notify::new());
    let acknowledge = Arc::new(Notify::new());
    let effects = Arc::new(AtomicUsize::new(0));
    let mut catalog = builder();
    catalog
        .declare::<Input, bool>(spec("write"), |value| value.to_string())
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table
        .bind(
            "write",
            Handoff {
                entered,
                owned,
                proceed: proceed.clone(),
                acknowledge: acknowledge.clone(),
                effects: effects.clone(),
            },
        )
        .unwrap();
    Fixture {
        table: Arc::new(table),
        entered: receive,
        owned: ownership,
        proceed,
        acknowledge,
        effects,
    }
}

#[tokio::test(start_paused = true)]
async fn expired_read_cannot_handoff_even_before_timer_task_polls() {
    let mut fixture = handoff();
    let table = fixture.table.clone();
    let request = timed("write", json!({}));
    let accepted = request.clone();
    let running = tokio::spawn(async move {
        table
            .call(request, Via::Cli, CancellationToken::new())
            .await
    });
    let context = fixture.entered.recv().await.unwrap();
    assert_eq!(
        serde_json::to_value(context.request()).unwrap(),
        serde_json::to_value(accepted).unwrap()
    );
    assert!(context.read_deadline().is_some());
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(
        context.finish_read_phase().unwrap_err().kind,
        FailureKind::Timeout
    );
    fixture.proceed.notify_one();
    assert_eq!(error(&running.await.unwrap()).kind, FailureKind::Timeout);
    assert_eq!(fixture.effects.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn owned_write_returns_actual_acknowledgement_after_deadline_and_cancellation() {
    let mut fixture = handoff();
    let table = fixture.table.clone();
    let cancellation = CancellationToken::new();
    let parent = cancellation.clone();
    let running = tokio::spawn(async move {
        table
            .call(timed("write", json!({})), Via::Cli, parent)
            .await
    });
    let context = fixture.entered.recv().await.unwrap();
    fixture.proceed.notify_one();
    fixture.owned.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    cancellation.cancel();
    tokio::task::yield_now().await;
    assert!(!running.is_finished());
    assert!(context.finish_read_phase().is_ok());
    assert_eq!(fixture.effects.load(Ordering::SeqCst), 1);
    fixture.acknowledge.notify_one();
    assert_eq!(running.await.unwrap().value(), Some(&json!(true)));
}

#[tokio::test]
async fn cancellation_winning_before_handoff_forbids_the_effect() {
    let mut fixture = handoff();
    let table = fixture.table.clone();
    let cancellation = CancellationToken::new();
    let parent = cancellation.clone();
    let running = tokio::spawn(async move {
        table
            .call(timed("write", json!({})), Via::Cli, parent)
            .await
    });
    let context = fixture.entered.recv().await.unwrap();
    cancellation.cancel();
    assert_eq!(
        context.finish_read_phase().unwrap_err().kind,
        FailureKind::Cancelled
    );
    fixture.proceed.notify_one();
    assert_eq!(error(&running.await.unwrap()).kind, FailureKind::Cancelled);
    assert_eq!(fixture.effects.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn connected_socket_client_waits_for_owned_writer_ack_beyond_read_deadline() {
    let mut fixture = handoff();
    let (client, server) = tokio::io::duplex(4096);
    let server_table = fixture.table.clone();
    let serving = tokio::spawn(async move {
        server_table.serve_socket(server).await.unwrap();
    });
    let client_table = fixture.table.clone();
    let calling = tokio::spawn(async move {
        client_table
            .socket_call(client, &timed("write", json!({})), CancellationToken::new())
            .await
    });
    fixture.entered.recv().await.unwrap();
    fixture.proceed.notify_one();
    fixture.owned.recv().await.unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    tokio::task::yield_now().await;
    assert!(!calling.is_finished());
    fixture.acknowledge.notify_one();
    assert_eq!(calling.await.unwrap().value(), Some(&json!(true)));
    serving.await.unwrap();
}

struct Transfer;
impl Operation for Transfer {
    type Request = Input;
    type Response = String;
    fn execute(&self, _: Input, _: RequestContext) -> OperationFuture<'_, Answer<String>> {
        Box::pin(async { Answer::ok("page".into()) })
    }
}
impl StreamOperation for Transfer {
    type Request = Input;
    type Event = u64;
    fn open(
        &self,
        _: Input,
        _: RequestContext,
    ) -> OperationFuture<'_, Result<EventReceiver<u64>, Failure>> {
        Box::pin(async {
            let (send, receive) = mpsc::channel(3);
            send.try_send(Ok(1)).unwrap();
            send.try_send(Err(Failure::new(
                FailureKind::StoppedEarly,
                "source ended early",
            )))
            .unwrap();
            send.try_send(Ok(2)).unwrap();
            Ok(receive)
        })
    }
}
impl TransferOperation for Transfer {
    fn streaming(&self, request: &Input) -> bool {
        request.events
    }
}
fn transfer() -> OperationTable {
    let mut catalog = builder();
    catalog
        .declare_transfer::<Input, String, u64>(
            spec("read"),
            |value| format!("{value}\n"),
            |value| format!("event {value}\n"),
        )
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table.bind_transfer("read", Transfer).unwrap();
    table
}

#[tokio::test]
async fn transfer_has_one_row_with_page_schema_and_separate_event_schema() {
    let table = transfer();
    assert_eq!(table.descriptions().len(), 1);
    assert!(table.descriptions()[0].event_schema.is_some());
    assert!(
        table
            .streaming(&timed("read", json!({"events": true})))
            .unwrap()
    );
    assert!(!table.streaming(&timed("read", json!({}))).unwrap());
    assert_eq!(
        table
            .call(timed("read", json!({})), Via::Mcp, CancellationToken::new())
            .await
            .value(),
        Some(&json!("page"))
    );
    let refused = table
        .call(
            timed("read", json!({"events": true})),
            Via::Mcp,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(error(&refused).kind, FailureKind::BadRequest);
    assert!(error(&refused).message.contains("bounded page"));
}

#[tokio::test]
async fn generated_mcp_rejects_streaming_transfer_and_keeps_bounded_pages() {
    let table = transfer();
    for (events, failed) in [(false, false), (true, true)] {
        let reply = table
            .mcp(
                json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                        "name": "example_query", "arguments": {
                            "arguments": {"events": events}, "request_id": "request"
                        }
                    }
                }),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(reply["result"]["isError"], failed);
        if failed {
            assert!(
                reply["result"]["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("bounded page")
            );
        }
    }
}

#[tokio::test]
async fn late_event_failure_is_terminal_and_cli_never_reports_completion() {
    let table = transfer();
    let mut events = table
        .open_stream(
            timed("read", json!({"events": true})),
            Via::Web,
            Caller::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(events.next().await.unwrap().unwrap(), json!(1));
    assert_eq!(
        events.next().await.unwrap().unwrap_err().kind,
        FailureKind::StoppedEarly
    );
    assert!(events.next().await.is_none());
    let output = table
        .cli(
            &["read".into(), "--events".into(), "true".into()],
            "request",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(output.stdout, "event 1\n");
    assert!(output.stderr.contains("stopped_early"));
    assert_ne!(output.exit_code, 0);
}

type Sender = mpsc::Sender<Result<u64, Failure>>;
struct WaitingStream(std::sync::Mutex<Option<Sender>>);
impl StreamOperation for WaitingStream {
    type Request = Input;
    type Event = u64;
    fn open(
        &self,
        _: Input,
        _: RequestContext,
    ) -> OperationFuture<'_, Result<EventReceiver<u64>, Failure>> {
        Box::pin(async move {
            let (send, receive) = mpsc::channel(1);
            *self.0.lock().unwrap() = Some(send);
            Ok(receive)
        })
    }
}
#[tokio::test(start_paused = true)]
async fn stream_late_receive_uses_the_original_absolute_deadline() {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    let mut declaration = spec("follow");
    declaration.kind = OperationKind::Stream;
    catalog
        .declare_stream::<Input, u64>(declaration, u64::to_string)
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table
        .bind_stream("follow", WaitingStream(std::sync::Mutex::new(None)))
        .unwrap();
    let mut events = table
        .open_stream(
            timed("follow", json!({})),
            Via::Web,
            Caller::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    assert_eq!(
        events.next().await.unwrap().unwrap_err().kind,
        FailureKind::Timeout
    );
    assert!(events.next().await.is_none());
}

struct DirectWrite;
impl Operation for DirectWrite {
    type Request = Input;
    type Response = bool;
    fn has_read_phase(&self, _: &Input) -> bool {
        false
    }
    fn execute(&self, _: Input, context: RequestContext) -> OperationFuture<'_, Answer<bool>> {
        Box::pin(async move { Answer::ok(context.read_deadline().is_none()) })
    }
}
#[tokio::test]
async fn typed_no_read_policy_is_selected_before_starting_a_read_timer() {
    let mut catalog = builder();
    catalog
        .declare::<Input, bool>(spec("direct"), bool::to_string)
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table.bind("direct", DirectWrite).unwrap();
    let mut request = timed("direct", json!({}));
    request.deadline = Deadline::Milliseconds(0);
    assert_eq!(
        table
            .call(request, Via::Cli, CancellationToken::new())
            .await
            .value(),
        Some(&json!(true))
    );
}

struct CallerAccess(Arc<AtomicUsize>);
impl Operation for CallerAccess {
    type Request = Input;
    type Response = bool;
    fn required_access(&self, _: &Input, _: Access, caller: &Caller) -> Access {
        if caller.identity::<bool>() == Some(&true) {
            Access::Write
        } else {
            Access::Read
        }
    }
    fn execute(&self, _: Input, _: RequestContext) -> OperationFuture<'_, Answer<bool>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Answer::ok(true) })
    }
}
#[tokio::test]
async fn typed_access_policy_reads_authenticated_caller_before_execution() {
    let effects = Arc::new(AtomicUsize::new(0));
    let mut catalog = builder();
    let mut declaration = spec("context");
    declaration.access = Access::Write;
    catalog
        .declare::<Input, bool>(declaration, bool::to_string)
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table
        .bind("context", CallerAccess(effects.clone()))
        .unwrap();
    let refused = table
        .call_as(
            timed("context", json!({})),
            Via::Mcp,
            Caller::authenticated("synthetic", true),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(error(&refused).kind, FailureKind::Refused);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(
        table
            .call(
                timed("context", json!({})),
                Via::Cli,
                CancellationToken::new()
            )
            .await
            .value(),
        Some(&json!(true))
    );
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}
