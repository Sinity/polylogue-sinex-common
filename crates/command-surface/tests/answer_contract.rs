//! The answer contract on every surface: the envelope, outcomes, failures
//! that teach, value states and the model's single view of the rows.
use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Find {
    query: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Rows {
    rows: Vec<String>,
}

/// Answers by the query text: `gap` reads a source with a gap, `degraded-
/// blank` is degraded naming nothing, `unknown` fails with a field the
/// catalog does not have.
struct Search;
impl Operation for Search {
    type Request = Find;
    type Response = Rows;
    fn spec(&self) -> OperationSpec {
        OperationSpec {
            failures: vec![FailureKind::UnknownField],
            ..OperationSpec::request(
                "find",
                &["find"],
                "query",
                "Search synthetic rows",
                "The query language's one read",
            )
        }
    }
    fn execute(&self, request: Find, _: RequestContext) -> OperationFuture<'_, Answer<Rows>> {
        Box::pin(async move {
            let rows = Rows {
                rows: vec!["row-one-unique".into()],
            };
            match request.query.as_str() {
                "gap" => {
                    let mut answer = Answer::ok(rows);
                    answer.envelope.count = Some(Count::exact(5));
                    answer.envelope.shown = Some(1);
                    answer.envelope.coverage = vec![SourceCoverage {
                        source: "shell".into(),
                        state: CoverageState::Gap {
                            reason: "overflow".into(),
                        },
                        not_complete: vec![],
                    }];
                    answer
                }
                "degraded-blank" => Answer::degraded(rows, vec![]),
                "unknown" => {
                    let query = "sessions where reop:sinex";
                    let error = query_language::Error::new(
                        query_language::ErrorKind::UnknownField,
                        "unknown field \"reop\" for sessions",
                        15,
                    )
                    .suggestion("sessions where repo:sinex");
                    Answer::error(
                        Failure::from_query_error(error, query)
                            .with_candidates(vec!["repo".into(), "role".into()]),
                    )
                }
                _ => {
                    let mut answer = Answer::ok(rows);
                    answer.envelope.count = Some(Count::exact(3));
                    answer.envelope.shown = Some(1);
                    answer.envelope.left_out.push(LeftOut {
                        what: "rows".into(),
                        count: 2,
                        expand: Some(RunnableRequest::new(
                            "find",
                            json!({"query": "rest"}),
                            "page-2",
                        )),
                    });
                    answer
                }
            }
        })
    }
    fn render_text(&self, rows: &Rows) -> String {
        rows.rows.iter().map(|row| format!("{row}\n")).collect()
    }
}

fn table() -> OperationTable {
    let mut table = OperationTable::new("example", Deadlines::default());
    table
        .declare_tool(ToolGroup {
            name: "query".into(),
            description: "Search rows".into(),
            open_world: false,
        })
        .unwrap();
    table.register(Search).unwrap();
    table
}
fn find(query: &str) -> RunnableRequest {
    RunnableRequest::new("find", json!({ "query": query }), "r-1")
}
async fn tool_call(table: &OperationTable, query: &str, format: &str) -> Value {
    table
        .mcp(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "example_query",
                "arguments": {"arguments": {"query": query}, "request_id": "r-1", "format": format}
            }}),
            CancellationToken::new(),
        )
        .await["result"]
        .clone()
}

#[tokio::test]
async fn answer_once() {
    let table = table();
    let text = tool_call(&table, "rows", "text").await;
    let shown = text["content"][0]["text"].as_str().unwrap();
    assert_eq!(shown.matches("row-one-unique").count(), 1, "{shown}");
    assert!(text["structuredContent"].get("value").is_none());
    assert_eq!(text["structuredContent"]["count"]["value"], 3);
    // The text ends with what was left out, written as the tool call that reads it.
    assert!(shown.contains("shown 1 of 3"));
    assert!(shown.contains(
        r#"left out: 2 entries of rows; read them: example_query {"arguments":{"query":"rest"},"request_id":"page-2"}"#
    ));
    let json = tool_call(&table, "rows", "json").await;
    assert_eq!(
        json["structuredContent"]["value"]["rows"],
        json!(["row-one-unique"])
    );
    let shown = json["content"][0]["text"].as_str().unwrap();
    assert_eq!(shown.matches("row-one-unique").count(), 1);
}

