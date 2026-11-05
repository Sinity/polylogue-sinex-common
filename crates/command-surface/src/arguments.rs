//! The command line of one row: checking its argument declarations against
//! its request type, reading a command line into a request, writing a
//! request back as a command line, and its usage line.
use crate::operation::{Forms, GLOBAL_FLAGS, valid_word};
use crate::{
    ArgumentForm, ArgumentSpec, Deadline, Failure, Format, OperationDescription, OperationKind,
    OperationTable, RequestOption, RunnableRequest, bad_request, quote_argument,
};
use serde_json::{Map, Value};

/// How a request field's value is written as words.
#[derive(Clone, Debug, PartialEq)]
enum ValueKind {
    /// Taken as written.
    Text,
    Boolean,
    /// Each word one item.
    List(Box<ValueKind>),
    /// Written as JSON (numbers, objects, enums of objects).
    Json,
}

fn resolve<'a>(schema: &'a Value, root: &'a Value) -> &'a Value {
    match schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
    {
        Some(name) => root["$defs"].get(name).unwrap_or(schema),
        None => schema,
    }
}

fn value_kind(schema: &Value, root: &Value) -> ValueKind {
    let schema = resolve(schema, root);
    for key in ["anyOf", "oneOf"] {
        if let Some(choices) = schema.get(key).and_then(Value::as_array) {
            let choices: Vec<&Value> = choices
                .iter()
                .filter(|choice| resolve(choice, root).get("type") != Some(&Value::from("null")))
                .collect();
            return match choices.as_slice() {
                [only] => value_kind(only, root),
                _ if choices
                    .iter()
                    .all(|choice| value_kind(choice, root) == ValueKind::Text) =>
                {
                    ValueKind::Text
                }
                _ => ValueKind::Json,
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
        _ => vec![],
    };
    match types.as_slice() {
        ["string"] => ValueKind::Text,
        ["boolean"] => ValueKind::Boolean,
        ["array"] => ValueKind::List(Box::new(value_kind(&schema["items"], root))),
        [] if schema.get("const").is_some_and(Value::is_string)
            || schema
                .get("enum")
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().all(Value::is_string)) =>
        {
            ValueKind::Text
        }
        _ => ValueKind::Json,
    }
}

fn property<'a>(description: &'a OperationDescription, name: &str) -> Option<&'a Value> {
    description.input_schema["properties"].get(name)
}
fn kind_of(description: &OperationDescription, name: &str) -> ValueKind {
    property(description, name).map_or(ValueKind::Json, |schema| {
        value_kind(schema, &description.input_schema)
    })
}
fn required(description: &OperationDescription, name: &str) -> bool {
    description.input_schema["required"]
        .as_array()
        .is_some_and(|names| names.iter().any(|n| n == name))
}
fn is_positional(form: ArgumentForm) -> bool {
    matches!(
        form,
        ArgumentForm::Positional | ArgumentForm::Words | ArgumentForm::Rest
    )
}

