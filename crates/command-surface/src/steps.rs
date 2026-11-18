//! Typed verb declarations for the steps of a product's command line.
//! The shared line scanner splits steps; these declarations own each step's words
//! and options, its JSON validation, and its help signature.
use crate::arguments::{ArgumentScope, check_arguments, parse_arguments, schema_has_type};
use crate::{
    ArgumentForm, ArgumentSpec, DeclarationsBuilder, Failure, OperationDeclarations,
    OperationDescription, OperationKind, OperationSpec, bad_request,
};
use schemars::{JsonSchema, schema_for};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::sync::Arc;

/// One verb's fields, typed schema and argument forms.
#[derive(Clone)]
pub struct StepDeclaration {
    pub(crate) description: OperationDescription,
    pub(crate) validate: fn(Value) -> Result<(), Failure>,
}

/// The verbs accepted by one operation's steps argument.
#[derive(Clone, Default)]
pub struct StepDeclarations {
    pub(crate) verbs: Vec<StepDeclaration>,
    pub(crate) cli_selection: Option<(String, String)>,
}

impl StepDeclaration {
    /// The verb as written in a step's `verb` field.
    pub fn verb(&self) -> &str {
        &self.description.spec.name
    }
    /// The field declarations in signature order.
    pub fn arguments(&self) -> &[ArgumentSpec] {
        &self.description.spec.arguments
    }
    /// The full typed schema, excluding the step's `verb` field.
    pub fn input_schema(&self) -> &Value {
        &self.description.input_schema
    }
}

impl StepDeclarations {
    /// Render this operation as one selection followed by its declared verbs.
    /// Products retain selection resolution and calling-process behavior.
    pub fn cli_selection(
        mut self,
        selection: impl Into<String>,
        references: impl Into<String>,
    ) -> Self {
        self.cli_selection = Some((selection.into(), references.into()));
        self
    }
    /// The declarations in help and schema order.
    pub fn verbs(&self) -> &[StepDeclaration] {
        &self.verbs
    }
}

impl StepDeclaration {
    /// A one-line signature, with optional fields followed by `?`.
    pub fn signature(&self) -> String {
        let fields = crate::arguments::projection(&self.description, &[])
            .into_iter()
            .filter(|argument| argument.spec.listed)
            .map(|argument| {
                format!(
                    "{}{}",
                    argument.spec.name,
                    if argument.required { "" } else { "?" }
                )
            })
            .collect::<Vec<_>>();
        format!("{}({})", self.verb(), fields.join(", "))
    }
}

impl StepDeclarations {
    /// Declare one verb's typed fields. Positional fields each take one word;
    /// flags use hyphens on the CLI and field names use underscores in JSON.
    pub fn declare<A: DeserializeOwned + JsonSchema>(
        &mut self,
        verb: impl Into<String>,
        arguments: Vec<ArgumentSpec>,
    ) -> Result<(), Failure> {
        let verb = verb.into();
        if !crate::operation::valid_word(&verb, '-')
            || self.verbs.iter().any(|entry| entry.verb() == verb)
        {
            return Err(bad_request(format!(
                "step verb {verb:?} is not a free name"
            )));
        }
        let mut spec = OperationSpec::request(&verb, &[], "", &verb, &verb);
        spec.kind = OperationKind::Local;
        spec.arguments = arguments;
        let description = OperationDescription {
            spec,
            input_schema: schema_for!(A).to_value(),
            output_schema: Value::Bool(true),
            event_schema: None,
        };
        let Some(properties) = description.input_schema["properties"].as_object() else {
            return Err(bad_request("step arguments must be an object"));
        };
        if properties.contains_key("verb")
            || properties
                .keys()
                .any(|name| !crate::operation::valid_word(name, '_'))
        {
            return Err(bad_request(
                "step fields use underscore names and reserve verb",
            ));
        }
        if description.spec.arguments.iter().any(|argument| {
            argument.calling_process
                || matches!(argument.form, ArgumentForm::Words | ArgumentForm::Rest)
        }) {
            return Err(bad_request(
                "step positionals take one word; options belong to the step",
            ));
        }
        check_arguments(&description, false)?;
        self.verbs.push(StepDeclaration {
            description,
            validate: |value| {
                serde_json::from_value::<A>(value)
                    .map(|_| ())
                    .map_err(|error| bad_request(error.to_string()))
            },
        });
        Ok(())
    }

