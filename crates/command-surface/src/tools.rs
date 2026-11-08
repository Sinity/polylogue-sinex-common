//! MCP tools grouped from the operation table, and the text an agent reads.
//!
//! Each declared tool family holds the request rows that name it. Its input
//! schema is generated small: `operation` names the row, `arguments` is the
//! rows' fields merged into one flat object, and the request options follow.
//! Its hints come from its rows' access, which registration keeps uniform.
//! Tools carry no output schema; `answer_schema` serves the answer's once.
//! Stream and local rows are not tools.
use crate::RequestOption;

/// An answer as a surface shows it: the answer (its envelope completed by
/// rendering), the value's text, and the envelope's lines.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedAnswer {
    pub answer: Answer<Value>,
    pub value_text: String,
    pub footer: String,
}
use crate::operation::{Forms, terminal_text};
use crate::{
    Access, Answer, Caller, CancellationToken, Deadline, Failure, Format, LeftOut,
    OperationCatalog, OperationTable, Outcome, RunnableRequest, Suggestion, TextStyle, Via,
    approximate_tokens, bad_request,
};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// The type of one request field as a tool list states it: a scalar keeps
/// its type and its enum values; an object, a list or a choice among
/// them is only its bare type. The full schema of each row is
/// `OperationCatalog::family_schemas`, for a product's explain answer.
fn shallow(schema: &Value, definitions: &Map<String, Value>) -> Value {
    let schema = match schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
    {
        Some(name) => definitions.get(name).unwrap_or(&Value::Null),
        None => schema,
    };
    if let Some(values) = schema.get("enum") {
        return json!({ "enum": values });
    }
    if let Some(value) = schema.get("const") {
        return json!({ "enum": [value] });
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(choices) = schema.get(key).and_then(Value::as_array) {
            let mut kinds: Vec<Value> = vec![];
            for choice in choices {
                let kind = shallow(choice, definitions);
                if kind != json!({"type": "null"}) && !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            return match kinds.len() {
                1 => kinds.pop().expect("one kind"),
                // Choices of plain values are one enum.
                _ if kinds.iter().all(|k| k.get("enum").is_some()) => json!({
                    "enum": kinds
                        .iter()
                        .flat_map(|k| k["enum"].as_array().cloned().unwrap_or_default())
                        .collect::<Vec<_>>()
                }),
                _ => json!({}),
            };
        }
    }
    let types: Vec<&str> = match &schema["type"] {
        Value::String(name) => vec![name.as_str()],
        Value::Array(names) => names
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| *name != "null")
            .collect(),
        _ if schema.get("properties").is_some() => vec!["object"],
        _ => vec![],
    };
    match types.as_slice() {
        [one] => json!({ "type": one }),
        _ => json!({}),
    }
}

/// One merged field of a family's arguments: each distinct type it has, with
/// the rows that give it that type.
struct MergedField {
    name: String,
    kinds: Vec<(Value, Vec<String>)>,
}

