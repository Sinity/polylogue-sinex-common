use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Read {
    text: String,
    #[serde(default)]
    wait: bool,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Rows {
    rows: Vec<String>,
    committed: u64,
}
struct Sample {
    writes: Arc<AtomicU64>,
    active: Arc<AtomicU64>,
    write: bool,
}
struct Active(Arc<AtomicU64>);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
impl Operation for Sample {
    type Request = Read;
    type Response = Rows;
    fn spec(&self) -> OperationSpec {
        let (name, tool) = if self.write {
            ("record", "write")
        } else {
            ("read", "rows")
        };
        OperationSpec {
            failures: vec![FailureKind::BadValue],
            access: if self.write {
                Access::Write
            } else {
                Access::Read
            },
            ..OperationSpec::request(
                name,
                &[name],
                tool,
                "Read synthetic rows",
                "Read a reference's typed rows",
            )
        }
    }
    fn execute(&self, request: Read, context: RequestContext) -> OperationFuture<'_, Answer<Rows>> {
        Box::pin(async move {
            self.active.fetch_add(1, Ordering::SeqCst);
            let _active = Active(self.active.clone());
            context.progress("scan", 42, None).unwrap();
            if request.wait {
                std::future::pending::<()>().await;
            }
            if request.text == "invalid" {
                return Answer::error(Failure::new(FailureKind::BadValue, "synthetic bad value"));
            }
            if request.text == "product-error" {
                let mut error = Failure::new(FailureKind::BadValue, "synthetic product error");
                error.code = Some("invalid_sample".into());
                return Answer::error(error);
            }
            if request.text == "identity" {
                return Answer::ok(Rows {
                    rows: vec![
                        context
                            .caller()
                            .identity::<String>()
                            .cloned()
                            .unwrap_or_default(),
                    ],
                    committed: 0,
                });
            }
            let committed = if self.write {
                self.writes.fetch_add(1, Ordering::SeqCst) + 1
            } else {
                0
            };
            let rows = Rows {
                rows: if request.text == "empty" {
                    vec![]
                } else {
                    vec![request.text.clone()]
                },
                committed,
            };
            match request.text.as_str() {
                "empty" => Answer::empty(rows),
                "degraded" => Answer::degraded(rows, vec!["one synthetic source".into()]),
                _ => Answer::ok(rows),
            }
        })
    }
    fn render_text(&self, response: &Rows) -> String {
        format!("{}\n", response.rows.join("\n"))
    }
}
fn table() -> (OperationTable, Arc<AtomicU64>, Arc<AtomicU64>) {
    let writes = Arc::new(AtomicU64::new(0));
    let active = Arc::new(AtomicU64::new(0));
    let mut deadlines = Deadlines::default();
    deadlines.set(LatencyClass::Read, Some(Duration::from_millis(10)));
    let mut table = OperationTable::new("example", deadlines);
    for (name, description) in [("rows", "Read synthetic rows"), ("write", "Record rows")] {
        table
            .declare_tool(ToolGroup {
                name: name.into(),
                description: description.into(),
                open_world: false,
            })
            .unwrap();
    }
    for write in [false, true] {
        table
            .register(Sample {
                writes: writes.clone(),
                active: active.clone(),
                write,
            })
            .unwrap();
    }
    (table, writes, active)
}
fn request(text: &str) -> RunnableRequest {
    RunnableRequest {
        operation: "read".into(),
        arguments: json!({"text":text}),
        request_id: "r-1".into(),
        deadline: Deadline::None,
        budget_tokens: None,
    }
}
async fn http(table: &OperationTable, request: &RunnableRequest) -> http::Response<Vec<u8>> {
    table
        .http(
            http::Request::post("/v1/read")
                .header("content-type", "application/json")
                .body(serde_json::to_vec(request).unwrap())
                .unwrap(),
            CancellationToken::new(),
        )
        .await
}
async fn socket(table: &OperationTable, request: &RunnableRequest) -> Answer<Value> {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let serving = table.serve_socket(server);
    let calling = table.socket_call(client, request, CancellationToken::new());
    let (answer, result) = tokio::join!(calling, serving);
    result.unwrap();
    answer
}
/// The MCP call of `request` asking for rows as JSON, so its structured
/// content is the whole answer.
async fn mcp(table: &OperationTable, request: &RunnableRequest) -> Value {
    let mut arguments = serde_json::to_value(request).unwrap();
    arguments.as_object_mut().unwrap().remove("operation");
    arguments["format"] = json!("json");
    table.mcp(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"example_rows","arguments":arguments}}),CancellationToken::new()).await
}

