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
impl Operation for DurableWrite {
    type Request = Write;
    type Response = Committed;
    fn spec(&self) -> OperationSpec {
        OperationSpec {
            name: "write".into(),
            command: vec!["write".into()],
            summary: "Synthetic atomic writer".into(),
            reason: "Exercise the product transaction boundary".into(),
            http_route: "/v1/write".into(),
            latency: LatencyClass::Read,
            failures: vec![FailureKind::BadValue],
            write: true,
        }
    }
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
    fn render_text(&self, value: &Committed) -> String {
        value.row.to_string()
    }
}
fn make_table(store: &Arc<Store>, gate: Option<Arc<Gate>>) -> OperationTable {
    let mut table = OperationTable::new("fixture", Deadlines::default());
    table
        .register(DurableWrite {
            store: store.clone(),
            gate,
        })
        .unwrap();
    table
}
fn request(id: &str, value: &str) -> RunnableRequest {
    RunnableRequest {
        operation: "write".into(),
        arguments: json!({"value":value}),
        request_id: id.into(),
        deadline: Deadline::None,
    }
}
fn entries(table: &OperationTable) -> usize {
    table.in_flight.lock().unwrap().len()
}
fn kind(answer: &Answer<Value>) -> FailureKind {
    match answer {
        Answer::Error { error, .. } => error.kind,
        _ => panic!("expected error: {answer:?}"),
    }
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
async fn completed_entries_leave_sequential_retry_and_conflict_to_the_durable_log() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("product.sqlite");
    let store = Store::open(&path);
    let table = make_table(&store, None);
    let first = table
        .call(
            request("same", "one"),
            Origin::Http,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(
        entries(&table),
        0,
        "completed writes must not be retained by common"
    );
    let replay = table
        .call(
            request("same", "one"),
            Origin::Socket,
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
        .call(
            request("same", "other"),
            Origin::Mcp,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&conflict), FailureKind::Conflict);
    assert_eq!(entries(&table), 0);
    assert_eq!(store.count(), 1);
    drop(table);
    drop(store);
    let reopened = Store::open(&path);
    let restarted = make_table(&reopened, None);
    let answer = restarted
        .call(
            request("same", "one"),
            Origin::Cli,
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
async fn sequential_retry_reads_current_confirmed_availability_instead_of_stale_pending_answer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let table = make_table(&store, None);
    let first = table
        .call(
            request("publication", "one"),
            Origin::Http,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(first.value().unwrap()["availability"], "pending");
    let row = first.value().unwrap()["row"].as_i64().unwrap();
    store.confirm(row);
    let next = table
        .call(
            request("publication", "one"),
            Origin::Http,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(next.value().unwrap()["availability"], "confirmed");
    assert_eq!(next.value().unwrap()["row"], row);
    assert_eq!(store.count(), 1);
}

#[tokio::test]
async fn existing_waiters_keep_their_result_after_registry_removal_and_new_admission() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let mut owner = Box::pin(table.call(
        request("overlap", "one"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    let mut waiter = Box::pin(table.call(
        request("overlap", "one"),
        Origin::Socket,
        CancellationToken::new(),
    ));
    pending(waiter.as_mut()).await;
    let weak = Arc::downgrade(table.in_flight.lock().unwrap().values().next().unwrap());
    assert_eq!(store.attempts.load(Ordering::SeqCst), 1);
    gate.open();
    let first = owner.await;
    assert_eq!(entries(&table), 0);
    assert!(weak.upgrade().is_some(), "admitted waiter owns its flight");
    let later = table
        .call(
            request("overlap", "one"),
            Origin::Mcp,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(later.value().unwrap()["replayed"], true);
    assert_eq!(waiter.await, first);
    assert!(weak.upgrade().is_none());
    assert_eq!(store.count(), 1);
}

#[tokio::test]
async fn cancelling_or_dropping_waiters_never_cancels_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let mut owner = Box::pin(table.call(
        request("waiters", "one"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    let token = CancellationToken::new();
    let mut cancelled =
        Box::pin(table.call(request("waiters", "one"), Origin::Socket, token.clone()));
    pending(cancelled.as_mut()).await;
    token.cancel();
    assert_eq!(kind(&cancelled.await), FailureKind::Cancelled);
    let mut dropped = Box::pin(table.call(
        request("waiters", "one"),
        Origin::Mcp,
        CancellationToken::new(),
    ));
    pending(dropped.as_mut()).await;
    drop(dropped);
    assert_eq!(entries(&table), 1);
    assert_eq!(store.active.load(Ordering::SeqCst), 1);
    gate.open();
    assert!(owner.await.value().is_some());
    assert_eq!(entries(&table), 0);
    assert_eq!(store.count(), 1);
}

#[tokio::test]
async fn dropping_owner_releases_registry_work_and_all_waiters() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let mut owner = Box::pin(table.call(
        request("drop", "one"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    let mut waiter = Box::pin(table.call(
        request("drop", "one"),
        Origin::Socket,
        CancellationToken::new(),
    ));
    pending(waiter.as_mut()).await;
    drop(owner);
    assert_eq!(entries(&table), 0, "a dropped owner must retire its flight");
    assert_eq!(store.active.load(Ordering::SeqCst), 0);
    assert_eq!(kind(&waiter.await), FailureKind::Cancelled);
    assert_eq!(store.count(), 0);
    gate.open();
    assert!(
        table
            .call(
                request("drop", "one"),
                Origin::Mcp,
                CancellationToken::new()
            )
            .await
            .value()
            .is_some()
    );
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
    let mut owner = Box::pin(table.call(timed, Origin::Http, CancellationToken::new()));
    pending(owner.as_mut()).await;
    assert_eq!(store.count(), 1);
    tokio::time::advance(Duration::from_millis(2)).await;
    let timed_out = owner.await;
    assert_eq!(kind(&timed_out), FailureKind::Timeout);
    let Answer::Error { error, .. } = &timed_out else {
        unreachable!()
    };
    assert!(error.message.contains("rows scanned unknown"));
    assert!(!error.message.contains("after 0 rows"));
    assert_eq!(entries(&table), 0);
    assert_eq!(store.active.load(Ordering::SeqCst), 0);
    gate.open();
    let retry = table
        .call(
            request("committed", "one"),
            Origin::Mcp,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(retry.value().unwrap()["replayed"], true);
    assert_eq!(store.count(), 1);
}

#[tokio::test]
async fn refusals_are_shared_only_while_in_flight_and_unique_completed_ids_do_not_accumulate() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let table = make_table(&store, None);
    for index in 0..64 {
        let answer = table
            .call(
                request(
                    &index.to_string(),
                    if index % 2 == 0 { "refuse" } else { "one" },
                ),
                Origin::Http,
                CancellationToken::new(),
            )
            .await;
        if index % 2 == 0 {
            assert_eq!(kind(&answer), FailureKind::BadValue);
        }
        assert_eq!(
            entries(&table),
            0,
            "completed/refused identity {index} retained"
        );
    }
    assert_eq!(store.count(), 32);
}

#[tokio::test]
async fn owner_cancellation_retires_the_flight_and_releases_existing_waiters() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let token = CancellationToken::new();
    let mut owner = Box::pin(table.call(request("cancel", "one"), Origin::Http, token.clone()));
    pending(owner.as_mut()).await;
    let mut waiter = Box::pin(table.call(
        request("cancel", "one"),
        Origin::Socket,
        CancellationToken::new(),
    ));
    pending(waiter.as_mut()).await;
    token.cancel();
    let answer = owner.await;
    assert_eq!(kind(&answer), FailureKind::Cancelled);
    assert_eq!(entries(&table), 0);
    assert_eq!(store.active.load(Ordering::SeqCst), 0);
    assert_eq!(waiter.await, answer);
    gate.open();
    assert!(
        table
            .call(
                request("cancel", "one"),
                Origin::Mcp,
                CancellationToken::new()
            )
            .await
            .value()
            .is_some()
    );
    assert_eq!(store.count(), 1);
}

#[tokio::test]
async fn concurrent_refusals_share_one_final_error_but_sequential_refusals_execute_again() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let mut owner = Box::pin(table.call(
        request("refusal", "refuse"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    let mut waiter = Box::pin(table.call(
        request("refusal", "refuse"),
        Origin::Socket,
        CancellationToken::new(),
    ));
    pending(waiter.as_mut()).await;
    gate.open();
    let answer = owner.await;
    assert_eq!(kind(&answer), FailureKind::BadValue);
    assert_eq!(entries(&table), 0);
    assert_eq!(waiter.await, answer);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 1);
    let again = table
        .call(
            request("refusal", "refuse"),
            Origin::Mcp,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&again), FailureKind::BadValue);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(store.count(), 0);
    assert_eq!(entries(&table), 0);
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
                Origin::Mcp,
                Caller::authenticated(scope, ()),
                CancellationToken::new(),
            )
            .await;
        ids.push(answer.value().unwrap()["row"].as_i64().unwrap());
        assert_eq!(entries(&table), 0);
    }
    assert_eq!(ids, [1, 2, 1]);
    assert_eq!(store.count(), 2);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn normalized_failure_is_published_before_waiters_resume() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let mut owner = Box::pin(table.call(
        request("bad-code", "misclassified"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    let mut waiter = Box::pin(table.call(
        request("bad-code", "misclassified"),
        Origin::Mcp,
        CancellationToken::new(),
    ));
    pending(waiter.as_mut()).await;
    let published = table
        .in_flight
        .lock()
        .unwrap()
        .values()
        .next()
        .unwrap()
        .result
        .subscribe();
    gate.open();
    let answer = owner.await;
    assert_eq!(kind(&answer), FailureKind::Io);
    assert_eq!(
        kind(published.borrow().as_deref().unwrap()),
        FailureKind::Io
    );
    assert_eq!(waiter.await, answer);
    assert_eq!(entries(&table), 0);
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
        let mut owner = Box::pin(table.call(input, Origin::Http, CancellationToken::new()));
        pending(owner.as_mut()).await;
        tokio::time::advance(Duration::from_millis(2)).await;
        let Answer::Error { error, next, .. } = owner.await else {
            panic!("expected timeout")
        };
        assert!(error.message.contains("writer"));
        assert!(error.message.contains(diagnostic), "{}", error.message);
        assert_eq!(next[0].request.request_id, "narrower");
        assert_eq!(next[1].request.deadline, Deadline::None);
        assert_eq!(entries(&table), 0);
    }
}

#[tokio::test]
async fn conflicting_in_flight_intent_is_refused_without_starting_a_second_writer() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = make_table(&store, Some(gate.clone()));
    let mut owner = Box::pin(table.call(
        request("intent", "one"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    let mut other = Box::pin(table.call(
        request("intent", "other"),
        Origin::Mcp,
        CancellationToken::new(),
    ));
    let answer = poll_fn(|cx| match other.as_mut().poll(cx) {
        Poll::Ready(answer) => Poll::Ready(answer),
        Poll::Pending => panic!("conflict must not wait for the writer"),
    })
    .await;
    assert_eq!(kind(&answer), FailureKind::Conflict);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(entries(&table), 1);
    gate.open();
    assert!(owner.await.value().is_some());
    let retry = table
        .call(
            request("intent", "other"),
            Origin::Mcp,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(kind(&retry), FailureKind::Conflict);
    assert_eq!(store.attempts.load(Ordering::SeqCst), 2);
    assert_eq!(store.count(), 1);
    assert_eq!(entries(&table), 0);
}

#[tokio::test]
async fn dropped_owner_after_commit_leaves_retry_to_the_durable_result() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::after();
    let table = make_table(&store, Some(gate));
    let mut owner = Box::pin(table.call(
        request("drop-committed", "one"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    assert_eq!(store.count(), 1);
    let mut waiter = Box::pin(table.call(
        request("drop-committed", "one"),
        Origin::Mcp,
        CancellationToken::new(),
    ));
    pending(waiter.as_mut()).await;
    drop(owner);
    assert_eq!(entries(&table), 0);
    assert_eq!(kind(&waiter.await), FailureKind::Cancelled);
    assert_eq!(store.active.load(Ordering::SeqCst), 0);
    let retry = table
        .call(
            request("drop-committed", "one"),
            Origin::Socket,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(retry.value().unwrap()["replayed"], true);
    assert_eq!(store.count(), 1);
}

struct ReentrantWake {
    table: Arc<OperationTable>,
    gate: Arc<Gate>,
    attempted: AtomicBool,
    locked: AtomicBool,
    unexpected_ready: AtomicBool,
    next: Mutex<Option<OperationFuture<'static, Answer<Value>>>>,
}
impl std::task::Wake for ReentrantWake {
    fn wake(self: Arc<Self>) {
        if self.attempted.swap(true, Ordering::SeqCst) {
            return;
        }
        if self.table.in_flight.try_lock().is_err() {
            self.locked.store(true, Ordering::SeqCst);
            return;
        }
        self.gate.before.store(true, Ordering::SeqCst);
        let table = self.table.clone();
        let mut next: OperationFuture<'static, Answer<Value>> = Box::pin(async move {
            table
                .call(
                    request("wake-race", "one"),
                    Origin::Mcp,
                    CancellationToken::new(),
                )
                .await
        });
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        if next.as_mut().poll(&mut context).is_ready() {
            self.unexpected_ready.store(true, Ordering::SeqCst);
        }
        *self.next.lock().unwrap() = Some(next);
    }
}

#[tokio::test]
async fn waking_waiters_can_reenter_without_locking_or_removing_the_replacement_flight() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(&dir.path().join("product.sqlite"));
    let gate = Gate::before();
    let table = Arc::new(make_table(&store, Some(gate.clone())));
    let mut owner = Box::pin(table.call(
        request("wake-race", "one"),
        Origin::Http,
        CancellationToken::new(),
    ));
    pending(owner.as_mut()).await;
    let mut waiter = Box::pin(table.call(
        request("wake-race", "one"),
        Origin::Socket,
        CancellationToken::new(),
    ));
    let wake = Arc::new(ReentrantWake {
        table: table.clone(),
        gate: gate.clone(),
        attempted: AtomicBool::new(false),
        locked: AtomicBool::new(false),
        unexpected_ready: AtomicBool::new(false),
        next: Mutex::new(None),
    });
    let waker = std::task::Waker::from(wake.clone());
    let mut context = std::task::Context::from_waker(&waker);
    assert!(waiter.as_mut().poll(&mut context).is_pending());
    gate.open();
    let first = owner.await;
    assert!(wake.attempted.load(Ordering::SeqCst));
    assert!(!wake.locked.load(Ordering::SeqCst));
    assert!(!wake.unexpected_ready.load(Ordering::SeqCst));
    assert_eq!(
        entries(&table),
        1,
        "old guard must not retire the newly admitted owner"
    );
    assert_eq!(waiter.await, first);
    let next = wake.next.lock().unwrap().take().unwrap();
    gate.open();
    let retry = next.await;
    assert_eq!(retry.value().unwrap()["replayed"], true);
    assert_eq!(entries(&table), 0);
    assert_eq!(store.count(), 1);
}
