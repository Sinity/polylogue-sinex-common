//! Typed operations and one dispatch boundary shared by every adapter.
use crate::{
    Answer, Authorship, Budget, Deadline, Failure, FailureKind, Fitted, LatencyClass, LeftOut,
    Outcome, RunnableRequest, Section, bad_request,
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    any::Any,
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

pub type OperationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Who may call an operation. Each level includes the ones below it.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    #[default]
    Read,
    /// Attributed, reversible writes.
    Write,
    /// Changes that cannot be undone (a tombstone).
    Permanent,
}
impl Access {
    pub fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Permanent => "permanent",
        }
    }
}

/// How an operation runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OperationKind {
    /// The daemon answers it once. It is offered on every surface; over MCP
    /// it is a row of the named tool.
    Request { tool: String },
    /// The daemon sends events until the stream ends or the caller leaves
    /// (a change feed, `follow`). Offered on the CLI, the socket and HTTP
    /// (as server-sent events); not over MCP, whose calls answer once.
    Stream,
    /// Runs in the calling process without the daemon: help, installers,
    /// the MCP server and hook entry points. Command line only.
    Local,
}

/// How one argument is written on the command line. Every surface other
/// than the command line uses the request's field of the same name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArgumentForm {
    /// One word by position. Required when the request type requires it.
    Positional,
    /// The remaining words by position, flags between them still read as
    /// flags: joined by spaces for a string field, a list for an array field.
    Words,
    /// Everything after it, flags included, the same way.
    Rest,
    /// `--name VALUE`.
    Named,
    /// `--name VALUE`, repeatable; the values form a list.
    Repeated,
    /// `--name` alone, for a boolean field.
    Flag,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArgumentSpec {
    /// The request field (`request_id` style); `--request-id` on the command line.
    pub name: String,
    pub form: ArgumentForm,
    pub help: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub short: Option<char>,
    /// Acts on the calling machine (`--out FILE`): read by the command line
    /// and kept out of the request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub calling_process: bool,
}
impl ArgumentSpec {
    pub fn new(name: impl Into<String>, form: ArgumentForm, help: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            form,
            help: help.into(),
            short: None,
            calling_process: false,
        }
    }
    pub fn short(mut self, short: char) -> Self {
        self.short = Some(short);
        self
    }
    pub fn calling_process(mut self) -> Self {
        self.calling_process = true;
        self
    }
    pub(crate) fn flag(&self) -> String {
        format!("--{}", self.name.replace('_', "-"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OperationSpec {
    pub name: String,
    pub command: Vec<String>,
    pub summary: String,
    /// Why this operation cannot be a query plus an existing operation.
    pub reason: String,
    #[serde(flatten)]
    pub kind: OperationKind,
    /// Its own HTTP route; none for a local operation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_route: Option<String>,
    pub latency: LatencyClass,
    pub failures: Vec<FailureKind>,
    pub access: Access,
    /// How each request field is written on the command line. A field with
    /// no entry is written `--name VALUE`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<ArgumentSpec>,
    /// Shown in help; a row that programs run and people do not is not.
    #[serde(default = "listed")]
    pub listed: bool,
    /// The token budget of an answer whose request names none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_budget_tokens: Option<u32>,
}
fn listed() -> bool {
    true
}
impl OperationSpec {
    /// A request row of `tool`, readable by anyone, with no declared arguments.
    pub fn request(
        name: impl Into<String>,
        command: &[&str],
        tool: impl Into<String>,
        summary: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        let command: Vec<String> = command.iter().map(|word| (*word).to_owned()).collect();
        Self {
            http_route: Some(format!("/v1/{}", command.join("/"))),
            name: name.into(),
            command,
            summary: summary.into(),
            reason: reason.into(),
            kind: OperationKind::Request { tool: tool.into() },
            latency: LatencyClass::Read,
            failures: vec![],
            access: Access::Read,
            arguments: vec![],
            listed: true,
            default_budget_tokens: None,
        }
    }
    pub fn tool(&self) -> Option<&str> {
        match &self.kind {
            OperationKind::Request { tool } => Some(tool),
            _ => None,
        }
    }
    pub fn argument(&self, name: &str) -> Option<&ArgumentSpec> {
        self.arguments.iter().find(|argument| argument.name == name)
    }
}

/// One MCP tool: a family of request rows whose arguments share a shape and
/// whose access is the same.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolFamily {
    /// `query`; the tool is `<product>_query`.
    pub name: String,
    /// Sent to the model verbatim.
    pub description: String,
    /// Its rows reach beyond the product (another session, a web chat).
    #[serde(default)]
    pub open_world: bool,
}

/// One request row. Its response type supplies every surface, including
/// terminal text.
///
/// The returned future owns its work. Drop cancels it, and blocking workers
/// must check the context token at least every 100 ms. Do not spawn unowned
/// work. A write stores request identity and its result atomically with its
/// mutation in the product's transaction, so a retry, concurrent or after a
/// restart, is applied once.
pub trait Operation: Send + Sync + 'static {
    type Request: Serialize + DeserializeOwned + JsonSchema + Send + 'static;
    type Response: Serialize + DeserializeOwned + JsonSchema + Send + 'static;
    fn spec(&self) -> OperationSpec;
    fn execute(
        &self,
        request: Self::Request,
        context: RequestContext,
    ) -> OperationFuture<'_, Answer<Self::Response>>;
    fn render_text(&self, answer: &Self::Response) -> String;
}

/// One stream row: opening it yields its events until the sender closes.
pub trait StreamOperation: Send + Sync + 'static {
    type Request: Serialize + DeserializeOwned + JsonSchema + Send + 'static;
    type Event: Serialize + DeserializeOwned + JsonSchema + Send + 'static;
    fn spec(&self) -> OperationSpec;
    fn open(
        &self,
        request: Self::Request,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<tokio::sync::mpsc::Receiver<Self::Event>, Failure>>;
    fn render_event(&self, event: &Self::Event) -> String;
}

/// A stream's events as JSON, in the order sent.
pub struct Events {
    events: Box<dyn ErasedEvents>,
}
impl Events {
    /// The next event; `None` when the stream has ended.
    pub async fn next(&mut self) -> Option<Result<Value, Failure>> {
        self.events.receive().await
    }
}
trait ErasedEvents: Send {
    fn receive(&mut self) -> OperationFuture<'_, Option<Result<Value, Failure>>>;
}
impl<T: Serialize + Send + 'static> ErasedEvents for tokio::sync::mpsc::Receiver<T> {
    fn receive(&mut self) -> OperationFuture<'_, Option<Result<Value, Failure>>> {
        Box::pin(async move {
            self.recv().await.map(|event| {
                serde_json::to_value(event)
                    .map_err(|e| Failure::new(FailureKind::Io, e.to_string()))
            })
        })
    }
}

