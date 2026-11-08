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
struct Echo(String);
fn echo_render_text(text: &Text) -> String {
    format!("{}\n", text.text)
}
impl Operation for Echo {
    type Request = Text;
    type Response = Text;

    fn execute(&self, request: Text, _: RequestContext) -> OperationFuture<'_, Answer<Text>> {
        let name = self.0.clone();
        Box::pin(async move {
            Answer::ok(Text {
                text: format!("{name}: {}", request.text),
            })
        })
    }
}

/// Counts to the number in its text, one event each.
struct Follow;
fn follow_spec() -> OperationSpec {
    OperationSpec {
        kind: OperationKind::Stream,
        ..OperationSpec::request("follow", &["follow"], "", "Follow changes", "A live stream")
    }
}
fn follow_render_event(event: &u64) -> String {
    format!("event {event}\n")
}
impl StreamOperation for Follow {
    type Request = Text;
    type Event = u64;

    fn open(
        &self,
        request: Text,
        _: RequestContext,
    ) -> OperationFuture<'_, Result<EventReceiver<u64>, Failure>> {
        Box::pin(async move {
            let count: u64 = request
                .text
                .parse()
                .map_err(|_| Failure::new(FailureKind::BadValue, "not a count"))?;
            let (sender, receiver) = tokio::sync::mpsc::channel(8);
            for n in 1..=count {
                sender.try_send(Ok(n)).unwrap();
            }
            Ok(receiver)
        })
    }
}

/// Says how its call arrived: the transport's channel and the MCP call's
/// own metadata, with a follow-up written for that channel.
struct WhoAmI;
fn who_am_i_spec() -> OperationSpec {
    OperationSpec::request(
        "whoami",
        &["whoami"],
        "inspect",
        "Say how this call came",
        "Shows the channel",
    )
}
fn who_am_i_render_text(text: &Text) -> String {
    format!("{}\n", text.text)
}
impl Operation for WhoAmI {
    type Request = Text;
    type Response = Text;

    fn execute(&self, _: Text, context: RequestContext) -> OperationFuture<'_, Answer<Text>> {
        Box::pin(async move {
            Answer::ok(Text {
                text: format!(
                    "{:?} {} {}",
                    context.via,
                    context
                        .caller()
                        .authorship()
                        .map_or("unknown".into(), |a| a.author.to_string()),
                    context.metadata().cloned().unwrap_or_default()
                ),
            })
            .with_next(
                "find more",
                RunnableRequest::new("find", json!({"text": "more"}), "r-2"),
            )
        })
    }
}

/// Runs until it is cancelled.
struct Wait;
fn wait_spec() -> OperationSpec {
    OperationSpec::request("wait", &["wait"], "inspect", "Wait", "Never finishes")
}
fn wait_render_text(text: &Text) -> String {
    text.text.clone()
}
impl Operation for Wait {
    type Request = Text;
    type Response = Text;

    fn execute(&self, _: Text, _: RequestContext) -> OperationFuture<'_, Answer<Text>> {
        Box::pin(std::future::pending())
    }
}

/// Sends what the test gives it, as it gives it.
struct Ticks(std::sync::Mutex<Option<EventReceiver<u64>>>);
fn ticks_spec() -> OperationSpec {
    OperationSpec {
        kind: OperationKind::Stream,
        ..OperationSpec::request("ticks", &["ticks"], "", "Live ticks", "A live stream")
    }
}
fn ticks_render_event(event: &u64) -> String {
    format!("tick {event}\n")
}
impl StreamOperation for Ticks {
    type Request = Text;
    type Event = u64;

    fn open(
        &self,
        _: Text,
        _: RequestContext,
    ) -> OperationFuture<'_, Result<EventReceiver<u64>, Failure>> {
        let receiver = self.0.lock().unwrap().take();
        Box::pin(
            async move { receiver.ok_or_else(|| Failure::new(FailureKind::Io, "opened twice")) },
        )
    }
}

fn row(name: &str, tool: &str, access: Access) -> OperationSpec {
    OperationSpec {
        access,
        ..OperationSpec::request(
            name,
            &[name],
            tool,
            format!("Run {name}"),
            "A synthetic row",
        )
    }
}
const ECHO_ROWS: &[(&str, &str, Access)] = &[
    ("status", "status", Access::Read),
    ("find", "query", Access::Read),
    ("diff", "query", Access::Read),
    ("note", "assert", Access::Write),
    ("ping", "act", Access::Write),
];

