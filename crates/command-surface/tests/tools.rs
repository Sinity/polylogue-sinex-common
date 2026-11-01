//! MCP tools grouped from the table, streams and local rows.
use command_surface::*;
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Text {
    text: String,
}

/// A row that echoes its text, declared with any spec.
struct Echo(OperationSpec);
impl Operation for Echo {
    type Request = Text;
    type Response = Text;
    fn spec(&self) -> OperationSpec {
        self.0.clone()
    }
    fn execute(&self, request: Text, _: RequestContext) -> OperationFuture<'_, Answer<Text>> {
        let name = self.0.name.clone();
        Box::pin(async move {
            Answer::ok(Text {
                text: format!("{name}: {}", request.text),
            })
        })
    }
    fn render_text(&self, text: &Text) -> String {
        format!("{}\n", text.text)
    }
}

/// Counts to the number in its text, one event each.
struct Follow;
impl StreamOperation for Follow {
    type Request = Text;
    type Event = u64;
    fn spec(&self) -> OperationSpec {
        OperationSpec {
            kind: OperationKind::Stream,
            ..OperationSpec::request("follow", &["follow"], "", "Follow changes", "A live stream")
        }
    }
    fn open(
        &self,
        request: Text,
        _: RequestContext,
    ) -> OperationFuture<'_, Result<tokio::sync::mpsc::Receiver<u64>, Failure>> {
        Box::pin(async move {
            let count: u64 = request
                .text
                .parse()
                .map_err(|_| Failure::new(FailureKind::BadValue, "not a count"))?;
            let (sender, receiver) = tokio::sync::mpsc::channel(8);
            for n in 1..=count {
                sender.try_send(n).unwrap();
            }
            Ok(receiver)
        })
    }
    fn render_event(&self, event: &u64) -> String {
        format!("event {event}\n")
    }
}

/// Says how its call arrived: the transport's channel and the MCP call's
/// own metadata, with a follow-up written for that channel.
struct WhoAmI;
impl Operation for WhoAmI {
    type Request = Text;
    type Response = Text;
    fn spec(&self) -> OperationSpec {
        OperationSpec::request(
            "whoami",
            &["whoami"],
            "inspect",
            "Say how this call came",
            "Shows the channel",
        )
    }
    fn execute(&self, _: Text, context: RequestContext) -> OperationFuture<'_, Answer<Text>> {
        Box::pin(async move {
            Answer::ok(Text {
                text: format!(
                    "{:?} {}",
                    context.via,
                    context.metadata().cloned().unwrap_or_default()
                ),
            })
            .with_next(
                "find more",
                RunnableRequest::new("find", json!({"text": "more"}), "r-2"),
            )
        })
    }
    fn render_text(&self, text: &Text) -> String {
        format!("{}\n", text.text)
    }
}

/// Sends what the test gives it, as it gives it.
struct Ticks(std::sync::Mutex<Option<tokio::sync::mpsc::Receiver<u64>>>);
impl StreamOperation for Ticks {
    type Request = Text;
    type Event = u64;
    fn spec(&self) -> OperationSpec {
        OperationSpec {
            kind: OperationKind::Stream,
            ..OperationSpec::request("ticks", &["ticks"], "", "Live ticks", "A live stream")
        }
    }
    fn open(
        &self,
        _: Text,
        _: RequestContext,
    ) -> OperationFuture<'_, Result<tokio::sync::mpsc::Receiver<u64>, Failure>> {
        let receiver = self.0.lock().unwrap().take();
        Box::pin(
            async move { receiver.ok_or_else(|| Failure::new(FailureKind::Io, "opened twice")) },
        )
    }
    fn render_event(&self, event: &u64) -> String {
        format!("tick {event}\n")
    }
}

fn row(name: &str, tool: &str, access: Access) -> Echo {
    Echo(OperationSpec {
        access,
        ..OperationSpec::request(
            name,
            &[name],
            tool,
            format!("Run {name}"),
            "A synthetic row",
        )
    })
}
fn tool(name: &str) -> ToolFamily {
    ToolFamily {
        name: name.into(),
        description: format!("The {name} tool"),
        open_world: name == "act",
    }
}

