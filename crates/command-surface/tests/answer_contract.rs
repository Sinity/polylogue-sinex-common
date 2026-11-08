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
fn search_spec() -> OperationSpec {
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
fn search_render_text(rows: &Rows) -> String {
    rows.rows.iter().map(|row| format!("{row}\n")).collect()
}
impl Operation for Search {
    type Request = Find;
    type Response = Rows;

    fn execute(
        &self,
        request: Find,
        _context: RequestContext,
    ) -> OperationFuture<'_, Answer<Rows>> {
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
                "degraded" => {
                    let mut answer = Answer::degraded(rows, vec!["the shell source".into()]);
                    answer.envelope.count = Some(Count::exact(4));
                    answer.envelope.facets = vec![Facet {
                        field: "repo".into(),
                        counts: vec![FacetCount {
                            value: "sinex".into(),
                            count: 4,
                        }],
                        lower_bound: false,
                    }];
                    answer
                }
                "long" => Answer::ok(Rows {
                    rows: (0..100)
                        .map(|n| format!("row {n} of a long answer"))
                        .collect(),
                }),
                "unknown" => {
                    let query = "sessions where reop:sinex";
                    let error = query_language::Error::new(
                        query_language::ErrorKind::UnknownField,
                        "unknown field \"reop\" for sessions",
                        15,
                    )
                    .length(10)
                    .suggestion("sessions where repo:sinex")
                    .candidates(vec!["repo".into(), "role".into()]);
                    Answer::error(Failure::from_query_error(error, query))
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
}

/// An operation fits its typed logical entries before the transport renders them.
struct PackingSearch;
fn packing_search_spec() -> OperationSpec {
    let mut spec = search_spec();
    spec.default_budget_tokens = Some(120);
    spec
}
impl Operation for PackingSearch {
    type Request = Find;
    type Response = Vec<Section>;

    fn execute(
        &self,
        request: Find,
        context: RequestContext,
    ) -> OperationFuture<'_, Answer<Vec<Section>>> {
        Box::pin(async move {
            let count = if request.query == "long" { 100 } else { 40 };
            let degraded = request.query == "degraded";
            let cap = (request.query == "capped").then_some(180);
            let envelope = Envelope {
                count: Some(Count::exact(count)),
                coverage: vec![SourceCoverage {
                    source: "shell".into(),
                    state: CoverageState::Gap {
                        reason: "overflow".into(),
                    },
                    not_complete: vec![],
                }],
                ..Envelope::default()
            };
            let section = Section {
                title: "rows".into(),
                status: SectionStatus::Complete,
                pending: false,
                entries: (0..count)
                    .map(|n| {
                        let mut lines = vec![format!("row {n} of a long answer")];
                        if request.query == "long" {
                            lines.push(format!("  detail {n} belongs to that row"));
                        }
                        Entry::with_lines(lines, vec![format!("ref:{n}")]).unwrap()
                    })
                    .collect(),
                expand: Some(RunnableRequest::new(
                    "find",
                    json!({"query":request.query}),
                    "more",
                )),
            };
            let outcome = if degraded {
                Outcome::Degraded {
                    value: vec![section],
                    missing: vec!["mail is offline".into()],
                }
            } else {
                Outcome::Ok(vec![section])
            };
            context.fit(Answer { outcome, envelope }, context.budget(60, cap))
        })
    }
}
fn table() -> OperationTable {
    table_with(Search, search_spec(), search_render_text)
}
fn table_with<O: Operation<Request = Find>>(
    operation: O,
    spec: OperationSpec,
    render: fn(&O::Response) -> String,
) -> OperationTable {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    for option in RequestOption::ALL {
        catalog.accept(option);
    }
    catalog
        .declare_tool(ToolFamily {
            name: "query".into(),
            description: "Search rows".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    catalog.declare::<Find, O::Response>(spec, render).unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table.bind("find", operation).unwrap();
    table
}
fn packing_table() -> OperationTable {
    table_with(PackingSearch, packing_search_spec(), |sections| {
        render_sections(sections)
    })
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
async fn once() {
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
    // "Rows travel as data only with `format: json`. Over MCP the text is
    // the model's view and the envelope is `structuredContent`; the rows and
    // the text are never both sent, so a model reads each row once."
    // (README, Answers: Text and data.)
    let shown = json["content"][0]["text"].as_str().unwrap();
    assert_eq!(shown.matches("row-one-unique").count(), 0, "{shown}");
    assert!(shown.contains("shown 1 of 3"));
}

#[tokio::test]
async fn text_answers_read_back() {
    let table = table();
    let text = tool_call(&table, "rows", "text").await;
    let structured = text["structuredContent"].clone();
    assert_eq!(structured["value_in_text"], true);
    let read: Answer<Option<Rows>> = serde_json::from_value(structured.clone()).unwrap();
    assert_eq!(read.value(), Some(&None));
    assert!(read.envelope.value_in_text);
    assert_eq!(read.envelope.count, Some(Count::exact(3)));
    // A reader that expects the rows is told where they are.
    let error = serde_json::from_value::<Answer<Rows>>(structured)
        .unwrap_err()
        .to_string();
    assert!(error.contains("text part"), "{error}");
    let json = tool_call(&table, "rows", "json").await;
    let read: Answer<Option<Rows>> =
        serde_json::from_value(json["structuredContent"].clone()).unwrap();
    assert_eq!(
        read.value(),
        Some(&Some(Rows {
            rows: vec!["row-one-unique".into()]
        }))
    );
}

#[tokio::test]
async fn text_within_budget() {
    let table = packing_table();
    let call = |query: &str, budget: u32| {
        table.mcp(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "example_query",
                "arguments": {"arguments": {"query": query}, "request_id": "r-1", "budget_tokens": budget}
            }}),
            CancellationToken::new(),
        )
    };
    // An operation that fits through its context fits once dispatch has
    // finished the envelope: the count's line and its lower bound included.
    let packed = call("packed", 60).await["result"].clone();
    assert_eq!(packed["isError"], false);
    let text = packed["content"][0]["text"].as_str().unwrap();
    assert!(approximate_tokens(text) <= 60, "{text}");
    assert!(text.contains("shown "), "{text}");
    assert!(text.contains("of at least 40"), "{text}");
    assert!(!text.contains("lines of this answer"), "{text}");
    assert_eq!(packed["structuredContent"]["left_out"][0]["what"], "rows");
    // The operation fits complete two-line entries. Rendering never cuts one
    // of those entries or changes the structured shown count afterward.
    let over = call("long", 100).await["result"].clone();
    assert_eq!(over["isError"], false);
    assert_eq!(over["structuredContent"]["outcome"], "ok");
    let text = over["content"][0]["text"].as_str().unwrap();
    assert!(approximate_tokens(text) <= 100, "{text}");
    assert!(text.starts_with("## rows\nrow 0 of a long answer\n  detail 0 belongs to that row\n"));
    let left = &over["structuredContent"]["left_out"][0];
    assert_eq!(left["what"], "rows");
    assert!(left["count"].as_u64().unwrap() > 0);
    assert_eq!(left["expand"]["budget_tokens"], Value::Null);
    let shown = over["structuredContent"]["shown"].as_u64().unwrap();
    assert_eq!(left["count"].as_u64().unwrap(), 100 - shown);
    assert_eq!(
        text.lines().filter(|line| line.starts_with("row ")).count() as u64,
        shown
    );
    assert_eq!(
        text.lines()
            .filter(|line| line.starts_with("  detail "))
            .count() as u64,
        shown
    );
    assert_eq!(
        over["structuredContent"]["used_tokens"],
        json!(approximate_tokens(text))
    );
    // The command line takes the same path.
    let cli = table
        .cli(
            &["find", "--query", "long", "--budget", "100"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(cli.exit_code, 0);
    assert!(
        cli.stdout
            .starts_with("## rows\nrow 0 of a long answer\n  detail 0 belongs to that row\n")
    );
    assert!(cli.stderr.contains("left out: "), "{}", cli.stderr);
    assert!(approximate_tokens(&cli.stdout) + approximate_tokens(&cli.stderr) <= 100);
    // A product's client renders an answer it read from the socket the
    // same way.
    let request = RunnableRequest {
        budget_tokens: command_surface::TokenBudget::Tokens(100),
        ..find("long")
    };
    let read = table
        .call(request.clone(), Via::Cli, CancellationToken::new())
        .await;
    let rendered = table.render_answer(Via::Cli, Some(&request), read);
    assert_eq!(rendered.value_text, cli.stdout);
    assert_eq!(rendered.footer, cli.stderr);
}

#[tokio::test]
async fn degraded_counts_are_lower_bounds() {
    let table = table();
    let answer = table
        .call(find("degraded"), Via::Cli, CancellationToken::new())
        .await;
    assert!(answer.envelope.count.unwrap().lower_bound);
    assert!(answer.envelope.facets[0].lower_bound);
    let complete = table
        .call(find("rows"), Via::Cli, CancellationToken::new())
        .await;
    assert!(!complete.envelope.count.unwrap().lower_bound);
}

#[tokio::test]
async fn lower_bound_under_gaps() {
    let table = table();
    let answer = table
        .call(find("gap"), Via::Cli, CancellationToken::new())
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
        .call(find("degraded-blank"), Via::Cli, CancellationToken::new())
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
        let error = Error::new(kind, "synthetic", 4)
            .length(6)
            .hint("a hint")
            .candidates(vec!["near".into()]);
        let failure = Failure::from_query_error(error, "abc reop:x more");
        assert_eq!(failure.kind, expected);
        assert_eq!(failure.kind.name(), kind.code());
        assert_eq!(
            failure.position,
            Some(InputSpan {
                offset: 4,
                length: 6
            })
        );
        assert_eq!(failure.caret(), Some("abc reop:x more\n    ^^^^^^"));
        assert_eq!(failure.candidates(), ["near"]);
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
            Via::Mcp,
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
        answer.envelope.positions = vec![ReadAt {
            product: "example".into(),
            position: Position::new("g1", 12),
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

#[test]
fn coverage_footer_uses_the_typed_state_and_detail() {
    let mut answer = Answer::ok(Value::Null);
    answer.envelope.coverage = vec![SourceCoverage {
        source: "terminal".into(),
        state: CoverageState::Unavailable {
            cause: "offline".into(),
        },
        not_complete: vec![CoverageInterval {
            start: "2026-11-01T10:00:00Z".parse().unwrap(),
            end: "2026-11-01T11:00:00Z".parse().unwrap(),
            state: CoverageState::Gap {
                reason: "overflow".into(),
            },
        }],
    }];
    let rendered = table().render_answer(Via::Cli, None, answer);
    assert!(
        rendered
            .footer
            .contains("coverage: terminal unavailable (offline)"),
        "{}",
        rendered.footer
    );
    assert!(rendered.footer.contains("gap"), "{}", rendered.footer);
    assert!(!rendered.footer.contains("complete"), "{}", rendered.footer);
}

#[tokio::test]
async fn expanding_a_default_budget_reads_the_whole_logical_entries() {
    let table = packing_table();
    let first = table
        .call(find("long"), Via::Mcp, CancellationToken::new())
        .await;
    assert!(first.envelope.shown.unwrap() < 100);
    let expand = first.envelope.left_out[0].expand.clone().unwrap();
    assert_eq!(expand.budget_tokens, TokenBudget::Unlimited);
    let follow = table
        .call(expand.clone(), Via::Mcp, CancellationToken::new())
        .await;
    assert_eq!(follow.envelope.shown, Some(100));
    assert!(follow.envelope.left_out.is_empty());
    let rendered = table.render_answer(Via::Mcp, Some(&expand), follow);
    assert!(
        rendered
            .value_text
            .contains("row 99 of a long answer\n  detail 99 belongs to that row\n")
    );
    let wire = table
        .mcp(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"example_query","arguments":serde_json::to_value(&expand).unwrap()
            }}),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(wire["result"]["structuredContent"]["shown"], 100);
    assert!(
        wire["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("detail 99 belongs to that row")
    );
    let cli = table
        .cli(
            &["find", "--query", "long", "--budget", "none"].map(str::to_owned),
            "r-cli",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(cli.exit_code, 0, "{}", cli.stderr);
    assert!(cli.stdout.contains("detail 99 belongs to that row"));
}

#[test]
fn rendering_preserves_an_operations_complete_response() {
    let mut request = find("long");
    request.budget_tokens = TokenBudget::Tokens(1);
    let rows = Rows {
        rows: vec!["a whole entry\nwith its detail".into()],
    };
    let answer = Answer::ok(serde_json::to_value(rows).unwrap());
    let rendered = table().render_answer(Via::Cli, Some(&request), answer);
    assert_eq!(rendered.value_text, "a whole entry\nwith its detail\n");
    assert!(rendered.answer.envelope.left_out.is_empty());
}

#[tokio::test]
async fn a_text_budget_does_not_cut_json_rows() {
    let table = packing_table();
    let mut request = find("long");
    request.budget_tokens = TokenBudget::Tokens(1);
    request.format = Format::Json;
    let answer = table
        .call(request.clone(), Via::Mcp, CancellationToken::new())
        .await;
    let sections: Vec<Section> = serde_json::from_value(answer.value().unwrap().clone()).unwrap();
    assert_eq!(
        sections
            .iter()
            .map(|section| section.entries.len())
            .sum::<usize>(),
        100
    );
    assert_eq!(answer.envelope.shown, Some(100));
    assert!(answer.envelope.left_out.is_empty());
    let rendered = table.render_answer(Via::Mcp, Some(&request), answer);
    assert!(rendered.value_text.is_empty());
}

#[tokio::test]
async fn fitting_preserves_degradation_and_reserves_the_cap_note() {
    let table = packing_table();
    let degraded = table
        .call(find("degraded"), Via::Mcp, CancellationToken::new())
        .await;
    assert!(
        matches!(degraded.outcome,Outcome::Degraded {ref missing,..} if missing==&["mail is offline"])
    );
    let rendered = table.render_answer(Via::Mcp, Some(&find("degraded")), degraded);
    assert!(approximate_tokens(&format!("{}{}", rendered.value_text, rendered.footer)) <= 120);
    let mut capped = find("capped");
    capped.budget_tokens = TokenBudget::Unlimited;
    let answer = table
        .call(capped.clone(), Via::Mcp, CancellationToken::new())
        .await;
    assert!(
        answer
            .envelope
            .notes
            .iter()
            .any(|note| note.contains("unlimited to 180"))
    );
    assert!(answer.envelope.shown.unwrap() < 40);
    let rendered = table.render_answer(Via::Mcp, Some(&capped), answer);
    assert!(approximate_tokens(&format!("{}{}", rendered.value_text, rendered.footer)) <= 180);
}