    fn named(&self, verb: &str) -> Result<&StepDeclaration, Failure> {
        self.verbs
            .iter()
            .find(|entry| entry.verb() == verb)
            .ok_or_else(|| {
                bad_request(format!(
                    "unknown step {verb:?}; use {}",
                    self.verbs
                        .iter()
                        .map(StepDeclaration::verb)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
    }

    /// Parse words separated from the line by `split_step` or another caller.
    /// Returns the MCP/HTTP object, including `verb`, after typed validation.
    pub fn parse_step(&self, verb: &str, words: &[String]) -> Result<Value, Failure> {
        let declaration = self.named(verb)?;
        let parsed = parse_arguments(&declaration.description, words, "", ArgumentScope::Step)
            .map_err(|mut failure| {
                failure.hint = Some(declaration.signature());
                failure
            })?;
        let mut step = parsed.request.arguments;
        step.as_object_mut()
            .expect("parsed fields are an object")
            .insert("verb".into(), verb.into());
        self.validate_step(&step)?;
        Ok(step)
    }

    /// Validate one JSON step against its own verb, including unknown fields.
    /// The product retains the original value and decodes its own request.
    pub fn validate_step(&self, step: &Value) -> Result<(), Failure> {
        let object = step
            .as_object()
            .ok_or_else(|| bad_request("a step must be an object"))?;
        let verb = object
            .get("verb")
            .and_then(Value::as_str)
            .ok_or_else(|| bad_request("a step needs a text verb"))?;
        let declaration = self.named(verb)?;
        let properties = declaration.input_schema()["properties"]
            .as_object()
            .expect("declared object");
        if let Some(name) = object
            .keys()
            .find(|name| name.as_str() != "verb" && !properties.contains_key(*name))
        {
            return Err(bad_request(format!("{verb}: unknown argument {name}"))
                .with_hint(declaration.signature()));
        }
        let mut fields = object.clone();
        fields.remove("verb");
        (declaration.validate)(Value::Object(fields)).map_err(|failure| {
            bad_request(format!("{verb}: {}", failure.message)).with_hint(declaration.signature())
        })
    }

    /// Small tool-list schema: verbs are closed; each verb validates its fields.
    pub fn input_schema(&self) -> Value {
        json!({"type": "array", "items": {"type": "object", "properties": {"verb": {"enum": self.verbs.iter().map(StepDeclaration::verb).collect::<Vec<_>>() }}, "required": ["verb"]}})
    }
}

impl DeclarationsBuilder {
    /// Attach a verb catalog to an existing array argument, without changing
    /// the operation's typed request or the product's line splitter.
    pub fn declare_steps(
        &mut self,
        operation: &str,
        argument: &str,
        steps: StepDeclarations,
    ) -> Result<(), Failure> {
        let description = self
            .forms()
            .description(operation)
            .ok_or_else(|| bad_request(format!("unknown operation {operation}")))?;
        let field = description.input_schema["properties"]
            .get(argument)
            .ok_or_else(|| bad_request(format!("{operation} has no argument {argument}")))?;
        let array = schema_has_type(field, &description.input_schema, "array");
        if !array || steps.verbs.is_empty() {
            return Err(bad_request(
                "steps need an array argument and at least one verb",
            ));
        }
        if let Some((selection, references)) = &steps.cli_selection {
            let properties = &description.input_schema["properties"];
            if selection == references
                || selection == argument
                || references == argument
                || !properties.get(selection).is_some_and(|field| {
                    schema_has_type(field, &description.input_schema, "string")
                })
                || !properties
                    .get(references)
                    .is_some_and(|field| schema_has_type(field, &description.input_schema, "array"))
            {
                return Err(bad_request(
                    "a command line needs distinct selection, reference-list and step-list fields",
                ));
            }
            if self.forms().steps.iter().any(|((row, _), declarations)| {
                row == operation && declarations.cli_selection.is_some()
            }) {
                return Err(bad_request("an operation has one command line"));
            }
        }
        let key = (operation.to_owned(), argument.to_owned());
        if self.forms().steps.contains_key(&key) {
            return Err(bad_request(format!(
                "{operation}.{argument} has step declarations already"
            )));
        }
        Arc::make_mut(&mut self.declarations.forms)
            .steps
            .insert(key, steps);
        Ok(())
    }
}

impl OperationDeclarations {
    /// The catalog attached to one operation argument.
    pub fn steps(&self, operation: &str, argument: &str) -> Option<&StepDeclarations> {
        self.forms()
            .steps
            .get(&(operation.to_owned(), argument.to_owned()))
    }

    pub(crate) fn validate_steps(&self, operation: &str, arguments: &Value) -> Result<(), Failure> {
        for ((row, argument), steps) in &self.forms().steps {
            if row != operation {
                continue;
            }
            let Some(value) = arguments.get(argument) else {
                continue;
            };
            let description = self
                .forms()
                .description(operation)
                .expect("declared operation");
            if value.is_null()
                && schema_has_type(
                    &description.input_schema["properties"][argument],
                    &description.input_schema,
                    "null",
                )
            {
                continue;
            }
            let values = value
                .as_array()
                .ok_or_else(|| bad_request(format!("{argument} must be an array")))?;
            for (index, step) in values.iter().enumerate() {
                steps.validate_step(step).map_err(|mut failure| {
                    failure.message = format!("{argument}[{index}]: {}", failure.message);
                    failure
                })?;
            }
        }
        Ok(())
    }
}