/// The input schema of `tool`: the family's rows' listed request fields
/// merged into one flat `arguments` object, deduplicated by name, each field
/// reduced to its shallow type (`arguments` stays open, so unlisted fields
/// are accepted and each row checks its own at dispatch); `operation` naming the row (required when the family
/// has several); and the request options. A field two rows type differently
/// is a choice of its types, described by the rows that use each. Each row
/// checks its own arguments at dispatch.
pub(crate) fn tool_input(forms: &Forms, tool: &str) -> Result<Value, Failure> {
    let rows: Vec<_> = forms
        .descriptions
        .iter()
        .filter(|d| d.spec.tool() == Some(tool))
        .collect();
    let mut fields: Vec<MergedField> = vec![];
    for row in &rows {
        let definitions = row.input_schema["$defs"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (name, property) in row.input_schema["properties"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if row
                .spec
                .argument(name)
                .is_some_and(|argument| !argument.listed)
            {
                continue;
            }
            let kind = shallow(property, &definitions);
            let index = match fields.iter().position(|field| field.name == *name) {
                Some(index) => index,
                None => {
                    fields.push(MergedField {
                        name: name.clone(),
                        kinds: vec![],
                    });
                    fields.len() - 1
                }
            };
            let kinds = &mut fields[index].kinds;
            match kinds.iter_mut().find(|(existing, _)| *existing == kind) {
                Some((_, users)) => users.push(row.spec.name.clone()),
                None => kinds.push((kind, vec![row.spec.name.clone()])),
            }
        }
    }
    let arguments: Map<String, Value> = fields
        .into_iter()
        .map(|mut field| {
            let schema = if field.kinds.len() == 1 {
                field.kinds.pop().expect("one kind").0
            } else {
                let used_by = field
                    .kinds
                    .iter()
                    .map(|(_, users)| users.join(", "))
                    .collect::<Vec<_>>()
                    .join("; ");
                json!({
                    "anyOf": field.kinds.into_iter().map(|(kind, _)| kind).collect::<Vec<_>>(),
                    "description": format!("typed per operation: {used_by}"),
                })
            };
            (field.name, schema)
        })
        .collect();
    let mut properties = Map::new();
    let mut required = vec![json!("arguments"), json!("request_id")];
    properties.insert(
        "operation".into(),
        json!({ "enum": rows.iter().map(|r| r.spec.name.clone()).collect::<Vec<_>>() }),
    );
    if rows.len() > 1 {
        required.insert(0, json!("operation"));
    }
    properties.insert(
        "arguments".into(),
        json!({ "type": "object", "properties": arguments }),
    );
    properties.insert("request_id".into(), json!({"type": "string"}));
    properties.insert(
        "deadline".into(),
        json!({"type": ["integer", "null"], "description": "ms; null: none"}),
    );
    for option in &forms.options {
        let schema = match option {
            RequestOption::BudgetTokens => {
                json!({"type": ["integer", "null"], "description": "tokens; null: unlimited"})
            }
            RequestOption::Cursor => json!({"type": "string"}),
            RequestOption::Fresh => json!({"type": "boolean"}),
            RequestOption::Format => json!({"enum": ["text", "json"]}),
        };
        properties.insert(option.name().into(), schema);
    }
    Ok(json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    }))
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
    budget_tokens: crate::TokenBudget,
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
        call.insert("arguments".into(), request.arguments.clone());
        if several {
            call.insert("operation".into(), request.operation.clone().into());
        }
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
        match request.budget_tokens {
            crate::TokenBudget::Default => {}
            crate::TokenBudget::Unlimited => {
                call.insert("budget_tokens".into(), Value::Null);
            }
            crate::TokenBudget::Tokens(n) => {
                call.insert("budget_tokens".into(), n.into());
            }
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
    /// A surface-ready call; absence never falls back to a different surface.
    pub(crate) fn follow_up(&self, via: Via, request: &RunnableRequest) -> Result<String, Failure> {
        if via == Via::Mcp {
            return self
                .tool_call(request)
                .ok_or_else(|| bad_request(format!("{} has no MCP surface", request.operation)));
        }
        self.cli_command(request)
    }
    fn display_follow_up(&self, via: Via, request: &RunnableRequest) -> String {
        self.follow_up(via, request)
            .unwrap_or_else(|error| format!("unavailable: {}", terminal_text(&error.message)))
    }
    /// The one line an answer prints for something it left out.
    pub(crate) fn left_out_line(&self, via: Via, item: &LeftOut) -> String {
        let unit = if item.count == 1 { "entry" } else { "entries" };
        match &item.expand {
            Some(expand) => format!(
                "left out: {} {unit} of {}; read them: {}\n",
                item.count,
                terminal_text(&item.what),
                self.display_follow_up(via, expand)
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
    pub(crate) fn footer<T>(
        &self,
        via: Via,
        outcome: &Outcome<T>,
        envelope: &crate::Envelope,
    ) -> String {
        let mut text = String::new();
        if let Outcome::Error(error) = outcome {
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
                    text.push_str(&format!("try: {}\n", self.display_follow_up(via, request)));
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
        if let Outcome::Degraded { missing, .. } = outcome {
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
            let mut words = vec![source.state.state().to_owned()];
            if let Some(why) = source.state.detail() {
                words.push(format!("({why})"));
            }
            for interval in &source.not_complete {
                words.push(format!(
                    "{}–{} {}",
                    interval.start,
                    interval.end,
                    interval.state.state()
                ));
            }
            text.push_str(&format!(
                "coverage: {} {}\n",
                terminal_text(&source.source),
                terminal_text(&words.join(" "))
            ));
        }
        for item in &envelope.left_out {
            text.push_str(&self.left_out_line(via, item));
        }
        for note in &envelope.notes {
            text.push_str(&format!("note: {}\n", terminal_text(note)));
        }
        for next in &envelope.next {
            text.push_str(&format!(
                "next: {}: {}\n",
                terminal_text(&next.label),
                self.display_follow_up(via, &next.request)
            ));
        }
        text
    }
}

impl OperationCatalog {
    /// Every row of a tool family with its full request schema, for a
    /// product's explain or capabilities answer: the tool list states only
    /// each field's shallow type.
    pub fn family_schemas(&self, family: &str) -> Value {
        Value::Object(
            self.descriptions()
                .iter()
                .filter(|d| d.spec.tool() == Some(family))
                .map(|d| (d.spec.name.clone(), d.input_schema.clone()))
                .collect(),
        )
    }
    /// The schema of an answer (`Answer<Value>`): the envelope every
    /// operation's answer carries. MCP tools carry no output schema; a
    /// product serves this once, from its explain or capabilities answer.
    pub fn answer_schema() -> Value {
        schemars::schema_for!(Answer<Value>).to_value()
    }
    /// The command line that runs `request`, in its row's own argument forms
    /// when they can hold every field, else with `--request JSON`.
    pub fn cli_command(&self, request: &RunnableRequest) -> Result<String, Failure> {
        self.forms().cli_command(request)
    }
    /// A follow-up as the surface of `via` writes it.
    pub fn follow_up(&self, via: Via, request: &RunnableRequest) -> Result<String, Failure> {
        self.forms().follow_up(via, request)
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
                let input = tool_input(self.forms(), &tool.name)
                    .expect("registration checked every tool's schema");
                Some(json!({
                    "name": self.forms().tool_name(&tool.name),
                    "description": tool.description,
                    "inputSchema": input,
                    "annotations": {
                        "readOnlyHint": access == Access::Read,
                        "destructiveHint": access == Access::Permanent,
                        "idempotentHint": tool.idempotent,
                        "openWorldHint": tool.open_world,
                    }
                }))
            })
            .collect()
    }
}

impl OperationTable {
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
                    Some(tool) => {
                        let metadata = message["params"].get("_meta").cloned();
                        let caller = caller.for_mcp_call(metadata.as_ref());
                        Ok(self
                            .tool_result(
                                &tool.name,
                                message["params"]["arguments"].clone(),
                                metadata,
                                caller,
                                cancellation,
                            )
                            .await)
                    }
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
        let call = serde_json::from_value::<ToolCall>(arguments)
            .map_err(|error| bad_request(error.to_string()));
        let format = call.as_ref().map_or(Format::Text, |call| call.format);
        let (request, answer) = match call.and_then(|call| self.tool_request(&rows, call)) {
            Err(failure) => (None, Answer::error(failure)),
            Ok(request) => {
                let answer = self
                    .call_with(request.clone(), Via::Mcp, caller, metadata, cancellation)
                    .await;
                (Some(request), answer)
            }
        };
        let RenderedAnswer {
            mut answer,
            value_text,
            footer,
        } = self.render_answer(Via::Mcp, request.as_ref(), answer, TextStyle::Plain);
        if format == Format::Text && !answer.is_error() {
            // The rows are in the text part; the structured part says so
            // instead of sending them twice.
            answer.envelope.value_in_text = true;
        }
        let mut structured = serde_json::to_value(&answer).expect("answers serialize");
        if answer.envelope.value_in_text
            && let Some(object) = structured.as_object_mut()
        {
            object.remove("value");
        }
        json!({
            "isError": answer.is_error(),
            "structuredContent": structured,
            "content": [{"type": "text", "text": format!("{value_text}{footer}")}],
        })
    }
}

impl OperationCatalog {
    /// The request a tool call makes: its row (named by `operation` when the
    /// tool has several) and the call's options. The row's own schema checks
    /// the arguments at dispatch.
    fn tool_request(&self, rows: &[&str], call: ToolCall) -> Result<RunnableRequest, Failure> {
        let operation = match (call.operation, rows) {
            (Some(operation), _) if rows.contains(&operation.as_str()) => operation,
            (Some(operation), _) => {
                return Err(bad_request(format!(
                    "{operation} is not an operation of this tool ({})",
                    rows.join(", ")
                )));
            }
            (None, [only]) => (*only).to_owned(),
            (None, _) => {
                return Err(bad_request(format!(
                    "name the operation: one of {}",
                    rows.join(", ")
                )));
            }
        };
        let arguments = call.arguments;
        Ok(RunnableRequest {
            operation,
            arguments,
            request_id: call.request_id,
            deadline: call.deadline,
            budget_tokens: call.budget_tokens,
            cursor: call.cursor,
            fresh: call.fresh,
            format: call.format,
        })
    }
    /// The envelope's presentation, without decoding or rendering the response value.
    pub fn render_footer(&self, via: Via, answer: &Answer<Value>) -> String {
        self.forms().footer(via, &answer.outcome, &answer.envelope)
    }
    /// An answer as text for a person or model: the value's rendering
    /// (nothing with `format: json`, whose rows travel as data, or for
    /// `operation` empty) and the envelope's lines, measured after the
    /// envelope is final. Every surface that shows an answer as text calls
    /// this, the command line and MCP here and a product's client for an
    /// answer it read from the socket.
    ///
    /// Budget fitting belongs to the operation while its logical entries are
    /// still typed (`RequestContext::fit`). Rendering preserves those entries
    /// and measures the final text; it never cuts the rendered value again.
    pub fn render_answer(
        &self,
        via: Via,
        request: Option<&RunnableRequest>,
        mut answer: Answer<Value>,
        style: TextStyle,
    ) -> RenderedAnswer {
        let operation = request.map_or("", |request| request.operation.as_str());
        let streamed = self
            .spec_named(operation)
            .is_some_and(|spec| spec.kind == crate::OperationKind::Stream);
        let json = request.is_some_and(|request| request.format == Format::Json);
        let full = match &answer.outcome {
            Outcome::Error(_) => self.render_text("", &answer, style).unwrap_or_default(),
            _ if json || streamed || operation.is_empty() => String::new(),
            _ => self
                .render_text(operation, &answer, style)
                .unwrap_or_else(|failure| format!("error (io): {}\n", failure.message)),
        };
        let value_text = full;
        let footer = self.render_footer(via, &answer);
        let used = approximate_tokens(&format!("{value_text}{footer}"));
        answer.envelope.used_tokens = Some(used);
        RenderedAnswer {
            answer,
            value_text,
            footer,
        }
    }
}