fn table() -> OperationTable {
    let mut table = OperationTable::new("example", Deadlines::default());
    for name in ["status", "query", "assert", "act", "inspect"] {
        table.declare_tool(tool(name)).unwrap();
    }
    table
        .register(row("status", "status", Access::Read))
        .unwrap();
    table.register(row("find", "query", Access::Read)).unwrap();
    table.register(row("diff", "query", Access::Read)).unwrap();
    table
        .register(row("note", "assert", Access::Write))
        .unwrap();
    table.register(row("ping", "act", Access::Write)).unwrap();
    table.register_stream(Follow).unwrap();
    table
        .register_local::<Text>(OperationSpec {
            kind: OperationKind::Local,
            http_route: None,
            listed: false,
            ..OperationSpec::request(
                "install",
                &["install"],
                "",
                "Install hooks",
                "Runs on the caller's machine",
            )
        })
        .unwrap();
    table.register(WhoAmI).unwrap();
    table
}

async fn call(table: &OperationTable, tool: &str, arguments: Value) -> Value {
    table
        .mcp(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": tool, "arguments": arguments}}),
            CancellationToken::new(),
        )
        .await
}

#[test]
fn tools_generated() {
    let table = table();
    laws::tools_generated(&table).unwrap();
    let tools = table.mcp_tools();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "example_status",
            "example_query",
            "example_assert",
            "example_act",
            "example_inspect"
        ]
    );
    let query = &tools[1];
    let input = &query["inputSchema"];
    // One flat set of arguments per family, `operation` naming the row.
    assert_eq!(
        input["properties"]["operation"]["enum"],
        json!(["find", "diff"])
    );
    assert_eq!(input["required"][0], "operation");
    assert_eq!(
        input["properties"]["arguments"],
        json!({"type": "object", "properties": {"text": {"type": "string"}}})
    );
    assert!(input.get("anyOf").is_none());
    assert_eq!(query["annotations"]["readOnlyHint"], true);
    let status = &tools[0];
    assert_eq!(
        status["inputSchema"]["properties"]["operation"]["enum"],
        json!(["status"])
    );
    assert_eq!(status["inputSchema"]["required"][0], "arguments");
    assert_eq!(tools[2]["annotations"]["readOnlyHint"], false);
    assert_eq!(tools[3]["annotations"]["openWorldHint"], true);
    // No tool carries an output schema; the answer's is served once.
    for tool in &tools {
        assert!(tool.get("outputSchema").is_none());
    }
    assert!(
        OperationTable::answer_schema()["properties"]
            .get("outcome")
            .is_some()
    );
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Counted {
    /// The text to count in.
    text: String,
    limit: u64,
    #[serde(default)]
    nested: Option<Inner>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct Inner {
    depth: u32,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Numbered {
    text: u64,
}
struct Typed<R>(&'static str, std::marker::PhantomData<R>);
impl<R> Operation for Typed<R>
where
    R: Serialize + serde::de::DeserializeOwned + JsonSchema + Send + Sync + 'static,
{
    type Request = R;
    type Response = Value;
    fn spec(&self) -> OperationSpec {
        OperationSpec::request(self.0, &[self.0], "count", "Count", "A synthetic row")
    }
    fn execute(&self, request: R, _: RequestContext) -> OperationFuture<'_, Answer<Value>> {
        let value = serde_json::to_value(request).unwrap();
        Box::pin(async move { Answer::ok(value) })
    }
    fn render_text(&self, value: &Value) -> String {
        format!("{value}\n")
    }
}

#[tokio::test]
async fn family_arguments_merge_flat_and_each_row_checks_its_own() {
    let mut table = OperationTable::new("example", Deadlines::default());
    table.declare_tool(tool("count")).unwrap();
    table
        .register(Typed::<Counted>("words", std::marker::PhantomData))
        .unwrap();
    table
        .register(Typed::<Numbered>("chars", std::marker::PhantomData))
        .unwrap();
    let tools = table.mcp_tools();
    let arguments = &tools[0]["inputSchema"]["properties"]["arguments"]["properties"];
    // A scalar keeps its type; a nested object is only its bare type; a
    // field two rows type differently is a choice that names its rows.
    assert_eq!(arguments["limit"], json!({"type": "integer"}));
    assert_eq!(arguments["nested"], json!({"type": "object"}));
    assert_eq!(
        arguments["text"],
        json!({"anyOf": [{"type": "string"}, {"type": "integer"}], "description": "typed per operation: words; chars"})
    );
    assert!(
        serde_json::to_string(&tools)
            .unwrap()
            .find("$defs")
            .is_none()
    );
    // The full schema of each row is served apart from the tool list.
    let full = table.family_schemas("count");
    assert!(
        full["words"]["$defs"]["Inner"]["properties"]
            .get("depth")
            .is_some()
    );
    assert_eq!(full["chars"]["properties"]["text"]["type"], "integer");
    // The flat schema admits a field of another row; dispatch refuses it.
    let crossed = call(
        &table,
        "example_count",
        json!({"operation": "chars", "arguments": {"text": 3, "limit": 1}, "request_id": "r-1"}),
    )
    .await;
    assert_eq!(
        crossed["result"]["structuredContent"]["error"]["kind"],
        "bad_request"
    );
    let typed = call(
        &table,
        "example_count",
        json!({"operation": "chars", "arguments": {"text": "three"}, "request_id": "r-1"}),
    )
    .await;
    assert_eq!(
        typed["result"]["structuredContent"]["error"]["kind"],
        "bad_request"
    );
    let fine = call(
        &table,
        "example_count",
        json!({"operation": "chars", "arguments": {"text": 3}, "request_id": "r-1"}),
    )
    .await;
    assert_eq!(fine["result"]["isError"], false);
}

/// A request type nested six levels deep, with an enum and a list at each.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Mode {
    Fast,
    Careful,
}
macro_rules! nested {
    ($name:ident, $inner:ty) => {
        #[derive(Clone, Serialize, Deserialize, JsonSchema)]
        struct $name {
            /// A long description that a tool list must not repeat for every level.
            label: String,
            mode: Mode,
            items: Vec<$inner>,
            child: Option<$inner>,
        }
    };
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
struct Leaf {
    value: u64,
}
nested!(Level5, Leaf);
nested!(Level4, Level5);
nested!(Level3, Level4);
nested!(Level2, Level3);
nested!(Level1, Level2);
nested!(Deep, Level1);

/// A row whose rarely used field is left out of the tool list.
struct Search;
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchRequest {
    text: String,
    #[serde(default)]
    sort_by_the_earliest_time_first: bool,
}
impl Operation for Search {
    type Request = SearchRequest;
    type Response = Value;
    fn spec(&self) -> OperationSpec {
        OperationSpec {
            arguments: vec![
                ArgumentSpec::new(
                    "sort_by_the_earliest_time_first",
                    ArgumentForm::Flag,
                    "oldest first",
                )
                .unlisted(),
            ],
            ..OperationSpec::request("search", &["search"], "count", "Search", "A synthetic row")
        }
    }
    fn execute(
        &self,
        request: SearchRequest,
        _: RequestContext,
    ) -> OperationFuture<'_, Answer<Value>> {
        Box::pin(async move { Answer::ok(json!(request.sort_by_the_earliest_time_first)) })
    }
    fn render_text(&self, value: &Value) -> String {
        format!("oldest first: {value}\n")
    }
}

#[tokio::test]
async fn unlisted_arguments_stay_off_the_list_and_still_dispatch() {
    let mut table = OperationTable::new("example", Deadlines::default());
    table.declare_tool(tool("count")).unwrap();
    table.register(Search).unwrap();
    let with_unlisted = serde_json::to_vec(&json!({ "tools": table.mcp_tools() }))
        .unwrap()
        .len();
    let arguments = &table.mcp_tools()[0]["inputSchema"]["properties"]["arguments"];
    assert_eq!(arguments["properties"], json!({"text": {"type": "string"}}));
    assert!(arguments.get("additionalProperties").is_none());
    // The size law counts only what is listed.
    laws::tools_list_within(&table, with_unlisted).unwrap();
    assert!(
        !String::from_utf8(serde_json::to_vec(&table.mcp_tools()).unwrap())
            .unwrap()
            .contains("earliest")
    );
    // The explain answer still has it, and a call that uses it runs.
    assert!(
        table.family_schemas("count")["search"]["properties"]
            .get("sort_by_the_earliest_time_first")
            .is_some()
    );
    let reply = call(
        &table,
        "example_count",
        json!({"arguments": {"text": "x", "sort_by_the_earliest_time_first": true}, "request_id": "r-1"}),
    )
    .await;
    assert_eq!(
        reply["result"]["content"][0]["text"],
        "oldest first: true\n"
    );
    // A mistyped unlisted field is refused by its row.
    let refused = call(
        &table,
        "example_count",
        json!({"arguments": {"text": "x", "sort_by_the_earliest_time_first": "yes"}, "request_id": "r-1"}),
    )
    .await;
    assert_eq!(
        refused["result"]["structuredContent"]["error"]["kind"],
        "bad_request"
    );
}

#[test]
fn tools_list_within() {
    let mut deep = OperationTable::new("example", Deadlines::default());
    deep.declare_tool(tool("count")).unwrap();
    deep.register(Typed::<Deep>("deep", std::marker::PhantomData))
        .unwrap();
    let full = serde_json::to_vec(&deep.family_schemas("count"))
        .unwrap()
        .len();
    let listed = serde_json::to_vec(&json!({ "tools": deep.mcp_tools() }))
        .unwrap()
        .len();
    assert!(full > 2 * listed, "full {full}, listed {listed}");
    laws::tools_list_within(&deep, 1024).unwrap();
    let arguments = &deep.mcp_tools()[0]["inputSchema"]["properties"]["arguments"]["properties"];
    assert_eq!(arguments["mode"], json!({"enum": ["fast", "careful"]}));
    assert_eq!(arguments["items"], json!({"type": "array"}));
    assert_eq!(arguments["child"], json!({"type": "object"}));

    let table = table();
    let bytes = serde_json::to_vec(&json!({ "tools": table.mcp_tools() }))
        .unwrap()
        .len();
    laws::tools_list_within(&table, bytes).unwrap();
    let error = laws::tools_list_within(&table, bytes - 1).unwrap_err();
    assert!(error.contains(&format!("{bytes} bytes")), "{error}");
    // Five families of synthetic rows stay small without output schemas.
    assert!(bytes < 4096, "{bytes}");
}

#[test]
fn tools_generated_refuses_an_empty_tool() {
    let mut table = table();
    table.declare_tool(tool("unused")).unwrap();
    assert!(laws::tools_generated(&table).is_err());
}

#[test]
fn one_tool_holds_one_access_level() {
    let mut table = table();
    let refused = table
        .register(row("forget", "query", Access::Write))
        .unwrap_err();
    assert!(
        refused.message.contains("read access"),
        "{}",
        refused.message
    );
    assert!(
        table
            .register(row("orphan", "missing", Access::Read))
            .is_err()
    );
    assert!(table.declare_tool(tool("query")).is_err());
}

#[tokio::test]
async fn tool_calls_name_their_operation() {
    let table = table();
    let diff = call(
        &table,
        "example_query",
        json!({"operation": "diff", "arguments": {"text": "a"}, "request_id": "r-1"}),
    )
    .await;
    assert_eq!(diff["result"]["content"][0]["text"], "diff: a\n");
    let single = call(
        &table,
        "example_status",
        json!({"arguments": {"text": "now"}, "request_id": "r-1"}),
    )
    .await;
    assert_eq!(single["result"]["content"][0]["text"], "status: now\n");
    for arguments in [
        json!({"arguments": {"text": "a"}, "request_id": "r-1"}),
        json!({"operation": "note", "arguments": {"text": "a"}, "request_id": "r-1"}),
        json!({"operation": "find", "arguments": {"text": "a"}, "request_id": "r-1", "author": "x"}),
    ] {
        let refused = call(&table, "example_query", arguments).await;
        assert_eq!(refused["result"]["isError"], true);
        assert_eq!(
            refused["result"]["structuredContent"]["error"]["kind"],
            "bad_request"
        );
    }
    let unknown = call(&table, "example_follow", json!({})).await;
    assert_eq!(unknown["error"]["code"], -32602);
}

#[tokio::test]
async fn streams_and_local_rows_are_not_tools() {
    let table = table();
    let names: Vec<String> = table
        .mcp_tools()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n.contains("follow") || n.contains("install"))
    );
    let local = table
        .call(
            RunnableRequest::new("install", json!({"text": "x"}), "r-1"),
            Via::Cli,
            CancellationToken::new(),
        )
        .await;
    assert_eq!(local.failure().unwrap().kind, FailureKind::BadRequest);
    let parsed = table
        .parse_cli(&["install".into(), "--text".into(), "x".into()], "r-1")
        .unwrap();
    assert_eq!(parsed.kind, OperationKind::Local);
    assert!(!table.help().contains("install"));
    assert!(
        table
            .help()
            .contains("example follow --text TEXT  Follow changes")
    );
}

