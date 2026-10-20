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
    let mut expected = None;
    let mut complete = false;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Empty(e) if e.name().as_ref() == b"testsuites" => {
                expected = attribute(&e, b"tests").and_then(|value| value.parse::<usize>().ok());
                complete = true;
            }
            Event::Start(e) if e.name().as_ref() == b"testsuites" => {
                expected = attribute(&e, b"tests").and_then(|value| value.parse::<usize>().ok());
            }
            Event::End(e) if e.name().as_ref() == b"testsuites" => complete = true,
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
                        b"skipped" => {
                            case.skipped = Some(match attribute(&e, b"message").as_deref() {
                                Some("Skipped: test does not match the run-ignored option") => {
                                    TestStatus::Ignored
                                }
                                Some(
                                    "Skipped: test does not match the provided expression filters"
                                    | "Skipped: test does not match the provided string filters"
                                    | "Skipped: test does not match the default filter"
                                    | "Skipped: test does not match the partition",
                                ) => TestStatus::Filtered,
                                _ => TestStatus::Skipped,
                            });
                        }
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
            Event::Eof => {
                if !complete || case.is_some() || expected != Some(out.len()) {
                    return Err("missing, incomplete or inconsistent test cases".into());
                }
                return Ok(out);
            }
            _ => {}
        }
    }
}

struct Case {
    name: String,
    duration: Duration,
    failed: bool,
    flaky: bool,
    skipped: Option<TestStatus>,
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
            skipped: None,
            output: String::new(),
        }
    }

    fn finish(self, suite: &str) -> TestOutcome {
        let status = if self.failed {
            TestStatus::Failed
        } else if self.flaky {
            TestStatus::Flaky
        } else if let Some(skipped) = self.skipped {
            skipped
        } else {
            TestStatus::Passed
        };
        let keep = status != TestStatus::Passed && !self.output.is_empty();
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
mod tests;
