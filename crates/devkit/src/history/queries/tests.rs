use super::*;
use std::path::Path;

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
fn equal_git_trees_across_commits_group_results_from_the_named_checkouts() {
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
    let out = s.intermittent("demo", &selected()).unwrap();
    assert_eq!(out["tests"].as_array().unwrap().len(), 1);
    assert_eq!(out["tests"][0]["runs"].as_array().unwrap().len(), 3);
    assert_eq!(out["tests"][0]["git_tree"], "same-tree");
    assert_eq!(out["results_without_tree"], 1);
    let one = s.intermittent("demo", &["/repo/a".into()]).unwrap();
    assert!(one["tests"].as_array().unwrap().is_empty());
}

#[test]
fn different_changes_and_unreadable_checkouts_do_not_group() {
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
    let out = s.intermittent("demo", &selected()).unwrap();
    assert!(out["tests"].as_array().unwrap().is_empty());
    assert_eq!(out["results_without_tree"], 2);
    // Duplicate pass/fail rows from one run plus another skipped run do not
    // establish contradictory outcomes across distinct runs.
    test(&s, 1, "suite", "duplicate", "passed");
    test(&s, 1, "suite", "duplicate", "failed");
    test(&s, 2, "suite", "duplicate", "skipped");
    assert!(
        s.intermittent("demo", &selected()).unwrap()["tests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn first_failure_follows_start_order_and_names_uncommitted_changes() {
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
    assert_eq!(out["first_failure"]["run"], 3);
    assert_eq!(out["first_failure"]["revision"], "uncommitted changes");
    assert_eq!(out["results"].as_array().unwrap().len(), 5);
    s.conn
        .execute(
            "UPDATE runs SET observation_error='git unavailable' WHERE id=3",
            [],
        )
        .unwrap();
    assert_eq!(
        s.first_failing("demo", &selected(), "suite", "test")
            .unwrap()["first_failure"]["revision"],
        "unknown"
    );
    assert!(
        s.first_failing("demo", &selected(), "suite", "missing")
            .unwrap()["first_failure"]
            .is_null()
    );
}

#[test]
fn durations_list_runs_left_out_of_the_summary() {
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
    assert_eq!(total["run_count"], 4);
    assert_eq!(total["runs"][1]["left_out"], "run not passed");
    let stage = s
        .durations("demo", &selected(), "check", &[], Some("test"))
        .unwrap();
    assert_eq!(stage["sample_count"], 1);
    assert_eq!(stage["median_ms"], 30.0);
    assert_eq!(stage["runs"][3]["left_out"], "stage not recorded");
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
fn diagnostic_groups_include_only_the_named_project_and_checkouts() {
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
