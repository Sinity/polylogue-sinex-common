use super::*;
#[test]
fn missing_and_malformed_manifests_fail() {
    let dir = tempfile::tempdir().unwrap();
    assert!(targets(dir.path()).is_err());
    fs::create_dir(dir.path().join("fuzz")).unwrap();
    fs::write(dir.path().join("fuzz/Cargo.toml"), "not valid toml [").unwrap();
    assert!(targets(dir.path()).is_err());
}
#[test]
fn manifest_binary_names_are_discovered_without_building() {
    let dir = tempfile::tempdir().unwrap();
    let fuzz = dir.path().join("fuzz");
    fs::create_dir_all(fuzz.join("src")).unwrap();
    fs::write(fuzz.join("src/main.rs"), "fn main(){}").unwrap();
    fs::write(fuzz.join("Cargo.toml"),"[package]\nname='example-fuzz'\nversion='0.0.0'\nedition='2024'\n[package.metadata]\ncargo-fuzz=true\n[workspace]\n[[bin]]\nname='roundtrip'\npath='src/main.rs'\n").unwrap();
    assert_eq!(targets(dir.path()).unwrap(), ["roundtrip"]);
}

#[test]
fn library_search_paths_include_nix_cxx_and_existing_without_duplicates() {
    assert_eq!(
        library_paths(
            Some(std::ffi::OsStr::new("/db:/extra")),
            Some("-L/gcc -L /db -rpath /ignored"),
            Some(Path::new("/cxx"))
        ),
        ["/gcc", "/db", "/cxx", "/extra"].map(PathBuf::from)
    );
}
#[test]
fn unknown_target_and_zero_budget_fail_before_starting() {
    let dir = tempfile::tempdir().unwrap();
    let mut run = Run::unrecorded("demo", "fuzz", &[]);
    for (target, seconds) in [("missing", 1), ("query", 0)] {
        let options = Fuzz::Run {
            target: target.into(),
            seconds,
            memory_mb: 100,
            output: dir.path().into(),
        };
        assert!(
            execute_with(&mut run, &options, vec!["query".into()], || {
                std::process::Command::new("/does-not-exist")
            })
            .is_err()
        );
    }
}
#[test]
fn crash_artifacts_are_kept_and_replay_passes_the_original_input() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("fuzz")).unwrap();
    let output = dir.path().join("experiment");
    let mut run = Run::unrecorded("demo", "fuzz", &[]);
    run.root = dir.path().into();
    let options = Fuzz::Run {
        target: "query".into(),
        seconds: 1,
        memory_mb: 100,
        output: output.clone(),
    };
    let script = r#"for arg in "$@"; do case "$arg" in -artifact_prefix=*) folder=${arg#-artifact_prefix=}; printf '%s' synthetic > "${folder}crash-example";; esac; done; exit 1"#;
    let result = execute_with(&mut run, &options, vec!["query".into()], || {
        let mut command = std::process::Command::new("sh");
        command.args(["-c", script, "fake-cargo"]);
        command
    });
    assert!(result.is_err());
    let input = output.join("artifacts/crash-example");
    assert_eq!(fs::read(&input).unwrap(), b"synthetic");
    let options = Fuzz::Replay {
        target: "query".into(),
        input: input.clone(),
        output: dir.path().join("replay"),
    };
    let recorded = dir.path().join("args");
    execute_with(&mut run, &options, vec!["query".into()], || {
        let mut command = std::process::Command::new("sh");
        command
            .args(["-c", "printf '%s\\n' \"$@\" > \"$RECORD\"", "fake-cargo"])
            .env("RECORD", &recorded);
        command
    })
    .unwrap();
    let args = fs::read_to_string(recorded).unwrap();
    assert!(args.contains(input.to_str().unwrap()));
    assert!(args.contains("-runs=1"));
}

#[test]
fn synthetic_seeds_are_copied_once_and_read_failures_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    let seeds = dir.path().join("seeds");
    let corpus = dir.path().join("corpus");
    fs::create_dir(&seeds).unwrap();
    fs::create_dir(&corpus).unwrap();
    fs::write(seeds.join("example"), "synthetic").unwrap();
    seed_corpus(&seeds, &corpus).unwrap();
    seed_corpus(&seeds, &corpus).unwrap();
    assert_eq!(fs::read_dir(&corpus).unwrap().count(), 1);
    fs::create_dir(seeds.join("invalid")).unwrap();
    assert!(seed_corpus(&seeds, &corpus).is_err());
}