#[tokio::test]
async fn surface_parity_and_typed_output_schema() {
    let (table, _, _) = table();
    for text in ["hello", "invalid", "001", "empty", "degraded"] {
        let request = request(text);
        let expected = table
            .call(request.clone(), Origin::Socket, CancellationToken::new())
            .await;
        let cli = table
            .cli(
                &["read", "--text", text, "--deadline", "none", "--json"].map(str::to_string),
                "r-1",
                CancellationToken::new(),
            )
            .await;
        assert_eq!(
            serde_json::from_str::<Answer<Value>>(&cli.stdout).unwrap(),
            expected
        );
        assert_eq!(cli.stderr, "");
        assert_eq!(cli.exit_code, expected.exit_code());
        assert_eq!(socket(&table, &request).await, expected);
        let http = http(&table, &request).await;
        assert_eq!(http.status().as_u16(), expected.http_status().unwrap());
        assert_eq!(
            serde_json::from_slice::<Answer<Value>>(http.body()).unwrap(),
            expected
        );
        let mcp = mcp(&table, &request).await;
        assert_eq!(
            mcp["result"]["structuredContent"],
            serde_json::to_value(&expected).unwrap()
        );
        assert_eq!(mcp["result"]["isError"], expected.is_error());
    }
    let tool = &table.mcp_tools()[0];
    assert_eq!(tool["name"], "example_rows");
    assert!(
        tool["outputSchema"]["$defs"]["Rows"]["properties"]
            .get("committed")
            .is_some()
    );
    assert!(
        tool["inputSchema"]["properties"]["arguments"]["properties"]
            .get("text")
            .is_some()
    );
    assert_eq!(table.complete("re"), vec!["read", "record"]);
}

