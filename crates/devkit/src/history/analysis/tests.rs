use super::*;
fn store() -> Store {
    let s = Store::open_at(std::path::Path::new(":memory:")).unwrap();
    for (id, ms) in [(1, 100), (2, 200)] {
        s.conn.execute("INSERT INTO runs(id,project,checkout,command,args,host,toolchain,pid,started_ms,duration_ms,status) VALUES(?1,'demo','/repo','test','[]','host','rust',0,?1*50,?2,'passed')",params![id,ms]).unwrap();
    }
    s
}
#[test]
fn comparisons_preserve_scope_and_unknowns() {
    let s = store();
    assert_eq!(
        s.compare("demo", 1, 2).unwrap()["duration_change"]["trend"],
        "slower"
    );
    assert_eq!(
        s.compare("demo", 2, 1).unwrap()["duration_change"]["trend"],
        "faster"
    );
    assert_eq!(
        s.compare("demo", 1, 1).unwrap()["duration_change"]["trend"],
        "stable"
    );
    assert!(s.compare("other", 1, 2).is_err());
    for field in ["args", "host", "toolchain", "status"] {
        let original: String = s
            .conn
            .query_row(&format!("SELECT {field} FROM runs WHERE id=2"), [], |r| {
                r.get(0)
            })
            .unwrap();
        s.conn
            .execute(
                &format!("UPDATE runs SET {field}='different' WHERE id=2"),
                [],
            )
            .unwrap();
        assert!(s.compare("demo", 1, 2).is_err());
        s.conn
            .execute(
                &format!("UPDATE runs SET {field}=?1 WHERE id=2"),
                [original],
            )
            .unwrap();
    }
    s.conn
        .execute("UPDATE runs SET duration_ms=NULL WHERE id=2", [])
        .unwrap();
    assert!(s.compare("demo", 1, 2).unwrap()["duration_change"]["delta_ms"].is_null());
}
#[test]
fn pass_rates_stages_and_overlap_do_not_sum_parallel_test_time() {
    let s = store();
    s.conn.execute_batch("INSERT INTO tests VALUES(1,'suite','a','passed',90,NULL),(1,'suite','b','failed',80,NULL),(1,'suite','c','filtered',0,NULL);INSERT INTO stages VALUES(1,0,'database',10,1),(1,1,'test',90,1);").unwrap();
    let report = s.explain("demo", 1).unwrap();
    assert_eq!(report["pass_rate_percent"], 50.0);
    assert_eq!(report["stages"][0]["name"], "database");
    assert_eq!(report["overlaps"][0]["overlap_ms"], 50);
    assert!(report["queue_ms"].is_null());
    assert!(report["resources"].is_null());
    assert!(s.explain("demo", 2).unwrap()["pass_rate_percent"].is_null());
}
#[test]
fn daily_comparisons_keep_exact_scope_and_validate_dates() {
    let s = store();
    assert!(
        s.compare_days("demo", "/repo", "yesterday", "2026-10-27")
            .is_err()
    );
    s.conn
        .execute(
            "UPDATE runs SET started_ms=started_ms+86400000 WHERE id=2",
            [],
        )
        .unwrap();
    assert_eq!(
        s.compare_days("demo", "/repo", "1970-01-01", "1970-01-02")
            .unwrap()["changes"][0]["change"]["trend"],
        "slower"
    );
    s.conn
        .execute(r#"UPDATE runs SET args='["-p","one"]' WHERE id=2"#, [])
        .unwrap();
    let different = s
        .compare_days("demo", "/repo", "1970-01-01", "1970-01-02")
        .unwrap();
    assert!(
        different["changes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|group| group["change"]["delta_ms"].is_null())
    );
}

#[test]
fn interrupted_runs_keep_unknown_duration_and_are_not_duration_baselines() {
    let s = store();
    s.conn
        .execute(
            "UPDATE runs SET status='running',pid=-1,duration_ms=NULL WHERE id=2",
            [],
        )
        .unwrap();
    let report = s.explain("demo", 2).unwrap();
    assert_eq!(report["header"]["status"], "interrupted");
    assert!(report["header"]["duration_ms"].is_null());
    assert!(s.compare("demo", 1, 2).is_err());
}

#[test]
fn reports_show_source_identity_and_refuse_changed_input() {
    let store = store();
    store
        .conn
        .execute(
            "UPDATE runs SET head='revision',git_tree='tree',dirty='changes' WHERE id=1",
            [],
        )
        .unwrap();
    let report = store.explain("demo", 1).unwrap();
    assert_eq!(report["header"]["head"], "revision");
    assert_eq!(report["header"]["checkout"], "/repo");
    assert_eq!(report["header"]["git_tree"], "tree");
    assert_eq!(report["header"]["dirty"], true);
    assert_eq!(report["header"]["dirty_hash"], "changes");
    store
        .conn
        .execute(
            "UPDATE runs SET observation_error='source changed during run' WHERE id=1",
            [],
        )
        .unwrap();
    assert!(store.compare("demo", 1, 2).unwrap_err().contains("source"));
}

#[test]
fn added_stages_and_daily_setup_costs_stay_visible() {
    let store = store();
    store.conn.execute_batch("INSERT INTO stages VALUES(1,0,'database',10,1),(2,0,'database',30,1),(2,1,'cleanup',5,1);UPDATE runs SET started_ms=86400000 WHERE id=2;").unwrap();
    let compared = store.compare("demo", 1, 2).unwrap();
    let added = compared["stage_changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "cleanup")
        .unwrap();
    assert!(added["change"]["before_ms"].is_null());
    assert_eq!(added["change"]["after_ms"], 5.0);
    assert!(added["change"]["trend"].is_null());
    let daily = store
        .compare_days("demo", "/repo", "1970-01-01", "1970-01-02")
        .unwrap();
    let setup = daily["stage_changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "database")
        .unwrap();
    assert_eq!(setup["change"]["delta_ms"], 20.0);
    let added = daily["stage_changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|stage| stage["name"] == "cleanup")
        .unwrap();
    assert_eq!(added["change"]["after_ms"], 5.0);
    assert!(added["change"]["delta_ms"].is_null());
}

#[test]
fn zero_and_missing_duration_metrics_stay_unknown() {
    let change = duration_delta(&json!(0), &json!(10));
    assert!(change["percent"].is_null());
    assert_eq!(change["delta_ms"], 10.0);
    let change = duration_delta(&Value::Null, &json!(10));
    assert_eq!(change["after_ms"], 10.0);
    assert!(change["delta_ms"].is_null());
    assert!(change["trend"].is_null());
}

#[test]
fn daily_missing_measurements_are_not_zero_durations() {
    let store = store();
    store.conn.execute_batch("UPDATE runs SET duration_ms=NULL WHERE id=1;UPDATE runs SET observation_error='changed' WHERE id=2;").unwrap();
    let report = store
        .compare_days("demo", "/repo", "1970-01-01", "1970-01-02")
        .unwrap();
    assert_eq!(report["groups"][0]["duration_samples"], 0);
    assert!(report["groups"][0]["mean_passed_ms"].is_null());
    let report = store
        .compare_days("other", "/repo", "1970-01-01", "1970-01-02")
        .unwrap();
    assert!(report["groups"].as_array().unwrap().is_empty());
    assert!(report["stage_groups"].as_array().unwrap().is_empty());
}
