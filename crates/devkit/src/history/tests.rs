use super::*;
use crate::{Diagnostic, Stage, TestOutcome, TestStatus, Tree};

#[test]
fn a_run_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open_at(&dir.path().join("runs.db")).unwrap();
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
    let store = Store::open_at(&dir.path().join("runs.db")).unwrap();
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

/// Adds outcomes to a run through the production encoding, keeping the
/// run's earlier set entries.
pub(super) fn record_tests(store: &Store, run: i64, tests: &[(&str, &str, &str, u64)]) {
    let names = outcomes::Names::load(&store.conn).unwrap();
    let status = |text: &str| match text {
        "passed" => TestStatus::Passed,
        "failed" => TestStatus::Failed,
        "flaky" => TestStatus::Flaky,
        "ignored" => TestStatus::Ignored,
        "filtered" => TestStatus::Filtered,
        "skipped" => TestStatus::Skipped,
        other => panic!("unknown status {other}"),
    };
    let earlier: Vec<TestOutcome> = store
        .conn
        .query_row("SELECT tests FROM test_sets WHERE run = ?1", [run], |row| {
            row.get::<_, Vec<u8>>(0)
        })
        .optional()
        .unwrap()
        .map(|blob| outcomes::decode(&blob).unwrap())
        .unwrap_or_default()
        .into_iter()
        .map(|outcome| TestOutcome {
            binary: names.binary(outcome.binary).to_owned(),
            name: names.name(outcome.name).to_owned(),
            status: status(outcome.status),
            duration: Duration::from_millis(outcome.duration_ms as u64),
            output: None,
        })
        .collect();
    store
        .conn
        .execute("DELETE FROM test_sets WHERE run = ?1", [run])
        .unwrap();
    let mut all = earlier;
    all.extend(tests.iter().map(|(binary, name, text, ms)| TestOutcome {
        binary: (*binary).into(),
        name: (*name).into(),
        status: status(text),
        duration: Duration::from_millis(*ms),
        output: None,
    }));
    outcomes::insert(&store.conn, run, &all).unwrap();
}

#[test]
fn every_outcome_reads_back_with_its_status_and_duration() {
    let store = Store::open_at(Path::new(":memory:")).unwrap();
    let id = store.begin(&Run::unrecorded("demo", "test", &[])).unwrap();
    let mut run = Run::unrecorded("demo", "test", &[]);
    let expected = [
        ("suite", "a", TestStatus::Passed, 3, None),
        ("suite", "b", TestStatus::Ignored, 0, None),
        ("suite", "c", TestStatus::Filtered, 0, None),
        ("suite", "d", TestStatus::Failed, 70_000, Some("panicked")),
        ("suite@stress-1", "a", TestStatus::Passed, 300, None),
        ("other", "a", TestStatus::Flaky, 12, Some("retried")),
        ("other", "e", TestStatus::Skipped, 0, Some("reason")),
    ];
    for (binary, name, status, ms, output) in expected {
        run.tests.push(TestOutcome {
            binary: binary.into(),
            name: name.into(),
            status,
            duration: Duration::from_millis(ms),
            output: output.map(str::to_owned),
        });
    }
    store.finish(id, &run, Duration::from_millis(1)).unwrap();
    let names = outcomes::Names::load(&store.conn).unwrap();
    let mut read: Vec<_> = store
        .outcomes(id)
        .unwrap()
        .into_iter()
        .map(|o| {
            (
                names.binary(o.binary).to_owned(),
                names.name(o.name).to_owned(),
                o.status,
                o.duration_ms,
            )
        })
        .collect();
    read.sort();
    let mut wanted: Vec<_> = expected
        .iter()
        .map(|(b, n, s, ms, _)| ((*b).to_owned(), (*n).to_owned(), s.as_str(), *ms as i64))
        .collect();
    wanted.sort();
    assert_eq!(read, wanted);
    assert_eq!(
        store.test_counts(id).unwrap(),
        [
            ("failed", 1),
            ("filtered", 1),
            ("flaky", 1),
            ("ignored", 1),
            ("passed", 2),
            ("skipped", 1)
        ]
        .map(|(s, n)| (s.to_owned(), n))
    );
    let failures = store.failures(id).unwrap();
    assert_eq!(
        failures
            .iter()
            .map(|f| (f.name.as_str(), f.output.as_deref()))
            .collect::<Vec<_>>(),
        [("a", Some("retried")), ("d", Some("panicked"))]
    );
}

/// A store of a thousand runs of three thousand passing tests each stays
/// under 20 MB, so recording every run needs no deletion.
#[test]
fn a_thousand_full_runs_stay_small() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runs.db");
    let store = Store::open_at(&path).unwrap();
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let names: Vec<(String, String)> = (0..3000)
        .map(|i| {
            (
                format!("synthetic-package-{}::bin/suite", i % 12),
                format!("module_{}::tests::case_number_{i}_keeps_its_result", i / 40),
            )
        })
        .collect();
    for _ in 0..1000 {
        let mut run = Run::unrecorded("demo", "check", &[]);
        let id = store.begin(&run).unwrap();
        run.tests = names
            .iter()
            .map(|(binary, name)| TestOutcome {
                binary: binary.clone(),
                name: name.clone(),
                status: TestStatus::Passed,
                // Mostly fast tests with a long tail, as real suites have.
                duration: Duration::from_millis(match random() % 100 {
                    0 => 1000 + random() % 30_000,
                    1..10 => 100 + random() % 900,
                    _ => random() % 100,
                }),
                output: None,
            })
            .collect();
        store.finish(id, &run, Duration::from_secs(60)).unwrap();
    }
    drop(store);
    let size = std::fs::metadata(&path).unwrap().len();
    assert!(size < 20_000_000, "{size} bytes");
}
