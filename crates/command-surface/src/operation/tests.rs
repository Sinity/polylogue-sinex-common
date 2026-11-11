use super::*;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::json;
use std::{
    future::poll_fn,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    task::Poll,
};
use tokio::sync::Semaphore;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct Write {
    value: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Committed {
    row: i64,
    availability: String,
    replayed: bool,
}
struct Store {
    db: Mutex<Connection>,
    attempts: AtomicUsize,
    active: Arc<AtomicUsize>,
}
impl Store {
    fn open(path: &Path) -> Arc<Self> {
        let db = Connection::open(path).unwrap();
        db.execute_batch("CREATE TABLE IF NOT EXISTS records(id INTEGER PRIMARY KEY, value TEXT NOT NULL, availability TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS requests(scope TEXT NOT NULL, request_id TEXT NOT NULL, intent TEXT NOT NULL, row_id INTEGER NOT NULL, PRIMARY KEY(scope,request_id));").unwrap();
        Arc::new(Self {
            db: Mutex::new(db),
            attempts: AtomicUsize::new(0),
            active: Arc::new(AtomicUsize::new(0)),
        })
    }
    // This is the product fixture's atomic request log, not state in OperationTable.
    fn commit(&self, scope: &str, id: &str, value: &str) -> Result<Committed, Failure> {
        let mut db = self.db.lock().unwrap();
        let tx = db.transaction().unwrap();
        let previous: Option<(String, i64)> = tx
            .query_row(
                "SELECT intent,row_id FROM requests WHERE scope=?1 AND request_id=?2",
                params![scope, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .unwrap();
        let (row, replayed) = match previous {
            Some((intent, row)) if intent == value => (row, true),
            Some(_) => {
                return Err(Failure::new(
                    FailureKind::Conflict,
                    "durable request intent changed",
                ));
            }
            None => {
                tx.execute(
                    "INSERT INTO records(value,availability) VALUES(?1,'pending')",
                    [value],
                )
                .unwrap();
                let row = tx.last_insert_rowid();
                tx.execute(
                    "INSERT INTO requests(scope,request_id,intent,row_id) VALUES(?1,?2,?3,?4)",
                    params![scope, id, value, row],
                )
                .unwrap();
                (row, false)
            }
        };
        let availability = tx
            .query_row("SELECT availability FROM records WHERE id=?1", [row], |r| {
                r.get(0)
            })
            .unwrap();
        tx.commit().unwrap();
        Ok(Committed {
            row,
            availability,
            replayed,
        })
    }
    fn count(&self) -> i64 {
        self.db
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM records", [], |r| r.get(0))
            .unwrap()
    }
    fn confirm(&self, row: i64) {
        self.db
            .lock()
            .unwrap()
            .execute(
                "UPDATE records SET availability='confirmed' WHERE id=?1",
                [row],
            )
            .unwrap();
    }
}
struct Active(Arc<AtomicUsize>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
struct Gate {
    before: AtomicBool,
    after: AtomicBool,
    release: Semaphore,
}
impl Gate {
    fn before() -> Arc<Self> {
        Arc::new(Self {
            before: AtomicBool::new(true),
            after: AtomicBool::new(false),
            release: Semaphore::new(0),
        })
    }
    fn after() -> Arc<Self> {
        Arc::new(Self {
            before: AtomicBool::new(false),
            after: AtomicBool::new(true),
            release: Semaphore::new(0),
        })
    }
    fn open(&self) {
        self.before.store(false, Ordering::SeqCst);
        self.after.store(false, Ordering::SeqCst);
        self.release.add_permits(1);
    }
}
struct DurableWrite {
    store: Arc<Store>,
    gate: Option<Arc<Gate>>,
}
fn durable_write_spec() -> OperationSpec {
    OperationSpec {
        failures: vec![FailureKind::BadValue],
        access: Access::Write,
        ..OperationSpec::request(
            "write",
            &["write"],
            "write",
            "Synthetic atomic writer",
            "Exercise the product transaction boundary",
        )
    }
}
fn durable_write_render_text(value: &Committed, _style: TextStyle) -> String {
    value.row.to_string()
}
impl Operation for DurableWrite {
    type Request = Write;
    type Response = Committed;

    fn execute(
        &self,
        request: Write,
        context: RequestContext,
    ) -> OperationFuture<'_, Answer<Committed>> {
        Box::pin(async move {
            self.store.attempts.fetch_add(1, Ordering::SeqCst);
            self.store.active.fetch_add(1, Ordering::SeqCst);
            let _active = Active(self.store.active.clone());
            if let Some(gate) = &self.gate
                && gate.before.load(Ordering::SeqCst)
            {
                gate.release.acquire().await.unwrap().forget();
            }
            if request.value == "unknown-progress" || request.value == "zero-progress" {
                let narrower = Some(super::tests::request("narrower", "one"));
                if request.value == "unknown-progress" {
                    context.progress_unknown("writer", narrower).unwrap();
                } else {
                    context.progress("writer", 0, narrower).unwrap();
                }
            }
            if request.value == "misclassified" {
                let mut error = Failure::new(FailureKind::BadValue, "synthetic unregistered code");
                error.code = Some("not_registered".into());
                return Answer::error(error);
            }
            if request.value == "refuse" {
                return Answer::error(Failure::new(FailureKind::BadValue, "synthetic refusal"));
            }
            let committed = match self.store.commit(
                context.caller().scope(),
                &context.request_id,
                &request.value,
            ) {
                Ok(value) => value,
                Err(error) => return Answer::error(error),
            };
            if !committed.replayed
                && let Some(gate) = &self.gate
                && gate.after.load(Ordering::SeqCst)
            {
                gate.release.acquire().await.unwrap().forget();
            }
            Answer::ok(committed)
        })
    }
}
fn make_table(store: &Arc<Store>, gate: Option<Arc<Gate>>) -> OperationTable {
    let mut declarations = OperationDeclarations::builder("fixture", Deadlines::default());
    declarations
        .declare_tool(ToolFamily {
            name: "write".into(),
            description: "Write".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    declarations
        .declare::<Write, Committed>(durable_write_spec(), durable_write_render_text)
        .unwrap();
    let mut table = OperationTable::new(declarations.finish());
    table
        .bind(
            "write",
            DurableWrite {
                store: store.clone(),
                gate,
            },
        )
        .unwrap();
    table
}
fn request(id: &str, value: &str) -> RunnableRequest {
    RunnableRequest {
        operation: "write".into(),
        arguments: json!({"value":value}),
        request_id: id.into(),
        deadline: Deadline::None,
        budget_tokens: crate::TokenBudget::Default,
        cursor: None,
        fresh: false,
        format: crate::Format::Text,
    }
}
/// A caller allowed to write, as a product's server grants one.
fn writer() -> Caller {
    Caller::default().with_access(Access::Write)
}
fn kind(answer: &Answer<Value>) -> FailureKind {
    answer
        .failure()
        .unwrap_or_else(|| panic!("expected error: {answer:?}"))
        .kind
}
async fn pending<F: Future>(future: Pin<&mut F>) {
    let mut future = future;
    poll_fn(|cx| match future.as_mut().poll(cx) {
        Poll::Pending => Poll::Ready(()),
        Poll::Ready(_) => panic!("expected controlled pending operation"),
    })
    .await;
}

#[tokio::test]
async fn retry_applies_once_through_the_durable_log_across_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("product.sqlite");
    let store = Store::open(&path);
    let table = make_table(&store, None);
    let first = table
        .call_as(
            request("same", "one"),
            Via::Web,
            writer(),
            CancellationToken::new(),
        )
        .await;
    let replay = table
        .call_as(
            request("same", "one"),
            Via::Cli,
            writer(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(
        first.value().unwrap()["row"],
        replay.value().unwrap()["row"]
    );
    assert_eq!(replay.value().unwrap()["replayed"], true);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 2);
    let conflict = table
        .call_as(
            request("same", "other"),
            Via::Mcp,
            writer(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&conflict), FailureKind::Conflict);
    assert_eq!(store.count(), 1);
    drop(table);
    drop(store);
    let reopened = Store::open(&path);
    let restarted = make_table(&reopened, None);
    let answer = restarted
        .call_as(
            request("same", "one"),
            Via::Cli,
            writer(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(
        answer.value().unwrap()["row"],
        first.value().unwrap()["row"]
    );
    assert_eq!(answer.value().unwrap()["replayed"], true);
    assert_eq!(reopened.count(), 1);
}

#[tokio::test]
async fn retry_applies_once_when_duplicates_arrive_together() {
    // Without coalescing in the table, concurrent duplicates both reach the
    // writer, whose transaction admits the first and replays it for the
    // second.
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let mut first = Box::pin(table.call_as(
        request("together", "one"),
        Via::Web,
        writer(),
        CancellationToken::new(),
    ));
    pending(first.as_mut()).await;
    let mut second = Box::pin(table.call_as(
        request("together", "one"),
        Via::Mcp,
        writer(),
        CancellationToken::new(),
    ));
    pending(second.as_mut()).await;
    gate.open();
    gate.open();
    let (first, second) = tokio::join!(first, second);
    assert_eq!(
        first.value().unwrap()["row"],
        second.value().unwrap()["row"]
    );
    assert_eq!(store.count(), 1);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn sequential_retry_reads_current_confirmed_availability_instead_of_stale_pending_answer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let table = make_table(&store, None);
    let first = table
        .call_as(
            request("publication", "one"),
            Via::Web,
            writer(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(first.value().unwrap()["availability"], "pending");
    let row = first.value().unwrap()["row"].as_i64().unwrap();
    store.confirm(row);
    let next = table
        .call_as(
            request("publication", "one"),
            Via::Web,
            writer(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(next.value().unwrap()["availability"], "confirmed");
    assert_eq!(next.value().unwrap()["row"], row);
    assert_eq!(store.count(), 1);
}

#[tokio::test(start_paused = true)]
async fn timeout_after_commit_replays_the_atomic_result_without_applying_twice() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::after();
    let table = make_table(&store, Some(gate.clone()));
    let mut timed = request("committed", "one");
    timed.deadline = Deadline::Milliseconds(1);
    let mut owner = Box::pin(table.call_as(timed, Via::Web, writer(), CancellationToken::new()));
    pending(owner.as_mut()).await;
    assert_eq!(store.count(), 1);
    tokio::time::advance(Duration::from_millis(2)).await;
    let timed_out = owner.await;
    assert_eq!(kind(&timed_out), FailureKind::Timeout);
    let error = timed_out.failure().unwrap();
    assert_eq!(error.code.as_deref(), Some(crate::DEADLINE_EXCEEDED));
    assert!(error.message.contains("rows scanned unknown"));
    assert!(!error.message.contains("after 0 rows"));
    assert_eq!(store.active.load(Ordering::SeqCst), 0);
    gate.open();
    let retry = table
        .call_as(
            request("committed", "one"),
            Via::Mcp,
            writer(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(retry.value().unwrap()["replayed"], true);
    assert_eq!(store.count(), 1);
}

#[tokio::test]
async fn authenticated_scopes_namespace_the_atomic_product_request_log() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let table = make_table(&store, None);
    let mut ids = Vec::new();
    for scope in ["person-a", "person-b", "person-a"] {
        let answer = table
            .call_as(
                request("scoped", "one"),
                Via::Mcp,
                Caller::authenticated(scope, ()).with_access(Access::Write),
                CancellationToken::new(),
            )
            .await;
        ids.push(answer.value().unwrap()["row"].as_i64().unwrap());
    }
    assert_eq!(ids, [1, 2, 1]);
    assert_eq!(store.count(), 2);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn access_checked_before_the_writer_runs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let table = make_table(&store, None);
    let refused = table
        .call_as(
            request("limited", "one"),
            Via::Web,
            Caller::authenticated("reader", ()).with_access(Access::Read),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&refused), FailureKind::Refused);
    assert!(refused.failure().unwrap().message.contains("write access"));
    assert_eq!(store.attempts.load(Ordering::SeqCst), 0);
    // A caller the server granted nothing more reads only.
    let ungranted = table
        .call(
            request("limited", "one"),
            Via::Cli,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&ungranted), FailureKind::Refused);
    let authenticated = table
        .call_as(
            request("limited", "one"),
            Via::Web,
            Caller::authenticated("someone", ()),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&authenticated), FailureKind::Refused);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 0);
    let allowed = table
        .call_as(
            request("limited", "one"),
            Via::Web,
            Caller::authenticated("writer", ()).with_access(Access::Write),
            CancellationToken::new(),
        )
        .await;
    assert!(allowed.value().is_some());
}

#[tokio::test]
async fn misclassified_failure_codes_are_reported_as_product_bugs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let table = make_table(&store, None);
    let answer = table
        .call_as(
            request("code", "misclassified"),
            Via::Cli,
            writer(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&answer), FailureKind::Io);
}

#[tokio::test(start_paused = true)]
async fn timeout_distinguishes_unmeasured_work_from_measured_zero_and_keeps_followups() {
    for (value, diagnostic) in [
        ("unknown-progress", "rows scanned unknown"),
        ("zero-progress", "after 0 rows scanned"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("product.sqlite"));
        let gate = Gate::after();
        let table = make_table(&store, Some(gate));
        let mut input = request("progress", value);
        input.deadline = Deadline::Milliseconds(1);
        let mut owner =
            Box::pin(table.call_as(input, Via::Web, writer(), CancellationToken::new()));
        pending(owner.as_mut()).await;
        tokio::time::advance(Duration::from_millis(2)).await;
        let answer = owner.await;
        let error = answer.failure().expect("expected timeout");
        assert!(error.message.contains("writer"));
        assert!(error.message.contains(diagnostic), "{}", error.message);
        let next = &answer.envelope.next;
        assert_eq!(next[0].request.request_id, "narrower");
        assert_eq!(next[1].request.deadline, Deadline::None);
    }
}

#[tokio::test]
async fn mixed_rows_authorize_the_typed_request_before_any_write() {
    #[derive(Serialize, Deserialize, schemars::JsonSchema)]
    struct Arguments {
        value: Option<String>,
    }
    struct Mixed {
        store: Arc<Store>,
    }
    fn mixed_spec() -> OperationSpec {
        OperationSpec {
            access: Access::Write,
            ..OperationSpec::request(
                "setting",
                &["setting"],
                "settings",
                "Read or write",
                "Reads or writes a setting",
            )
        }
    }
    fn mixed_render_text(value: &Value, _style: TextStyle) -> String {
        value.to_string()
    }
    impl Operation for Mixed {
        type Request = Arguments;
        type Response = Value;

        fn required_access(
            &self,
            request: &Arguments,
            _declared: Access,
            _caller: &Caller,
        ) -> Access {
            if request.value.is_some() {
                Access::Write
            } else {
                Access::Read
            }
        }
        fn execute(
            &self,
            request: Arguments,
            context: RequestContext,
        ) -> OperationFuture<'_, Answer<Value>> {
            Box::pin(async move {
                match request.value {
                    Some(value) => match self.store.commit(
                        context.caller().scope(),
                        &context.request_id,
                        &value,
                    ) {
                        Ok(row) => Answer::ok(json!({"row":row.row})),
                        Err(error) => Answer::error(error),
                    },
                    None => Answer::ok(json!({"records":self.store.count()})),
                }
            })
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("records.db"));
    let mut declarations = OperationDeclarations::builder("example", Deadlines::default());
    declarations
        .declare_tool(crate::ToolFamily {
            name: "settings".into(),
            description: "settings".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    declarations
        .declare::<Arguments, Value>(mixed_spec(), mixed_render_text)
        .unwrap();
    let mut table = OperationTable::new(declarations.finish());
    table
        .bind(
            "setting",
            Mixed {
                store: store.clone(),
            },
        )
        .unwrap();
    let read = table
        .call_as(
            RunnableRequest::new("setting", json!({}), "declarations"),
            Via::Cli,
            Caller::default(),
            Default::default(),
        )
        .await;
    assert_eq!(read.value(), Some(&json!({"records":0})));
    let refused = table
        .call_as(
            RunnableRequest::new("setting", json!({"value":"new"}), "blocked"),
            Via::Cli,
            Caller::default(),
            Default::default(),
        )
        .await;
    assert!(
        matches!(refused.outcome, Outcome::Error(ref error) if error.kind == FailureKind::Refused)
    );
    assert_eq!(store.count(), 0);
    let written = table
        .call_as(
            RunnableRequest::new("setting", json!({"value":"new"}), "allowed"),
            Via::Cli,
            Caller::default().with_access(Access::Write),
            Default::default(),
        )
        .await;
    assert!(written.value().is_some(), "{written:?}");
    assert_eq!(store.count(), 1);
}
