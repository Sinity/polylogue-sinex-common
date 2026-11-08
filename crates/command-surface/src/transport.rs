//! Transport adapters contain framing only. None builds an answer of its own.
use crate::{
    Answer, Caller, CancellationToken, Failure, FailureKind, Format, OperationCatalog,
    OperationKind, OperationTable, RunnableRequest, TextStyle, Via, bad_request,
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

impl OperationCatalog {
    /// Generated help: one line per listed row, with its usage.
    pub fn help(&self) -> String {
        self.descriptions()
            .iter()
            .filter(|d| d.spec.listed && !d.spec.command.is_empty())
            .map(|d| {
                format!(
                    "{} {}  {}\n",
                    self.product(),
                    crate::arguments::usage(d, self.cli_globals()),
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
            .find(|d| d.spec.name == operation && !d.spec.command.is_empty())?;
        let mut text = format!(
            "{} {}\n{}\n",
            self.product(),
            crate::arguments::usage(description, self.cli_globals()),
            description.spec.summary
        );
        for argument in self.cli_arguments(operation).ok()? {
            let argument = &argument.spec;
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
            .filter(|d| d.spec.listed && !d.spec.command.is_empty())
            .map(|d| d.spec.command.join(" "))
            .filter(|command| command.starts_with(prefix))
            .collect()
    }
}

impl OperationTable {
    pub async fn cli(
        &self,
        args: &[String],
        request_id: impl Into<String>,
        cancellation: CancellationToken,
    ) -> CliOutput {
        self.cli_as(args, request_id, Caller::default(), cancellation)
            .await
    }
    /// Run a command line in this process and collect what it prints.
    pub async fn cli_as(
        &self,
        args: &[String],
        request_id: impl Into<String>,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> CliOutput {
        let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
        let exit_code = self
            .cli_write(
                args,
                request_id,
                caller,
                cancellation,
                &mut stdout,
                &mut stderr,
            )
            .await
            .expect("writing to memory does not fail");
        CliOutput {
            stdout: String::from_utf8(stdout).expect("rendered text is UTF-8"),
            stderr: String::from_utf8(stderr).expect("rendered text is UTF-8"),
            exit_code,
        }
    }
    /// Run a command line in this process, writing as it goes: a request
    /// row's value text to `stdout` and the envelope's lines to `stderr`
    /// (all of it to `stdout` as JSON with `--json`), and a stream's events
    /// to `stdout` as each arrives. Returns the exit code. A local row is the
    /// product's own to run: dispatch on `parse_cli` first.
    pub async fn cli_write<O, E>(
        &self,
        args: &[String],
        request_id: impl Into<String>,
        caller: Caller,
        cancellation: CancellationToken,
        stdout: &mut O,
        stderr: &mut E,
    ) -> std::io::Result<u8>
    where
        O: AsyncWrite + Unpin,
        E: AsyncWrite + Unpin,
    {
        if args == ["--help"] {
            stdout.write_all(self.help().as_bytes()).await?;
            stdout.flush().await?;
            return Ok(0);
        }
        let parsed = self.parse_cli(args, request_id);
        let json = parsed
            .as_ref()
            .map_or_else(|_| args.iter().any(|a| a == "--json"), |p| p.json);
        let (request, answer) = match parsed {
            Err(error) => (None, Answer::error(error)),
            Ok(parsed) => {
                let request = parsed.request.clone();
                let operation = request.operation.clone();
                match self.streaming(&request) {
                    Err(failure) => (Some(request), Answer::error(failure)),
                    Ok(true) => (
                        None,
                        self.write_stream(parsed.request, caller, cancellation, json, stdout)
                            .await?,
                    ),
                    Ok(false) => {
                        let answer = if parsed.kind == OperationKind::Local {
                            Answer::error(bad_request(format!(
                                "{operation} runs in the calling process"
                            )))
                        } else {
                            self.call_as(parsed.request, Via::Cli, caller, cancellation)
                                .await
                        };
                        (Some(request), answer)
                    }
                }
            }
        };
        if json {
            let line = format!(
                "{}\n",
                serde_json::to_string(&answer).expect("JSON value serializes")
            );
            stdout.write_all(line.as_bytes()).await?;
            stdout.flush().await?;
            return Ok(answer.exit_code());
        }
        // The command line's text is never JSON rows: `--json` printed above.
        let request = request.map(|mut request| {
            request.format = Format::Text;
            request
        });
        let crate::RenderedAnswer {
            answer,
            value_text,
            footer,
        } = self.render_answer(Via::Cli, request.as_ref(), answer, TextStyle::Plain);
        if answer.is_error() {
            stderr
                .write_all(format!("{value_text}{footer}").as_bytes())
                .await?;
        } else {
            stdout.write_all(value_text.as_bytes()).await?;
            stderr.write_all(footer.as_bytes()).await?;
        }
        stdout.flush().await?;
        stderr.flush().await?;
        Ok(answer.exit_code())
    }
    /// Run a stream to its end, writing each event as it arrives: rendered,
    /// or as an `{"event": …}` line with `--json`. The answer that ends it is
    /// ok with no value of its own.
    async fn write_stream<O: AsyncWrite + Unpin>(
        &self,
        request: RunnableRequest,
        caller: Caller,
        cancellation: CancellationToken,
        json: bool,
        stdout: &mut O,
    ) -> std::io::Result<Answer<Value>> {
        let operation = request.operation.clone();
        let mut events = match self
            .open_stream(request, Via::Cli, caller, cancellation)
            .await
        {
            Ok(events) => events,
            Err(failure) => return Ok(Answer::error(failure)),
        };
        while let Some(event) = events.next().await {
            let line = match event {
                Err(failure) => return Ok(Answer::error(failure)),
                Ok(event) if json => format!("{}\n", json!({ "event": event })),
                Ok(event) => match self.render_event(&operation, event, TextStyle::Plain) {
                    Ok(line) => line,
                    Err(failure) => return Ok(Answer::error(failure)),
                },
            };
            stdout.write_all(line.as_bytes()).await?;
            stdout.flush().await?;
        }
        Ok(Answer::ok(Value::Null))
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
        // Whole lines only, and reading one is cancel-safe: a line arriving in
        // parts never holds up the call that is running.
        let mut lines = BufReader::new(reader).lines();
        // A request frame arrives as the server's caller says it came (a
        // command line or a hook); an MCP frame is an MCP call.
        let socket_via = caller.authorship().map_or(Via::Cli, |a| a.via);
        while let Some(line) = lines.next_line().await? {
            if let Ok(message) = serde_json::from_str::<Value>(&line)
                && message.get("jsonrpc").is_some()
            {
                // An MCP message forwarded by a stdio proxy, answered by the
                // same adapter as `mcp_as`.
                let id = message.get("id").cloned();
                let cancellation = CancellationToken::new();
                let call = self.mcp_as(message, caller.clone(), cancellation.clone());
                tokio::pin!(call);
                // While the call runs, notifications may arrive: a
                // cancellation of this call cancels it (and it gets no
                // reply), any other is taken and ignored. A second request is
                // a framing error.
                let mut cancelled = false;
                let outcome = loop {
                    tokio::select! {
                        biased;
                        reply = &mut call => break Some(reply),
                        next = lines.next_line() => {
                            let Some(next) = next? else {
                                cancellation.cancel();
                                return Ok(());
                            };
                            match serde_json::from_str::<Value>(&next) {
                                Ok(other) if other.get("id").is_none() => {
                                    if other["method"] == "notifications/cancelled"
                                        && id.is_some()
                                        && other["params"]["requestId"] == *id.as_ref().expect("checked")
                                    {
                                        // Its answer is discarded; wait for
                                        // the work to stop before reading on.
                                        cancelled = true;
                                        cancellation.cancel();
                                        break Some((&mut call).await);
                                    }
                                }
                                _ => {
                                    cancellation.cancel();
                                    break None;
                                }
                            }
                        },
                    }
                };
                let Some(reply) = outcome else {
                    let refusal = json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32600, "message": "wait for the reply before sending another request"}});
                    let mut encoded = serde_json::to_vec(&refusal)?;
                    encoded.push(b'\n');
                    writer.write_all(&encoded).await?;
                    writer.flush().await?;
                    return Ok(());
                };
                // A notification and a cancelled call get no reply.
                if id.is_some() && !cancelled {
                    let mut encoded = serde_json::to_vec(&reply)?;
                    encoded.push(b'\n');
                    writer.write_all(&encoded).await?;
                    writer.flush().await?;
                }
                continue;
            }
            let request = serde_json::from_str::<RunnableRequest>(&line)
                .map_err(|error| bad_request(error.to_string()))
                .and_then(|request| {
                    self.streaming(&request)
                        .map(|streaming| (request, streaming))
                });
            let mut close_after_reply = false;
            let answer = match request {
                Err(error) => Answer::error(error),
                Ok((request, true)) => {
                    let cancellation = CancellationToken::new();
                    let opened = self
                        .open_stream(request, socket_via, caller.clone(), cancellation.clone())
                        .await;
                    match opened {
                        Err(failure) => Answer::error(failure),
                        Ok(mut events) => loop {
                            // An event or a disconnect, whichever comes first;
                            // a second request mid-stream is a framing error.
                            let event = tokio::select! {
                                biased;
                                incoming = lines.next_line() => {
                                    cancellation.cancel();
                                    if incoming?.is_none() { return Ok(()); }
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
                Ok((request, false)) => {
                    let cancellation = CancellationToken::new();
                    let call =
                        self.call_as(request, socket_via, caller.clone(), cancellation.clone());
                    // This protocol has one outstanding request per connection.
                    // Reading a second request before its reply is a framing error.
                    tokio::select! {
                        biased;
                        incoming = lines.next_line() => {
                            cancellation.cancel();
                            if incoming?.is_none() { return Ok(()); }
                            close_after_reply = true;
                            Answer::error(bad_request("wait for the reply before sending another request"))
                        },
                        answer = call => answer,
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
        Ok(())
    }
    /// An already-connected client awaits the server's terminal result.
    /// Only explicit caller cancellation ends this wait; the server owns read deadlines.
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
        tokio::select! {biased;
            _=cancellation.cancelled()=>Answer::error(Failure::new(FailureKind::Cancelled,"request cancelled")),
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
                    let matches = spec.is_some_and(|spec| {
                        spec.http_route
                            .as_deref()
                            .is_some_and(|route| path == "/v1" || route == path)
                    });
                    match spec.map(|spec| &spec.kind) {
                        _ if !matches => Answer::error(Failure::new(
                            FailureKind::NotFound,
                            "route does not match operation",
                        )),
                        Some(OperationKind::Stream) => Answer::error(bad_request(
                            "a stream is served as server-sent events at its route",
                        )),
                        _ => self.call_as(envelope, Via::Web, caller, cancellation).await,
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
