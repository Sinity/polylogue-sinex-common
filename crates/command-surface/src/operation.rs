//! Typed operations and one dispatch boundary shared by every adapter.
use crate::{
    Answer, Authorship, Budget, Deadline, Failure, FailureKind, LatencyClass, LeftOut, Outcome,
    RunnableRequest, Section, Via, approximate_tokens, bad_request,
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
use tokio::time::Instant;
use tokio_util::sync::{CancellationToken, DropGuard};

pub type OperationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Explicit text presentation, supplied by the accepting client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TextStyle {
    #[default]
    Plain,
    Ansi,
}

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
#[cfg_attr(feature = "typescript", derive(ts_rs::TS))]
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
    /// The daemon answers it once. Only a named tool exposes it over MCP.
    Request {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool: Option<String>,
    },
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
    /// One word by position, with lists and objects written as JSON.
    /// Required when the request type requires it.
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
    /// Split one repeated CLI value into several textual items. HTTP pairs
    /// and canonical JSON retain their own value representation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_delimiter: Option<char>,
    /// Named in the MCP tool list. An unlisted field is still accepted and
    /// checked by its row at dispatch; a product's explain answer lists it
    /// (`OperationCatalog::family_schemas`).
    #[serde(default = "listed", skip_serializing_if = "is_listed")]
    pub listed: bool,
}
impl ArgumentSpec {
    pub fn new(name: impl Into<String>, form: ArgumentForm, help: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            form,
            help: help.into(),
            short: None,
            calling_process: false,
            value_delimiter: None,
            listed: true,
        }
    }
    /// Leave the field out of the MCP tool list; it stays accepted.
    pub fn unlisted(mut self) -> Self {
        self.listed = false;
        self
    }
    pub fn delimiter(mut self, delimiter: char) -> Self {
        self.value_delimiter = Some(delimiter);
        self
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
    /// Empty when the row has no command-line surface.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
fn is_listed(listed: &bool) -> bool {
    *listed
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
            http_route: (!command.is_empty()).then(|| format!("/v1/{}", command.join("/"))),
            name: name.into(),
            command,
            summary: summary.into(),
            reason: reason.into(),
            kind: OperationKind::Request {
                tool: Some(tool.into()),
            },
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
            OperationKind::Request { tool } => tool.as_deref(),
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
    /// A retried call with the same `request_id` is applied once. A family
    /// whose rows start a new action on every call says no.
    pub idempotent: bool,
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
    /// Access for this typed request. The declaration advertises the row's maximum.
    fn required_access(
        &self,
        _request: &Self::Request,
        declared: Access,
        _caller: &Caller,
    ) -> Access {
        declared
    }
    /// Whether this typed request has a cancellable read phase.
    fn has_read_phase(&self, _request: &Self::Request) -> bool {
        true
    }
    fn execute(
        &self,
        request: Self::Request,
        context: RequestContext,
    ) -> OperationFuture<'_, Answer<Self::Response>>;
}

/// Typed stream items include late failures as well as successful events.
pub type EventReceiver<E> = tokio::sync::mpsc::Receiver<Result<E, Failure>>;

/// One stream row: opening it yields its events until the sender closes.
pub trait StreamOperation: Send + Sync + 'static {
    type Request: Serialize + DeserializeOwned + JsonSchema + Send + 'static;
    type Event: Serialize + DeserializeOwned + JsonSchema + Send + 'static;
    fn open(
        &self,
        request: Self::Request,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<EventReceiver<Self::Event>, Failure>>;
}

/// One request row with both a bounded answer and a typed event delivery.
pub trait TransferOperation:
    Operation + StreamOperation<Request = <Self as Operation>::Request>
{
    fn streaming(&self, request: &<Self as Operation>::Request) -> bool;
}

/// A stream's typed failures end delivery without a successful terminal answer.
pub struct Events {
    events: Option<Box<dyn ErasedEvents>>,
    context: RequestContext,
    catalog: Arc<OperationCatalog>,
    done: bool,
    _guard: DropGuard,
}
impl Events {
    pub async fn next(&mut self) -> Option<Result<Value, Failure>> {
        if self.done {
            return None;
        }
        let event = tokio::select! {
            biased;
            failure = self.context.wait_for_read_stop() => Some(Err(failure)),
            event = self.events.as_mut().expect("active stream has a receiver").receive() => event,
        };
        if event.as_ref().is_none_or(Result::is_err) {
            self.done = true;
            self.events = None;
            self.context.cancellation.cancel();
        }
        event.map(|value| {
            value.map_err(|error| {
                let spec = self
                    .catalog
                    .spec_named(&self.context.request().operation)
                    .expect("bound stream is declared");
                self.catalog.checked_failure(error, spec)
            })
        })
    }
}
trait ErasedEvents: Send {
    fn receive(&mut self) -> OperationFuture<'_, Option<Result<Value, Failure>>>;
}
impl<T: Serialize + Send + 'static> ErasedEvents for EventReceiver<T> {
    fn receive(&mut self) -> OperationFuture<'_, Option<Result<Value, Failure>>> {
        Box::pin(async move {
            self.recv().await.map(|event| {
                event.and_then(|event| {
                    serde_json::to_value(event)
                        .map_err(|error| Failure::new(FailureKind::Io, error.to_string()))
                })
            })
        })
    }
}