fn tool(name: &str) -> ToolFamily {
    ToolFamily {
        name: name.into(),
        description: format!("The {name} tool"),
        open_world: name == "act",
        idempotent: name != "act",
    }
}

fn catalog() -> CatalogBuilder {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    for name in ["status", "query", "assert", "act", "inspect"] {
        catalog.declare_tool(tool(name)).unwrap();
    }
    for &(name, family, access) in ECHO_ROWS {
        catalog
            .declare::<Text, Text>(row(name, family, access), echo_render_text)
            .unwrap();
    }
    catalog
        .declare_stream::<Text, u64>(follow_spec(), follow_render_event)
        .unwrap();
    catalog
        .declare_local::<Text>(OperationSpec {
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
    catalog
        .declare::<Text, Text>(who_am_i_spec(), who_am_i_render_text)
        .unwrap();
    catalog
        .declare::<Text, Text>(wait_spec(), wait_render_text)
        .unwrap();
    catalog
}
fn table_with_options(options: impl IntoIterator<Item = RequestOption>) -> OperationTable {
    let mut catalog = catalog();
    for option in options {
        catalog.accept(option);
    }
    let mut table = OperationTable::new(catalog.finish());
    for &(name, _, _) in ECHO_ROWS {
        table.bind(name, Echo(name.into())).unwrap();
    }
    table.bind_stream("follow", Follow).unwrap();
    table.bind("whoami", WhoAmI).unwrap();
    table.bind("wait", Wait).unwrap();
    table
}
fn table() -> OperationTable {
    table_with_options([])
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
    let table = catalog().finish();
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
    // Hints are what each family declares, not derived.
    assert_eq!(tools[2]["annotations"]["idempotentHint"], true);
    assert_eq!(tools[3]["annotations"]["idempotentHint"], false);
    // No tool carries an output schema; the answer's is served once.
    for tool in &tools {
        assert!(tool.get("outputSchema").is_none());
    }
    assert!(
        OperationCatalog::answer_schema()["properties"]
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
struct Typed<R>(std::marker::PhantomData<R>);
fn typed_spec(name: &str) -> OperationSpec {
    OperationSpec::request(name, &[name], "count", "Count", "A synthetic row")
}
fn typed_render_text(value: &Value) -> String {
    format!("{value}\n")
}
impl<R> Operation for Typed<R>
where
    R: Serialize + serde::de::DeserializeOwned + JsonSchema + Send + Sync + 'static,
{
    type Request = R;
    type Response = Value;

    fn execute(&self, request: R, _: RequestContext) -> OperationFuture<'_, Answer<Value>> {
        let value = serde_json::to_value(request).unwrap();
        Box::pin(async move { Answer::ok(value) })
    }
}

#[tokio::test]
async fn family_arguments_merge_flat_and_each_row_checks_its_own() {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    catalog.declare_tool(tool("count")).unwrap();
    catalog
        .declare::<Counted, Value>(typed_spec("words"), typed_render_text)
        .unwrap();
    catalog
        .declare::<Numbered, Value>(typed_spec("chars"), typed_render_text)
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table
        .bind("words", Typed::<Counted>(std::marker::PhantomData))
        .unwrap();
    table
        .bind("chars", Typed::<Numbered>(std::marker::PhantomData))
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
fn search_spec() -> OperationSpec {
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
fn search_render_text(value: &Value) -> String {
    format!("oldest first: {value}\n")
}
impl Operation for Search {
    type Request = SearchRequest;
    type Response = Value;

    fn execute(
        &self,
        request: SearchRequest,
        _: RequestContext,
    ) -> OperationFuture<'_, Answer<Value>> {
        Box::pin(async move { Answer::ok(json!(request.sort_by_the_earliest_time_first)) })
    }
}

#[tokio::test]
async fn unlisted_arguments_stay_off_the_list_and_still_dispatch() {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    catalog.declare_tool(tool("count")).unwrap();
    catalog
        .declare::<SearchRequest, Value>(search_spec(), search_render_text)
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table.bind("search", Search).unwrap();
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
    let mut deep = OperationCatalog::builder("example", Deadlines::default());
    deep.declare_tool(tool("count")).unwrap();
    deep.declare::<Deep, Value>(typed_spec("deep"), typed_render_text)
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

    let table = catalog().finish();
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
    let mut table = catalog();
    table.declare_tool(tool("unused")).unwrap();
    assert!(laws::tools_generated(&table).is_err());
}

#[test]
fn one_tool_holds_one_access_level() {
    let mut table = catalog();
    let refused = table
        .declare::<Text, Text>(row("forget", "query", Access::Write), echo_render_text)
        .unwrap_err();
    assert!(
        refused.message.contains("read access"),
        "{}",
        refused.message
    );
    assert!(
        table
            .declare::<Text, Text>(row("orphan", "missing", Access::Read), echo_render_text)
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
        let mut table = OperationCatalog::builder("generated", Deadlines::default());
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
                    table.declare::<Text, Text>(row(&name, tools[plan.tool], access), echo_render_text).unwrap();
                }
                1 => table
                    .declare_local::<Text>(OperationSpec {
                        kind: OperationKind::Local,
                        http_route: None,
                        ..OperationSpec::request(name.as_str(), &[name.as_str()], "", "Local", "Local row")
                    })
                    .unwrap(),
                _ => {
                    let mut spec = OperationSpec::request(name.as_str(), &[name.as_str()], "", "Echo", "Generated");
                    spec.kind = OperationKind::Stream;
                    table.declare::<Text, Text>(spec, echo_render_text).unwrap_err();
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
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    catalog
        .declare_stream::<Text, u64>(ticks_spec(), ticks_render_event)
        .unwrap();
    let mut table = OperationTable::new(catalog.finish());
    table
        .bind_stream("ticks", Ticks(std::sync::Mutex::new(Some(receiver))))
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
        sender.send(Ok(1)).await.unwrap();
        // The first line arrives while the stream is still open.
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("tick 1"));
        sender.send(Ok(2)).await.unwrap();
        assert_eq!(lines.next_line().await.unwrap().as_deref(), Some("tick 2"));
        drop(sender);
    };
    let (exit, ()) = tokio::join!(running, watching);
    assert_eq!(exit.unwrap(), 0);
}

/// The evidence of a shared Codex MCP server, as a product's transport
/// gathers it for its socket peer.
fn shared_server() -> Caller {
    Caller::default().with_evidence(CallerEvidence {
        harness: Some(Harness::new("codex").unwrap()),
        session: AgentSession::parse("codex:server-start"),
        shared_server: true,
        ..CallerEvidence::default()
    })
}

/// Send `lines` over a socket served for `caller`, then read `replies`
/// lines back before closing.
async fn socket_exchange(
    table: &OperationTable,
    caller: Caller,
    lines: &[&str],
    replies: usize,
) -> Vec<Value> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let talking = async {
        let (reading, mut writing) = client.into_split();
        let mut read = BufReader::new(reading).lines();
        for line in lines {
            writing.write_all(line.as_bytes()).await.unwrap();
        }
        let mut received = vec![];
        for _ in 0..replies {
            received.push(serde_json::from_str(&read.next_line().await.unwrap().unwrap()).unwrap());
        }
        drop(writing);
        received
    };
    let (served, received) = tokio::join!(table.serve_socket_as(server, caller), talking);
    served.unwrap();
    received
}

fn whoami(id: u64, meta: Value) -> String {
    format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
            "name": "example_inspect",
            "arguments": {"operation": "whoami", "arguments": {"text": "x"}, "request_id": "r-1"},
            "_meta": meta
        }})
    )
}

