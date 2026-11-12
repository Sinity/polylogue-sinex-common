use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum View {
    Plain,
    Detail,
}
#[derive(Serialize, Deserialize, JsonSchema)]
struct Options {
    target: String,
    view: Option<View>,
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    tags: Vec<View>,
    count: Option<u64>,
}
fn tool() -> ToolFamily {
    ToolFamily {
        name: "query".into(),
        description: "Read synthetic values".into(),
        open_world: false,
        idempotent: true,
    }
}
fn options() -> DeclarationsBuilder {
    let mut declarations = OperationDeclarations::builder("example", Deadlines::default());
    declarations.declare_tool(tool()).unwrap();
    declarations.accept(RequestOption::Format);
    for (name, words) in [("read", &["read"][..]), ("add", &["sink", "add"][..])] {
        let mut spec = OperationSpec::request(
            name,
            words,
            "query",
            "Read options",
            "Exercise the common CLI projection",
        );
        spec.arguments = vec![
            ArgumentSpec::new("target", ArgumentForm::Positional, "target"),
            ArgumentSpec::new("view", ArgumentForm::Named, "view"),
            ArgumentSpec::new("enabled", ArgumentForm::Flag, "enabled"),
            ArgumentSpec::new("tags", ArgumentForm::Repeated, "tags"),
            ArgumentSpec::new("out", ArgumentForm::Named, "local output").calling_process(),
        ];
        declarations
            .declare::<Options, String>(spec, |text, _| text.clone())
            .unwrap();
    }
    declarations
}
fn words(input: &[&str]) -> Vec<String> {
    input.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn cli_projection_owns_required_kinds_choices_and_local_flags() {
    let declarations = options().finish();
    let arguments = declarations.cli_arguments("read").unwrap();
    let argument = |name| {
        arguments
            .iter()
            .find(|argument| argument.spec.name == name)
            .unwrap()
    };
    assert_eq!(arguments.len(), 6);
    assert!(argument("target").required);
    assert_eq!(argument("target").kind, ValueKind::Text);
    assert_eq!(argument("view").choices, vec!["plain", "detail"]);
    assert!(!argument("view").required);
    assert_eq!(
        argument("tags").kind,
        ValueKind::List(Box::new(ValueKind::Text))
    );
    assert_eq!(argument("tags").choices, vec!["plain", "detail"]);
    assert_eq!(argument("enabled").kind, ValueKind::Boolean);
    assert_eq!(argument("count").spec.form, ArgumentForm::Named);
    assert_eq!(argument("count").kind, ValueKind::Json);
    assert!(argument("out").spec.calling_process);
    assert!(!argument("out").required);
    assert_eq!(argument("out").kind, ValueKind::Text);
    let parsed = declarations
        .parse_cli(
            &words(&[
                "read",
                "item",
                "--view",
                "detail",
                "--enabled",
                "--tags",
                "plain",
                "--count",
                "4",
                "--out",
                "result",
            ]),
            "r",
        )
        .unwrap();
    assert_eq!(
        parsed.request.arguments,
        json!({"target":"item","view":"detail","enabled":true,"tags":["plain"],"count":4})
    );
    assert_eq!(parsed.calling_process["out"], "result");
}

#[test]
fn leading_globals_match_trailing_globals_for_longest_command() {
    let declarations = options().finish();
    for command in [&["read"][..], &["sink", "add"][..]] {
        let mut before = words(&["--json", "--deadline", "100", "--request-id", "chosen"]);
        before.extend(words(command));
        before.extend(words(&["item", "--view", "detail"]));
        let mut after = words(command);
        after.extend(words(&[
            "--json",
            "--deadline",
            "100",
            "--request-id",
            "chosen",
            "item",
            "--view",
            "detail",
        ]));
        assert_eq!(
            declarations.parse_cli(&before, "unused").unwrap(),
            declarations.parse_cli(&after, "unused").unwrap()
        );
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Script {
    command: String,
}
#[test]
fn leading_globals_leave_rest_arguments_and_literal_separator_untouched() {
    let mut declarations = OperationDeclarations::builder("example", Deadlines::default());
    declarations.declare_tool(tool()).unwrap();
    let mut spec = OperationSpec::request(
        "exec",
        &["exec"],
        "query",
        "Run script",
        "Preserve literal rest words",
    );
    spec.arguments
        .push(ArgumentSpec::new("command", ArgumentForm::Rest, "script"));
    declarations
        .declare::<Script, String>(spec, |text, _| text.clone())
        .unwrap();
    declarations
        .set_default_operation("exec", |_| true)
        .unwrap();
    let declarations = declarations.finish();
    let explicit = declarations
        .parse_cli(
            &words(&["--json", "exec", "echo", "--json", "--deadline", "100"]),
            "r",
        )
        .unwrap();
    assert!(explicit.json);
    assert_eq!(
        explicit.request.arguments,
        json!({"command":"echo --json --deadline 100"})
    );
    assert_eq!(explicit.request.deadline, Deadline::Default);
    let literal = declarations
        .parse_cli(&words(&["--json", "--", "--deadline", "100", "exec"]), "r")
        .unwrap();
    assert!(literal.json);
    assert_eq!(
        literal.request.arguments,
        json!({"command":"--deadline 100 exec"})
    );
    assert_eq!(literal.request.deadline, Deadline::Default);
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Text {
    text: String,
}
fn declaration(
    name: &str,
    command: &[&str],
    mcp: Option<&str>,
    http: Option<&str>,
    access: Access,
    listed: bool,
) -> OperationSpec {
    OperationSpec {
        name: name.into(),
        command: words(command),
        summary: format!("Run {name}"),
        reason: "Exercise actual exposure".into(),
        kind: OperationKind::Request {
            tool: mcp.map(str::to_owned),
        },
        http_route: http.map(str::to_owned),
        latency: LatencyClass::Read,
        failures: vec![],
        access,
        arguments: vec![],
        listed,
        default_budget_tokens: None,
    }
}
fn styled(text: &str, style: TextStyle) -> String {
    match style {
        TextStyle::Plain => format!("{text}\n"),
        TextStyle::Ansi => format!("\u{1b}[31m{text}\u{1b}[0m\n"),
    }
}
struct Echo(Arc<AtomicUsize>);
impl Operation for Echo {
    type Request = Text;
    type Response = String;
    fn execute(&self, request: Text, _: RequestContext) -> OperationFuture<'_, Answer<String>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            Answer::ok(request.text)
        })
    }
}
fn surfaces() -> (OperationTable, Arc<AtomicUsize>) {
    let mut declarations = OperationDeclarations::builder("example", Deadlines::default());
    declarations.declare_tool(tool()).unwrap();
    for spec in [
        declaration(
            "endpoint",
            &[],
            None,
            Some("/v1/extension/report"),
            Access::Write,
            true,
        ),
        declaration("native", &[], None, None, Access::Read, true),
        declaration("mcp", &[], Some("query"), None, Access::Read, true),
        declaration("cli", &["cli"], None, None, Access::Read, true),
        declaration(
            "hidden",
            &["hidden"],
            Some("query"),
            Some("/v1/hidden"),
            Access::Read,
            false,
        ),
    ] {
        declarations
            .declare::<Text, String>(spec, |text, style| styled(text, style))
            .unwrap();
    }
    assert!(
        declarations
            .set_default_operation("endpoint", |_| true)
            .is_err()
    );
    let mut table = OperationTable::new(declarations.finish());
    let effects = Arc::new(AtomicUsize::new(0));
    for name in ["endpoint", "native", "mcp", "cli", "hidden"] {
        table.bind(name, Echo(effects.clone())).unwrap();
    }
    (table, effects)
}
fn request(operation: &str) -> RunnableRequest {
    RunnableRequest::new(operation, json!({"text":"value"}), "r")
}
fn mcp(operation: &str) -> Value {
    json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"example_query","arguments":{"operation":operation,"arguments":{"text":"value"},"request_id":"r"}}})
}
fn http(path: &str, operation: &str) -> http::Request<Vec<u8>> {
    http::Request::post(path)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(serde_json::to_vec(&request(operation)).unwrap())
        .unwrap()
}

