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
