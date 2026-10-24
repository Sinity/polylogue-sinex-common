use super::*;
use rusqlite::Connection;

fn store() -> Store {
    Store::open_at(Path::new(":memory:")).unwrap()
}

fn run(store: &Store, id: i64, project: &str, checkout: &str, tree: Option<&str>, status: &str) {
    store.conn.execute(
        "INSERT INTO runs (id, project, checkout, command, args, head, git_tree, pid, started_ms, status, duration_ms)
         VALUES (?1, ?2, ?3, 'check', '[]', ?4, ?5, 0, ?1, ?6, ?1 * 100)",
        params![id, project, checkout, format!("revision-{id}"), tree, status],
    ).unwrap();
}

fn test(store: &Store, id: i64, binary: &str, name: &str, status: &str) {
    store
        .conn
        .execute(
            "INSERT INTO tests (run,binary,name,status,duration_ms) VALUES (?1,?2,?3,?4,1)",
            params![id, binary, name, status],
        )
        .unwrap();
}

fn selected() -> Vec<String> {
    vec!["/repo/a".into(), "/repo/b".into()]
}

#[test]
fn equal_git_trees_across_commits_preserve_observed_statuses_and_population() {
    let s = store();
    for (id, project, checkout, tree, status) in [
        (1, "demo", "/repo/a", Some("same-tree"), "failed"),
        (2, "demo", "/repo/b", Some("same-tree"), "passed"),
        (3, "other", "/repo/b", Some("same-tree"), "passed"),
        (4, "demo", "/unselected", Some("same-tree"), "passed"),
        (5, "demo", "/repo/a", None, "passed"),
        (6, "demo", "/repo/b", Some("same-tree"), "passed"),
    ] {
        run(&s, id, project, checkout, tree, status);
    }
    for (id, status) in [
        (1, "failed"),
        (2, "passed"),
        (3, "passed"),
        (4, "passed"),
        (5, "passed"),
        (6, "filtered"),
    ] {
        test(&s, id, "suite", "sometimes", status);
    }
    test(&s, 1, "different-suite", "sometimes", "failed");
    test(&s, 2, "different-suite", "another-name", "passed");
    test(&s, 1, "suite", "never-ran", "ignored");
    test(&s, 2, "suite", "never-ran", "filtered");
    let out = s.intermittent("demo", &selected(), None).unwrap();
    assert_eq!(out["tests"].as_array().unwrap().len(), 1);
    assert_eq!(out["tests"][0]["observations"].as_array().unwrap().len(), 3);
    assert_eq!(out["tests"][0]["recorded_tree"]["git_tree"], "same-tree");
    assert_eq!(out["unknown_tree_observations"], 1);
    let one = s.intermittent("demo", &["/repo/a".into()], None).unwrap();
    assert!(one["tests"].as_array().unwrap().is_empty());
}

