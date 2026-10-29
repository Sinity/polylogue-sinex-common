//! Typed operations and one dispatch boundary shared by every adapter.
use crate::{
    Answer, Deadline, Failure, FailureKind, LatencyClass, Next, RunnableRequest, bad_request,
};
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    any::Any,
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub type OperationFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct OperationSpec {
    pub name: String,
    pub command: Vec<String>,
    pub summary: String,
    /// Why this operation cannot be a query plus an existing operation.
    pub reason: String,
    pub http_route: String,
    pub latency: LatencyClass,
    pub failures: Vec<FailureKind>,
    pub write: bool,
}
impl OperationSpec {
    pub fn mcp_name(&self, product: &str) -> String {
        format!("{}_{}", product, self.command.join("_"))
    }
}

/// One row. Its response type supplies every surface, including terminal text.
///
/// The returned future owns its work. Drop cancels it, and blocking workers
/// must check the context token at least every 100 ms. Do not spawn unowned
/// work. A write stores request identity and its result atomically with its
/// mutation in the product's transaction, so replay also survives restart.
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

#[derive(Clone)]
pub struct RequestContext {
    /// Computed at the transport boundary; a caller cannot set this in JSON.
    pub origin: Origin,
    pub request_id: String,
    pub cancellation: CancellationToken,
    caller: Caller,
    progress: Arc<Mutex<Progress>>,
}
/// Trusted product identity, supplied after authentication or peer inspection.
/// It is never deserialized from a request. Scope namespaces retry identities.
#[derive(Clone, Default)]
pub struct Caller {
    scope: String,
    identity: Option<Arc<dyn Any + Send + Sync>>,
}
impl Caller {
    pub fn authenticated<T: Any + Send + Sync>(scope: impl Into<String>, identity: T) -> Self {
        Self {
            scope: scope.into(),
            identity: Some(Arc::new(identity)),
        }
    }
    pub fn identity<T: Any>(&self) -> Option<&T> {
        self.identity.as_ref()?.downcast_ref()
    }
    pub fn scope(&self) -> &str {
        &self.scope
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
    rows_scanned: u64,
    narrower: Option<RunnableRequest>,
}
impl RequestContext {
    pub fn caller(&self) -> &Caller {
        &self.caller
    }
    /// Called between bounded work units. Also supplies the timeout diagnostic.
    pub fn progress(
        &self,
        stage: impl Into<String>,
        rows_scanned: u64,
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
    #[serde(rename = "outputSchema")]
    pub output_schema: Value,
}

trait ErasedOperation: Send + Sync {
    fn description(&self) -> &OperationDescription;
    fn execute(
        &self,
        arguments: Value,
        context: RequestContext,
    ) -> OperationFuture<'_, Answer<Value>>;
    fn render_text(&self, value: Value) -> Result<String, Failure>;
}
struct TypedOperation<O> {
    operation: O,
    description: OperationDescription,
}
impl<O: Operation> TypedOperation<O> {
    fn new(operation: O) -> Self {
        let description = OperationDescription {
            spec: operation.spec(),
            input_schema: schema_for!(O::Request).to_value(),
            output_schema: schema_for!(Answer<O::Response>).to_value(),
        };
        Self {
            operation,
            description,
        }
    }
}
impl<O: Operation> ErasedOperation for TypedOperation<O> {
    fn description(&self) -> &OperationDescription {
        &self.description
    }
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
            let answer = self.operation.execute(request, context).await;
            match answer {
                Answer::Error { error, notes, next } => Answer::Error { error, notes, next },
                other => match other.map(serde_json::to_value) {
                    Answer::Ok { value, notes, next } => match value {
                        Ok(value) => Answer::Ok { value, notes, next },
                        Err(e) => serialization_error(e),
                    },
                    Answer::Empty { value, notes, next } => match value {
                        Ok(value) => Answer::Empty { value, notes, next },
                        Err(e) => serialization_error(e),
                    },
                    Answer::Degraded { value, notes, next } => match value {
                        Ok(value) => Answer::Degraded { value, notes, next },
                        Err(e) => serialization_error(e),
                    },
                    Answer::Error { .. } => unreachable!("handled above"),
                },
            }
        })
    }
    fn render_text(&self, value: Value) -> Result<String, Failure> {
        serde_json::from_value(value)
            .map(|answer| self.operation.render_text(&answer))
            .map_err(|e| Failure::new(FailureKind::Io, e.to_string()))
    }
}
fn serialization_error(error: serde_json::Error) -> Answer<Value> {
    Answer::error(Failure::new(FailureKind::Io, error.to_string()))
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

type CachedAnswer = Arc<AsyncMutex<Option<(Value, Answer<Value>)>>>;

/// Heterogeneous typed table. Registration rejects ambiguity once at startup.
/// Completed writes are coalesced in process; products persist idempotency in
/// their own write transaction for retries after a daemon restart.
pub struct OperationTable {
    pub(crate) product: String,
    operations: Vec<Arc<dyn ErasedOperation>>,
    deadlines: Deadlines,
    writes: Mutex<HashMap<(String, String), CachedAnswer>>,
    failure_codes: HashMap<String, FailureKind>,
}
impl OperationTable {
    pub fn new(product: impl Into<String>, deadlines: Deadlines) -> Self {
        Self {
            product: product.into(),
            operations: vec![],
            deadlines,
            writes: Mutex::new(HashMap::new()),
            failure_codes: HashMap::new(),
        }
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
        let description = TypedOperation::new(operation);
        let spec = description.description().spec.clone();
        if self.product.is_empty()
            || !self
                .product
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            || spec.name.is_empty()
            || spec.command.is_empty()
            || spec.command.iter().any(|word| {
                word.is_empty() || !word.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
            })
            || !spec.http_route.starts_with('/')
            || spec.http_route.starts_with("//")
            || spec.http_route.contains(['?', '#', '\r', '\n'])
            || matches!(spec.http_route.as_str(), "/v1" | "/v1/commands")
            || spec.summary.is_empty()
            || spec.reason.is_empty()
        {
            return Err(bad_request(
                "operation needs a name, command words, route, summary and reason",
            ));
        }
        for existing in self.descriptions() {
            let other = existing.spec;
            if other.name == spec.name
                || other.command == spec.command
                || other.http_route == spec.http_route
                || other.mcp_name(&self.product) == spec.mcp_name(&self.product)
            {
                return Err(bad_request(format!(
                    "operation {} collides with {}",
                    spec.name, other.name
                )));
            }
        }
        self.operations.push(Arc::new(description));
        Ok(())
    }
    pub fn descriptions(&self) -> Vec<OperationDescription> {
        self.describe().cloned().collect()
    }
    pub fn describe(&self) -> impl Iterator<Item = &OperationDescription> {
        self.operations.iter().map(|op| op.description())
    }
    pub fn deadline(&self, request: &RunnableRequest) -> Option<Duration> {
        self.descriptions()
            .into_iter()
            .find(|d| d.spec.name == request.operation)
            .and_then(|d| {
                request
                    .deadline
                    .duration(self.deadlines.get(d.spec.latency))
            })
    }
    pub fn socket_timeout(&self, request: &RunnableRequest) -> Option<Duration> {
        self.deadline(request)
            .and_then(|d| d.checked_add(Duration::from_millis(50)))
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
        let Some(operation) = self
            .operations
            .iter()
            .find(|op| op.description().spec.name == request.operation)
        else {
            return Answer::error(Failure::new(
                FailureKind::NotFound,
                format!("unknown operation {}", request.operation),
            ));
        };
        if request.request_id.is_empty() {
            return Answer::error(bad_request("request_id must not be empty"));
        }
        let description = operation.description();
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
            return Answer::error(bad_request(format!("unknown argument {key}")));
        }
        let progress = Arc::new(Mutex::new(Progress::default()));
        let token = cancellation.child_token();
        let guard = token.clone().drop_guard();
        let context = RequestContext {
            origin,
            request_id: request.request_id.clone(),
            cancellation: token.clone(),
            caller: caller.clone(),
            progress: progress.clone(),
        };
        let execution = async {
            if description.spec.write {
                let cache = {
                    let mut writes = self.writes.lock().unwrap_or_else(|e| e.into_inner());
                    writes
                        .entry((caller.scope.clone(), request.request_id.clone()))
                        .or_insert_with(|| Arc::new(AsyncMutex::new(None)))
                        .clone()
                };
                let mut cached = cache.lock().await;
                // Deadline and origin can change on retry. Operation and arguments cannot.
                let identity = serde_json::json!({"operation":request.operation,"arguments":request.arguments});
                if let Some((previous, answer)) = &*cached {
                    return if *previous == identity {
                        answer.clone()
                    } else {
                        Answer::error(Failure::new(
                            FailureKind::Conflict,
                            "request_id was already used for different arguments",
                        ))
                    };
                }
                let answer = operation.execute(request.arguments.clone(), context).await;
                // A refusal is not a committed write, so a caller may retry it.
                if !matches!(answer, Answer::Error { .. }) {
                    *cached = Some((identity, answer.clone()));
                }
                answer
            } else {
                operation.execute(request.arguments.clone(), context).await
            }
        };
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
            _ = cancellation.cancelled() => Answer::error(Failure::new(FailureKind::Cancelled,"request cancelled")),
            _ = wait_deadline => {
                let progress = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let mut failure = Failure::new(FailureKind::Timeout, format!("deadline passed during {} after {} rows scanned", if progress.stage.is_empty(){"admission"}else{&progress.stage},progress.rows_scanned));
                failure.hint = Some("narrow the request or lift its deadline".into());
                let mut answer = Answer::error(failure);
                if let Some(narrower) = progress.narrower {answer.next_mut().push(Next{label:"narrower request".into(),request:narrower});}
                let mut lifted=request.clone();lifted.deadline=Deadline::None;
                answer.next_mut().push(Next{label:"complete without a deadline".into(),request:lifted});answer
            },
            answer = execution => answer,
        };
        drop(guard);
        if let Answer::Error { error, .. } = &answer {
            if error
                .code
                .as_ref()
                .is_some_and(|code| self.failure_codes.get(code) != Some(&error.kind))
            {
                return Answer::error(Failure::new(
                    FailureKind::Io,
                    "operation returned an unregistered or misclassified product failure code",
                ));
            }
            let infrastructure = matches!(
                error.kind,
                FailureKind::BadRequest
                    | FailureKind::Timeout
                    | FailureKind::Cancelled
                    | FailureKind::Conflict
                    | FailureKind::Io
            );
            if !infrastructure && !description.spec.failures.contains(&error.kind) {
                return Answer::error(Failure::new(
                    FailureKind::Io,
                    format!(
                        "operation {} returned undeclared failure {:?}",
                        description.spec.name, error.kind
                    ),
                ));
            }
        }
        answer
    }
    pub fn render_text(&self, operation: &str, answer: &Answer<Value>) -> Result<String, Failure> {
        match answer {
            Answer::Error { error, .. } => {
                let kind = serde_json::to_value(error.kind).expect("failure kind serializes");
                Ok(format!(
                    "error ({}): {}\n",
                    kind.as_str().expect("failure kind is a string"),
                    terminal_text(&error.message)
                ))
            }
            _ => {
                let operation = self
                    .operations
                    .iter()
                    .find(|op| op.description().spec.name == operation)
                    .ok_or_else(|| bad_request("unknown operation"))?;
                operation.render_text(answer.value().expect("non-error has a value").clone())
            }
        }
    }
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
