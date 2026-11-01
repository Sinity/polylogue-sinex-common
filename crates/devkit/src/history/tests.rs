use super::*;
use crate::{Diagnostic, Stage, TestOutcome, TestStatus, Tree};

#[test]
fn a_run_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_at(&dir.path().join("history.db")).unwrap();
    let mut run = Run::unrecorded("demo", "check", &[]);
    run.tree = Tree {
        head: Some("abc".into()),
        dirty: None,
        git_tree: Some("synthetic-tree".into()),
        observation_error: None,
    };
    let id = store.begin(&run).unwrap();
    run.stages.push(Stage {
        name: "test".into(),
        duration: Duration::from_millis(5),
        ok: false,
    });
    run.tests.push(TestOutcome {
        binary: "demo".into(),
        name: "fails".into(),
        status: TestStatus::Failed,
        duration: Duration::from_millis(7),
        output: Some("panicked at x".into()),
    });
    run.tests.push(TestOutcome {
        binary: "demo".into(),
        name: "passes".into(),
        status: TestStatus::Passed,
        duration: Duration::from_millis(1),
        output: None,
    });
    let warning = Diagnostic {
        package: "demo".into(),
        level: "warning".into(),
        code: None,
        file: Some("src/lib.rs".into()),
        line: Some(4),
        text: "warning: unused".into(),
    };
    run.diagnostics.push(warning.clone());
    run.diagnostics.push(warning);
    run.failure = Some("test: failed".into());
    store.finish(id, &run, Duration::from_millis(9)).unwrap();

    let row = store.run(id).unwrap().unwrap();
    assert_eq!((row.status.as_str(), row.duration_ms), ("failed", Some(9)));
    let failures = store.failures(id).unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].output.as_deref(), Some("panicked at x"));
    assert_eq!(store.diagnostics(id, "warning").unwrap().len(), 2);
    assert!(store.diagnostics(id, "error").unwrap().is_empty());
    let texts: i64 = store
        .conn
        .query_row("SELECT count(*) FROM diagnostic_texts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(texts, 1);
}

#[test]
fn a_running_run_reads_interrupted_once_its_process_or_boot_is_gone() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_at(&dir.path().join("history.db")).unwrap();
    let id = store.begin(&Run::unrecorded("demo", "test", &[])).unwrap();
    let status = |store: &Store| store.run(id).unwrap().unwrap().status;
    assert_eq!(status(&store), "running");
    // The same pid, but a process that started at another time.
    store
        .conn
        .execute(
            "UPDATE runs SET process_start = process_start + 1 WHERE id = ?1",
            [id],
        )
        .unwrap();
    assert_eq!(status(&store), "interrupted");
    store
        .conn
        .execute(
            "UPDATE runs SET process_start = process_start - 1, boot_id = 'an earlier boot' WHERE id = ?1",
            [id],
        )
        .unwrap();
    assert_eq!(status(&store), "interrupted");
    // Rows recorded before process identity was: the pid alone decides.
    store
        .conn
        .execute(
            "UPDATE runs SET boot_id = NULL, process_start = NULL WHERE id = ?1",
            [id],
        )
        .unwrap();
    assert_eq!(status(&store), "running");
    store
        .conn
        .execute("UPDATE runs SET pid = -1 WHERE id = ?1", [id])
        .unwrap();
    assert_eq!(status(&store), "interrupted");
}

#[test]
fn a_store_without_process_identity_columns_gains_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let earlier = Connection::open(&path).unwrap();
    earlier
        .execute_batch(&SCHEMA.replace(
            "observation_error TEXT,\n    -- the kernel's boot ID and the process's start time in clock ticks after\n    -- boot: together with pid they identify the running process\n    boot_id TEXT,\n    process_start INTEGER\n",
            "observation_error TEXT\n",
        ))
        .unwrap();
    let columns = |conn: &Connection| -> i64 {
        conn.query_row(
            "SELECT count(*) FROM pragma_table_info('runs') WHERE name IN ('boot_id', 'process_start')",
            [],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(columns(&earlier), 0);
    drop(earlier);
    let store = Store::open_at(&path).unwrap();
    assert_eq!(columns(&store.conn), 2);
    drop(store);
    // Opening again leaves them as they are.
    assert_eq!(columns(&Store::open_at(&path).unwrap().conn), 2);
}
