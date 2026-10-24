use super::*;

#[test]
fn overridden_target_records_this_runs_outcomes_and_leaves_stale_xml_alone() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"devkit-report-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    fs::write(root.join("src/lib.rs"), "#[test] fn fresh_pass() {}\n#[test] fn fresh_failure() { panic!(\"expected failure\"); }\n#[test] #[ignore] fn ignored_case() {}\n").unwrap();
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
    assert_eq!(outcomes.len(), 3, "{:?}", run.tests);
    assert_eq!(outcomes.get("fresh_pass"), Some(&&TestStatus::Passed));
    assert_eq!(outcomes.get("fresh_failure"), Some(&&TestStatus::Failed));
    assert_eq!(outcomes.get("ignored_case"), Some(&&TestStatus::Ignored));
    assert_eq!(fs::read_to_string(stale_path).unwrap(), stale);
    assert!(target.join("nextest/default/devkit-junit.xml").is_file());
}

#[test]
fn selected_report_keeps_skip_reasons() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    fs::create_dir(root.join("src")).unwrap();
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"devkit-selection-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n").unwrap();
    fs::write(root.join("src/lib.rs"), "#[test] fn selected() {}\n#[test] fn excluded() {}\n#[test] #[ignore] fn ignored_case() {}\n").unwrap();
    let mut run = Run::unrecorded("devkit-selection-probe", "test", &[]);
    run.root = root.into();
    // Nested fixture builds must not inherit the enclosing job's selected cache.
    let target = root.join("selected target \"quoted\"");
    run.target = Some(target.clone());
    let target_env = vec![("CARGO_TARGET_DIR".into(), target.display().to_string())];
    run.nextest(
        "test",
        &Nextest {
            args: vec!["--offline".into(), "-E".into(), "test(selected)".into()],
            env: target_env.clone(),
            ..Nextest::default()
        },
    )
    .unwrap();
    let report = target.join("nextest/default/devkit-junit.xml");
    assert!(
        target.join("debug").is_dir(),
        "the fixture compiles in its own selected cache"
    );
    // Nextest may create workspace-relative store metadata, but Cargo's build
    // artifacts must stay under this fixture's selected target.
    assert!(!root.join("target/debug").exists());
    assert!(
        fs::read_to_string(report)
            .unwrap()
            .contains("skipped=\"2\"")
    );
    assert_eq!(run.tests.len(), 3, "{:?}", run.tests);
    let status = |run: &Run, name: &str| {
        run.tests
            .iter()
            .find(|test| test.name == name)
            .unwrap()
            .status
    };
    assert_eq!(status(&run, "selected"), TestStatus::Passed);
    assert_eq!(status(&run, "excluded"), TestStatus::Filtered);
    assert_eq!(status(&run, "ignored_case"), TestStatus::Ignored);
    assert!(
        run.tests
            .iter()
            .filter(|test| test.status != TestStatus::Passed)
            .all(|test| test.output.is_some())
    );
    let summary = run.summary(Duration::ZERO, None);
    assert!(
        summary
            .contains("1 passed, 0 failed, 0 flaky, 2 skipped (1 by ignore selection, 1 filtered)"),
        "{summary}"
    );
    run.tests.clear();
    run.nextest(
        "ignored only",
        &Nextest {
            args: vec!["--offline".into(), "--run-ignored".into(), "only".into()],
            env: target_env.clone(),
            ..Nextest::default()
        },
    )
    .unwrap();
    assert_eq!(status(&run, "ignored_case"), TestStatus::Passed);
    assert_eq!(status(&run, "selected"), TestStatus::Ignored);
    assert_eq!(status(&run, "excluded"), TestStatus::Ignored);
    run.tests.clear();
    run.nextest(
        "empty selection",
        &Nextest {
            args: vec!["--offline".into(), "-E".into(), "none()".into()],
            allow_no_tests: true,
            env: target_env.clone(),
            ..Nextest::default()
        },
    )
    .unwrap();
    assert_eq!(run.tests.len(), 0);
    assert!(
        run.summary(Duration::ZERO, None)
            .contains("no outcomes reported")
    );
}
