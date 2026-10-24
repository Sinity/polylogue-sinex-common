use super::*;
use clap::Parser;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[test]
fn history_queries_parse_exact_identity_selection_and_explicit_population() {
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
