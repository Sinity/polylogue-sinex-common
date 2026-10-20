use super::*;

#[test]
fn overridden_target_records_this_runs_outcomes_and_leaves_stale_xml_alone() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"devkit-report-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    fs::write(root.join("src/lib.rs"), "#[test] fn fresh_pass() {}\n#[test] fn fresh_failure() { panic!(\"expected witness failure\"); }\n").unwrap();
    let stale_path = root.join("target/nextest/default/devkit-junit.xml");
    fs::create_dir_all(stale_path.parent().unwrap()).unwrap();
    let stale = "<testsuites><testsuite name=\"stale\"><testcase name=\"stale_test\" classname=\"stale\" time=\"0\"/></testsuite></testsuites>";
    fs::write(&stale_path, stale).unwrap();
    // A quote and space also distinguish correctly escaped TOML paths.
    let target = root.join("other target \"quoted\"");
    let mut run = Run::unrecorded("devkit-report-probe", "test", &[]);
    run.root = root.into();
    run.target = Some(target.clone());
    let result = run.nextest(
        "test",
        &Nextest {
            args: vec!["--offline".into()],
            env: vec![("CARGO_TARGET_DIR".into(), target.display().to_string())],
            ..Nextest::default()
        },
    );
    assert!(
        result.is_err(),
        "the planted fixture failure must be returned"
    );
    let outcomes: std::collections::BTreeMap<_, _> = run
        .tests
        .iter()
        .map(|test| (test.name.as_str(), &test.status))
        .collect();
    assert_eq!(outcomes.len(), 2, "{:?}", run.tests);
    assert_eq!(outcomes.get("fresh_pass"), Some(&&TestStatus::Passed));
    assert_eq!(outcomes.get("fresh_failure"), Some(&&TestStatus::Failed));
    assert_eq!(fs::read_to_string(stale_path).unwrap(), stale);
    assert!(target.join("nextest/default/devkit-junit.xml").is_file());
}
