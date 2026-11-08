use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Input {
    events: bool,
}
type Sender = mpsc::Sender<Result<u64, Failure>>;
struct Probe {
    contexts: mpsc::UnboundedSender<RequestContext>,
    sender: Arc<Mutex<Option<Sender>>>,
}
impl Operation for Probe {
    type Request = Input;
    type Response = bool;
    fn default_read_deadline(
        &self,
        request: &Input,
        declared: Option<Duration>,
    ) -> Option<Duration> {
        if request.events { None } else { declared }
    }
    fn execute(&self, _: Input, context: RequestContext) -> OperationFuture<'_, Answer<bool>> {
        Box::pin(async move {
            self.contexts.send(context).unwrap();
            std::future::pending().await
        })
    }
}
impl StreamOperation for Probe {
    type Request = Input;
    type Event = u64;
    fn open(
        &self,
        _: Input,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<EventReceiver<u64>, Failure>> {
        Box::pin(async move {
            self.contexts.send(context).unwrap();
            let (send, receive) = mpsc::channel(1);
            *self.sender.lock().unwrap() = Some(send);
            Ok(receive)
        })
    }
}
impl TransferOperation for Probe {
    fn streaming(&self, request: &Input) -> bool {
        request.events
    }
}
struct Fixture {
    table: Arc<OperationTable>,
    contexts: mpsc::UnboundedReceiver<RequestContext>,
    sender: Arc<Mutex<Option<Sender>>>,
}
fn fixture(ordinary_stream: bool) -> Fixture {
    let mut deadlines = Deadlines::default();
    deadlines.set(LatencyClass::Read, Some(Duration::from_millis(250)));
    let mut catalog = OperationCatalog::builder("example", deadlines);
    catalog
        .declare_tool(ToolFamily {
            name: "query".into(),
            description: "Read values".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    let name = if ordinary_stream { "follow" } else { "read" };
    let mut spec = OperationSpec::request(
        name,
        &[name],
        "query",
        "Read values",
        "Exercise typed default deadlines",
    );
    spec.latency = LatencyClass::Read;
    if ordinary_stream {
        spec.kind = OperationKind::Stream;
        catalog
            .declare_stream::<Input, u64>(spec, |event, _| event.to_string())
            .unwrap();
    } else {
        catalog
            .declare_transfer::<Input, bool, u64>(
                spec,
                |value, _| value.to_string(),
                |event, _| event.to_string(),
            )
            .unwrap();
    }
    let mut table = OperationTable::new(catalog.finish());
    let (contexts, receive) = mpsc::unbounded_channel();
    let sender = Arc::new(Mutex::new(None));
    let probe = Probe {
        contexts,
        sender: sender.clone(),
    };
    if ordinary_stream {
        table.bind_stream(name, probe).unwrap();
    } else {
        table.bind_transfer(name, probe).unwrap();
    }
    Fixture {
        table: Arc::new(table),
        contexts: receive,
        sender,
    }
}
fn request(name: &str, events: bool, deadline: Deadline) -> RunnableRequest {
    let mut request = RunnableRequest::new(name, json!({"events":events}), "original");
    request.deadline = deadline;
    request
}
fn failure(answer: &Answer<Value>) -> &Failure {
    answer.failure().expect("timeout or cancellation")
}

#[tokio::test(start_paused = true)]
async fn finite_page_keeps_declared_default_and_timeout_names_the_operation() {
    let mut fixture = fixture(false);
    let table = fixture.table.clone();
    let original = request("read", false, Deadline::Default);
    let admitted = original.clone();
    let started = tokio::time::Instant::now();
    let running = tokio::spawn(async move {
        table
            .call(admitted, Via::Cli, CancellationToken::new())
            .await
    });
    let context = fixture.contexts.recv().await.unwrap();
    assert_eq!(context.request(), &original);
    assert_eq!(
        context.read_deadline(),
        Some(started + Duration::from_millis(250))
    );
    tokio::time::advance(Duration::from_millis(250)).await;
    let answer = running.await.unwrap();
    assert_eq!(failure(&answer).kind, FailureKind::Timeout);
    assert!(failure(&answer).retryable);
    assert!(
        failure(&answer)
            .message
            .contains("during admission for read")
    );
    assert!(failure(&answer).message.contains("rows scanned unknown"));
    assert_eq!(
        answer.envelope.next.last().unwrap().request.deadline,
        Deadline::None
    );
}
#[tokio::test(start_paused = true)]
async fn typed_stream_default_is_unbounded_but_parent_cancellation_still_stops_it() {
    let mut fixture = fixture(false);
    let original = request("read", true, Deadline::Default);
    let parent = CancellationToken::new();
    let mut events = fixture
        .table
        .open_stream(
            original.clone(),
            Via::Cli,
            Caller::default(),
            parent.clone(),
        )
        .await
        .unwrap();
    let context = fixture.contexts.recv().await.unwrap();
    assert_eq!(context.request(), &original);
    assert_eq!(context.read_deadline(), None);
    tokio::time::advance(Duration::from_secs(1)).await;
    let sender = fixture.sender.lock().unwrap().as_ref().unwrap().clone();
    sender.send(Ok(7)).await.unwrap();
    assert_eq!(events.next().await.unwrap().unwrap(), json!(7));
    parent.cancel();
    assert_eq!(
        events.next().await.unwrap().unwrap_err().kind,
        FailureKind::Cancelled
    );
    assert!(events.next().await.is_none());
}
#[tokio::test(start_paused = true)]
async fn typed_stream_still_honors_explicit_zero_and_absolute_deadlines() {
    let mut fixture = fixture(false);
    let failed = fixture
        .table
        .open_stream(
            request("read", true, Deadline::Milliseconds(0)),
            Via::Cli,
            Caller::default(),
            CancellationToken::new(),
        )
        .await;
    let error = match failed {
        Ok(_) => panic!("expired stream must not open"),
        Err(error) => error,
    };
    assert_eq!(error.kind, FailureKind::Timeout);
    assert!(error.message.contains("admission for read"));
    assert!(fixture.contexts.try_recv().is_err());
    let started = tokio::time::Instant::now();
    let mut events = fixture
        .table
        .open_stream(
            request("read", true, Deadline::Milliseconds(50)),
            Via::Cli,
            Caller::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let context = fixture.contexts.recv().await.unwrap();
    assert_eq!(
        context.read_deadline(),
        Some(started + Duration::from_millis(50))
    );
    tokio::time::advance(Duration::from_millis(50)).await;
    let error = events.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind, FailureKind::Timeout);
    assert!(error.message.contains("admission for read"));
    assert!(events.next().await.is_none());
}
#[tokio::test(start_paused = true)]
async fn ordinary_stream_keeps_its_declared_default_and_explicit_null_still_lifts_pages() {
    let mut ordinary = fixture(true);
    let started = tokio::time::Instant::now();
    let mut events = ordinary
        .table
        .open_stream(
            request("follow", true, Deadline::Default),
            Via::Cli,
            Caller::default(),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let context = ordinary.contexts.recv().await.unwrap();
    assert_eq!(
        context.read_deadline(),
        Some(started + Duration::from_millis(250))
    );
    tokio::time::advance(Duration::from_millis(250)).await;
    let error = events.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind, FailureKind::Timeout);
    assert!(error.message.contains("admission for follow"));
    let mut page = fixture(false);
    let table = page.table.clone();
    let parent = CancellationToken::new();
    let cancellation = parent.clone();
    let running = tokio::spawn(async move {
        table
            .call(
                request("read", false, Deadline::None),
                Via::Cli,
                cancellation,
            )
            .await
    });
    let context = page.contexts.recv().await.unwrap();
    assert_eq!(context.read_deadline(), None);
    parent.cancel();
    assert_eq!(
        failure(&running.await.unwrap()).kind,
        FailureKind::Cancelled
    );
}