/// Reject argument declarations the request type cannot hold or the command
/// line cannot read back unambiguously.
pub(crate) fn check(description: &OperationDescription) -> Result<(), Failure> {
    let spec = &description.spec;
    let refuse = |message: String| Err(bad_request(format!("{}: {message}", spec.name)));
    // A field is written `--name` unless it is declared positional, so no
    // field written as a flag may take a flag every row already has.
    let positional = |name: &str| {
        spec.argument(name)
            .is_some_and(|argument| is_positional(argument.form))
    };
    if let Some(properties) = description.input_schema["properties"].as_object()
        && let Some(name) = properties.keys().find(|name| {
            !positional(name) && GLOBAL_FLAGS.contains(&name.replace('_', "-").as_str())
        })
    {
        return refuse(format!(
            "request field {name} would be written as --{}, a flag of every row",
            name.replace('_', "-")
        ));
    }
    let mut seen_optional_positional = false;
    let mut seen_words = false;
    for (index, argument) in spec.arguments.iter().enumerate() {
        if !valid_word(&argument.name, '_') {
            return refuse(format!("argument name {:?} is not plain", argument.name));
        }
        if spec.arguments[..index]
            .iter()
            .any(|other| other.name == argument.name)
        {
            return refuse(format!("argument {} is declared twice", argument.name));
        }
        if !is_positional(argument.form)
            && GLOBAL_FLAGS.contains(&argument.flag().trim_start_matches('-'))
        {
            return refuse(format!("{} is a flag of every row", argument.flag()));
        }
        if let Some(short) = argument.short
            && (!short.is_ascii_alphabetic()
                || spec.arguments[..index]
                    .iter()
                    .any(|other| other.short == Some(short)))
        {
            return refuse(format!("short flag -{short} is not a free letter"));
        }
        let exists = property(description, &argument.name).is_some();
        if argument.calling_process {
            if exists || is_positional(argument.form) {
                return refuse(format!(
                    "{} acts on the calling process, so it is a named flag outside the request",
                    argument.name
                ));
            }
            continue;
        }
        if !exists {
            return refuse(format!("{} is not a field of the request", argument.name));
        }
        let kind = kind_of(description, &argument.name);
        let fits = match argument.form {
            ArgumentForm::Flag => kind == ValueKind::Boolean,
            ArgumentForm::Repeated => matches!(kind, ValueKind::List(_)),
            ArgumentForm::Words | ArgumentForm::Rest => {
                matches!(kind, ValueKind::Text | ValueKind::List(_))
            }
            ArgumentForm::Positional | ArgumentForm::Named => true,
        };
        if !fits {
            return refuse(format!(
                "{} cannot be written as {:?} given its type",
                argument.name, argument.form
            ));
        }
        if is_positional(argument.form) {
            if seen_words {
                return refuse(format!(
                    "{} follows a positional that takes the remaining words",
                    argument.name
                ));
            }
            if seen_optional_positional && required(description, &argument.name) {
                return refuse(format!(
                    "required {} follows an optional positional",
                    argument.name
                ));
            }
            seen_optional_positional |= !required(description, &argument.name);
            seen_words |= matches!(argument.form, ArgumentForm::Words | ArgumentForm::Rest);
        }
    }
    Ok(())
}

/// A command line read against the table: the request (`--json` is its
/// format), how the row runs, and the flags that act on the calling process.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedCommand {
    pub request: RunnableRequest,
    pub kind: OperationKind,
    /// `--json`: print the answer as JSON. The request asks for JSON rows
    /// too when the table serves the `format` option.
    pub json: bool,
    pub calling_process: Map<String, Value>,
}

fn word_value(kind: &ValueKind, word: &str, flag: &str) -> Result<Value, Failure> {
    match kind {
        ValueKind::Text => Ok(Value::String(word.to_owned())),
        ValueKind::Boolean => match word {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(bad_request(format!("{flag} is true or false"))),
        },
        ValueKind::List(item) => word_value(item, word, flag),
        ValueKind::Json => {
            serde_json::from_str(word).map_err(|e| bad_request(format!("{flag}: {e}")))
        }
    }
}

impl OperationTable {
    /// The row a command line names: the longest matching command, else the
    /// default operation with every word its own.
    fn command_row(&self, args: &[String]) -> Option<(&OperationDescription, usize)> {
        self.descriptions()
            .iter()
            .filter(|d| args.starts_with(&d.spec.command))
            .max_by_key(|d| d.spec.command.len())
            .map(|d| (d, d.spec.command.len()))
            .or_else(|| self.default_operation().map(|d| (d, 0)))
    }

