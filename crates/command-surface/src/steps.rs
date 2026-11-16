//! Typed verb declarations for the steps of a product's command line.
//! Products split lines into steps; these declarations own each step's words
//! and options, its JSON validation, and its help signature.
use crate::{ArgumentSpec, Failure, OperationDescription};
use serde_json::Value;

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
    /// The declarations in help and schema order.
    pub fn verbs(&self) -> &[StepDeclaration] {
        &self.verbs
    }
}
