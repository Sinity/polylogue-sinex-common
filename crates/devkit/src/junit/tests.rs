use super::*;

#[test]
fn statuses_and_failure_output() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<testsuites name="nextest-run" tests="4" failures="1" errors="0">
  <testsuite name="polylogue-store" tests="4">
    <testcase name="passes" classname="polylogue-store" time="0.010"/>
    <testcase name="fails" classname="polylogue-store" time="1.500">
      <failure message="thread panicked" type="test failure">assertion failed</failure>
      <system-out>out line</system-out>
      <system-err>err line</system-err>
    </testcase>
    <testcase name="retried" classname="polylogue-store" time="0.2">
      <flakyFailure message="first try" type="test failure">boom</flakyFailure>
    </testcase>
    <testcase name="ignored" classname="polylogue-store" time="0"><skipped/></testcase>
  </testsuite>
</testsuites>"#;
    let tests = outcomes(xml).unwrap();
    let status: Vec<_> = tests.iter().map(|t| (t.name.as_str(), t.status)).collect();
    assert_eq!(
        status,
        [
            ("passes", TestStatus::Passed),
            ("fails", TestStatus::Failed),
            ("retried", TestStatus::Flaky),
            ("ignored", TestStatus::Skipped)
        ]
    );
    let failed = &tests[1];
    assert_eq!(failed.binary, "polylogue-store");
    assert_eq!(failed.duration, Duration::from_millis(1500));
    let output = failed.output.as_deref().unwrap();
    assert!(
        output.contains("thread panicked")
            && output.contains("out line")
            && output.contains("err line")
    );
    assert!(!output.contains("assertion failed"));
    assert!(tests[0].output.is_none());
}

#[test]
fn unknown_skip_reasons_stay_skipped_and_keep_the_reason() {
    let tests = outcomes(r#"<testsuites tests="1"><testsuite name="probe"><testcase name="unknown"><skipped message="a future reason"/></testcase></testsuite></testsuites>"#).unwrap();
    assert_eq!(tests[0].status, TestStatus::Skipped);
    assert!(
        tests[0]
            .output
            .as_deref()
            .unwrap()
            .contains("a future reason")
    );
}

#[test]
fn absent_truncated_and_inconsistent_reports_are_not_empty_successes() {
    for xml in [
        "",
        "<testsuites tests=\"0\">",
        "<testsuites tests=\"1\"></testsuites>",
        "<testsuites tests=\"1\"><testsuite><testcase name=\"unfinished\">",
    ] {
        assert!(outcomes(xml).is_err(), "{xml}");
    }
    assert!(outcomes("<testsuites tests=\"0\"/>").unwrap().is_empty());
}