    /// Read a command line into its row's request. Fields come from the
    /// row's argument declarations and request schema; `--request JSON`
    /// supplies the whole value for any type. `--json`, `--deadline MS|none`,
    /// `--request-id ID`, `--budget N`, `--cursor C` and `--fresh` work on
    /// every row.
    pub fn parse_cli(
        &self,
        args: &[String],
        request_id: impl Into<String>,
    ) -> Result<ParsedCommand, Failure> {
        let (description, start) = self
            .command_row(args)
            .ok_or_else(|| bad_request("unknown command; use --help"))?;
        let spec = &description.spec;
        let mut fields = Map::new();
        let mut calling_process = Map::new();
        let mut request = None;
        let mut deadline = Deadline::Default;
        let mut budget_tokens = crate::TokenBudget::Default;
        let mut cursor = None;
        let mut fresh = false;
        let mut id = request_id.into();
        let mut json = false;
        let positionals: Vec<&ArgumentSpec> = spec
            .arguments
            .iter()
            .filter(|a| is_positional(a.form))
            .collect();
        let mut positional = 0;
        let mut words: Vec<String> = vec![];
        let mut options_done = false;
        let mut rest = false;
        let mut index = start;
        let argument_named = |flag: &str| -> Option<ArgumentSpec> {
            if let Some(argument) = spec.arguments.iter().find(|a| a.flag() == flag) {
                return Some(argument.clone());
            }
            let name = flag.trim_start_matches("--");
            let name = if property(description, name).is_some() {
                name.to_owned()
            } else {
                name.replace('-', "_")
            };
            property(description, &name).map(|_| ArgumentSpec::new(name, ArgumentForm::Named, ""))
        };
        while index < args.len() {
            let word = &args[index];
            index += 1;
            if rest {
                words.push(word.clone());
                continue;
            }
            if !options_done && word == "--" {
                options_done = true;
                continue;
            }
            let flag = if options_done {
                None
            } else if word.starts_with("--") {
                Some(word.clone())
            } else {
                word.strip_prefix('-')
                    .and_then(|short| {
                        let mut chars = short.chars();
                        match (chars.next(), chars.next()) {
                            (Some(letter), None) if letter.is_ascii_alphabetic() => Some(letter),
                            _ => None,
                        }
                    })
                    .and_then(|letter| spec.arguments.iter().find(|a| a.short == Some(letter)))
                    .map(ArgumentSpec::flag)
            };
            let Some(flag) = flag else {
                let argument = positionals
                    .get(positional)
                    .ok_or_else(|| bad_request(format!("unexpected word {word}")))?;
                match argument.form {
                    ArgumentForm::Positional => {
                        let kind = match kind_of(description, &argument.name) {
                            ValueKind::List(_) => ValueKind::Json,
                            kind => kind,
                        };
                        let value = word_value(&kind, word, &argument.name)?;
                        fields.insert(argument.name.clone(), value);
                        positional += 1;
                    }
                    ArgumentForm::Rest => {
                        words.push(word.clone());
                        rest = true;
                    }
                    _ => words.push(word.clone()),
                }
                continue;
            };
            let (flag, inline) = match flag.split_once('=') {
                Some((flag, value)) => (flag.to_owned(), Some(value.to_owned())),
                None => (flag, None),
            };
            let mut value = || -> Result<String, Failure> {
                if let Some(value) = &inline {
                    return Ok(value.clone());
                }
                let value = args
                    .get(index)
                    .cloned()
                    .ok_or_else(|| bad_request(format!("{flag} needs a value")))?;
                index += 1;
                Ok(value)
            };
            match flag.as_str() {
                "--json" if inline.is_none() => json = true,
                "--deadline" => {
                    let value = value()?;
                    deadline =
                        if value == "none" {
                            Deadline::None
                        } else {
                            Deadline::Milliseconds(value.parse().map_err(|_| {
                                bad_request("deadline must be milliseconds or none")
                            })?)
                        };
                }
                "--request-id" => id = value()?,
                "--cursor" => cursor = Some(value()?),
                "--fresh" if inline.is_none() => fresh = true,
                "--budget" => {
                    let value = value()?;
                    budget_tokens = if value == "none" {
                        crate::TokenBudget::Unlimited
                    } else {
                        crate::TokenBudget::Tokens(value.parse().map_err(|_| {
                            bad_request("budget must be a number of tokens or none")
                        })?)
                    };
                }
                "--request" => {
                    if request.is_some() {
                        return Err(bad_request("duplicate --request"));
                    }
                    request = Some(
                        serde_json::from_str::<Value>(&value()?)
                            .map_err(|e| bad_request(e.to_string()))?,
                    );
                }
                _ => {
                    let argument = argument_named(&flag)
                        .ok_or_else(|| bad_request(format!("unknown argument {flag}")))?;
                    let target = if argument.calling_process {
                        &mut calling_process
                    } else {
                        &mut fields
                    };
                    let kind = if argument.calling_process {
                        match argument.form {
                            ArgumentForm::Flag => ValueKind::Boolean,
                            ArgumentForm::Repeated => ValueKind::List(Box::new(ValueKind::Text)),
                            _ => ValueKind::Text,
                        }
                    } else {
                        kind_of(description, &argument.name)
                    };
                    match argument.form {
                        ArgumentForm::Flag => {
                            let value = match &inline {
                                None => Value::Bool(true),
                                Some(word) => word_value(&ValueKind::Boolean, word, &flag)?,
                            };
                            if target.insert(argument.name.clone(), value).is_some() {
                                return Err(bad_request(format!("duplicate {flag}")));
                            }
                        }
                        ArgumentForm::Repeated => {
                            let item = word_value(&kind, &value()?, &flag)?;
                            target
                                .entry(argument.name.clone())
                                .or_insert_with(|| Value::Array(vec![]))
                                .as_array_mut()
                                .expect("repeated values are a list")
                                .push(item);
                        }
                        _ => {
                            // A list written with one flag is written whole, as JSON.
                            let kind = match kind {
                                ValueKind::List(_) => ValueKind::Json,
                                kind => kind,
                            };
                            let item = word_value(&kind, &value()?, &flag)?;
                            if target.insert(argument.name.clone(), item).is_some() {
                                return Err(bad_request(format!("duplicate {flag}")));
                            }
                        }
                    }
                }
            }
        }
        if let Some(argument) = positionals.get(positional)
            && matches!(argument.form, ArgumentForm::Words | ArgumentForm::Rest)
            && (!words.is_empty() || rest)
        {
            let value = match kind_of(description, &argument.name) {
                ValueKind::List(item) => Value::Array(
                    words
                        .iter()
                        .map(|word| word_value(&item, word, &argument.name))
                        .collect::<Result<_, _>>()?,
                ),
                _ => Value::String(words.join(" ")),
            };
            fields.insert(argument.name.clone(), value);
        }
        let arguments = match request {
            Some(_) if !fields.is_empty() => {
                return Err(bad_request("--request cannot be mixed with named fields"));
            }
            Some(value) => value,
            None => {
                if let Some(missing) = positionals
                    .iter()
                    .find(|a| required(description, &a.name) && !fields.contains_key(&a.name))
                {
                    return Err(bad_request(format!(
                        "missing {}",
                        missing.name.to_uppercase()
                    )));
                }
                Value::Object(fields)
            }
        };
        Ok(ParsedCommand {
            request: RunnableRequest {
                operation: spec.name.clone(),
                arguments,
                request_id: id,
                deadline,
                budget_tokens,
                cursor,
                fresh,
                format: if json && self.accepts(RequestOption::Format) {
                    Format::Json
                } else {
                    Format::Text
                },
            },
            kind: spec.kind.clone(),
            json,
            calling_process,
        })
    }
}