#[tokio::test]
async fn socket_carries_mcp_messages() {
    let table = table();
    let call = whoami(4, json!({"session": "codex:thread-3"}));
    let replies = socket_exchange(
        &table,
        shared_server(),
        &[
            &call,
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n",
        ],
        1,
    )
    .await;
    let reply = &replies[0];
    assert_eq!(reply["id"], 4);
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    // The call is authored by the session its own metadata names, through
    // the same gathering as a stdio MCP server.
    assert!(
        text.starts_with(r#"Mcp agent:codex:thread-3 {"session":"codex:thread-3"}"#),
        "{text}"
    );
    assert_eq!(
        reply["result"]["structuredContent"]["caller"],
        json!({"author": "agent:codex:thread-3", "via": "mcp"})
    );
    // Its follow-up is written as the tool call that runs it.
    assert!(
        text.contains(r#"next: find more: example_query {"arguments":{"text":"more"},"operation":"find","request_id":"r-2"}"#),
        "{text}"
    );
    // Without its own identity, a shared server's call is the harness.
    let replies = socket_exchange(&table, shared_server(), &[&whoami(5, json!({}))], 1).await;
    let text = replies[0]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("Mcp agent:codex {}"), "{text}");
    // A stdio server calling `mcp_as` with the same evidence agrees.
    let stdio: Value =
        serde_json::from_str(&whoami(6, json!({"session": "codex:thread-3"}))).unwrap();
    let stdio = table
        .mcp_as(stdio, shared_server(), CancellationToken::new())
        .await;
    assert_eq!(
        stdio["result"]["structuredContent"]["caller"]["author"],
        "agent:codex:thread-3"
    );
}

#[tokio::test]
async fn socket_cancellation_answers_nothing_for_the_cancelled_call() {
    let table = table();
    let wait = format!(
        "{}\n",
        json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {
            "name": "example_inspect",
            "arguments": {"operation": "wait", "arguments": {"text": "x"}, "request_id": "r-9"}
        }})
    );
    let other = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":8}}\n";
    let cancel = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/cancelled\",\"params\":{\"requestId\":9}}\n";
    let next = whoami(10, json!({}));
    // A cancellation of another request leaves the call running; its own
    // cancels it, and the only reply is the next call's.
    let replies =
        socket_exchange(&table, Caller::default(), &[&wait, other, cancel, &next], 1).await;
    assert_eq!(replies[0]["id"], 10);
}

