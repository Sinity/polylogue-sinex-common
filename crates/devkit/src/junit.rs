//! Test outcomes from nextest's JUnit report, its stable machine-readable
//! output.
use crate::{TestOutcome, TestStatus};
use quick_xml::{Reader, events::Event};
use std::time::Duration;

pub(crate) fn outcomes(xml: &str) -> Result<Vec<TestOutcome>, String> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut suite = String::new();
    let mut case: Option<Case> = None;
    let mut capture = false;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref() == b"testsuite" => {
                suite = attribute(&e, b"name").unwrap_or_default();
            }
            Event::Start(e) if e.name().as_ref() == b"testcase" => {
                case = Some(Case::new(&e));
            }
            Event::Empty(e) if e.name().as_ref() == b"testcase" => {
                out.push(Case::new(&e).finish(&suite));
            }
            Event::Start(e) | Event::Empty(e) => {
                if let Some(case) = case.as_mut() {
                    match e.name().as_ref() {
                        b"failure" | b"error" | b"rerunFailure" | b"rerunError" => {
                            case.failed = true
                        }
                        b"flakyFailure" | b"flakyError" => case.flaky = true,
                        b"skipped" => case.skipped = true,
                        _ => {}
                    }
                    if let Some(message) = attribute(&e, b"message") {
                        case.output.push_str(&message);
                        case.output.push('\n');
                    }
                    // A failure element's body repeats the test's stderr, so
                    // only the captured streams are kept.
                    capture = matches!(e.name().as_ref(), b"system-out" | b"system-err");
                }
            }
            Event::End(e) if matches!(e.name().as_ref(), b"system-out" | b"system-err") => {
                capture = false;
            }
            Event::Text(text) if capture => {
                if let Some(case) = case.as_mut() {
                    case.output
                        .push_str(&text.decode().map_err(|e| e.to_string())?);
                }
            }
            Event::CData(text) if capture => {
                if let Some(case) = case.as_mut() {
                    case.output.push_str(&String::from_utf8_lossy(&text));
                }
            }
            Event::End(e) if e.name().as_ref() == b"testcase" => {
                if let Some(case) = case.take() {
                    out.push(case.finish(&suite));
                }
                capture = false;
            }
            Event::Eof => return Ok(out),
            _ => {}
        }
    }
}

struct Case {
    name: String,
    duration: Duration,
    failed: bool,
    flaky: bool,
    skipped: bool,
    output: String,
}

impl Case {
    fn new(e: &quick_xml::events::BytesStart<'_>) -> Self {
        let seconds = attribute(e, b"time")
            .and_then(|t| t.parse::<f64>().ok())
            .unwrap_or(0.0);
        Self {
            name: attribute(e, b"name").unwrap_or_default(),
            duration: Duration::from_secs_f64(seconds.max(0.0)),
            failed: false,
            flaky: false,
            skipped: false,
            output: String::new(),
        }
    }

    fn finish(self, suite: &str) -> TestOutcome {
        let status = if self.failed {
            TestStatus::Failed
        } else if self.flaky {
            TestStatus::Flaky
        } else if self.skipped {
            TestStatus::Skipped
        } else {
            TestStatus::Passed
        };
        let keep =
            matches!(status, TestStatus::Failed | TestStatus::Flaky) && !self.output.is_empty();
        TestOutcome {
            binary: suite.to_owned(),
            name: self.name,
            status,
            duration: self.duration,
            output: keep.then_some(self.output),
        }
    }
}

fn attribute(e: &quick_xml::events::BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.as_ref() == name)
        .and_then(|a| a.unescape_value().ok().map(|v| v.into_owned()))
}

#[cfg(test)]
mod tests {
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
}
