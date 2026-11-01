//! MCP tools grouped from the operation table, and the text an agent reads.
//!
//! Each declared tool family holds the request rows that name it. Its input schema
//! is generated: the common envelope fields, the row as `operation` (when
//! the tool has several), and `arguments` as one choice per row. Its hints
//! come from its rows' access, which registration keeps uniform. Stream and
//! local rows are not tools.
use crate::operation::{Forms, terminal_text};
use crate::{
    Access, Answer, Caller, CancellationToken, Deadline, Failure, Format, LeftOut, OperationTable,
    Origin, Outcome, RunnableRequest, Suggestion, bad_request,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Move `schema`'s `$defs` into `definitions`; a name defined twice with
/// different content is refused.
fn take_definitions(
    schema: &mut Value,
    definitions: &mut Map<String, Value>,
    tool: &str,
) -> Result<(), Failure> {
    let Some(own) = schema.as_object_mut().and_then(|o| o.remove("$defs")) else {
        return Ok(());
    };
    for (name, definition) in own.as_object().into_iter().flatten() {
        match definitions.get(name) {
            Some(existing) if existing != definition => {
                return Err(bad_request(format!(
                    "tool {tool}: rows define {name} differently"
                )));
            }
            _ => {
                definitions.insert(name.clone(), definition.clone());
            }
        }
    }
    Ok(())
}

/// The input and output schemas of `tool`.
pub(crate) fn tool_schemas(forms: &Forms, tool: &str) -> Result<(Value, Value), Failure> {
    let rows: Vec<_> = forms
        .descriptions
        .iter()
        .filter(|d| d.spec.tool() == Some(tool))
        .collect();
    let mut definitions = Map::new();
    let mut choices = vec![];
    let mut values = vec![];
    let mut output = None;
    for row in &rows {
        let mut input = match &row.input_schema {
            // `true` accepts any value; as an object it can carry a title.
            Value::Bool(true) => json!({}),
            schema => schema.clone(),
        };
        take_definitions(&mut input, &mut definitions, tool)?;
        if let Some(object) = input.as_object_mut() {
            object.remove("$schema");
            object.insert("title".into(), row.spec.name.clone().into());
            object.insert("description".into(), row.spec.summary.clone().into());
        }
        choices.push(input);
        let mut answer = row.output_schema.clone();
        take_definitions(&mut answer, &mut definitions, tool)?;
        values.push(answer["properties"]["value"].clone());
        output.get_or_insert(answer);
    }
    let mut properties = Map::new();
    let mut required = vec![json!("arguments"), json!("request_id")];
    if rows.len() > 1 {
        properties.insert(
            "operation".into(),
            json!({
                "type": "string",
                "enum": rows.iter().map(|r| r.spec.name.clone()).collect::<Vec<_>>(),
                "description": "Which of the tool's operations to run; `arguments` follows its schema."
            }),
        );
        required.insert(0, json!("operation"));
    }
    let arguments = if choices.len() == 1 {
        choices.pop().expect("one row")
    } else {
        json!({ "anyOf": choices })
    };
    properties.insert("arguments".into(), arguments);
    properties.insert("request_id".into(), json!({"type": "string", "minLength": 1, "description": "Reused when retrying the same call, so a write is applied once."}));
    properties.insert("deadline".into(), json!({"type": ["integer", "null"], "minimum": 0, "description": "Milliseconds; null lifts the deadline."}));
    properties.insert("budget_tokens".into(), json!({"type": "integer", "minimum": 0, "description": "How many tokens the answer's text may use."}));
    properties.insert("cursor".into(), json!({"type": "string", "description": "Continues an earlier answer from where its page ended."}));
    properties.insert(
        "fresh".into(),
        json!({"type": "boolean", "description": "Render in full what you already hold."}),
    );
    properties.insert("format".into(), json!({"type": "string", "enum": ["text", "json"], "description": "json adds the rows to the structured answer."}));
    let mut input = json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    });
    let mut output = output.unwrap_or_else(|| json!({"type": "object"}));
    if let Some(object) = output.as_object_mut() {
        object.remove("$schema");
        object.remove("title");
        if values.len() > 1
            && let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut)
        {
            properties.insert("value".into(), json!({ "anyOf": values }));
        }
    }
    if !definitions.is_empty() {
        input["$defs"] = Value::Object(definitions.clone());
        output["$defs"] = Value::Object(definitions);
    }
    Ok((input, output))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolCall {
    #[serde(default)]
    operation: Option<String>,
    arguments: Value,
    request_id: String,
    #[serde(default)]
    deadline: Deadline,
    #[serde(default)]
    budget_tokens: Option<u32>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    fresh: bool,
    #[serde(default)]
    format: Format,
}