#[tokio::test]
async fn streams_reach_the_command_line_and_the_socket() {
    let table = table();
    let cli = table
        .cli(
            &["follow", "--text", "3"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(cli.stdout, "event 1\nevent 2\nevent 3\n");
    assert_eq!(cli.exit_code, 0);
    let json = table
        .cli(
            &["follow", "--text", "2", "--json"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    let lines: Vec<StreamLine> = json
        .stdout
        .lines()
        .map(|line| StreamLine::parse(line).unwrap())
        .collect();
    assert_eq!(lines[0], StreamLine::Event(json!(1)));
    assert_eq!(lines[1], StreamLine::Event(json!(2)));
    assert!(matches!(&lines[2], StreamLine::End(answer) if !answer.is_error()));

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (mut client, server) = tokio::net::UnixStream::pair().unwrap();
    let request = RunnableRequest::new("follow", json!({"text": "2"}), "r-1");
    let reading = async {
        client
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        let mut reader = BufReader::new(client);
        let mut received = vec![];
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let parsed = StreamLine::parse(&line).unwrap();
            let end = matches!(parsed, StreamLine::End(_));
            received.push(parsed);
            if end {
                break;
            }
        }
        received
    };
    let ((), received) = tokio::join!(
        async {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                table.serve_socket(server),
            )
            .await;
        },
        reading
    );
    assert_eq!(received.len(), 3);
    assert_eq!(received[1], StreamLine::Event(json!(2)));

    let http = table
        .http(
            http::Request::post("/v1/follow")
                .header("content-type", "application/json")
                .body(serde_json::to_vec(&request).unwrap())
                .unwrap(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(http.status(), 400);
}

#[derive(Clone, Debug)]
struct RowPlan {
    tool: usize,
    kind: usize,
}

proptest! {
    #[test]
    fn tools_generated_holds_for_any_registered_table(
        plans in proptest::collection::vec((0usize..4, 0usize..3).prop_map(|(tool, kind)| RowPlan { tool, kind }), 1..12),
        writes in proptest::collection::vec(any::<bool>(), 4),
    ) {
        let mut table = OperationTable::new("generated", Deadlines::default());
        let tools = ["alpha", "beta", "gamma", "delta"];
        let used: std::collections::BTreeSet<usize> = plans.iter().filter(|p| p.kind == 0).map(|p| p.tool).collect();
        for &index in &used {
            table.declare_tool(tool(tools[index])).unwrap();
        }
        for (number, plan) in plans.iter().enumerate() {
            let name = format!("row{number}");
            match plan.kind {
                0 => {
                    let access = if writes[plan.tool] { Access::Write } else { Access::Read };
                    table.register(row(&name, tools[plan.tool], access)).unwrap();
                }
                1 => table
                    .register_local::<Text>(OperationSpec {
                        kind: OperationKind::Local,
                        http_route: None,
                        ..OperationSpec::request(name.as_str(), &[name.as_str()], "", "Local", "Local row")
                    })
                    .unwrap(),
                _ => {
                    let mut spec = OperationSpec::request(name.as_str(), &[name.as_str()], "", "Echo", "Generated");
                    spec.kind = OperationKind::Stream;
                    table.register(Echo(spec)).unwrap_err();
                }
            }
        }
        prop_assert_eq!(laws::tools_generated(&table), Ok(()));
        let reachable: usize = table
            .mcp_tools()
            .iter()
            .map(|t| t["inputSchema"]["properties"]["operation"]["enum"].as_array().map_or(0, Vec::len))
            .sum();
        prop_assert_eq!(reachable, plans.iter().filter(|p| p.kind == 0).count());
    }
}

#[tokio::test]
async fn commands_list_reads_back_as_descriptions() {
    let table = table();
    let response = table
        .http(
            http::Request::get("/v1/commands").body(vec![]).unwrap(),
            CancellationToken::new(),
        )
        .await;
    assert_eq!(response.status(), 200);
    let read: Vec<OperationDescription> = serde_json::from_slice(response.body()).unwrap();
    let specs: Vec<&OperationSpec> = read.iter().map(|d| &d.spec).collect();
    let expected: Vec<&OperationSpec> = table.descriptions().iter().map(|d| &d.spec).collect();
    assert_eq!(specs, expected);
    assert_eq!(read[5].spec.kind, OperationKind::Stream);
    assert_eq!(read[1].spec.tool(), Some("query"));
}

#[tokio::test]
async fn zero_event_stream_prints_nothing_and_succeeds() {
    let table = table();
    let cli = table
        .cli(
            &["follow", "--text", "0"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(
        (cli.stdout.as_str(), cli.stderr.as_str(), cli.exit_code),
        ("", "", 0)
    );
}

#[tokio::test]
async fn stream_lines_print_as_they_arrive() {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let mut table = OperationTable::new("example", Deadlines::default());
    table
        .register_stream(Ticks(std::sync::Mutex::new(Some(receiver))))
        .unwrap();
    let (mut stdout, reading) = tokio::io::duplex(1024);
    let mut stderr = Vec::new();
    let args = ["ticks".to_string(), "--text".into(), "x".into()];
    let running = table.cli_write(
        &args,
        "r-1",
        Caller::default(),
        CancellationToken::new(),
        &mut stdout,
        &mut stderr,
    );
    let watching = async {
        let mut lines = BufReader::new(reading).lines();
        sender.send(1).await.unwrap();
        // The first line arrives while the stream is still open.
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("tick 1"));
        sender.send(2).await.unwrap();
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("tick 2"));
        drop(sender);
    };
    let (exit, ()) = tokio::join!(running, watching);
    assert_eq!(exit.unwrap(), 0);
}

#[tokio::test]
async fn socket_carries_mcp_messages() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let table = table();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let talking = async {
        let (reading, mut writing) = client.into_split();
        let mut lines = BufReader::new(reading).lines();
        let call = json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "example_inspect",
            "arguments": {"arguments": {"text": "x"}, "request_id": "r-1"},
            "_meta": {"session": "codex:thread-3"}
        }});
        writing
            .write_all(format!("{call}\n").as_bytes())
            .await
            .unwrap();
        // A notification gets no reply; the next line answers the call.
        writing
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .unwrap();
        let reply: Value =
            serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        drop(writing);
        reply
    };
    let (served, reply) = tokio::join!(table.serve_socket(server), talking);
    served.unwrap();
    assert_eq!(reply["id"], 4);
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with(r#"Mcp {"session":"codex:thread-3"}"#),
        "{text}"
    );
    // Its follow-up is written as the tool call that runs it.
    assert!(
        text.contains(r#"next: find more: example_query {"arguments":{"text":"more"},"operation":"find","request_id":"r-2"}"#),
        "{text}"
    );
}