#[test]
fn absent_cli_and_mcp_are_capabilities_while_listed_only_controls_visibility() {
    let (table, _) = surfaces();
    assert_eq!(table.complete(""), vec!["cli"]);
    assert!(table.help_for("endpoint").is_none());
    assert!(
        table
            .parse_cli(&words(&["endpoint", "--text", "value"]), "r")
            .is_err()
    );
    assert!(
        table
            .parse_cli(&words(&["hidden", "--text", "value"]), "r")
            .is_ok()
    );
    assert!(table.cli_arguments("endpoint").is_err());
    assert!(table.cli_command(&request("mcp")).is_err());
    assert!(table.follow_up(Via::Cli, &request("mcp")).is_err());
    assert!(table.follow_up(Via::Mcp, &request("endpoint")).is_err());
    assert!(table.follow_up(Via::Mcp, &request("cli")).is_err());
    assert!(table.mcp_call(&request("endpoint")).is_err());
    assert!(table.mcp_call(&request("cli")).is_err());
    assert_eq!(
        table.mcp_call(&request("mcp")).unwrap()["arguments"]["operation"],
        "mcp"
    );
    assert!(
        table
            .follow_up(Via::Mcp, &request("mcp"))
            .unwrap()
            .starts_with("example_query ")
    );
    let offered = table.mcp_tools();
    assert_eq!(
        offered[0]["inputSchema"]["properties"]["operation"]["enum"],
        json!(["mcp", "hidden"])
    );
    checks::tools_generated(&table).unwrap();
    let rendered = table.render_answer(
        Via::Mcp,
        None,
        Answer::ok(Value::Null).with_next("extension", request("endpoint")),
        TextStyle::Plain,
    );
    assert!(
        rendered
            .footer
            .contains("unavailable: endpoint has no MCP surface")
    );
    assert!(!rendered.footer.contains("example endpoint"));
}