impl Forms {
    fn tool_name(&self, tool: &str) -> String {
        format!("{}_{tool}", self.product)
    }
    /// The call that runs `request` on the tool its row belongs to.
    fn tool_call(&self, request: &RunnableRequest) -> Option<String> {
        let description = self.description(&request.operation)?;
        let tool = description.spec.tool()?;
        let several = self
            .descriptions
            .iter()
            .filter(|d| d.spec.tool() == Some(tool))
            .count()
            > 1;
        let mut call = Map::new();
        if several {
            call.insert("operation".into(), request.operation.clone().into());
        }
        call.insert("arguments".into(), request.arguments.clone());
        call.insert("request_id".into(), request.request_id.clone().into());
        match request.deadline {
            Deadline::Default => {}
            Deadline::None => {
                call.insert("deadline".into(), Value::Null);
            }
            Deadline::Milliseconds(ms) => {
                call.insert("deadline".into(), ms.into());
            }
        }
        if let Some(budget) = request.budget_tokens {
            call.insert("budget_tokens".into(), budget.into());
        }
        if let Some(cursor) = &request.cursor {
            call.insert("cursor".into(), cursor.clone().into());
        }
        if request.fresh {
            call.insert("fresh".into(), true.into());
        }
        if request.format == Format::Json {
            call.insert("format".into(), "json".into());
        }
        Some(format!("{} {}", self.tool_name(tool), Value::Object(call)))
    }
    /// A follow-up as the surface writes it: an MCP tool call, or a command
    /// line everywhere else (and for a row no tool holds).
    pub(crate) fn follow_up(&self, origin: Origin, request: &RunnableRequest) -> String {
        if origin == Origin::Mcp
            && let Some(call) = self.tool_call(request)
        {
            return call;
        }
        self.cli_command(request)
            .unwrap_or_else(|_| format!("{} (unknown operation)", request.operation))
    }
    /// The one line an answer prints for something it left out.
    pub(crate) fn left_out_line(&self, origin: Origin, item: &LeftOut) -> String {
        let unit = if item.count == 1 { "entry" } else { "entries" };
        match &item.expand {
            Some(expand) => format!(
                "left out: {} {unit} of {}; read them: {}\n",
                item.count,
                terminal_text(&item.what),
                self.follow_up(origin, expand)
            ),
            None => format!(
                "left out: {} {unit} of {}\n",
                item.count,
                terminal_text(&item.what)
            ),
        }
    }
    /// The lines after an answer's value: how much was shown, facets,
    /// coverage that is not complete, what was left out, notes and next
    /// steps. Each fact appears once; rows are never repeated here.
    pub(crate) fn footer(&self, origin: Origin, answer: &Answer<Value>) -> String {
        let envelope = &answer.envelope;
        let mut text = String::new();
        if let Outcome::Error(error) = &answer.outcome {
            match (error.caret(), error.position) {
                (Some(caret), _) => {
                    text.push_str(&terminal_text(caret));
                    text.push('\n');
                }
                (None, Some(span)) => {
                    text.push_str(&format!(
                        "at byte {} ({} bytes)\n",
                        span.offset, span.length
                    ));
                }
                (None, None) => {}
            }
            if let Some(hint) = &error.hint {
                text.push_str(&format!("hint: {}\n", terminal_text(hint)));
            }
            match error.suggestion() {
                Some(Suggestion::Query(query)) => {
                    text.push_str(&format!("did you mean: {}\n", terminal_text(query)));
                }
                Some(Suggestion::Request(request)) => {
                    text.push_str(&format!("try: {}\n", self.follow_up(origin, request)));
                }
                None => {}
            }
            if !error.candidates().is_empty() {
                text.push_str(&format!(
                    "candidates: {}\n",
                    terminal_text(&error.candidates().join(" · "))
                ));
            }
        }
        if let Outcome::Degraded { missing, .. } = &answer.outcome {
            text.push_str(&format!(
                "missing: {}\n",
                terminal_text(&missing.join("; "))
            ));
        }
        if let (Some(count), Some(shown)) = (envelope.count, envelope.shown)
            && (count.value > shown || count.lower_bound)
        {
            let bound = if count.lower_bound { "at least " } else { "" };
            text.push_str(&format!("shown {shown} of {bound}{}\n", count.value));
        }
        for facet in &envelope.facets {
            let counts: Vec<String> = facet
                .counts
                .iter()
                .map(|c| format!("{} {}", terminal_text(&c.value), c.count))
                .collect();
            text.push_str(&format!(
                "by {}: {}\n",
                terminal_text(&facet.field),
                counts.join(" · ")
            ));
        }
        for source in envelope.coverage.iter().filter(|s| !s.is_complete()) {
            let state = serde_json::to_value(&source.state).expect("coverage serializes");
            let mut words = vec![state["state"].as_str().unwrap_or_default().to_owned()];
            if let Some(why) = state
                .get("reason")
                .or(state.get("cause"))
                .and_then(Value::as_str)
            {
                words.push(format!("({why})"));
            }
            for interval in &source.not_complete {
                let state = serde_json::to_value(&interval.state).expect("coverage serializes");
                words.push(format!(
                    "{}–{} {}",
                    interval.start,
                    interval.end,
                    state["state"].as_str().unwrap_or_default()
                ));
            }
            text.push_str(&format!(
                "coverage: {} {}\n",
                terminal_text(&source.source),
                terminal_text(&words.join(" "))
            ));
        }
        for item in &envelope.left_out {
            text.push_str(&self.left_out_line(origin, item));
        }
        for note in &envelope.notes {
            text.push_str(&format!("note: {}\n", terminal_text(note)));
        }
        for next in &envelope.next {
            text.push_str(&format!(
                "next: {}: {}\n",
                terminal_text(&next.label),
                self.follow_up(origin, &next.request)
            ));
        }
        text
    }
}

