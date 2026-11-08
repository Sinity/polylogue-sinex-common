use super::*;
use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[test]
fn history_queries_parse_test_names_and_checkouts() {
    let parsed = Cli::try_parse_from([
        "xtask",
        "history",
        "first-failing",
        "suite",
        "name",
        "--checkout",
        "/repo/a",
        "--checkout",
        "/repo/b",
        "--json",
    ])
    .unwrap();
    let Command::History(History::FirstFailing {
        binary,
        name,
        scope,
        json,
    }) = parsed.command
    else {
        panic!()
    };
    assert_eq!(
        (binary.as_str(), name.as_str(), json),
        ("suite", "name", true)
    );
    assert_eq!(
        scope.paths(Path::new("/default")).unwrap(),
        vec!["/repo/a", "/repo/b"]
    );
    assert!(
        Cli::try_parse_from([
            "xtask",
            "history",
            "durations",
            "check",
            "--stage",
            "test build",
            "--args-json",
            "[]"
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from(["xtask", "history", "diagnostic-groups", "--level", "info"]).is_err()
    );
    let scope = HistoryScope {
        checkouts: vec!["relative".into()],
    };
    assert!(scope.paths(Path::new("/default")).is_err());
}

#[test]
fn explicit_run_ids_keep_project_ownership_and_allow_named_cross_checkout_runs() {
    let store = Store::open_at(Path::new(":memory:")).unwrap();
    for (id, project, checkout) in [
        (1, "demo", "/repo"),
        (2, "other", "/private"),
        (3, "demo", "/another-checkout"),
    ] {
        store.connection().execute(
            "INSERT INTO runs(id,project,checkout,command,args,pid,started_ms,status) VALUES(?1,?2,?3,'test','[]',0,0,'passed')",
            rusqlite::params![id, project, checkout],
        ).unwrap();
    }
    assert_eq!(
        resolve(&store, "demo", Path::new("/repo"), None)
            .unwrap()
            .unwrap()
            .id,
        1
    );
    assert_eq!(
        resolve(&store, "demo", Path::new("/repo"), Some(3))
            .unwrap()
            .unwrap()
            .checkout,
        "/another-checkout"
    );
    let refused = resolve(&store, "demo", Path::new("/repo"), Some(2)).unwrap_err();
    assert_eq!(refused, "run is not in this project");
    assert!(!refused.contains("other"));
    assert!(!refused.contains("/private"));
    assert!(
        resolve(&store, "demo", Path::new("/repo"), Some(999))
            .unwrap()
            .is_none()
    );
}