#[tokio::test]
async fn forged_mcp_endpoint_selection_is_denied_before_handler_execution() {
    let (table, effects) = surfaces();
    let forged = table.mcp(mcp("endpoint"), CancellationToken::new()).await;
    assert_eq!(forged["result"]["isError"], true);
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    let hidden = table.mcp(mcp("hidden"), CancellationToken::new()).await;
    assert_eq!(hidden["result"]["isError"], false);
    assert_eq!(effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn http_and_native_execution_respect_actual_exposure_and_access() {
    let (table, effects) = surfaces();
    assert_eq!(
        table
            .http(http("/v1", "mcp"), CancellationToken::new())
            .await
            .status(),
        404
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(
        table
            .http(
                http("/v1/extension/report", "endpoint"),
                CancellationToken::new()
            )
            .await
            .status(),
        403
    );
    assert_eq!(effects.load(Ordering::SeqCst), 0);
    assert_eq!(
        table
            .http_as(
                http("/v1/extension/report", "endpoint"),
                Caller::default().with_access(Access::Write),
                CancellationToken::new()
            )
            .await
            .status(),
        200
    );
    assert_eq!(
        table
            .call(request("native"), Via::Daemon, CancellationToken::new())
            .await
            .value(),
        Some(&json!("value"))
    );
    assert_eq!(effects.load(Ordering::SeqCst), 2);
    let listing = table
        .http(
            http::Request::get("/v1/commands").body(vec![]).unwrap(),
            CancellationToken::new(),
        )
        .await;
    let rows: Vec<Value> = serde_json::from_slice(listing.body()).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["name"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["endpoint", "native", "mcp", "cli", "hidden"]
    );
}

#[tokio::test]
async fn one_renderer_receives_explicit_style_while_mcp_stays_plain() {
    let (table, _) = surfaces();
    let answer = Answer::ok(json!("value"));
    assert_eq!(
        table.render_text("mcp", &answer, TextStyle::Plain).unwrap(),
        "value\n"
    );
    assert_eq!(
        table.render_text("mcp", &answer, TextStyle::Ansi).unwrap(),
        "\u{1b}[31mvalue\u{1b}[0m\n"
    );
    let cli = table.render_answer(Via::Cli, Some(&request("cli")), answer, TextStyle::Ansi);
    assert!(cli.value_text.contains('\u{1b}'));
    let mcp = table.mcp(mcp("mcp"), CancellationToken::new()).await;
    assert!(
        !mcp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains('\u{1b}')
    );
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct QueryArgs {
    target: String,
    #[serde(default)]
    show: Vec<String>,
    format: Option<String>,
    count: Option<u64>,
    ready: Option<bool>,
    assertions: Option<Vec<Value>>,
}
fn query_declarations() -> std::sync::Arc<OperationDeclarations> {
    let mut declarations = OperationDeclarations::builder("example", Deadlines::default());
    declarations.declare_tool(tool()).unwrap();
    let mut spec = OperationSpec::request(
        "query",
        &["query"],
        "query",
        "Query values",
        "Preserve real lexical input forms",
    );
    spec.arguments
        .push(ArgumentSpec::new("show", ArgumentForm::Repeated, "shown fields").delimiter(','));
    declarations
        .declare::<QueryArgs, String>(spec, |text, _| text.clone())
        .unwrap();
    declarations.finish()
}
fn pairs(input: &[(&str, &str)]) -> Vec<(String, String)> {
    input
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn declared_cli_delimiter_splits_flags_but_preserves_canonical_items() {
    let declarations = query_declarations();
    let parsed = declarations
        .parse_cli(
            &words(&[
                "query", "--target", "x", "--show", "id,cost", "--show", "text",
            ]),
            "r",
        )
        .unwrap();
    assert_eq!(
        parsed.request.arguments["show"],
        json!(["id", "cost", "text"])
    );
    assert_eq!(
        declarations
            .cli_arguments("query")
            .unwrap()
            .iter()
            .find(|argument| argument.spec.name == "show")
            .unwrap()
            .spec
            .value_delimiter,
        Some(',')
    );
    let request =
        RunnableRequest::new("query", json!({"target":"x","show":["literal,comma"]}), "r");
    let command = declarations.cli_command(&request).unwrap();
    assert!(command.contains("--request "));
    let raw = declarations
        .parse_cli(
            &words(&[
                "query",
                "--request",
                "{\"target\":\"x\",\"show\":[\"literal,comma\"]}",
            ]),
            "r",
        )
        .unwrap();
    assert_eq!(raw.request.arguments, request.arguments);
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Paths {
    paths: Vec<String>,
}
#[test]
fn words_lists_preserve_literal_commas_without_a_declared_delimiter() {
    let mut declarations = OperationDeclarations::builder("example", Deadlines::default());
    declarations.declare_tool(tool()).unwrap();
    let mut spec = OperationSpec::request(
        "paths",
        &["paths"],
        "query",
        "Read paths",
        "Preserve positional list spelling",
    );
    spec.arguments
        .push(ArgumentSpec::new("paths", ArgumentForm::Words, "paths"));
    declarations
        .declare::<Paths, String>(spec, |text, _| text.clone())
        .unwrap();
    assert_eq!(
        declarations
            .parse_cli(&words(&["paths", "a,b", "c"]), "r")
            .unwrap()
            .request
            .arguments,
        json!({"paths":["a,b","c"]})
    );
}

#[test]
fn flat_http_uses_typed_fields_and_envelope_identity_without_cli_delimiters() {
    let declarations = query_declarations();
    let request = declarations
        .parse_http_query(
            "query",
            &pairs(&[
                ("target", "x"),
                ("show", "id,cost"),
                ("show", "text"),
                ("format", "html"),
                ("count", "4"),
                ("ready", ""),
                ("assertions", "[{\"kind\":\"synthetic\"}]"),
                ("request_id", "chosen"),
                ("cursor", "next"),
                ("deadline", "100"),
                ("budget_tokens", "none"),
                ("fresh", "1"),
            ]),
            "fallback",
        )
        .unwrap();
    assert_eq!(
        request.arguments,
        json!({"target":"x","show":["id,cost","text"],"format":"html","count":4,"ready":true,"assertions":[{"kind":"synthetic"}]})
    );
    assert_eq!(request.request_id, "chosen");
    assert_eq!(request.cursor.as_deref(), Some("next"));
    assert_eq!(request.deadline, Deadline::Milliseconds(100));
    assert_eq!(request.budget_tokens, TokenBudget::Unlimited);
    assert!(request.fresh);
    assert_eq!(request.format, Format::Text);
    let false_value = declarations
        .parse_http_query("query", &pairs(&[("target", "x"), ("ready", "other")]), "r")
        .unwrap();
    assert_eq!(false_value.arguments["ready"], false);
}

#[test]
fn canonical_http_preserves_raw_arguments_and_refuses_identity_or_duplicate_conflicts() {
    let declarations = query_declarations();
    let raw = "{\"target\":\"x\",\"unknown_for_handler\":{\"deep\":[1]}}";
    let request = declarations
        .parse_http_query(
            "query",
            &pairs(&[("operation", "query"), ("arguments", raw)]),
            "fallback",
        )
        .unwrap();
    assert_eq!(
        request.arguments,
        serde_json::from_str::<Value>(raw).unwrap()
    );
    assert!(request.arguments.get("show").is_none());
    assert_eq!(request.deadline, Deadline::Default);
    assert_eq!(request.request_id, "fallback");
    for invalid in [
        pairs(&[("operation", "other"), ("arguments", "{}")]),
        pairs(&[("arguments", "{}"), ("arguments", "{}")]),
        pairs(&[("target", "x"), ("target", "y")]),
        pairs(&[("arguments", "{}"), ("fresh", "1")]),
        pairs(&[("target", "x"), ("assertions", "[]"), ("assertions", "[]")]),
    ] {
        assert!(
            declarations
                .parse_http_query("query", &invalid, "r")
                .is_err()
        );
    }
}

#[derive(Serialize, Deserialize, JsonSchema)]
struct Reserved {
    deadline: String,
}
#[test]
fn native_only_schema_does_not_acquire_cli_reserved_word_restrictions() {
    let mut declarations = OperationDeclarations::builder("example", Deadlines::default());
    declarations
        .declare::<Reserved, bool>(
            declaration("native", &[], None, None, Access::Read, true),
            |value, _| value.to_string(),
        )
        .unwrap();
    assert!(declarations.finish().cli_arguments("native").is_err());
}