enum Phase {
    Reading,
    OwnedWrite,
    Stopped(Failure),
}
struct ReadLifetime {
    phase: Phase,
    deadline: Option<Instant>,
    parent: CancellationToken,
}

#[derive(Clone)]
pub struct RequestContext {
    /// How the request arrived, set by the transport that accepted it; a
    /// caller cannot set this in JSON. Follow-ups are written for it: an MCP
    /// tool call for `mcp`, a command line otherwise.
    pub via: Via,
    pub request_id: String,
    pub cancellation: CancellationToken,
    caller: Caller,
    request: Arc<RunnableRequest>,
    lifetime: Arc<Mutex<ReadLifetime>>,
    budget_tokens: crate::TokenBudget,
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
    evidence: Option<crate::CallerEvidence>,
    access: Access,
}
impl Default for Caller {
    fn default() -> Self {
        Self {
            scope: String::new(),
            identity: None,
            authorship: None,
            evidence: None,
            access: Access::Read,
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
    /// What the transport observed about the caller. An MCP call through
    /// `mcp_as` (stdio, or an MCP frame on the socket) computes its author
    /// from this and the call's own `_meta`, the same way on every path.
    pub fn with_evidence(mut self, evidence: crate::CallerEvidence) -> Self {
        self.evidence = Some(evidence);
        self
    }
    pub fn evidence(&self) -> Option<&crate::CallerEvidence> {
        self.evidence.as_ref()
    }
    /// The caller of one MCP call: with evidence, its author is computed by
    /// `author_of` over that evidence and the session the call names in
    /// `_meta.session` (`harness:native`). Without evidence the server's
    /// authorship stands.
    pub(crate) fn for_mcp_call(&self, metadata: Option<&Value>) -> Self {
        let mut caller = self.clone();
        if let Some(evidence) = &self.evidence {
            let mut evidence = evidence.clone();
            evidence.call_session = metadata
                .and_then(|meta| meta.get("session"))
                .and_then(Value::as_str)
                .and_then(crate::AgentSession::parse);
            caller.authorship = Some(crate::author_of(&evidence, Via::Mcp, None));
            caller.evidence = Some(evidence);
        }
        caller
    }
    /// The highest access the caller's credential grants. A caller grants
    /// read access until the server says more.
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

#[derive(Clone, Debug, Default)]
struct Progress {
    stage: String,
    rows_scanned: Option<u64>,
    narrower: Option<RunnableRequest>,
}
impl RequestContext {
    /// The exact accepted envelope, including its original argument object.
    pub fn request(&self) -> &RunnableRequest {
        &self.request
    }
    pub fn read_deadline(&self) -> Option<Instant> {
        self.lifetime
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .deadline
    }
    /// Atomically win ownership before handing a mutation to its writer.
    pub fn finish_read_phase(&self) -> Result<(), Failure> {
        self.read_state(true)
    }
    fn read_state(&self, finish: bool) -> Result<(), Failure> {
        let mut lifetime = self
            .lifetime
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match &lifetime.phase {
            Phase::OwnedWrite => return Ok(()),
            Phase::Stopped(failure) => return Err(failure.clone()),
            Phase::Reading => {}
        }
        let failure = if lifetime.parent.is_cancelled() || self.cancellation.is_cancelled() {
            Some(Failure::new(FailureKind::Cancelled, "request cancelled"))
        } else if lifetime
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Some(timeout_failure(&self.progress))
        } else {
            None
        };
        if let Some(failure) = failure {
            lifetime.phase = Phase::Stopped(failure.clone());
            self.cancellation.cancel();
            return Err(failure);
        }
        if finish {
            lifetime.phase = Phase::OwnedWrite;
        }
        Ok(())
    }
    async fn wait_for_read_stop(&self) -> Failure {
        let (parent, deadline) = {
            let lifetime = self
                .lifetime
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (lifetime.parent.clone(), lifetime.deadline)
        };
        tokio::select! {
            biased;
            _ = parent.cancelled() => {},
            _ = self.cancellation.cancelled() => {},
            _ = async { match deadline { Some(at) => tokio::time::sleep_until(at).await, None => std::future::pending().await } } => {},
        }
        match self.read_state(false) {
            Err(failure) => failure,
            Ok(()) => std::future::pending().await,
        }
    }

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
    pub fn follow_up(&self, request: &RunnableRequest) -> Result<String, Failure> {
        self.forms.follow_up(self.via, request)
    }
    /// Fit a typed answer's logical entries once. Its outcome is preserved;
    /// this owner updates the shown count, omissions and cap note together.
    /// JSON carries the value as data, so a text budget does not cut its rows.
    pub fn fit(&self, mut answer: Answer<Vec<Section>>, budget: Budget) -> Answer<Vec<Section>> {
        if answer.is_error() {
            return answer;
        }
        let budget = if self.format == crate::Format::Json {
            Budget::new(crate::TokenBudget::Unlimited, 0, None)
        } else {
            budget
        };
        if let Some(note) = budget.note() {
            answer.envelope.notes.push(note);
        }
        let mut rest = answer.envelope.clone();
        // Reserve the count line in its longest form, including lower bounds
        // that dispatch can establish after fitting.
        rest.mark_lower_bounds(true);
        if let Some(count) = rest.count {
            rest.shown = Some(count.value);
        }
        let held = approximate_tokens(&self.forms.footer(self.via, &answer.outcome, &rest));
        let budget = budget.reserve(held);
        let mut left_out = Vec::new();
        answer.outcome = answer.outcome.map(|sections| {
            let fitted = crate::fit(sections, budget, &approximate_tokens, &|item: &LeftOut| {
                self.forms.left_out_line(self.via, item)
            });
            left_out = fitted.left_out;
            fitted.sections
        });
        answer.envelope.shown = answer.value().map(|sections| {
            sections
                .iter()
                .map(|section| section.entries.len() as u64)
                .sum()
        });
        answer.envelope.left_out.extend(left_out);
        answer
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
        self.read_state(false)
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
    /// A request row may also expose finite typed event delivery.
    #[serde(
        rename = "eventSchema",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub event_schema: Option<Value>,
}

/// What every surface needs to write a request back: the product name, the
/// rows' descriptions and the tool families. Shared with request contexts.
#[derive(Clone)]
pub(crate) struct Forms {
    pub(crate) product: String,
    pub(crate) descriptions: Vec<OperationDescription>,
    pub(crate) tools: Vec<ToolFamily>,
    pub(crate) options: Vec<crate::RequestOption>,
    pub(crate) cli_globals: Vec<ArgumentSpec>,
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

struct Policy {
    access: Access,
    read_phase: bool,
}
trait ErasedOperation: Send + Sync {
    fn policy(
        &self,
        arguments: &Value,
        declared: Access,
        caller: &Caller,
    ) -> Result<Policy, Failure>;
    fn streaming(&self, arguments: &Value) -> Result<bool, Failure>;
    fn execute(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Answer<Value>, Failure>>;
    fn open(
        &self,
        _arguments: Value,
        _context: RequestContext,
    ) -> OperationFuture<'_, Result<Box<dyn ErasedEvents>, Failure>> {
        Box::pin(async { Err(bad_request("operation has no event delivery")) })
    }
}
struct RequestHandler<O>(O);
struct TransferHandler<O>(O);
fn policy<O: Operation>(
    operation: &O,
    arguments: &Value,
    declared: Access,
    caller: &Caller,
) -> Result<Policy, Failure> {
    let request = serde_json::from_value(arguments.clone())
        .map_err(|error| bad_request(error.to_string()))?;
    Ok(Policy {
        access: operation.required_access(&request, declared, caller),
        read_phase: operation.has_read_phase(&request),
    })
}
fn execute_operation<O: Operation>(
    operation: &O,
    arguments: Value,
    context: RequestContext,
) -> OperationFuture<'_, Result<Answer<Value>, Failure>> {
    Box::pin(async move {
        let request =
            serde_json::from_value(arguments).map_err(|error| bad_request(error.to_string()))?;
        let answer = Operation::execute(operation, request, context).await;
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
        Ok(Answer { outcome, envelope })
    })
}
impl<O: Operation> ErasedOperation for RequestHandler<O> {
    fn policy(
        &self,
        arguments: &Value,
        declared: Access,
        caller: &Caller,
    ) -> Result<Policy, Failure> {
        policy(&self.0, arguments, declared, caller)
    }
    fn streaming(&self, _arguments: &Value) -> Result<bool, Failure> {
        Ok(false)
    }
    fn execute(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Answer<Value>, Failure>> {
        execute_operation(&self.0, arguments, context)
    }
}
impl<O: TransferOperation> ErasedOperation for TransferHandler<O> {
    fn policy(
        &self,
        arguments: &Value,
        declared: Access,
        caller: &Caller,
    ) -> Result<Policy, Failure> {
        policy(&self.0, arguments, declared, caller)
    }
    fn streaming(&self, arguments: &Value) -> Result<bool, Failure> {
        let request = serde_json::from_value(arguments.clone())
            .map_err(|error| bad_request(error.to_string()))?;
        Ok(TransferOperation::streaming(&self.0, &request))
    }
    fn execute(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Answer<Value>, Failure>> {
        execute_operation(&self.0, arguments, context)
    }
    fn open(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Box<dyn ErasedEvents>, Failure>> {
        open_events(&self.0, arguments, context)
    }
}
trait ErasedStream: Send + Sync {
    fn open(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Box<dyn ErasedEvents>, Failure>>;
}
fn open_events<S: StreamOperation>(
    stream: &S,
    arguments: Value,
    context: RequestContext,
) -> OperationFuture<'_, Result<Box<dyn ErasedEvents>, Failure>> {
    Box::pin(async move {
        let request =
            serde_json::from_value(arguments).map_err(|error| bad_request(error.to_string()))?;
        let receiver = stream.open(request, context).await?;
        Ok(Box::new(receiver) as Box<dyn ErasedEvents>)
    })
}
impl<S: StreamOperation> ErasedStream for S {
    fn open(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Result<Box<dyn ErasedEvents>, Failure>> {
        open_events(self, arguments, context)
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

/// Immutable declarations, schemas and presentation, independent of handlers.
pub struct OperationCatalog {
    forms: Arc<Forms>,
    renderers: Vec<Presentation>,
    deadlines: Deadlines,
    failure_codes: BTreeMap<String, FailureKind>,
    default_operation: Option<(String, fn(&str) -> bool)>,
}
type Renderer =
    Box<dyn Fn(Value, Option<TextStyle>) -> Result<Option<String>, Failure> + Send + Sync>;
#[derive(Default)]
struct Presentation {
    answer: Option<Renderer>,
    event: Option<Renderer>,
}
/// Builds the declaration owner before real runtime handlers are bound.
pub struct CatalogBuilder {
    catalog: OperationCatalog,
}
impl std::ops::Deref for CatalogBuilder {
    type Target = OperationCatalog;
    fn deref(&self) -> &Self::Target {
        &self.catalog
    }
}
impl OperationCatalog {
    pub fn builder(product: impl Into<String>, deadlines: Deadlines) -> CatalogBuilder {
        CatalogBuilder {
            catalog: Self {
                forms: Arc::new(Forms {
                    product: product.into(),
                    descriptions: vec![],
                    tools: vec![],
                    options: vec![],
                    cli_globals: vec![],
                }),
                renderers: vec![],
                deadlines,
                failure_codes: BTreeMap::new(),
                default_operation: None,
            },
        }
    }
    pub fn product(&self) -> &str {
        &self.forms.product
    }
    pub fn tools(&self) -> &[ToolFamily] {
        &self.forms.tools
    }
    /// Text options owned by the calling process, available on every CLI row.
    pub fn cli_globals(&self) -> &[ArgumentSpec] {
        &self.forms.cli_globals
    }
    pub fn accepts(&self, option: crate::RequestOption) -> bool {
        self.forms.options.contains(&option)
    }
    pub fn descriptions(&self) -> &[OperationDescription] {
        &self.forms.descriptions
    }
    /// The named operation's spec, borrowed from the catalog without cloning.
    pub fn spec_named(&self, name: &str) -> Option<&OperationSpec> {
        self.forms.description(name).map(|d| &d.spec)
    }
    pub(crate) fn default_operation(&self) -> Option<&OperationDescription> {
        self.default_operation
            .as_ref()
            .and_then(|(name, _)| self.forms.description(name))
    }
    pub(crate) fn accepts_default_first(&self, word: &str) -> bool {
        self.default_operation
            .as_ref()
            .is_some_and(|(_, accepts)| accepts(word))
    }
    fn index(&self, name: &str) -> Option<usize> {
        self.forms
            .descriptions
            .iter()
            .position(|d| d.spec.name == name)
    }
    pub fn deadline(&self, request: &RunnableRequest) -> Option<Duration> {
        self.spec_named(&request.operation)
            .and_then(|spec| request.deadline.duration(self.deadlines.get(spec.latency)))
    }
    /// Connection/readiness admission only; not an accepted request's reply timeout.
    pub fn connection_timeout(&self, request: &RunnableRequest) -> Option<Duration> {
        self.deadline(request)
            .and_then(|d| d.checked_add(Duration::from_millis(50)))
    }
    pub(crate) fn forms(&self) -> &Arc<Forms> {
        &self.forms
    }
    pub fn render_event(
        &self,
        operation: &str,
        event: Value,
        style: TextStyle,
    ) -> Result<String, Failure> {
        let index = self
            .index(operation)
            .ok_or_else(|| bad_request(format!("{operation} is not a stream")))?;
        if self.descriptions()[index].spec.kind != OperationKind::Stream
            && self.descriptions()[index].event_schema.is_none()
        {
            return Err(bad_request(format!("{operation} is not a stream")));
        }
        self.renderers[index]
            .event
            .as_ref()
            .expect("stream declaration has a renderer")(event, Some(style))
        .map(|text| text.expect("text style requests rendering"))
    }
    fn answer_renderer(&self, operation: &str) -> Result<&Renderer, Failure> {
        let index = self
            .index(operation)
            .ok_or_else(|| bad_request(format!("{operation} has no answer to render")))?;
        if !matches!(
            self.descriptions()[index].spec.kind,
            OperationKind::Request { .. }
        ) {
            return Err(bad_request(format!("{operation} has no answer to render")));
        }
        Ok(self.renderers[index]
            .answer
            .as_ref()
            .expect("request declaration has a renderer"))
    }
    /// Decode a response through its declared type without rendering text or
    /// modifying the original answer or envelope. Failures carry no response value.
    pub fn validate_answer(&self, operation: &str, answer: &Answer<Value>) -> Result<(), Failure> {
        let renderer = self.answer_renderer(operation)?;
        if let Some(value) = answer.value() {
            renderer(value.clone(), None)?;
        }
        Ok(())
    }
    /// Render an answer without constructing or binding a handler.
    pub fn render_text(
        &self,
        operation: &str,
        answer: &Answer<Value>,
        style: TextStyle,
    ) -> Result<String, Failure> {
        match &answer.outcome {
            Outcome::Error(error) => Ok(format!(
                "error ({}): {}\n",
                error.kind.name(),
                terminal_text(&error.message)
            )),
            Outcome::Ok(value) | Outcome::Empty(value) | Outcome::Degraded { value, .. } => {
                self.answer_renderer(operation)?(value.clone(), Some(style))
                    .map(|text| text.expect("text style requests rendering"))
            }
        }
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
}
impl CatalogBuilder {
    pub fn finish(self) -> Arc<OperationCatalog> {
        Arc::new(self.catalog)
    }
    /// Declare one optional text-valued CLI option for the calling process.
    /// Names use underscores; their flags use hyphens. Values never enter a request.
    pub fn declare_cli_global(
        &mut self,
        name: impl Into<String>,
        help: impl Into<String>,
    ) -> Result<(), Failure> {
        let argument = ArgumentSpec::new(name, ArgumentForm::Named, help).calling_process();
        if !valid_word(&argument.name, '_')
            || GLOBAL_FLAGS.contains(&argument.flag().trim_start_matches('-'))
            || self
                .cli_globals()
                .iter()
                .any(|other| other.name == argument.name)
        {
            return Err(bad_request(format!(
                "CLI global {} is not a free name",
                argument.name
            )));
        }
        for description in self
            .descriptions()
            .iter()
            .filter(|d| !d.spec.command.is_empty())
        {
            crate::arguments::check_globals(description, std::slice::from_ref(&argument))?;
        }
        Arc::make_mut(&mut self.catalog.forms)
            .cli_globals
            .push(argument);
        Ok(())
    }
    /// Declare an MCP tool before registering its rows.
    pub fn declare_tool(&mut self, tool: ToolFamily) -> Result<(), Failure> {
        if !valid_word(&tool.name, '_') || tool.description.is_empty() {
            return Err(bad_request("a tool needs a name and a description"));
        }
        if self.catalog.forms.tools.iter().any(|t| t.name == tool.name) {
            return Err(bad_request(format!("tool {} is declared twice", tool.name)));
        }
        Arc::make_mut(&mut self.catalog.forms).tools.push(tool);
        Ok(())
    }
    /// Serve a request option on every row: tools offer it, and dispatch
    /// takes it. A table serves only `request_id` and `deadline` until it
    /// names more.
    pub fn accept(&mut self, option: crate::RequestOption) {
        let options = &mut Arc::make_mut(&mut self.catalog.forms).options;
        if !options.contains(&option) {
            options.push(option);
            options.sort();
        }
    }
    /// The row a command line runs when its first words name no command
    /// (`polylogue QUERY`). The product predicate checks the first lexical
    /// operand before positional words are joined. Empty input and explicit
    /// structured `--request` input have no first operand to check.
    pub fn set_default_operation(
        &mut self,
        name: impl Into<String>,
        accepts_first: fn(&str) -> bool,
    ) -> Result<(), Failure> {
        let name = name.into();
        let spec = self
            .spec_named(&name)
            .ok_or_else(|| bad_request(format!("unknown operation {name}")))?;
        if matches!(spec.kind, OperationKind::Stream) {
            return Err(bad_request("a stream cannot be the default operation"));
        }
        if spec.command.is_empty() {
            return Err(bad_request(
                "the default operation needs a command-line surface",
            ));
        }
        self.catalog.default_operation = Some((name, accepts_first));
        Ok(())
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
        if let Some(existing) = self.catalog.failure_codes.get(&code) {
            if *existing == kind {
                return Ok(());
            }
            return Err(bad_request(format!(
                "failure code {code} already has a different classification"
            )));
        }
        self.catalog.failure_codes.insert(code, kind);
        Ok(())
    }
    pub fn declare<A: JsonSchema, R: Serialize + DeserializeOwned + JsonSchema + 'static>(
        &mut self,
        spec: OperationSpec,
        render: fn(&R, TextStyle) -> String,
    ) -> Result<(), Failure> {
        if !matches!(spec.kind, OperationKind::Request { .. }) {
            return Err(bad_request("request declaration needs a request kind"));
        }
        self.add(
            OperationDescription {
                spec,
                input_schema: schema_for!(A).to_value(),
                output_schema: schema_for!(Answer<R>).to_value(),
                event_schema: None,
            },
            Presentation {
                answer: Some(renderer(render)),
                event: None,
            },
        )
    }
    pub fn declare_stream<A: JsonSchema, E: Serialize + DeserializeOwned + JsonSchema + 'static>(
        &mut self,
        spec: OperationSpec,
        render: fn(&E, TextStyle) -> String,
    ) -> Result<(), Failure> {
        if spec.kind != OperationKind::Stream {
            return Err(bad_request("stream declaration needs a stream kind"));
        }
        self.add(
            OperationDescription {
                spec,
                input_schema: schema_for!(A).to_value(),
                output_schema: schema_for!(E).to_value(),
                event_schema: None,
            },
            Presentation {
                answer: None,
                event: Some(renderer(render)),
            },
        )
    }
    pub fn declare_transfer<
        A: JsonSchema,
        R: Serialize + DeserializeOwned + JsonSchema + 'static,
        E: Serialize + DeserializeOwned + JsonSchema + 'static,
    >(
        &mut self,
        spec: OperationSpec,
        answer: fn(&R, TextStyle) -> String,
        event: fn(&E, TextStyle) -> String,
    ) -> Result<(), Failure> {
        if !matches!(spec.kind, OperationKind::Request { .. }) {
            return Err(bad_request("transfer declaration needs a request kind"));
        }
        self.add(
            OperationDescription {
                spec,
                input_schema: schema_for!(A).to_value(),
                output_schema: schema_for!(Answer<R>).to_value(),
                event_schema: Some(schema_for!(E).to_value()),
            },
            Presentation {
                answer: Some(renderer(answer)),
                event: Some(renderer(event)),
            },
        )
    }
    pub fn declare_local<A: JsonSchema>(&mut self, spec: OperationSpec) -> Result<(), Failure> {
        if spec.kind != OperationKind::Local {
            return Err(bad_request("local declaration needs a local kind"));
        }
        self.add(
            OperationDescription {
                spec,
                input_schema: schema_for!(A).to_value(),
                output_schema: Value::Bool(true),
                event_schema: None,
            },
            Presentation::default(),
        )
    }
    fn add(
        &mut self,
        description: OperationDescription,
        renderer: Presentation,
    ) -> Result<(), Failure> {
        let spec = &description.spec;
        let product = &self.catalog.forms.product;
        if !valid_word(&product.replace('_', "-"), '-')
            || spec.name.is_empty()
            || (spec.kind == OperationKind::Local && spec.command.is_empty())
            || spec.command.iter().any(|word| !valid_word(word, '-'))
            || spec.summary.is_empty()
            || spec.reason.is_empty()
        {
            return Err(bad_request(
                "operation needs a name, valid command words, summary and reason",
            ));
        }
        match (&spec.kind, &spec.http_route) {
            (_, None) => {}
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
        for existing in &self.catalog.forms.descriptions {
            let other = &existing.spec;
            if other.name == spec.name
                || (!spec.command.is_empty() && other.command == spec.command)
                || (other.http_route.is_some() && other.http_route == spec.http_route)
            {
                return Err(bad_request(format!(
                    "operation {} collides with {}",
                    spec.name, other.name
                )));
            }
        }
        if let OperationKind::Request { tool: Some(tool) } = &spec.kind {
            if !self.catalog.forms.tools.iter().any(|t| &t.name == tool) {
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
        if !spec.command.is_empty() {
            crate::arguments::check(&description)?;
            crate::arguments::check_globals(&description, self.cli_globals())?;
        }
        let mut forms = (*self.catalog.forms).clone();
        forms.descriptions.push(description);
        if let Some(tool) = forms.descriptions.last().and_then(|d| d.spec.tool()) {
            // A family's schema must merge.
            crate::tools::tool_input(&forms, tool)?;
        }
        self.catalog.forms = Arc::new(forms);
        self.catalog.renderers.push(renderer);
        Ok(())
    }
}
fn renderer<R: DeserializeOwned + 'static>(render: fn(&R, TextStyle) -> String) -> Renderer {
    Box::new(move |value, style| {
        serde_json::from_value::<R>(value)
            .map(|typed| style.map(|style| render(&typed, style)))
            .map_err(|error| Failure::new(FailureKind::Io, error.to_string()))
    })
}

/// Real runtime handlers bound to one immutable declaration owner.
pub struct OperationTable {
    catalog: Arc<OperationCatalog>,
    rows: Vec<Option<Row>>,
}
impl std::ops::Deref for OperationTable {
    type Target = OperationCatalog;
    fn deref(&self) -> &Self::Target {
        &self.catalog
    }
}
impl OperationTable {
    pub fn new(catalog: Arc<OperationCatalog>) -> Self {
        let rows = catalog
            .descriptions()
            .iter()
            .map(|d| {
                if d.spec.kind == OperationKind::Local {
                    Some(Row::Local)
                } else {
                    None
                }
            })
            .collect();
        Self { catalog, rows }
    }
    pub fn catalog(&self) -> &Arc<OperationCatalog> {
        &self.catalog
    }
    pub fn bind<O: Operation>(&mut self, name: &str, operation: O) -> Result<(), Failure> {
        let index = self.binding(
            name,
            schema_for!(O::Request).to_value(),
            schema_for!(Answer<O::Response>).to_value(),
            true,
            None,
        )?;
        self.rows[index] = Some(Row::Request(Box::new(RequestHandler(operation))));
        Ok(())
    }
    pub fn bind_stream<S: StreamOperation>(
        &mut self,
        name: &str,
        stream: S,
    ) -> Result<(), Failure> {
        let index = self.binding(
            name,
            schema_for!(S::Request).to_value(),
            schema_for!(S::Event).to_value(),
            false,
            None,
        )?;
        self.rows[index] = Some(Row::Stream(Box::new(stream)));
        Ok(())
    }
    pub fn bind_transfer<O: TransferOperation>(
        &mut self,
        name: &str,
        operation: O,
    ) -> Result<(), Failure> {
        let index = self.binding(
            name,
            schema_for!(<O as Operation>::Request).to_value(),
            schema_for!(Answer<O::Response>).to_value(),
            true,
            Some(schema_for!(O::Event).to_value()),
        )?;
        self.rows[index] = Some(Row::Request(Box::new(TransferHandler(operation))));
        Ok(())
    }
    /// Decide event delivery from the admitted request type, never JSON field names.
    pub fn streaming(&self, request: &RunnableRequest) -> Result<bool, Failure> {
        let (_, row) = self.row(&request.operation).ok_or_else(|| {
            bad_request(format!(
                "unknown or unbound operation {}",
                request.operation
            ))
        })?;
        match row {
            Row::Request(operation) => operation.streaming(&request.arguments),
            Row::Stream(_) => Ok(true),
            Row::Local => Ok(false),
        }
    }
    fn binding(
        &self,
        name: &str,
        input: Value,
        output: Value,
        request: bool,
        event_schema: Option<Value>,
    ) -> Result<usize, Failure> {
        let index = self
            .catalog
            .index(name)
            .ok_or_else(|| bad_request(format!("unknown operation {name}")))?;
        let description = &self.catalog.descriptions()[index];
        let kind_matches = if request {
            matches!(description.spec.kind, OperationKind::Request { .. })
        } else {
            description.spec.kind == OperationKind::Stream
        };
        if !kind_matches
            || description.input_schema != input
            || description.output_schema != output
            || description.event_schema != event_schema
        {
            return Err(bad_request(format!(
                "binding {name} does not match its declared kind and schemas"
            )));
        }
        if self.rows[index].is_some() {
            return Err(bad_request(format!("operation {name} is already bound")));
        }
        Ok(index)
    }
    fn row(&self, name: &str) -> Option<(&OperationDescription, &Row)> {
        let index = self.catalog.index(name)?;
        Some((
            &self.catalog.descriptions()[index],
            self.rows[index].as_ref()?,
        ))
    }
    pub async fn call(
        &self,
        request: RunnableRequest,
        via: Via,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        self.call_as(request, via, Caller::default(), cancellation)
            .await
    }
    pub async fn call_as(
        &self,
        request: RunnableRequest,
        via: Via,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        self.call_with(request, via, caller, None, cancellation)
            .await
    }
    pub(crate) async fn call_with(
        &self,
        request: RunnableRequest,
        via: Via,
        caller: Caller,
        metadata: Option<Value>,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        let authorship = caller.authorship.clone();
        let mut answer = self
            .dispatch(request, via, caller, metadata, cancellation)
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
        if let Some(option) = crate::RequestOption::ALL
            .into_iter()
            .find(|option| option.used_by(request) && !self.accepts(*option))
        {
            return Err(bad_request(format!(
                "{} does not take {}",
                self.forms.product,
                option.name()
            )));
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
        if !matches!(row, Row::Request(_)) && description.spec.access > caller.access {
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
        via: Via,
        caller: Caller,
        metadata: Option<Value>,
        cancellation: CancellationToken,
        read_phase: bool,
    ) -> RequestContext {
        RequestContext {
            via,
            request_id: request.request_id.clone(),
            cancellation: CancellationToken::new(),
            caller,
            request: Arc::new(request.clone()),
            lifetime: Arc::new(Mutex::new(ReadLifetime {
                phase: if read_phase {
                    Phase::Reading
                } else {
                    Phase::OwnedWrite
                },
                deadline: if read_phase {
                    self.deadline(request)
                        .map(|duration| Instant::now() + duration)
                } else {
                    None
                },
                parent: cancellation,
            })),
            budget_tokens: request.budget_tokens,
            default_budget_tokens: self
                .spec_named(&request.operation)
                .and_then(|spec| spec.default_budget_tokens),
            cursor: request.cursor.clone(),
            fresh: request.fresh,
            format: request.format,
            metadata,
            forms: self.catalog.forms.clone(),
            progress: Arc::default(),
        }
    }
    async fn dispatch(
        &self,
        request: RunnableRequest,
        via: Via,
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
        let policy = match operation.policy(&request.arguments, description.spec.access, &caller) {
            Ok(policy) => policy,
            Err(failure) => return Answer::error(failure),
        };
        if policy.access > caller.access() {
            return Answer::error(access_failure(&description.spec, policy.access));
        }
        match operation.streaming(&request.arguments) {
            Ok(true) => return Answer::error(stream_transport_required()),
            Err(failure) => return Answer::error(failure),
            Ok(false) => {}
        }
        if cancellation.is_cancelled() {
            return Answer::error(Failure::new(FailureKind::Cancelled, "request cancelled"));
        }
        let context = self.context(
            &request,
            via,
            caller,
            metadata,
            cancellation,
            policy.read_phase,
        );
        let guard = context.cancellation.clone().drop_guard();
        let answer = tokio::select! {
            biased;
            failure = context.wait_for_read_stop() => {
                if failure.kind == FailureKind::Timeout { Ok(timeout_answer(&request, &context.progress)) } else { Err(failure) }
            },
            answer = operation.execute(request.arguments.clone(), context.clone()) => answer,
        };
        drop(guard);
        match answer {
            Ok(answer) => self.checked_answer(answer, &description.spec),
            Err(failure) => Answer::error(failure),
        }
    }
    /// Open ordinary streams or the event delivery of a transfer request.
    /// Opening and every late event share one absolute deadline and cancellation.
    pub async fn open_stream(
        &self,
        request: RunnableRequest,
        via: Via,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> Result<Events, Failure> {
        let (description, row) = self.admit(&request, &caller)?;
        let policy = match row {
            Row::Stream(_) => Policy {
                access: description.spec.access,
                read_phase: true,
            },
            Row::Request(operation) if operation.streaming(&request.arguments)? => {
                operation.policy(&request.arguments, description.spec.access, &caller)?
            }
            _ => {
                return Err(bad_request(format!(
                    "{} does not select event delivery",
                    request.operation
                )));
            }
        };
        if policy.access > caller.access() {
            return Err(access_failure(&description.spec, policy.access));
        }
        if cancellation.is_cancelled() {
            return Err(Failure::new(FailureKind::Cancelled, "request cancelled"));
        }
        let context = self.context(&request, via, caller, None, cancellation, policy.read_phase);
        let guard = context.cancellation.clone().drop_guard();
        let open = match row {
            Row::Stream(stream) => stream.open(request.arguments.clone(), context.clone()),
            Row::Request(operation) => operation.open(request.arguments.clone(), context.clone()),
            Row::Local => unreachable!("local row refused before opening"),
        };
        let events = tokio::select! { biased; failure = context.wait_for_read_stop() => Err(failure), events = open => events }?;
        Ok(Events {
            events: Some(events),
            context,
            catalog: self.catalog.clone(),
            done: false,
            _guard: guard,
        })
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
        let degraded = matches!(outcome, Outcome::Degraded { .. });
        envelope.mark_lower_bounds(degraded);
        Answer { outcome, envelope }
    }
}

fn access_failure(spec: &OperationSpec, access: Access) -> Failure {
    Failure::new(
        FailureKind::Refused,
        format!("{} needs {} access", spec.name, access.name()),
    )
}
fn stream_transport_required() -> Failure {
    bad_request(
        "event delivery requires a streaming transport; request a bounded page for this call",
    )
}

fn timeout_failure(progress: &Mutex<Progress>) -> Failure {
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
    Failure::new(
        FailureKind::Timeout,
        format!("deadline passed during {stage} {scanned}"),
    )
    .with_hint("narrow the request or lift its deadline")
}

fn timeout_answer(request: &RunnableRequest, progress: &Mutex<Progress>) -> Answer<Value> {
    let failure = timeout_failure(progress);
    let progress = progress
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
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