/// The usage of one row: `read REF [--view VIEW] [--fresh]`.
pub(crate) fn usage(description: &OperationDescription) -> String {
    let spec = &description.spec;
    let mut parts: Vec<String> = spec.command.clone();
    let mut declared = std::collections::BTreeSet::new();
    for argument in &spec.arguments {
        declared.insert(argument.name.as_str());
        let name = argument.name.to_uppercase();
        let needed = !argument.calling_process && required(description, &argument.name);
        let part = match argument.form {
            ArgumentForm::Positional => name,
            ArgumentForm::Words => format!("{name}..."),
            ArgumentForm::Rest => format!("-- {name}..."),
            ArgumentForm::Named => format!("{} {name}", argument.flag()),
            ArgumentForm::Repeated => format!("{} {name}...", argument.flag()),
            ArgumentForm::Flag => argument.flag(),
        };
        parts.push(if needed { part } else { format!("[{part}]") });
    }
    if let Some(properties) = description.input_schema["properties"].as_object() {
        for name in properties
            .keys()
            .filter(|name| !declared.contains(name.as_str()))
        {
            let flag = format!("--{}", name.replace('_', "-"));
            let part = format!("{flag} {}", name.to_uppercase());
            parts.push(if required(description, name) {
                part
            } else {
                format!("[{part}]")
            });
        }
    }
    parts.join(" ")
}