#[test]
fn dirty_hashes_and_unknown_observations_do_not_supply_equal_tree_evidence() {
    let s = store();
    for id in 1..=4 {
        run(&s, id, "demo", "/repo/a", Some("tree"), "passed");
    }
    test(&s, 1, "suite", "test", "failed");
    for id in 2..=4 {
        test(&s, id, "suite", "test", "passed");
    }
    s.conn
        .execute("UPDATE runs SET dirty='different' WHERE id=2", [])
        .unwrap();
    s.conn
        .execute(
            "UPDATE runs SET observation_error='unreadable' WHERE id=3",
            [],
        )
        .unwrap();
    s.conn
        .execute("UPDATE runs SET git_tree=NULL WHERE id=4", [])
        .unwrap();
    let out = s.intermittent("demo", &selected(), None).unwrap();
    assert!(out["tests"].as_array().unwrap().is_empty());
    assert_eq!(out["unknown_tree_observations"], 2);
    // Duplicate pass/fail rows from one run plus another skipped run do not
    // establish contradictory outcomes across distinct runs.
    test(&s, 1, "suite", "duplicate", "passed");
    test(&s, 1, "suite", "duplicate", "failed");
    test(&s, 2, "suite", "duplicate", "skipped");
    assert!(
        s.intermittent("demo", &selected(), None).unwrap()["tests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn first_failure_is_recorded_chronology_with_dirty_unknown_and_retry_attribution() {
    let s = store();
    for id in 1..=5 {
        run(&s, id, "demo", "/repo/a", Some("tree"), "failed");
    }
    for (id, status) in [
        (1, "ignored"),
        (2, "passed"),
        (3, "flaky"),
        (4, "failed"),
        (5, "filtered"),
    ] {
        test(&s, id, "suite", "test", status);
    }
    s.conn
        .execute("UPDATE runs SET dirty='patch' WHERE id=3", [])
        .unwrap();
    let out = s
        .first_failing("demo", &selected(), "suite", "test")
        .unwrap();
    assert_eq!(out["first_recorded_failure"]["run"], 3);
    assert_eq!(
        out["first_recorded_failure"]["revision_attribution"],
        "uncommitted changes"
    );
    assert_eq!(out["observations"].as_array().unwrap().len(), 5);
    s.conn
        .execute(
            "UPDATE runs SET observation_error='git unavailable' WHERE id=3",
            [],
        )
        .unwrap();
    assert_eq!(
        s.first_failing("demo", &selected(), "suite", "test")
            .unwrap()["first_recorded_failure"]["revision_attribution"],
        "unknown"
    );
    assert!(
        s.first_failing("demo", &selected(), "suite", "missing")
            .unwrap()["first_recorded_failure"]
            .is_null()
    );
}

#[test]
fn estimates_keep_selection_stage_failure_and_missing_samples_visible() {
    let s = store();
    for (id, status) in [
        (1, "passed"),
        (2, "failed"),
        (3, "running"),
        (4, "passed"),
        (5, "passed"),
    ] {
        run(&s, id, "demo", "/repo/a", Some("tree"), status);
    }
    s.conn
        .execute("UPDATE runs SET args='[\"-p\",\"one\"]' WHERE id=5", [])
        .unwrap();
    s.conn.execute("INSERT INTO stages VALUES (1,0,'test build',70,1),(1,1,'test',30,1),(2,0,'test',10,0),(4,0,'fmt',5,1)",[]).unwrap();
    let total = s
        .durations("demo", &selected(), "check", &[], None)
        .unwrap();
    assert_eq!(total["sample_count"], 2);
    assert_eq!(total["median_ms"], 250.0);
    assert_eq!(total["selected_run_count"], 4);
    assert_eq!(total["observations"][1]["excluded"], "run not passed");
    let stage = s
        .durations("demo", &selected(), "check", &[], Some("test"))
        .unwrap();
    assert_eq!(stage["sample_count"], 1);
    assert_eq!(stage["median_ms"], 30.0);
    assert_eq!(stage["observations"][3]["excluded"], "stage not recorded");
    let build = s
        .durations("demo", &selected(), "check", &[], Some("test build"))
        .unwrap();
    assert_eq!(build["median_ms"], 70.0);
    assert!(
        s.durations("demo", &selected(), "unknown", &[], None)
            .unwrap()["median_ms"]
            .is_null()
    );
}

#[test]
fn diagnostic_hash_groups_include_only_selected_project_and_checkout_occurrences() {
    let s = store();
    for (id, project, checkout) in [
        (1, "demo", "/repo/a"),
        (2, "demo", "/repo/b"),
        (3, "other", "/repo/a"),
        (4, "demo", "/elsewhere"),
    ] {
        run(&s, id, project, checkout, None, "failed");
    }
    s.conn.execute("INSERT INTO diagnostic_texts VALUES ('hash','error: synthetic compile fault'),('warning','warning: synthetic')",[]).unwrap();
    for id in 1..=4 {
        s.conn.execute("INSERT INTO diagnostics VALUES (?1,'crate','error','E0001','src/lib.rs',?1,'hash')",[id]).unwrap();
    }
    s.conn
        .execute(
            "INSERT INTO diagnostics VALUES (1,'crate','warning',NULL,NULL,NULL,'warning')",
            [],
        )
        .unwrap();
    let out = s.diagnostic_groups("demo", &selected(), "error").unwrap();
    assert_eq!(out["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(
        out["diagnostics"][0]["occurrences"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        s.diagnostic_groups("demo", &selected(), "warning").unwrap()["diagnostics"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn old_schema_migrates_additively_without_relabelling_legacy_identity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(super::super::SCHEMA).unwrap();
    drop(conn);
    let s = Store::open_at(&path).unwrap();
    run(&s, 1, "demo", "/repo/a", None, "passed");
    assert!(s.run(1).unwrap().unwrap().git_tree.is_none());
    assert!(s.run(1).unwrap().unwrap().observation_error.is_none());
    drop(s);
    assert!(Store::open_at(&path).is_ok());
}

#[test]
fn legacy_head_resolution_is_explicit_and_never_overrides_observation_failure() {
    let dir = tempfile::tempdir().unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=Synthetic",
        "-c",
        "user.email=synthetic@example.invalid",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "first",
    ]);
    let first = git(&["rev-parse", "HEAD"]);
    git(&[
        "-c",
        "user.name=Synthetic",
        "-c",
        "user.email=synthetic@example.invalid",
        "commit",
        "-q",
        "--allow-empty",
        "-m",
        "second",
    ]);
    let second = git(&["rev-parse", "HEAD"]);
    assert_ne!(first, second);
    let s = store();
    for (id, head, status) in [(1, first, "failed"), (2, second, "passed")] {
        run(&s, id, "demo", "/repo/a", None, status);
        s.conn
            .execute("UPDATE runs SET head=?2 WHERE id=?1", params![id, head])
            .unwrap();
        test(&s, id, "suite", "test", status);
    }
    assert!(
        s.intermittent("demo", &selected(), None).unwrap()["tests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let resolved = s
        .intermittent("demo", &selected(), Some(dir.path()))
        .unwrap();
    assert_eq!(resolved["tests"].as_array().unwrap().len(), 1);
    assert_eq!(resolved["unknown_tree_observations"], 0);
    assert_eq!(
        resolved["tests"][0]["observations"][0]["tree_source"],
        "resolved legacy HEAD"
    );
    s.conn
        .execute(
            "UPDATE runs SET observation_error='failed git diff' WHERE id=2",
            [],
        )
        .unwrap();
    let unknown = s
        .intermittent("demo", &selected(), Some(dir.path()))
        .unwrap();
    assert!(unknown["tests"].as_array().unwrap().is_empty());
    assert_eq!(unknown["unknown_tree_observations"], 1);
}
