use super::*;
#[test]
fn every_requested_scenario_must_be_exported() {
    let requested = vec!["vm-stack".into(), "vm-restore".into()];
    assert!(select(&requested, &requested[..1]).is_err());
    assert_eq!(
        select(&requested, &requested).unwrap(),
        ["vm-restore", "vm-stack"]
    );
    assert!(select(&[], &requested).is_err());
}

fn checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "--quiet"],
        vec![
            "-c",
            "user.name=Example",
            "-c",
            "user.email=example@example.invalid",
            "commit",
            "--allow-empty",
            "--quiet",
            "-m",
            "initial",
        ],
    ] {
        assert!(
            Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .unwrap()
                .success()
        );
    }
    dir
}
#[test]
fn untracked_and_tracked_changes_are_refused_before_discovery() {
    let dir = checkout();
    let mut run = Run::unrecorded("demo", "vm", &[]);
    run.root = dir.path().into();
    assert!(crate::Tree::observe(dir.path()).dirty.is_none());
    let options = Vm {
        scenarios: vec![],
        system: "x86_64-linux".into(),
        output: dir.path().join("logs"),
    };
    fs::write(dir.path().join("new.rs"), "new").unwrap();
    assert!(
        execute_with(&mut run, &options, &["vm-stack"], || Command::new(
            "/missing-nix"
        ))
        .unwrap_err()
        .contains("committed checkout")
    );
    assert!(
        Command::new("git")
            .args(["add", "new.rs"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        execute_with(&mut run, &options, &["vm-stack"], || Command::new(
            "/missing-nix"
        ))
        .unwrap_err()
        .contains("committed checkout")
    );
}
#[test]
fn failed_scenarios_keep_original_logs_and_do_not_skip_later_scenarios() {
    let dir = checkout();
    let output = tempfile::tempdir().unwrap();
    let mut run = Run::unrecorded("demo", "vm", &[]);
    run.root = dir.path().into();
    let options = Vm {
        scenarios: vec![],
        system: "x86_64-linux".into(),
        output: output.path().into(),
    };
    let script = r#"if [ "$1" = eval ]; then printf '%s' '["vm-stack","vm-topology"]'; exit 0; fi; echo 'assertion failed: żółć'; echo 'later test passed'; case "$*" in *vm-stack*) exit 7;; esac"#;
    let result = execute_with(&mut run, &options, &["vm-stack", "vm-topology"], || {
        let mut command = Command::new("sh");
        command.args(["-c", script, "fake-nix"]);
        command
    });
    assert!(result.is_err());
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(output.path().join("results.json")).unwrap()).unwrap();
    assert_eq!(report[0]["passed"], false);
    assert_eq!(report[1]["passed"], true);
    let log = fs::read_to_string(output.path().join("vm-stack.log")).unwrap();
    assert!(log.contains("assertion failed: żółć"));
    assert!(log.contains("later test passed"));
}
#[test]
fn failed_and_invalid_discovery_are_reported() {
    let dir = checkout();
    let mut run = Run::unrecorded("demo", "vm", &[]);
    run.root = dir.path().into();
    let options = Vm {
        scenarios: vec![],
        system: "x86_64-linux".into(),
        output: dir.path().join("logs"),
    };
    for script in ["echo 'discovery failed' >&2; exit 2", "printf invalid"] {
        assert!(
            execute_with(&mut run, &options, &["vm-stack"], || {
                let mut command = Command::new("sh");
                command.args(["-c", script, "fake-nix"]);
                command
            })
            .unwrap_err()
            .contains("discovery")
        );
    }
}

#[test]
fn cached_scenarios_record_the_derivation_log() {
    let dir = checkout();
    let output = tempfile::tempdir().unwrap();
    let mut run = Run::unrecorded("demo", "vm", &[]);
    run.root = dir.path().into();
    let options = Vm {
        scenarios: vec![],
        system: "x86_64-linux".into(),
        output: output.path().into(),
    };
    execute_with(&mut run, &options, &["vm-stack"], || {
        let mut command = Command::new("sh");
        command.args(["-c", r#"case "$1" in eval) printf '%s' '["vm-stack"]';; build) echo 'cached result';; log) echo 'original scenario assertions passed';; *) exit 9;; esac"#, "fake-nix"]);
        command
    }).unwrap();
    assert_eq!(
        fs::read_to_string(output.path().join("vm-stack.log")).unwrap(),
        "original scenario assertions passed\n"
    );
    assert!(
        fs::read_to_string(output.path().join("vm-stack.build.log"))
            .unwrap()
            .contains("cached result")
    );
}

#[test]
fn concurrent_disjoint_scenarios_cannot_share_one_results_document() {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    let source = checkout();
    let output = tempfile::tempdir().unwrap();
    let gate = Arc::new(Barrier::new(2));
    let results = std::thread::scope(|threads| {
        let handles: Vec<_> = ["vm-one", "vm-two"].into_iter().map(|scenario| {
            let root = source.path().to_path_buf();
            let output = output.path().to_path_buf();
            let gate = gate.clone();
            threads.spawn(move || {
                let mut run = Run::unrecorded("demo", "vm", &[]);
                run.root = root;
                let options = Vm { scenarios: vec![scenario.into()], system: "x86_64-linux".into(), output };
                let calls = AtomicUsize::new(0);
                let result = execute_with(&mut run, &options, &[], || {
                    if calls.fetch_add(1, Ordering::SeqCst) == 0 { gate.wait(); }
                    let mut command = Command::new("sh");
                    command.args(["-c", r#"if [ "$1" = eval ]; then printf '%s' '["vm-one","vm-two"]'; else printf '%s\n' 'synthetic scenario passed'; fi"#, "fake-nix"]);
                    command
                });
                (scenario, result)
            })
        }).collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    let passed: Vec<_> = results
        .iter()
        .filter(|(_, result)| result.is_ok())
        .collect();
    assert_eq!(passed.len(), 1, "{results:?}");
    let (winner, _) = passed[0];
    let (loser, failed) = results.iter().find(|(_, result)| result.is_err()).unwrap();
    assert!(failed.as_ref().unwrap_err().contains("results.json"));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(output.path().join("results.json")).unwrap()).unwrap();
    assert_eq!(report.as_array().unwrap().len(), 1);
    assert_eq!(report[0]["scenario"], *winner);
    assert_eq!(report[0]["passed"], true);
    assert!(output.path().join(format!("{winner}.log")).is_file());
    assert!(output.path().join(format!("{winner}.build.log")).is_file());
    assert!(!output.path().join(format!("{loser}.log")).exists());
    assert!(!output.path().join(format!("{loser}.build.log")).exists());
}