#[tokio::test]
async fn lower_bound_under_gaps() {
    let table = table();
    let answer = table
        .call(find("gap"), Origin::Cli, CancellationToken::new())
        .await;
    assert_eq!(
        answer.envelope.count,
        Some(Count {
            value: 5,
            lower_bound: true
        })
    );
    let cli = table
        .cli(
            &["find", "--query", "gap"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    assert!(
        cli.stderr.contains("shown 1 of at least 5"),
        "{}",
        cli.stderr
    );
    assert!(cli.stderr.contains("coverage: shell gap (overflow)"));
}

#[tokio::test]
async fn degraded_names_missing() {
    let table = table();
    let answer = table
        .call(
            find("degraded-blank"),
            Origin::Cli,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(answer.failure().unwrap().kind, FailureKind::Io);
    let missing = json!({"outcome": "degraded", "value": 1});
    assert!(serde_json::from_value::<Answer<Value>>(missing).is_err());
    let named = Answer::degraded(json!(1), vec!["the shell source".into()]);
    let wire = serde_json::to_value(&named).unwrap();
    assert_eq!(wire["missing"], json!(["the shell source"]));
    assert_eq!(
        serde_json::from_value::<Answer<Value>>(wire).unwrap(),
        named
    );
}

#[tokio::test]
async fn error_teaches() {
    let table = table();
    let cli = table
        .cli(
            &["find", "--query", "unknown"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(cli.exit_code, 2);
    assert_eq!(
        cli.stderr,
        concat!(
            "error (unknown_field): unknown field \"reop\" for sessions\n",
            "sessions where reop:sinex\n",
            "               ^^^^^^^^^^\n",
            "did you mean: sessions where repo:sinex\n",
            "candidates: repo · role\n",
        )
    );
    let mcp = tool_call(&table, "unknown", "text").await;
    assert_eq!(mcp["isError"], true);
    let error = &mcp["structuredContent"]["error"];
    assert_eq!(error["kind"], "unknown_field");
    assert_eq!(error["position"], json!({"offset": 15, "length": 10}));
    assert_eq!(
        error["suggestion"],
        json!({"query": "sessions where repo:sinex"})
    );
    assert_eq!(error["candidates"], json!(["repo", "role"]));
}

#[test]
fn query_error_converts_once() {
    use query_language::{Error, ErrorKind};
    for (kind, expected) in [
        (ErrorKind::InvalidQuery, FailureKind::InvalidQuery),
        (ErrorKind::UnknownField, FailureKind::UnknownField),
        (ErrorKind::BadValue, FailureKind::BadValue),
        (ErrorKind::NotAQuery, FailureKind::NotAQuery),
        (ErrorKind::Ambiguous, FailureKind::Ambiguous),
        (ErrorKind::StaleCursor, FailureKind::StaleCursor),
    ] {
        let failure = Failure::from(Error::new(kind, "synthetic", 3).hint("a hint"));
        assert_eq!(failure.kind, expected);
        assert_eq!(failure.kind.name(), kind.code());
        assert_eq!(
            failure.position,
            Some(InputSpan {
                offset: 3,
                length: 0
            })
        );
        assert_eq!(failure.hint.as_deref(), Some("a hint"));
        assert_eq!(failure.suggestion(), None);
    }
}

#[test]
fn caret_marks_the_span_on_its_own_line() {
    let input = "first line\nsessions where λreop:x\nlast";
    let offset = input.find("λreop").unwrap();
    assert_eq!(
        caret(
            input,
            InputSpan {
                offset,
                length: "λreop".len()
            }
        ),
        "sessions where λreop:x\n               ^^^^^"
    );
    assert_eq!(
        caret(
            "abc",
            InputSpan {
                offset: 3,
                length: 0
            }
        ),
        "abc\n   ^"
    );
}

#[tokio::test]
async fn caller_is_echoed_in_every_answer() {
    let table = table();
    let authorship = Authorship {
        author: Author::Agent(AgentSession::parse("codex:thread").unwrap()),
        via: Via::Mcp,
    };
    let answer = table
        .call_as(
            find("rows"),
            Origin::Mcp,
            Caller::authenticated("codex:thread", ()).with_authorship(authorship.clone()),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(answer.envelope.caller, Some(authorship));
    let wire = serde_json::to_value(&answer).unwrap();
    assert_eq!(
        wire["caller"],
        json!({"author": "agent:codex:thread", "via": "mcp"})
    );
}

#[test]
fn answer_wire_keeps_outcome_value_and_envelope_apart() {
    for answer in [
        Answer::ok(Value::Null),
        Answer::empty(json!([])),
        Answer::degraded(json!([1]), vec!["one source".into()]),
        Answer::error(Failure::new(FailureKind::NotFound, "missing")),
    ] {
        let mut answer = answer.with_note("a note");
        answer.envelope.read_as = Some("rows in the synthetic unit".into());
        answer.envelope.positions = vec![ReadPosition {
            product: "example".into(),
            position: 12,
            lag: Some(0),
        }];
        answer.envelope.used_tokens = Some(40);
        let wire = serde_json::to_value(&answer).unwrap();
        assert_eq!(wire["outcome"], answer.outcome.name());
        assert_eq!(
            serde_json::from_value::<Answer<Value>>(wire).unwrap(),
            answer
        );
    }
    let error_with_value = json!({"outcome": "error", "error": {"kind": "io", "message": "x", "retryable": false}, "value": [1]});
    assert!(serde_json::from_value::<Answer<Value>>(error_with_value).is_err());
    let ok_without_value = json!({"outcome": "ok"});
    assert!(serde_json::from_value::<Answer<Value>>(ok_without_value).is_err());
}

#[test]
fn unmeasured_never_zero() {
    let missing: Vec<ValueState<u64>> = vec![
        ValueState::Unknown {
            reason: "timed out".into(),
        },
        ValueState::NotRecorded,
        ValueState::NotApplicable,
        ValueState::Pending,
        ValueState::Unavailable {
            reason: "offline".into(),
        },
    ];
    for state in missing {
        assert_eq!(state.value(), None);
        assert!(!state.text().contains(char::is_numeric), "{}", state.text());
    }
    assert_eq!(ValueState::Complete { value: 0 }.text(), "0");
    let ratio = Number::ratio(
        3.0,
        Denominator {
            count: 0,
            unit: "sessions".into(),
            query: "sessions since:7d".into(),
        },
        "usd",
    );
    assert_eq!(ratio.value.reason(), Some("zero denominator"));
    let wire = serde_json::to_value(&ratio).unwrap();
    assert_eq!(wire["state"], "unknown");
    assert!(wire.get("value").is_none());
    assert_eq!(serde_json::from_value::<Number>(wire).unwrap(), ratio);
    let forged = json!({"state": "unknown", "reason": "x", "value": 0, "unit": "usd"});
    assert!(serde_json::from_value::<Number>(forged).is_err());
    let partial = json!({"state": "partial", "value": 3.0, "unknown_count": 2, "unit": "sessions"});
    let number: Number = serde_json::from_value(partial).unwrap();
    assert_eq!(number.value.text(), "≥ 3 (2 unknown)");
}

#[test]
fn coverage_states_round_trip() {
    for state in [
        CoverageState::Complete,
        CoverageState::Gap {
            reason: "overflow".into(),
        },
        CoverageState::Unknown,
        CoverageState::OutsideSpan,
        CoverageState::NotYet,
        CoverageState::Excluded,
        CoverageState::Unavailable {
            cause: "offline".into(),
        },
    ] {
        let source = SourceCoverage {
            source: "shell".into(),
            state: state.clone(),
            not_complete: vec![CoverageInterval {
                start: "2026-11-01T10:00:00Z".parse().unwrap(),
                end: "2026-11-01T11:00:00Z".parse().unwrap(),
                state: state.clone(),
            }],
        };
        let wire = serde_json::to_value(&source).unwrap();
        assert_eq!(
            serde_json::from_value::<SourceCoverage>(wire).unwrap(),
            source
        );
        assert_eq!(source.is_complete(), state.is_complete());
    }
}
