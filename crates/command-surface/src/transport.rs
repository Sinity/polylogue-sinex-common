//! Transport adapters contain framing only. None builds an answer of its own.
use crate::{
    Answer, Caller, CancellationToken, Failure, FailureKind, OperationKind, OperationTable, Origin,
    RunnableRequest, bad_request,
};
use http::{Request as HttpRequest, Response as HttpResponse, StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
}

/// One line a socket sends for a stream: an event, or the answer that ends it.
#[derive(Clone, Debug, PartialEq)]
pub enum StreamLine {
    Event(Value),
    End(Box<Answer<Value>>),
}
impl StreamLine {
    pub fn parse(line: &str) -> Result<Self, Failure> {
        let value: Value =
            serde_json::from_str(line).map_err(|e| Failure::new(FailureKind::Io, e.to_string()))?;
        match value.as_object() {
            Some(object) if object.len() == 1 && object.contains_key("event") => {
                Ok(Self::Event(value["event"].clone()))
            }
            _ => serde_json::from_value(value)
                .map(|answer| Self::End(Box::new(answer)))
                .map_err(|e| Failure::new(FailureKind::Io, e.to_string())),
        }
    }
}

impl OperationTable {
    /// Generated help: one line per listed row, with its usage.
    pub fn help(&self) -> String {
        self.descriptions()
            .iter()
            .filter(|d| d.spec.listed)
            .map(|d| {
                format!(
                    "{} {}  {}\n",
                    self.product(),
                    crate::arguments::usage(d),
                    d.spec.summary
                )
            })
            .collect()
    }
    /// One row's help: its usage, summary and each declared argument.
    pub fn help_for(&self, operation: &str) -> Option<String> {
        let description = self
            .descriptions()
            .iter()
            .find(|d| d.spec.name == operation)?;
        let mut text = format!(
            "{} {}\n{}\n",
            self.product(),
            crate::arguments::usage(description),
            description.spec.summary
        );
        for argument in &description.spec.arguments {
            text.push_str(&format!(
                "  {}  {}\n",
                match argument.form {
                    crate::ArgumentForm::Positional
                    | crate::ArgumentForm::Words
                    | crate::ArgumentForm::Rest => argument.name.to_uppercase(),
                    _ => argument.flag(),
                },
                argument.help
            ));
        }
        Some(text)
    }
    /// Command words of listed rows that start with `prefix`.
    pub fn complete(&self, prefix: &str) -> Vec<String> {
        self.descriptions()
            .iter()
            .filter(|d| d.spec.listed)
            .map(|d| d.spec.command.join(" "))
            .filter(|command| command.starts_with(prefix))
            .collect()
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
    /// Run a command line in this process against the table. A local row is
    /// the product's own to run: dispatch on `parse_cli` first.
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
        let parsed = self.parse_cli(args, request_id);
        let json = parsed
            .as_ref()
            .map_or_else(|_| args.iter().any(|a| a == "--json"), |p| p.json);
        let (operation, answer, events) = match parsed {
            Err(error) => (String::new(), Answer::error(error), String::new()),
            Ok(parsed) => {
                let operation = parsed.request.operation.clone();
                match parsed.kind {
                    OperationKind::Request { .. } => {
                        let answer = self
                            .call_as(parsed.request, Origin::Cli, caller, cancellation)
                            .await;
                        (operation, answer, String::new())
                    }
                    OperationKind::Stream => {
                        let (answer, events) = self
                            .collect_stream(parsed.request, caller, cancellation, json)
                            .await;
                        (operation, answer, events)
                    }
                    OperationKind::Local => (
                        operation.clone(),
                        Answer::error(bad_request(format!(
                            "{operation} runs in the calling process"
                        ))),
                        String::new(),
                    ),
                }
            }
        };
        let exit_code = answer.exit_code();
        if json {
            return CliOutput {
                stdout: format!(
                    "{events}{}\n",
                    serde_json::to_string(&answer).expect("JSON value serializes")
                ),
                stderr: String::new(),
                exit_code,
            };
        }
        let footer = self.forms().footer(Origin::Cli, &answer);
        if answer.is_error() {
            let text = self.render_text("", &answer).unwrap_or_default();
            return CliOutput {
                stdout: events,
                stderr: format!("{text}{footer}"),
                exit_code,
            };
        }
        let text = if events.is_empty() {
            self.render_text(&operation, &answer)
                .unwrap_or_else(|e| e.to_string())
        } else {
            events
        };
        CliOutput {
            stdout: text,
            stderr: footer,
            exit_code,
        }
    }
    /// Run a stream to its end, each event as a line of text or JSON.
    async fn collect_stream(
        &self,
        request: RunnableRequest,
        caller: Caller,
        cancellation: CancellationToken,
        json: bool,
    ) -> (Answer<Value>, String) {
        let operation = request.operation.clone();
        let mut events = match self
            .open_stream(request, Origin::Cli, caller, cancellation)
            .await
        {
            Ok(events) => events,
            Err(failure) => return (Answer::error(failure), String::new()),
        };
        let mut text = String::new();
        while let Some(event) = events.next().await {
            match event {
                Err(failure) => return (Answer::error(failure), text),
                Ok(event) if json => {
                    text.push_str(&format!("{}\n", json!({ "event": event })));
                }
                Ok(event) => match self.render_event(&operation, event) {
                    Ok(line) => text.push_str(&line),
                    Err(failure) => return (Answer::error(failure), text),
                },
            }
        }
        (Answer::ok(Value::Null), text)
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
                Ok(request)
                    if self
                        .spec_named(&request.operation)
                        .is_some_and(|spec| spec.kind == OperationKind::Stream) =>
                {
                    let cancellation = CancellationToken::new();
                    let opened = self
                        .open_stream(
                            request,
                            Origin::Socket,
                            caller.clone(),
                            cancellation.clone(),
                        )
                        .await;
                    match opened {
                        Err(failure) => Answer::error(failure),
                        Ok(mut events) => loop {
                            // An event or a disconnect, whichever comes first;
                            // a second request mid-stream is a framing error.
                            let event = tokio::select! {
                                biased;
                                incoming = reader.fill_buf() => {
                                    cancellation.cancel();
                                    if incoming?.is_empty() { return Ok(()); }
                                    close_after_reply = true;
                                    break Answer::error(bad_request("wait for the stream to end before sending another request"));
                                },
                                event = events.next() => event,
                            };
                            match event {
                                None => break Answer::ok(Value::Null),
                                Some(Err(failure)) => break Answer::error(failure),
                                Some(Ok(event)) => {
                                    let mut encoded =
                                        serde_json::to_vec(&json!({ "event": event }))?;
                                    encoded.push(b'\n');
                                    writer.write_all(&encoded).await?;
                                    writer.flush().await?;
                                }
                            }
                        },
                    }
                }
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
                    let spec = self.spec_named(&envelope.operation);
                    let matches = path == "/v1"
                        || spec.is_some_and(|spec| spec.http_route.as_deref() == Some(&path));
                    match spec.map(|spec| &spec.kind) {
                        _ if !matches => Answer::error(Failure::new(
                            FailureKind::NotFound,
                            "route does not match operation",
                        )),
                        Some(OperationKind::Stream) => Answer::error(bad_request(
                            "a stream is served as server-sent events at its route",
                        )),
                        _ => {
                            self.call_as(envelope, Origin::Http, caller, cancellation)
                                .await
                        }
                    }
                }
            }
        };
        // Cancelled/not_running have no HTTP status in the shared table. If a
        // response can still be sent, the transport reports their error at 500.
        json_response(answer.http_status().unwrap_or(500), &answer)
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
