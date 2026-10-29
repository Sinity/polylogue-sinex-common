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
    table.writes.lock().unwrap().len()
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
    let weak = Arc::downgrade(table.writes.lock().unwrap().values().next().unwrap());
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
    assert_eq!(kind(&owner.await), FailureKind::Timeout);
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