#[derive(Clone)]
pub struct RequestContext {
    /// Computed at the transport boundary; a caller cannot set this in JSON.
    pub origin: Origin,
    pub request_id: String,
    pub cancellation: CancellationToken,
    caller: Caller,
    budget_tokens: Option<u32>,
    default_budget_tokens: Option<u32>,
    cursor: Option<String>,
    fresh: bool,
    format: crate::Format,
    /// An MCP call's own `_meta`, where a harness puts per-call identity.
    metadata: Option<Value>,
    forms: Arc<Forms>,
    progress: Arc<Mutex<Progress>>,
}
/// Trusted product identity, supplied after authentication or peer inspection.
/// It is never deserialized from a request. Scope namespaces retry identities.
#[derive(Clone)]
pub struct Caller {
    scope: String,
    identity: Option<Arc<dyn Any + Send + Sync>>,
    authorship: Option<Authorship>,
    access: Access,
}
impl Default for Caller {
    fn default() -> Self {
        Self {
            scope: String::new(),
            identity: None,
            authorship: None,
            access: Access::Permanent,
        }
    }
}
impl Caller {
    pub fn authenticated<T: Any + Send + Sync>(scope: impl Into<String>, identity: T) -> Self {
        Self {
            scope: scope.into(),
            identity: Some(Arc::new(identity)),
            ..Self::default()
        }
    }
    /// The computed author, echoed in every answer's envelope.
    pub fn with_authorship(mut self, authorship: Authorship) -> Self {
        self.authorship = Some(authorship);
        self
    }
    /// The highest access the caller's credential grants (all, by default).
    pub fn with_access(mut self, access: Access) -> Self {
        self.access = access;
        self
    }
    pub fn identity<T: Any>(&self) -> Option<&T> {
        self.identity.as_ref()?.downcast_ref()
    }
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn authorship(&self) -> Option<&Authorship> {
        self.authorship.as_ref()
    }
    pub fn access(&self) -> Access {
        self.access
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Cli,
    Socket,
    Http,
    Mcp,
}
#[derive(Clone, Debug, Default)]
struct Progress {
    stage: String,
    rows_scanned: Option<u64>,
    narrower: Option<RunnableRequest>,
}
impl RequestContext {
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    pub fn metadata(&self) -> Option<&Value> {
        self.metadata.as_ref()
    }
    /// The cursor the request continues from.
    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }
    /// Whether the caller asked to see in full what it already holds.
    pub fn fresh(&self) -> bool {
        self.fresh
    }
    pub fn format(&self) -> crate::Format {
        self.format
    }
    /// The answer's budget: the request's, else the operation's default (or
    /// `fallback` when it declares none), lowered to the caller's `cap`.
    pub fn budget(&self, fallback: u32, cap: Option<u32>) -> Budget {
        Budget::new(
            self.budget_tokens,
            self.default_budget_tokens.unwrap_or(fallback),
            cap,
        )
    }
    /// How this request's surface writes a follow-up: a command line, or an
    /// MCP tool call.
    pub fn follow_up(&self, request: &RunnableRequest) -> String {
        self.forms.follow_up(self.origin, request)
    }
    /// Fit `sections` to `budget`, counting left-out lines as this request's
    /// surface prints them. The answer's envelope should carry the result's
    /// `left_out` and `used_tokens`, and the budget's clamp note.
    pub fn fit(
        &self,
        sections: Vec<Section>,
        budget: Budget,
        tokens: &dyn Fn(&str) -> u32,
    ) -> Fitted {
        crate::fit(sections, budget, tokens, &|item: &LeftOut| {
            self.forms.left_out_line(self.origin, item)
        })
    }
    /// Called between bounded work units. Also supplies the timeout diagnostic.
    pub fn progress(
        &self,
        stage: impl Into<String>,
        rows_scanned: u64,
        narrower: Option<RunnableRequest>,
    ) -> Result<(), Failure> {
        self.set_progress(stage, Some(rows_scanned), narrower)
    }
    /// Report a stage without inventing a row count when it was not measured.
    pub fn progress_unknown(
        &self,
        stage: impl Into<String>,
        narrower: Option<RunnableRequest>,
    ) -> Result<(), Failure> {
        self.set_progress(stage, None, narrower)
    }
    fn set_progress(
        &self,
        stage: impl Into<String>,
        rows_scanned: Option<u64>,
        narrower: Option<RunnableRequest>,
    ) -> Result<(), Failure> {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = Progress {
            stage: stage.into(),
            rows_scanned,
            narrower,
        };
        self.check_cancelled()
    }
    pub fn check_cancelled(&self) -> Result<(), Failure> {
        if self.cancellation.is_cancelled() {
            Err(Failure::new(FailureKind::Cancelled, "request cancelled"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationDescription {
    #[serde(flatten)]
    pub spec: OperationSpec,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
    /// The answer's schema; for a stream, one event's.
    #[serde(rename = "outputSchema")]
    pub output_schema: Value,
}

/// What every surface needs to write a request back: the product name, the
/// rows' descriptions and the tool families. Shared with request contexts.
#[derive(Clone)]
pub(crate) struct Forms {
    pub(crate) product: String,
    pub(crate) descriptions: Vec<OperationDescription>,
    pub(crate) tools: Vec<ToolFamily>,
}
impl Forms {
    pub(crate) fn description(&self, name: &str) -> Option<&OperationDescription> {
        self.descriptions.iter().find(|d| d.spec.name == name)
    }
}

enum Row {
    Request(Box<dyn ErasedOperation>),
    Stream(Box<dyn ErasedStream>),
    Local,
}

trait ErasedOperation: Send + Sync {
    fn execute(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Answer<Value>>;
    fn render_text(&self, value: Value) -> Result<String, Failure>;
}
impl<O: Operation> ErasedOperation for O {
    fn execute(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Answer<Value>> {
        Box::pin(async move {
            let request = match serde_json::from_value(arguments) {
                Ok(request) => request,
                Err(error) => return Answer::error(bad_request(error.to_string())),
            };
            let answer = Operation::execute(self, request, context).await;
            let Answer { outcome, envelope } = answer;
            let outcome = match outcome.map(serde_json::to_value) {
                Outcome::Ok(Ok(value)) => Outcome::Ok(value),
                Outcome::Empty(Ok(value)) => Outcome::Empty(value),
                Outcome::Degraded {
                    value: Ok(value),
                    missing,
                } => Outcome::Degraded { value, missing },
                Outcome::Error(error) => Outcome::Error(error),
                Outcome::Ok(Err(error))
                | Outcome::Empty(Err(error))
                | Outcome::Degraded {
                    value: Err(error), ..
                } => Outcome::Error(Failure::new(FailureKind::Io, error.to_string())),
            };
            Answer { outcome, envelope }
        })
    }
    fn render_text(&self, value: Value) -> Result<String, Failure> {
        serde_json::from_value(value)
            .map(|answer| Operation::render_text(self, &answer))
            .map_err(|e| Failure::new(FailureKind::Io, e.to_string()))
    }
}

trait ErasedStream: Send + Sync {
    fn open(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Events, Failure>>;
    fn render_event(&self, event: Value) -> Result<String, Failure>;
}
impl<S: StreamOperation> ErasedStream for S {
    fn open(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Events, Failure>> {
        Box::pin(async move {
            let request =
                serde_json::from_value(arguments).map_err(|e| bad_request(e.to_string()))?;
            let receiver = StreamOperation::open(self, request, context).await?;
            Ok(Events {
                events: Box::new(receiver),
            })
        })
    }
    fn render_event(&self, event: Value) -> Result<String, Failure> {
        serde_json::from_value(event)
            .map(|event| StreamOperation::render_event(self, &event))
            .map_err(|e| Failure::new(FailureKind::Io, e.to_string()))
    }
}

/// A product declares its own budgets. No deadline is a universal size limit.
#[derive(Clone, Debug, Default)]
pub struct Deadlines([Option<Duration>; 6]);
impl Deadlines {
    pub fn set(&mut self, class: LatencyClass, deadline: Option<Duration>) {
        self.0[class.index()] = deadline;
    }
    pub fn get(&self, class: LatencyClass) -> Option<Duration> {
        self.0[class.index()]
    }
}

/// The words the command line keeps for itself on every row.
pub(crate) const GLOBAL_FLAGS: [&str; 7] = [
    "json",
    "deadline",
    "request-id",
    "request",
    "budget",
    "cursor",
    "fresh",
];

/// Heterogeneous typed table. Registration rejects ambiguity once at startup.
/// Retry identity lives with the product's writer, in the same transaction
/// as its mutation; the table keeps no request state.
pub struct OperationTable {
    rows: Vec<Row>,
    forms: Arc<Forms>,
    deadlines: Deadlines,
    failure_codes: BTreeMap<String, FailureKind>,
    default_operation: Option<String>,
}
impl OperationTable {
    pub fn new(product: impl Into<String>, deadlines: Deadlines) -> Self {
        Self {
            rows: vec![],
            forms: Arc::new(Forms {
                product: product.into(),
                descriptions: vec![],
                tools: vec![],
            }),
            deadlines,
            failure_codes: BTreeMap::new(),
            default_operation: None,
        }
    }
    pub fn product(&self) -> &str {
        &self.forms.product
    }
    /// Declare an MCP tool before registering its rows.
    pub fn declare_tool(&mut self, tool: ToolFamily) -> Result<(), Failure> {
        if !valid_word(&tool.name, '_') || tool.description.is_empty() {
            return Err(bad_request("a tool needs a name and a description"));
        }
        if self.forms.tools.iter().any(|t| t.name == tool.name) {
            return Err(bad_request(format!("tool {} is declared twice", tool.name)));
        }
        Arc::make_mut(&mut self.forms).tools.push(tool);
        Ok(())
    }
    pub fn tools(&self) -> &[ToolFamily] {
        &self.forms.tools
    }
    /// The row a command line runs when its first words name no command
    /// (`polylogue QUERY`).
    pub fn set_default_operation(&mut self, name: impl Into<String>) -> Result<(), Failure> {
        let name = name.into();
        let spec = self
            .spec_named(&name)
            .ok_or_else(|| bad_request(format!("unknown operation {name}")))?;
        if matches!(spec.kind, OperationKind::Stream) {
            return Err(bad_request("a stream cannot be the default operation"));
        }
        self.default_operation = Some(name);
        Ok(())
    }
    pub(crate) fn default_operation(&self) -> Option<&OperationDescription> {
        self.default_operation
            .as_deref()
            .and_then(|name| self.forms.description(name))
    }
    /// Register each precise product code once, at table construction.
    pub fn register_failure_code(
        &mut self,
        code: impl Into<String>,
        kind: FailureKind,
    ) -> Result<(), Failure> {
        let code = code.into();
        if code.is_empty() {
            return Err(bad_request("failure code must not be empty"));
        }
        if let Some(existing) = self.failure_codes.get(&code) {
            if *existing == kind {
                return Ok(());
            }
            return Err(bad_request(format!(
                "failure code {code} already has a different classification"
            )));
        }
        self.failure_codes.insert(code, kind);
        Ok(())
    }
    pub fn register<O: Operation>(&mut self, operation: O) -> Result<(), Failure> {
        let description = OperationDescription {
            spec: operation.spec(),
            input_schema: schema_for!(O::Request).to_value(),
            output_schema: schema_for!(Answer<O::Response>).to_value(),
        };
        if !matches!(description.spec.kind, OperationKind::Request { .. }) {
            return Err(bad_request(format!(
                "operation {} implements Operation, so it is a request row",
                description.spec.name
            )));
        }
        self.add(description, Row::Request(Box::new(operation)))
    }
    pub fn register_stream<S: StreamOperation>(&mut self, stream: S) -> Result<(), Failure> {
        let description = OperationDescription {
            spec: stream.spec(),
            input_schema: schema_for!(S::Request).to_value(),
            output_schema: schema_for!(S::Event).to_value(),
        };
        if description.spec.kind != OperationKind::Stream {
            return Err(bad_request(format!(
                "operation {} implements StreamOperation, so it is a stream row",
                description.spec.name
            )));
        }
        self.add(description, Row::Stream(Box::new(stream)))
    }
    /// A row the calling process runs itself. The table parses its command
    /// line into `R` and lists it in help; nothing else reaches it.
    pub fn register_local<R: JsonSchema>(&mut self, spec: OperationSpec) -> Result<(), Failure> {
        if spec.kind != OperationKind::Local {
            return Err(bad_request(format!(
                "operation {} is registered as local, so its kind is local",
                spec.name
            )));
        }
        let description = OperationDescription {
            spec,
            input_schema: schema_for!(R).to_value(),
            output_schema: Value::Bool(true),
        };
        self.add(description, Row::Local)
    }
    fn add(&mut self, description: OperationDescription, row: Row) -> Result<(), Failure> {
        let spec = &description.spec;
        let product = &self.forms.product;
        if !valid_word(&product.replace('_', "-"), '-')
            || spec.name.is_empty()
            || spec.command.is_empty()
            || spec.command.iter().any(|word| !valid_word(word, '-'))
            || spec.summary.is_empty()
            || spec.reason.is_empty()
        {
            return Err(bad_request(
                "operation needs a name, command words, summary and reason",
            ));
        }
        match (&spec.kind, &spec.http_route) {
            (OperationKind::Local, None) => {}
            (OperationKind::Local, Some(_)) => {
                return Err(bad_request(format!(
                    "local operation {} has no HTTP route",
                    spec.name
                )));
            }
            (_, Some(route))
                if route.starts_with('/')
                    && !route.starts_with("//")
                    && !route.contains(['?', '#', '\r', '\n'])
                    && !matches!(route.as_str(), "/v1" | "/v1/commands") => {}
            _ => {
                return Err(bad_request(format!(
                    "operation {} needs an origin-relative HTTP route",
                    spec.name
                )));
            }
        }
        for existing in &self.forms.descriptions {
            let other = &existing.spec;
            if other.name == spec.name
                || other.command == spec.command
                || (other.http_route.is_some() && other.http_route == spec.http_route)
            {
                return Err(bad_request(format!(
                    "operation {} collides with {}",
                    spec.name, other.name
                )));
            }
        }
        if let OperationKind::Request { tool } = &spec.kind {
            if !self.forms.tools.iter().any(|t| &t.name == tool) {
                return Err(bad_request(format!(
                    "operation {} names undeclared tool {tool}",
                    spec.name
                )));
            }
            if let Some(other) = self
                .forms
                .descriptions
                .iter()
                .find(|d| d.spec.tool() == Some(tool) && d.spec.access != spec.access)
            {
                return Err(bad_request(format!(
                    "tool {tool} would hold {} with {} access and {} with {}",
                    other.spec.name,
                    other.spec.access.name(),
                    spec.name,
                    spec.access.name()
                )));
            }
        }
        crate::arguments::check(&description)?;
        let mut forms = (*self.forms).clone();
        forms.descriptions.push(description);
        if let Some(tool) = forms.descriptions.last().and_then(|d| d.spec.tool()) {
            // Rows of one tool share one set of schema definitions.
            crate::tools::tool_schemas(&forms, tool)?;
        }
        self.forms = Arc::new(forms);
        self.rows.push(row);
        Ok(())
    }
    pub fn descriptions(&self) -> &[OperationDescription] {
        &self.forms.descriptions
    }
    /// The named operation's spec, borrowed from the table without cloning.
    pub fn spec_named(&self, name: &str) -> Option<&OperationSpec> {
        self.forms.description(name).map(|d| &d.spec)
    }
    fn row(&self, name: &str) -> Option<(&OperationDescription, &Row)> {
        let index = self
            .forms
            .descriptions
            .iter()
            .position(|d| d.spec.name == name)?;
        Some((&self.forms.descriptions[index], &self.rows[index]))
    }
    pub fn deadline(&self, request: &RunnableRequest) -> Option<Duration> {
        self.spec_named(&request.operation)
            .and_then(|spec| request.deadline.duration(self.deadlines.get(spec.latency)))
    }
    pub fn socket_timeout(&self, request: &RunnableRequest) -> Option<Duration> {
        self.deadline(request)
            .and_then(|d| d.checked_add(Duration::from_millis(50)))
    }
    pub(crate) fn forms(&self) -> &Arc<Forms> {
        &self.forms
    }
    pub async fn call(
        &self,
        request: RunnableRequest,
        origin: Origin,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        self.call_as(request, origin, Caller::default(), cancellation)
            .await
    }
    pub async fn call_as(
        &self,
        request: RunnableRequest,
        origin: Origin,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        self.call_with(request, origin, caller, None, cancellation)
            .await
    }
    pub(crate) async fn call_with(
        &self,
        request: RunnableRequest,
        origin: Origin,
        caller: Caller,
        metadata: Option<Value>,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        let authorship = caller.authorship.clone();
        let mut answer = self
            .dispatch(request, origin, caller, metadata, cancellation)
            .await;
        if answer.envelope.caller.is_none() {
            answer.envelope.caller = authorship;
        }
        answer
    }
    /// Check a request against its row: it exists, has a request ID, names
    /// only known arguments, and the caller may run it.
    fn admit<'a>(
        &'a self,
        request: &RunnableRequest,
        caller: &Caller,
    ) -> Result<(&'a OperationDescription, &'a Row), Failure> {
        let (description, row) = self.row(&request.operation).ok_or_else(|| {
            Failure::new(
                FailureKind::NotFound,
                format!("unknown operation {}", request.operation),
            )
        })?;
        if request.request_id.is_empty() {
            return Err(bad_request("request_id must not be empty"));
        }
        // Even a Request implementation that omits deny_unknown_fields cannot
        // silently accept an author or misspelled top-level argument.
        if let (Some(object), Some(properties)) = (
            request.arguments.as_object(),
            description.input_schema["properties"].as_object(),
        ) && (description.input_schema["additionalProperties"] == false
            || description
                .input_schema
                .get("additionalProperties")
                .is_none())
            && let Some(key) = object.keys().find(|key| !properties.contains_key(*key))
        {
            return Err(bad_request(format!("unknown argument {key}")));
        }
        if description.spec.access > caller.access {
            return Err(Failure::new(
                FailureKind::Refused,
                format!(
                    "{} needs {} access",
                    description.spec.name,
                    description.spec.access.name()
                ),
            ));
        }
        Ok((description, row))
    }
    fn context(
        &self,
        request: &RunnableRequest,
        description: &OperationDescription,
        origin: Origin,
        caller: Caller,
        metadata: Option<Value>,
        cancellation: CancellationToken,
    ) -> RequestContext {
        RequestContext {
            origin,
            request_id: request.request_id.clone(),
            cancellation,
            caller,
            budget_tokens: request.budget_tokens,
            default_budget_tokens: description.spec.default_budget_tokens,
            cursor: request.cursor.clone(),
            fresh: request.fresh,
            format: request.format,
            metadata,
            forms: self.forms.clone(),
            progress: Arc::default(),
        }
    }
    async fn dispatch(
        &self,
        request: RunnableRequest,
        origin: Origin,
        caller: Caller,
        metadata: Option<Value>,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        let (description, row) = match self.admit(&request, &caller) {
            Ok(admitted) => admitted,
            Err(failure) => return Answer::error(failure),
        };
        let Row::Request(operation) = row else {
            return Answer::error(bad_request(match row {
                Row::Stream(_) => format!("{} is a stream; open it", request.operation),
                _ => format!("{} runs in the calling process", request.operation),
            }));
        };
        let token = cancellation.child_token();
        let guard = token.clone().drop_guard();
        let context = self.context(&request, description, origin, caller, metadata, token);
        let progress = context.progress.clone();
        let wait_deadline = async {
            match request
                .deadline
                .duration(self.deadlines.get(description.spec.latency))
            {
                Some(duration) => tokio::time::sleep(duration).await,
                None => std::future::pending::<()>().await,
            }
        };
        let answer = tokio::select! {
            biased;
            _ = cancellation.cancelled() => Answer::error(Failure::new(FailureKind::Cancelled, "request cancelled")),
            _ = wait_deadline => timeout_answer(&request, &progress),
            answer = operation.execute(request.arguments.clone(), context) => answer,
        };
        drop(guard);
        self.checked_answer(answer, &description.spec)
    }
    /// Open a stream row. Its events arrive until the stream ends, the
    /// caller drops `Events`, or `cancellation` fires; streams have no
    /// deadline.
    pub async fn open_stream(
        &self,
        request: RunnableRequest,
        origin: Origin,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> Result<Events, Failure> {
        let (description, row) = self.admit(&request, &caller)?;
        let Row::Stream(stream) = row else {
            return Err(bad_request(format!(
                "{} is not a stream",
                request.operation
            )));
        };
        let context = self.context(&request, description, origin, caller, None, cancellation);
        stream.open(request.arguments.clone(), context).await
    }
    pub fn render_event(&self, operation: &str, event: Value) -> Result<String, Failure> {
        match self.row(operation) {
            Some((_, Row::Stream(stream))) => stream.render_event(event),
            _ => Err(bad_request(format!("{operation} is not a stream"))),
        }
    }
    fn checked_answer(&self, answer: Answer<Value>, spec: &OperationSpec) -> Answer<Value> {
        let Answer {
            outcome,
            mut envelope,
        } = answer;
        let outcome = match outcome {
            Outcome::Error(error) => Outcome::Error(self.checked_failure(error, spec)),
            Outcome::Degraded { missing, .. } if missing.is_empty() => {
                Outcome::Error(Failure::new(
                    FailureKind::Io,
                    format!(
                        "operation {} answered degraded without naming what is missing",
                        spec.name
                    ),
                ))
            }
            other => other,
        };
        envelope.mark_lower_bounds();
        Answer { outcome, envelope }
    }
    fn checked_failure(&self, error: Failure, spec: &OperationSpec) -> Failure {
        if error
            .code
            .as_ref()
            .is_some_and(|code| self.failure_codes.get(code) != Some(&error.kind))
        {
            return Failure::new(
                FailureKind::Io,
                "operation returned an unregistered or misclassified product failure code",
            );
        }
        let infrastructure = matches!(
            error.kind,
            FailureKind::BadRequest
                | FailureKind::Timeout
                | FailureKind::Cancelled
                | FailureKind::Conflict
                | FailureKind::Io
        );
        if !infrastructure && !spec.failures.contains(&error.kind) {
            return Failure::new(
                FailureKind::Io,
                format!(
                    "operation {} returned undeclared failure {}",
                    spec.name,
                    error.kind.name()
                ),
            );
        }
        error
    }
    /// The answer's value as text, from its operation's renderer; a failure
    /// as its kind and message.
    pub fn render_text(&self, operation: &str, answer: &Answer<Value>) -> Result<String, Failure> {
        match &answer.outcome {
            Outcome::Error(error) => Ok(format!(
                "error ({}): {}\n",
                error.kind.name(),
                terminal_text(&error.message)
            )),
            Outcome::Ok(value) | Outcome::Empty(value) | Outcome::Degraded { value, .. } => {
                match self.row(operation) {
                    Some((_, Row::Request(row))) => row.render_text(value.clone()),
                    _ => Err(bad_request(format!("{operation} has no answer to render"))),
                }
            }
        }
    }
}

fn timeout_answer(request: &RunnableRequest, progress: &Mutex<Progress>) -> Answer<Value> {
    let progress = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let scanned = progress.rows_scanned.map_or_else(
        || "with rows scanned unknown".into(),
        |rows| format!("after {rows} rows scanned"),
    );
    let stage = if progress.stage.is_empty() {
        "admission"
    } else {
        &progress.stage
    };
    let failure = Failure::new(
        FailureKind::Timeout,
        format!("deadline passed during {stage} {scanned}"),
    )
    .with_hint("narrow the request or lift its deadline");
    let mut answer = Answer::error(failure);
    if let Some(narrower) = progress.narrower {
        answer = answer.with_next("narrower request", narrower);
    }
    let mut lifted = request.clone();
    lifted.deadline = Deadline::None;
    answer.with_next("complete without a deadline", lifted)
}

/// Lowercase letters, digits and `separator`, not empty.
pub(crate) fn valid_word(word: &str, separator: char) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == separator)
}

pub(crate) fn terminal_text(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                c.escape_default().collect::<Vec<_>>()
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