impl Forms {
    /// A runnable command line for `request`: its row's own argument forms
    /// when every field can be written that way, else `--request JSON`.
    /// Reading it back with `parse_cli` gives the same request.
    pub(crate) fn cli_command(&self, request: &RunnableRequest) -> Result<String, Failure> {
        let description = self
            .description(&request.operation)
            .ok_or_else(|| bad_request("unknown operation"))?;
        let mut words = vec![quote_argument(&self.product)];
        words.extend(description.spec.command.iter().map(|s| quote_argument(s)));
        // Flags come before positionals: a positional that takes the rest of
        // the line would take any flag after it.
        let positionals = match natural_words(description, &request.arguments) {
            Some((flags, positionals)) => {
                words.extend(flags);
                positionals
            }
            None => {
                words.push("--request".into());
                words.push(quote_argument(&request.arguments.to_string()));
                vec![]
            }
        };
        words.push("--request-id".into());
        words.push(quote_argument(&request.request_id));
        match request.deadline {
            Deadline::Default => {}
            Deadline::None => words.extend(["--deadline".into(), "none".into()]),
            Deadline::Milliseconds(ms) => words.extend(["--deadline".into(), ms.to_string()]),
        }
        match request.budget_tokens {
            crate::TokenBudget::Default => {}
            crate::TokenBudget::Unlimited => words.extend(["--budget".into(), "none".into()]),
            crate::TokenBudget::Tokens(n) => words.extend(["--budget".into(), n.to_string()]),
        }
        if let Some(cursor) = &request.cursor {
            words.extend(["--cursor".into(), quote_argument(cursor)]);
        }
        if request.fresh {
            words.push("--fresh".into());
        }
        if request.format == Format::Json {
            words.push("--json".into());
        }
        if positionals.iter().any(|w| w.starts_with('-')) {
            words.push("--".into());
        }
        words.extend(positionals.iter().map(|w| quote_argument(w)));
        Ok(words.join(" "))
    }
}

/// The request's fields as the row's own flags (quoted) and positional words
/// (unquoted); `None` when some field cannot be written that way and read
/// back the same.
fn natural_words(
    description: &OperationDescription,
    arguments: &Value,
) -> Option<(Vec<String>, Vec<String>)> {
    let object = arguments.as_object()?;
    let spec = &description.spec;
    let word = |kind: &ValueKind, value: &Value| -> Option<String> {
        match (kind, value) {
            (ValueKind::Text, Value::String(text)) => Some(text.clone()),
            (ValueKind::Boolean, Value::Bool(flag)) => Some(flag.to_string()),
            (ValueKind::Json, value) => Some(value.to_string()),
            _ => None,
        }
    };
    let mut flags = vec![];
    for (name, value) in object {
        let argument = spec.argument(name);
        if argument.is_some_and(|a| a.calling_process) || property(description, name).is_none() {
            return None;
        }
        let form = argument.map_or(ArgumentForm::Named, |a| a.form);
        let flag = argument.map_or_else(
            || format!("--{}", name.replace('_', "-")),
            ArgumentSpec::flag,
        );
        let kind = kind_of(description, name);
        match form {
            ArgumentForm::Flag => match value {
                Value::Bool(true) => flags.push(flag),
                Value::Bool(false) => flags.push(format!("{flag}=false")),
                _ => return None,
            },
            ArgumentForm::Named => {
                let kind = match kind {
                    ValueKind::List(_) => ValueKind::Json,
                    kind => kind,
                };
                flags.extend([flag, quote_argument(&word(&kind, value)?)]);
            }
            ArgumentForm::Repeated => {
                let ValueKind::List(item) = &kind else {
                    return None;
                };
                let items = value.as_array().filter(|items| !items.is_empty())?;
                for item_value in items {
                    flags.extend([flag.clone(), quote_argument(&word(item, item_value)?)]);
                }
            }
            ArgumentForm::Positional | ArgumentForm::Words | ArgumentForm::Rest => {}
        }
    }
    let mut positional_words = vec![];
    let mut gap = false;
    for argument in spec.arguments.iter().filter(|a| is_positional(a.form)) {
        let Some(value) = object.get(&argument.name) else {
            gap = true;
            continue;
        };
        if gap {
            return None;
        }
        let kind = kind_of(description, &argument.name);
        match (&kind, argument.form) {
            (ValueKind::List(_), ArgumentForm::Positional) => {
                positional_words.push(value.to_string());
            }
            (ValueKind::List(item), _) => {
                let items = value.as_array().filter(|items| !items.is_empty())?;
                for item_value in items {
                    positional_words.push(word(item, item_value)?);
                }
            }
            _ => positional_words.push(word(&kind, value)?),
        }
    }
    Some((flags, positional_words))
}
