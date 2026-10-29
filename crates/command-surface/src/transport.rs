//! Transport adapters contain framing only. None builds an answer of its own.
use crate::{
    Answer, Caller, CancellationToken, Deadline, Failure, FailureKind, OperationTable, Origin,
    RunnableRequest, bad_request,
};
use http::{Request as HttpRequest, Response as HttpResponse, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
}

impl OperationTable {
    /// Generated help and completion share the table's command spelling.
    pub fn help(&self) -> String {
        self.descriptions()
            .iter()
            .map(|d| {
                format!(
                    "{} {}  {}\n",
                    self.product,
                    d.spec.command.join(" "),
                    d.spec.summary
                )
            })
            .collect()
    }
    pub fn complete(&self, prefix: &str) -> Vec<String> {
        self.descriptions()
            .iter()
            .map(|d| d.spec.command.join(" "))
            .filter(|command| command.starts_with(prefix))
            .collect()
    }
    /// CLI fields come from the request schema. Complex values use JSON. A
    /// complete JSON value is also accepted as `--request JSON`, for every type.
    pub fn parse_cli(
        &self,
        args: &[String],
        request_id: impl Into<String>,
    ) -> Result<RunnableRequest, Failure> {
        let descriptions = self.descriptions();
        let description = descriptions
            .iter()
            .filter(|d| args.starts_with(&d.spec.command))
            .max_by_key(|d| d.spec.command.len())
            .ok_or_else(|| bad_request("unknown command; use --help"))?;
        let mut fields = Map::new();
        let mut deadline = Deadline::Default;
        let mut id = request_id.into();
        let mut i = description.spec.command.len();
        let mut supplied_value = None;
        while i < args.len() {
            let flag = args[i].strip_prefix("--").ok_or_else(|| {
                bad_request(format!("expected a named argument, got {}", args[i]))
            })?;
            if flag == "json" {
                i += 1;
                continue;
            }
            let value = args
                .get(i + 1)
                .ok_or_else(|| bad_request(format!("--{flag} needs a value")))?;
            match flag {
                "deadline" => {
                    deadline =
                        if value == "none" {
                            Deadline::None
                        } else {
                            Deadline::Milliseconds(value.parse().map_err(|_| {
                                bad_request("deadline must be milliseconds or none")
                            })?)
                        }
                }
                "request-id" => id = value.clone(),
                "request" => {
                    if !fields.is_empty() || supplied_value.is_some() {
                        return Err(bad_request("--request cannot be mixed with named fields"));
                    }
                    supplied_value = Some(
                        serde_json::from_str::<Value>(value)
                            .map_err(|e| bad_request(e.to_string()))?,
                    );
                }
                name => {
                    if supplied_value.is_some() {
                        return Err(bad_request("--request cannot be mixed with named fields"));
                    }
                    let name = if description.input_schema["properties"].get(name).is_some() {
                        name.to_owned()
                    } else {
                        name.replace('-', "_")
                    };
                    let property = description.input_schema["properties"]
                        .get(&name)
                        .ok_or_else(|| bad_request(format!("unknown argument --{flag}")))?;
                    // Plain strings remain strings, including numeric-looking IDs.
                    let string_type = property["type"] == "string"
                        || property["type"]
                            .as_array()
                            .is_some_and(|types| types.contains(&json!("string")));
                    let value = if string_type {
                        Value::String(value.clone())
                    } else {
                        serde_json::from_str(value)
                            .map_err(|e| bad_request(format!("--{flag}: {e}")))?
                    };
                    if fields.insert(name, value).is_some() {
                        return Err(bad_request(format!("duplicate --{flag}")));
                    }
                }
            }
            i += 2;
        }
        Ok(RunnableRequest {
            operation: description.spec.name.clone(),
            arguments: supplied_value.unwrap_or(Value::Object(fields)),
            request_id: id,
            deadline,
        })
    }
    pub async fn cli(
        &self,
        args: &[String],
        request_id: impl Into<String>,
        cancellation: CancellationToken,
    ) -> CliOutput {
        self.cli_as(args, request_id, Caller::default(), cancellation)
            .await
    }
    pub async fn cli_as(
        &self,
        args: &[String],
        request_id: impl Into<String>,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> CliOutput {
        if args == ["--help"] {
            return CliOutput {
                stdout: self.help(),
                stderr: String::new(),
                exit_code: 0,
            };
        }
        let command_length = self
            .descriptions()
            .iter()
            .filter(|d| args.starts_with(&d.spec.command))
            .map(|d| d.spec.command.len())
            .max()
            .unwrap_or(0);
        let mut index = command_length;
        let mut json_output = false;
        while index < args.len() {
            if args[index] == "--json" {
                json_output = true;
                index += 1;
            } else {
                index += 2;
            }
        }
        let (operation, answer) = match self.parse_cli(args, request_id) {
            Ok(request) => {
                let operation = request.operation.clone();
                (
                    operation,
                    self.call_as(request, Origin::Cli, caller, cancellation)
                        .await,
                )
            }
            Err(error) => (String::new(), Answer::error(error)),
        };
        let exit_code = answer.exit_code();
        if json_output {
            CliOutput {
                stdout: format!(
                    "{}\n",
                    serde_json::to_string(&answer).expect("JSON value serializes")
                ),
                stderr: String::new(),
                exit_code,
            }
        } else {
            let text = self
                .render_text(&operation, &answer)
                .unwrap_or_else(|e| e.to_string());
            let mut metadata = String::new();
            if let Answer::Error { error, .. } = &answer {
                if let Some(position) = &error.position {
                    metadata.push_str(&format!(
                        "position: byte {} ({} bytes)\n",
                        position.offset, position.length
                    ));
                }
                if let Some(hint) = &error.hint {
                    metadata.push_str(&format!(
                        "hint: {}\n",
                        crate::operation::terminal_text(hint)
                    ));
                }
                if let Some(suggestion) = &error.suggestion {
                    let suggestion = suggestion
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| suggestion.to_string());
                    metadata.push_str(&format!(
                        "try: {}\n",
                        crate::operation::terminal_text(&suggestion)
                    ));
                }
            }
            for note in answer.notes() {
                metadata.push_str(&format!(
                    "note: {}\n",
                    crate::operation::terminal_text(note)
                ));
            }
            for next in answer.next() {
                if let Ok(command) = self.cli_command(&next.request) {
                    metadata.push_str(&format!(
                        "next: {}: {}\n",
                        crate::operation::terminal_text(&next.label),
                        command
                    ));
                }
            }
            if matches!(answer, Answer::Error { .. }) {
                CliOutput {
                    stdout: String::new(),
                    stderr: format!("{text}{metadata}"),
                    exit_code,
                }
            } else {
                CliOutput {
                    stdout: text,
                    stderr: metadata,
                    exit_code,
                }
            }
        }
    }
    /// JSONL stream over a Unix socket (or any duplex byte stream). There is no
    /// input-size cap. EOF during execution cancels that request by dropping its
    /// owned future, and the context guard signals cooperative child work.
    pub async fn serve_socket<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        stream: S,
    ) -> std::io::Result<()> {
        self.serve_socket_as(stream, Caller::default()).await
    }
    pub async fn serve_socket_as<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        stream: S,
        caller: Caller,
    ) -> std::io::Result<()> {
        let (reader, mut writer) = tokio::io::split(stream);
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).await? == 0 {
                return Ok(());
            }
            let request = serde_json::from_str::<RunnableRequest>(&line);
            let mut close_after_reply = false;
            let answer = match request {
                Err(error) => Answer::error(bad_request(error.to_string())),
                Ok(request) => {
                    let cancellation = CancellationToken::new();
                    let call = self.call_as(
                        request,
                        Origin::Socket,
                        caller.clone(),
                        cancellation.clone(),
                    );
                    // This protocol has one outstanding request per connection.
                    // Reading a second request before its reply is a framing error.
                    tokio::select! {
                        biased;
                        disconnected=reader.fill_buf()=>{
                            cancellation.cancel();
                            if disconnected?.is_empty(){return Ok(());}
                            close_after_reply=true;
                            Answer::error(bad_request("wait for the reply before sending another request"))
                        },
                        answer=call=>answer,
                    }
                }
            };
            let mut encoded = serde_json::to_vec(&answer)?;
            encoded.push(b'\n');
            writer.write_all(&encoded).await?;
            writer.flush().await?;
            if close_after_reply {
                return Ok(());
            }
        }
    }
    /// A socket client uses the class deadline plus the protocol's 50 ms grace.
    pub async fn socket_call<S: AsyncRead + AsyncWrite + Unpin>(
        &self,
        mut stream: S,
        request: &RunnableRequest,
        cancellation: CancellationToken,
    ) -> Answer<Value> {
        let exchange = async {
            let mut encoded =
                serde_json::to_vec(request).map_err(|e| bad_request(e.to_string()))?;
            encoded.push(b'\n');
            stream.write_all(&encoded).await.map_err(io_failure)?;
            stream.flush().await.map_err(io_failure)?;
            let mut response = String::new();
            BufReader::new(&mut stream)
                .read_line(&mut response)
                .await
                .map_err(io_failure)?;
            serde_json::from_str(&response)
                .map_err(|e| Failure::new(FailureKind::Io, e.to_string()))
        };
        let timeout = async {
            match self.socket_timeout(request) {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {biased;
            _=cancellation.cancelled()=>Answer::error(Failure::new(FailureKind::Cancelled,"request cancelled")),
            _=timeout=>Answer::error(Failure::new(FailureKind::Timeout,"daemon socket deadline passed")),
            answer=exchange=>answer.unwrap_or_else(Answer::error),
        }
    }
    /// HTTP body is the same envelope as the socket. One route per operation
    /// and `/v1` are generated; GET discovery requires no product handler.
    /// Authentication and origin validation happen before calling this adapter.
    pub async fn http(
        &self,
        request: HttpRequest<Vec<u8>>,
        cancellation: CancellationToken,
    ) -> HttpResponse<Vec<u8>> {
        self.http_as(request, Caller::default(), cancellation).await
    }
    pub async fn http_as(
        &self,
        request: HttpRequest<Vec<u8>>,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> HttpResponse<Vec<u8>> {
        if request.method() == http::Method::GET && request.uri().path() == "/v1/commands" {
            return json_response(200, &self.descriptions());
        }
        let path = request.uri().path().to_owned();
        let answer = if request.method() != http::Method::POST {
            Answer::error(bad_request("operations use POST with a request envelope"))
        } else {
            let content_type = request
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|h| h.to_str().ok())
                .and_then(|h| h.split(';').next())
                .map(str::trim);
            let envelope = match content_type {
                Some("application/json") => {
                    serde_json::from_slice::<RunnableRequest>(request.body())
                        .map_err(|e| bad_request(e.to_string()))
                }
                Some("application/x-www-form-urlencoded") => decode_form_request(request.body()),
                _ => Err(bad_request(
                    "content-type must be application/json or application/x-www-form-urlencoded",
                )),
            };
            match envelope {
                Err(error) => Answer::error(error),
                Ok(envelope) => {
                    let matches = path == "/v1"
                        || self.descriptions().iter().any(|d| {
                            d.spec.http_route == path && d.spec.name == envelope.operation
                        });
                    if matches {
                        self.call_as(envelope, Origin::Http, caller, cancellation)
                            .await
                    } else {
                        Answer::error(Failure::new(
                            FailureKind::NotFound,
                            "route does not match operation",
                        ))
                    }
                }
            }
        };
        // Cancelled/not_running have no HTTP status in the shared table. If a
        // response can still be sent, the transport reports their error at 500.
        json_response(answer.http_status().unwrap_or(500), &answer)
    }
    pub fn mcp_tools(&self) -> Vec<Value> {
        self.descriptions().iter().map(|d|{
            // MCP arguments wrap the typed operation arguments and carry the
            // same explicit request identity and nullable deadline as HTTP.
            let mut argument_schema=d.input_schema.clone();
            // Schema refs are rooted at the tool's schema, even when the typed
            // request is nested inside its transport envelope.
            let definitions=argument_schema.as_object_mut().and_then(|o|o.remove("$defs"));
            let mut input=json!({"type":"object","properties":{"arguments":argument_schema,"request_id":{"type":"string","minLength":1},"deadline":{"type":["integer","null"],"minimum":0}},"required":["arguments","request_id"],"additionalProperties":false});
            if let Some(definitions)=definitions{input["$defs"]=definitions;}
            json!({"name":d.spec.mcp_name(&self.product),"description":d.spec.summary,"inputSchema":input,"outputSchema":d.output_schema,"annotations":{"readOnlyHint":!d.spec.write}})
        }).collect()
    }
    /// Complete MCP JSON-RPC adapter, suitable for stdio or streamable HTTP.
    /// The host supplies cancellation for notifications/cancelled or disconnect.
    pub async fn mcp(&self, message: Value, cancellation: CancellationToken) -> Value {
        self.mcp_as(message, Caller::default(), cancellation).await
    }
    pub async fn mcp_as(
        &self,
        message: Value,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> Value {
        let id = message.get("id").cloned().unwrap_or(Value::Null);
        let result = match message["method"].as_str() {
            Some("tools/list") => Ok(json!({"tools":self.mcp_tools()})),
            Some("tools/call") => {
                let name = message["params"]["name"].as_str().unwrap_or_default();
                let operation = self
                    .descriptions()
                    .into_iter()
                    .find(|d| d.spec.mcp_name(&self.product) == name);
                if let Some(operation) = operation {
                    #[derive(Deserialize)]
                    #[serde(deny_unknown_fields)]
                    struct Arguments {
                        arguments: Value,
                        request_id: String,
                        #[serde(default)]
                        deadline: Deadline,
                    }
                    let answer = match serde_json::from_value::<Arguments>(
                        message["params"]["arguments"].clone(),
                    ) {
                        Ok(a) => {
                            self.call_as(
                                RunnableRequest {
                                    operation: operation.spec.name,
                                    arguments: a.arguments,
                                    request_id: a.request_id,
                                    deadline: a.deadline,
                                },
                                Origin::Mcp,
                                caller,
                                cancellation,
                            )
                            .await
                        }
                        Err(error) => Answer::error(bad_request(error.to_string())),
                    };
                    let structured = serde_json::to_value(&answer).expect("JSON value serializes");
                    Ok(
                        json!({"isError":matches!(answer,Answer::Error{..}),"structuredContent":structured,"content":[{"type":"text","text":structured.to_string()}]}),
                    )
                } else {
                    Err((-32602, "unknown tool"))
                }
            }
            _ => Err((-32601, "method not found")),
        };
        match result {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err((code, message)) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
            }
        }
    }
}
fn io_failure(error: std::io::Error) -> Failure {
    Failure::new(FailureKind::NotRunning, error.to_string())
}
fn decode_form_request(bytes: &[u8]) -> Result<RunnableRequest, Failure> {
    let text = std::str::from_utf8(bytes).map_err(|e| bad_request(e.to_string()))?;
    let mut request = None;
    for pair in text.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        if decode_form_component(key)? != "request" || request.is_some() {
            return Err(bad_request("form requires exactly one request field"));
        }
        request = Some(decode_form_component(value)?);
    }
    serde_json::from_str(&request.ok_or_else(|| bad_request("missing request field"))?)
        .map_err(|e| bad_request(e.to_string()))
}
fn decode_form_component(value: &str) -> Result<String, Failure> {
    let bytes = value.as_bytes();
    let mut decoded = vec![];
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => decoded.push(b' '),
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .ok_or_else(|| bad_request("incomplete form escape"))?;
                let high = (hex[0] as char)
                    .to_digit(16)
                    .ok_or_else(|| bad_request("invalid form escape"))?;
                let low = (hex[1] as char)
                    .to_digit(16)
                    .ok_or_else(|| bad_request("invalid form escape"))?;
                decoded.push((high * 16 + low) as u8);
                i += 2;
            }
            byte => decoded.push(byte),
        }
        i += 1;
    }
    String::from_utf8(decoded).map_err(|e| bad_request(e.to_string()))
}
fn json_response<T: Serialize>(status: u16, value: &T) -> HttpResponse<Vec<u8>> {
    let body = serde_json::to_vec(value).expect("wire types serialize");
    HttpResponse::builder()
        .status(StatusCode::from_u16(status).expect("failure table has valid HTTP statuses"))
        .header(http::header::CONTENT_TYPE, "application/json")
        .header(http::header::CACHE_CONTROL, "no-store")
        .body(body)
        .expect("static headers are valid")
}

/// Quote one CLI argument so the generated command runs as given in a POSIX shell.
pub fn quote_argument(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:".contains(c))
    {
        value.into()
    } else {
        format!("'{}'", value.replace('\'', "'\\''"))
    }
}
impl OperationTable {
    pub fn cli_command(&self, request: &RunnableRequest) -> Result<String, Failure> {
        let spec = self
            .descriptions()
            .into_iter()
            .find(|d| d.spec.name == request.operation)
            .ok_or_else(|| bad_request("unknown operation"))?
            .spec;
        let mut words = vec![quote_argument(&self.product)];
        words.extend(spec.command.iter().map(|s| quote_argument(s)));
        words.push("--request".into());
        words.push(quote_argument(&request.arguments.to_string()));
        words.push("--request-id".into());
        words.push(quote_argument(&request.request_id));
        match request.deadline {
            Deadline::Default => {}
            Deadline::None => words.extend(["--deadline".into(), "none".into()]),
            Deadline::Milliseconds(ms) => words.extend(["--deadline".into(), ms.to_string()]),
        }
        Ok(words.join(" "))
    }
}
