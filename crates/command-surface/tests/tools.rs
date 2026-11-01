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
    for name in ["status", "query", "assert", "act"] {
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
            "example_act"
        ]
    );
    let query = &tools[1];
    assert_eq!(
        query["inputSchema"]["properties"]["operation"]["enum"],
        json!(["find", "diff"])
    );
    assert_eq!(
        query["inputSchema"]["properties"]["arguments"]["anyOf"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(query["inputSchema"]["required"][0], "operation");
    assert!(query["inputSchema"].get("oneOf").is_none());
    assert_eq!(query["annotations"]["readOnlyHint"], true);
    let status = &tools[0];
    assert!(
        status["inputSchema"]["properties"]
            .get("operation")
            .is_none()
    );
    assert_eq!(
        status["inputSchema"]["properties"]["arguments"]["title"],
        "status"
    );
    assert_eq!(tools[2]["annotations"]["readOnlyHint"], false);
    assert_eq!(tools[3]["annotations"]["openWorldHint"], true);
    for tool in &tools {
        assert!(tool["outputSchema"]["properties"].get("outcome").is_some());
    }
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
            Origin::Socket,
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
            .map(|t| t["inputSchema"]["properties"]["operation"]["enum"].as_array().map_or(1, Vec::len))
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