#[tokio::test(start_paused = true)]
async fn timeout_never_partial_and_any_caller_lifts_deadline() {
    let (table, _, active) = table();
    let mut request = request("waiting");
    request.arguments["wait"] = json!(true);
    request.deadline = Deadline::Default;
    for origin in [Origin::Cli, Origin::Socket, Origin::Http, Origin::Mcp] {
        let answer = table
            .call(request.clone(), origin, CancellationToken::new())
            .await;
        let error = answer.failure().expect("deadline returned rows");
        assert_eq!(error.kind, FailureKind::Timeout);
        assert!(error.message.contains("42 rows"));
        assert_eq!(
            answer.envelope.next.last().unwrap().request.deadline,
            Deadline::None
        );
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
    request.deadline = Deadline::None;
    let cancellation = CancellationToken::new();
    let call = table.call(request.clone(), Origin::Mcp, cancellation.clone());
    tokio::pin!(call);
    tokio::select! {biased;_= &mut call=>panic!("lifted request ended"),_=tokio::time::sleep(Duration::from_secs(1000))=>{}}
    cancellation.cancel();
    let answer = call.await;
    assert_eq!(answer.exit_code(), 7);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(table.socket_timeout(&request), None);
    request.deadline = Deadline::Milliseconds(11);
    assert_eq!(
        table.socket_timeout(&request),
        Some(Duration::from_millis(61))
    );
}

#[tokio::test]
async fn cancel_stops_work_on_socket_disconnect() {
    use tokio::io::AsyncWriteExt;
    let (table, _, active) = table();
    let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
    let mut request = request("wait");
    request.arguments["wait"] = json!(true);
    let serving = table.serve_socket(server);
    let calling = async {
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        client.write_all(&bytes).await.unwrap();
        tokio::task::yield_now().await;
        drop(client);
    };
    let (result, ()) = tokio::join!(serving, calling);
    result.unwrap();
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn malformed_envelopes_have_one_failure_and_no_authorship() {
    let (table, _, _) = table();
    let mut value = serde_json::to_value(request("hello")).unwrap();
    value["author"] = json!("forged");
    assert!(serde_json::from_value::<RunnableRequest>(value).is_err());
    let mut request = request("hello");
    request.arguments["unknown"] = json!(true);
    let expected = table
        .call(request.clone(), Origin::Cli, CancellationToken::new())
        .await;
    assert_eq!(expected.exit_code(), 2);
    assert_eq!(socket(&table, &request).await, expected);
    assert_eq!(
        serde_json::from_slice::<Answer<Value>>(http(&table, &request).await.body()).unwrap(),
        expected
    );
    assert_eq!(
        mcp(&table, &request).await["result"]["structuredContent"],
        serde_json::to_value(expected).unwrap()
    );
    let unknown = table
        .mcp(
            json!({"id":8,"method":"tools/call","params":{"name":"absent"}}),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
}

#[test]
fn one_kind_one_exit_independent_table() {
    let expected = [
        ("invalid_query", 2, Some(400)),
        ("unknown_field", 2, Some(400)),
        ("bad_value", 2, Some(400)),
        ("not_a_query", 2, Some(400)),
        ("bad_request", 2, Some(400)),
        ("ambiguous", 2, Some(400)),
        ("stale_cursor", 2, Some(400)),
        ("not_built", 2, Some(501)),
        ("not_configured", 2, Some(501)),
        ("not_yet_available", 2, Some(501)),
        ("refused", 3, Some(403)),
        ("not_found", 4, Some(404)),
        ("conflict", 4, Some(409)),
        ("io", 4, Some(500)),
        ("stopped_early", 5, Some(500)),
        ("not_running", 6, None),
        ("timeout", 7, Some(504)),
        ("cancelled", 7, None),
    ];
    assert_eq!(FailureKind::ALL.len(), expected.len());
    for (kind, (name, exit, http)) in FailureKind::ALL.into_iter().zip(expected) {
        assert_eq!(kind.name(), name);
        assert_eq!(serde_json::to_value(kind).unwrap(), name);
        assert_eq!(kind.mapping(), (exit, http));
    }
    // Outcomes are not failures: empty and degraded answers carry a value.
    assert_eq!(Answer::empty(json!([])).exit_code(), 1);
    assert_eq!(
        Answer::degraded(json!([1]), vec!["one source".into()]).exit_code(),
        5
    );
    for outcome in ["empty", "degraded"] {
        assert!(serde_json::from_value::<FailureKind>(json!(outcome)).is_err());
    }
}

#[tokio::test(start_paused = true)]
async fn every_surface_times_out_without_partial_rows() {
    let (table, _, active) = table();
    let mut request = request("wait");
    request.arguments["wait"] = json!(true);
    request.deadline = Deadline::Default;
    let cli = table
        .cli(
            &["read", "--text", "wait", "--wait", "true", "--json"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    let expected: Answer<Value> = serde_json::from_str(&cli.stdout).unwrap();
    assert_eq!(cli.exit_code, 7);
    assert_eq!(socket(&table, &request).await, expected);
    let http = http(&table, &request).await;
    assert_eq!(http.status(), 504);
    assert_eq!(
        serde_json::from_slice::<Answer<Value>>(http.body()).unwrap(),
        expected
    );
    assert_eq!(
        mcp(&table, &request).await["result"]["structuredContent"],
        serde_json::to_value(&expected).unwrap()
    );
    assert_eq!(active.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn terminal_timeout_prints_runnable_lifted_followup() {
    let (table, _, _) = table();
    let cli = table
        .cli(
            &["read", "--text", "wait", "--wait", "true"].map(str::to_string),
            "request-identity",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(cli.exit_code, 7);
    assert!(cli.stdout.is_empty());
    assert!(cli.stderr.contains("error (timeout):"));
    assert!(
        cli.stderr
            .contains("--request-id request-identity --deadline none")
    );
    assert!(
        cli.stderr
            .contains("next: complete without a deadline: example read")
    );
}

#[tokio::test]
async fn ordinary_form_posts_use_the_same_request() {
    let (table, _, _) = table();
    let request = request("quote '&+ = λ");
    let body = serde_json::to_vec(&request)
        .unwrap()
        .iter()
        .map(|byte| format!("%{byte:02X}"))
        .collect::<String>();
    let response = table
        .http(
            http::Request::post("/v1/read")
                .header("content-type", "application/x-www-form-urlencoded")
                .body(format!("request={body}").into_bytes())
                .unwrap(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        serde_json::from_slice::<Answer<Value>>(response.body()).unwrap(),
        table
            .call(request, Origin::Cli, CancellationToken::new())
            .await
    );
    for invalid in [
        "request=%GG",
        "request=%",
        "author=x",
        "request=%FF",
        "request=x&request=x",
    ] {
        let response = table
            .http(
                http::Request::post("/v1/read")
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(invalid.as_bytes().to_vec())
                    .unwrap(),
                CancellationToken::new(),
            )
            .await;
        assert_eq!(response.status(), 400);
    }
}

#[tokio::test]
async fn trusted_identity_is_typed() {
    let (table, _, _) = table();
    let answer = table
        .call_as(
            request("identity"),
            Origin::Http,
            Caller::authenticated("person-a", "trusted author".to_string()),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(answer.value().unwrap()["rows"], json!(["trusted author"]));
}

#[tokio::test]
async fn product_failure_codes_have_one_registered_classification() {
    let (mut table, _, _) = table();
    let request = request("product-error");
    let unregistered = table
        .call(request.clone(), Origin::Cli, CancellationToken::new())
        .await;
    assert_eq!(unregistered.failure().unwrap().kind, FailureKind::Io);
    table
        .register_failure_code("invalid_sample", FailureKind::BadValue)
        .unwrap();
    assert!(
        table
            .register_failure_code("invalid_sample", FailureKind::Refused)
            .is_err()
    );
    let registered = table
        .call(request, Origin::Cli, CancellationToken::new())
        .await;
    let registered = registered.failure().unwrap();
    assert_eq!(registered.kind, FailureKind::BadValue);
    assert_eq!(registered.code.as_deref(), Some("invalid_sample"));
}

struct Echo;
impl Operation for Echo {
    type Request = Value;
    type Response = Value;
    fn spec(&self) -> OperationSpec {
        OperationSpec::request(
            "echo",
            &["echo"],
            "echo",
            "Echo typed JSON",
            "Exercise arbitrary JSON arguments",
        )
    }
    fn execute(&self, value: Value, _: RequestContext) -> OperationFuture<'_, Answer<Value>> {
        Box::pin(async move { Answer::ok(value) })
    }
    fn render_text(&self, value: &Value) -> String {
        value.to_string()
    }
}
#[tokio::test]
async fn arbitrary_typed_requests_are_reachable_and_string_flags_do_not_change_mode() {
    let (mut table, _, _) = table();
    table
        .declare_tool(ToolGroup {
            name: "echo".into(),
            description: "Echo".into(),
            open_world: false,
        })
        .unwrap();
    table.register(Echo).unwrap();
    for value in [
        json!(true),
        json!(null),
        json!(3),
        json!(["one", "two"]),
        json!("string"),
    ] {
        let cli = table
            .cli(
                &[
                    "echo".into(),
                    "--request".into(),
                    value.to_string(),
                    "--json".into(),
                ],
                "id",
                CancellationToken::new(),
            )
            .await;
        let answer: Answer<Value> = serde_json::from_str(&cli.stdout).unwrap();
        assert_eq!(answer.value(), Some(&value));
    }
    let cli = table
        .cli(
            &["read", "--text", "--json"].map(str::to_string),
            "id",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(cli.stdout, "--json\n");
}

#[tokio::test]
async fn socket_rejects_pipeline_without_running_the_second_request() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (table, writes, active) = table();
    let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
    let mut waiting = request("wait");
    waiting.arguments["wait"] = json!(true);
    let mut write = request("write");
    write.operation = "record".into();
    let client = async {
        client
            .write_all(
                format!(
                    "{}\n{}\n",
                    serde_json::to_string(&waiting).unwrap(),
                    serde_json::to_string(&write).unwrap()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut line = String::new();
        BufReader::new(client).read_line(&mut line).await.unwrap();
        let answer: Answer<Value> = serde_json::from_str(&line).unwrap();
        assert_eq!(answer.exit_code(), 2);
    };
    let (result, ()) = tokio::join!(table.serve_socket(server), client);
    result.unwrap();
    assert_eq!(writes.load(Ordering::SeqCst), 0);
    assert_eq!(active.load(Ordering::SeqCst), 0);
}