#[tokio::test]
async fn a_partial_line_does_not_hold_up_the_running_call() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let table = table();
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let talking = async {
        let (reading, mut writing) = client.into_split();
        let mut read = BufReader::new(reading).lines();
        // The call and the first half of a notification arrive together; the
        // call is answered while the notification's line is still open.
        let call = whoami(11, json!({}));
        writing
            .write_all(format!("{call}{{\"jsonrpc\":\"2.0\",\"meth").as_bytes())
            .await
            .unwrap();
        let reply: Value = serde_json::from_str(&read.next_line().await.unwrap().unwrap()).unwrap();
        writing
            .write_all(b"od\":\"notifications/initialized\"}\n")
            .await
            .unwrap();
        drop(writing);
        // The notification gets no reply; the connection then ends.
        assert!(read.next_line().await.unwrap().is_none());
        reply
    };
    let (served, reply) = tokio::join!(table.serve_socket(server), talking);
    served.unwrap();
    assert_eq!(reply["id"], 11);
}

#[tokio::test]
async fn request_options_are_offered_and_taken_only_when_accepted() {
    let mut table = table();
    let options = |table: &OperationTable| -> Vec<String> {
        let properties = table.mcp_tools()[0]["inputSchema"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let mut names: Vec<String> = properties
            .into_iter()
            .filter(|name| !["operation", "arguments"].contains(&name.as_str()))
            .collect();
        names.sort();
        names
    };
    // Every table takes request_id and deadline; nothing else until named.
    assert_eq!(options(&table), ["deadline", "request_id"]);
    for (option, call_options) in [
        (RequestOption::BudgetTokens, json!({"budget_tokens": 10})),
        (RequestOption::Cursor, json!({"cursor": "c"})),
        (RequestOption::Fresh, json!({"fresh": true})),
        (RequestOption::Format, json!({"format": "json"})),
    ] {
        let mut arguments = json!({"arguments": {"text": "x"}, "request_id": "r-1"});
        for (key, value) in call_options.as_object().unwrap() {
            arguments[key] = value.clone();
        }
        let refused = call(&table, "example_status", arguments.clone()).await;
        assert_eq!(
            refused["result"]["structuredContent"]["error"]["message"],
            format!("example does not take {}", option.name())
        );
        let accepting = table_with_options([option]);
        assert!(options(&accepting).contains(&option.name().to_owned()));
        let taken = call(&accepting, "example_status", arguments).await;
        assert_eq!(taken["result"]["isError"], false, "{option:?}");
    }
    table = table_with_options(RequestOption::ALL);
    assert_eq!(
        options(&table),
        [
            "budget_tokens",
            "cursor",
            "deadline",
            "format",
            "fresh",
            "request_id"
        ]
    );
    // The command line's --json prints JSON either way.
    let plain = self::table();
    let cli = plain
        .cli(
            &["status", "--text", "x", "--json"].map(str::to_string),
            "r-1",
            CancellationToken::new(),
        )
        .await;
    assert_eq!(cli.exit_code, 0);
    assert!(cli.stdout.starts_with('{'));
}