impl OperationTable {
    /// The command line that runs `request`, in its row's own argument forms
    /// when they can hold every field, else with `--request JSON`.
    pub fn cli_command(&self, request: &RunnableRequest) -> Result<String, Failure> {
        self.forms().cli_command(request)
    }
    /// A follow-up as the surface of `origin` writes it.
    pub fn follow_up(&self, origin: Origin, request: &RunnableRequest) -> String {
        self.forms().follow_up(origin, request)
    }
    /// The MCP `tools/list` entries: one per declared tool that holds a row.
    pub fn mcp_tools(&self) -> Vec<Value> {
        self.tools()
            .iter()
            .filter_map(|tool| {
                let access = self
                    .descriptions()
                    .iter()
                    .find(|d| d.spec.tool() == Some(tool.name.as_str()))?
                    .spec
                    .access;
                let (input, output) = tool_schemas(self.forms(), &tool.name)
                    .expect("registration checked every tool's schemas");
                Some(json!({
                    "name": self.forms().tool_name(&tool.name),
                    "description": tool.description,
                    "inputSchema": input,
                    "outputSchema": output,
                    "annotations": {
                        "readOnlyHint": access == Access::Read,
                        "destructiveHint": access == Access::Permanent,
                        "idempotentHint": true,
                        "openWorldHint": tool.open_world,
                    }
                }))
            })
            .collect()
    }
    /// The complete MCP JSON-RPC adapter for `tools/list` and `tools/call`,
    /// suitable for stdio or streamable HTTP. The host owns initialization,
    /// authentication and cancellation notifications.
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
            Some("tools/list") => Ok(json!({ "tools": self.mcp_tools() })),
            Some("tools/call") => {
                let name = message["params"]["name"].as_str().unwrap_or_default();
                match self
                    .tools()
                    .iter()
                    .find(|tool| self.forms().tool_name(&tool.name) == name)
                {
                    Some(tool) => Ok(self
                        .tool_result(
                            &tool.name,
                            message["params"]["arguments"].clone(),
                            message["params"].get("_meta").cloned(),
                            caller,
                            cancellation,
                        )
                        .await),
                    None => Err((-32602, "unknown tool")),
                }
            }
            _ => Err((-32601, "method not found")),
        };
        match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
            }
        }
    }
    async fn tool_result(
        &self,
        tool: &str,
        arguments: Value,
        metadata: Option<Value>,
        caller: Caller,
        cancellation: CancellationToken,
    ) -> Value {
        let rows: Vec<&str> = self
            .descriptions()
            .iter()
            .filter(|d| d.spec.tool() == Some(tool))
            .map(|d| d.spec.name.as_str())
            .collect();
        let (operation, answer, format) = match serde_json::from_value::<ToolCall>(arguments) {
            Err(error) => (
                None,
                Answer::error(bad_request(error.to_string())),
                Format::Text,
            ),
            Ok(call) => {
                let operation = match (call.operation, rows.as_slice()) {
                    (Some(operation), _) if rows.contains(&operation.as_str()) => Ok(operation),
                    (Some(operation), _) => Err(bad_request(format!(
                        "{operation} is not an operation of this tool ({})",
                        rows.join(", ")
                    ))),
                    (None, [only]) => Ok((*only).to_owned()),
                    (None, _) => Err(bad_request(format!(
                        "name the operation: one of {}",
                        rows.join(", ")
                    ))),
                };
                match operation {
                    Err(failure) => (None, Answer::error(failure), call.format),
                    Ok(operation) => {
                        let request = RunnableRequest {
                            operation: operation.clone(),
                            arguments: call.arguments,
                            request_id: call.request_id,
                            deadline: call.deadline,
                            budget_tokens: call.budget_tokens,
                            cursor: call.cursor,
                            fresh: call.fresh,
                            format: call.format,
                        };
                        let answer = self
                            .call_with(request, Origin::Mcp, caller, metadata, cancellation)
                            .await;
                        (Some(operation), answer, call.format)
                    }
                }
            }
        };
        let mut text = match (&operation, &answer.outcome) {
            (Some(operation), Outcome::Ok(_) | Outcome::Empty(_) | Outcome::Degraded { .. }) => {
                self.render_text(operation, &answer)
                    .unwrap_or_else(|failure| format!("error (io): {}\n", failure.message))
            }
            _ => self.render_text("", &answer).unwrap_or_default(),
        };
        text.push_str(&self.forms().footer(Origin::Mcp, &answer));
        let mut structured = serde_json::to_value(&answer).expect("answers serialize");
        if format == Format::Text
            && let Some(object) = structured.as_object_mut()
        {
            // The rows are in the text part; JSON repeats them only on request.
            object.remove("value");
        }
        json!({
            "isError": answer.is_error(),
            "structuredContent": structured,
            "content": [{"type": "text", "text": text}],
        })
    }
}
