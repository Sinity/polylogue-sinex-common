//! Semantic scopes captured by binding, with paths into its returned syntax tree.
//!
//! These owned facts describe the language's declared types. Products retain
//! physical types (for example filesystem paths and refined JSON schemas) and
//! propagate them through the declared origins; they do not infer stages again.
use crate::{
    ast::{AggregateFunction, TemporalRelation},
    catalog::{Field, OutputSource},
};
use std::sync::Arc;

/// A structural address in the returned bound tree. Field names and array
/// indexes follow the AST, including enum payloads; byte spans are never keys.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodePath(pub Vec<PathElement>);
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathElement {
    Field(String),
    Index(usize),
}
impl NodePath {
    pub fn field(&self, name: impl Into<String>) -> Self {
        let mut path = self.clone();
        path.0.push(PathElement::Field(name.into()));
        path
    }
    pub fn index(&self, index: usize) -> Self {
        let mut path = self.clone();
        path.0.push(PathElement::Index(index));
        path
    }
}
/// Products can preserve a physical input type only for `Input`. A fixed
/// result (count, average, a bucket label, etc.) uses the declared output kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRule {
    Input,
    Fixed,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemporalOutput {
    Match,
    Offset,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Catalog {
        unit: String,
        field: String,
    },
    Output {
        source: OutputSource,
        field: String,
    },
    /// The exact product-declared projection selected by shared binding.
    Matches {
        source: crate::catalog::MatchSource,
    },
    Group {
        operand: NodePath,
        input: Box<FieldInfo>,
    },
    Function {
        name: String,
        arguments: Vec<FieldInfo>,
        rule: TypeRule,
    },
    Aggregate {
        at: NodePath,
        function: AggregateFunction,
        inputs: Vec<FieldInfo>,
        rule: TypeRule,
    },
    Temporal {
        query: NodePath,
        relation: TemporalRelation,
        output: TemporalOutput,
        all: bool,
    },
    Generated,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub field: Field,
    pub origin: Origin,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Actual underlying row domain, not a saved name or a grouped alias.
    pub unit: Option<String>,
    pub fields: Arc<[FieldInfo]>,
}
impl Scope {
    pub fn field(&self, name: &str) -> Option<&FieldInfo> {
        self.fields.iter().find(|f| f.field.name == name)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageScope {
    pub before: Scope,
    pub after: Scope,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryScope {
    pub path: NodePath,
    pub source: Scope,
    pub stages: Vec<StageScope>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedOperand {
    pub path: NodePath,
    pub unit: Option<String>,
    pub result: FieldInfo,
}
/// Associated only with the tree returned alongside it by shared binding.
/// Cloning this snapshot does not validate a modified syntax tree; bind again
/// after transformations or catalog/schema changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Typing {
    pub queries: Vec<QueryScope>,
    pub operands: Vec<TypedOperand>,
}
impl Typing {
    pub fn query(&self, path: &NodePath) -> Option<&QueryScope> {
        self.queries.iter().find(|query| &query.path == path)
    }
    pub fn operand(&self, path: &NodePath) -> Option<&TypedOperand> {
        self.operands.iter().find(|operand| &operand.path == path)
    }
}

/// One aggregate type rule shared by parsing and identity normalization.
pub(crate) fn aggregate_kind(
    function: &AggregateFunction,
    input: Option<&crate::catalog::FieldKind>,
) -> (crate::catalog::FieldKind, TypeRule) {
    use crate::catalog::FieldKind;
    match function {
        AggregateFunction::Count | AggregateFunction::CountDistinct => {
            (FieldKind::Integer, TypeRule::Fixed)
        }
        AggregateFunction::Avg => (
            match input {
                Some(FieldKind::Money) => FieldKind::Money,
                Some(FieldKind::Duration) => FieldKind::Duration,
                _ => FieldKind::Number,
            },
            TypeRule::Fixed,
        ),
        AggregateFunction::Correlation => (FieldKind::Number, TypeRule::Fixed),
        AggregateFunction::Histogram => (FieldKind::Json, TypeRule::Fixed),
        _ => (input.cloned().unwrap_or(FieldKind::Number), TypeRule::Input),
    }
}
pub(crate) fn function_kind(name: &str) -> crate::catalog::FieldKind {
    use crate::catalog::FieldKind;
    match name {
        "day" | "week" | "month" => FieldKind::Time,
        "hour" => FieldKind::Integer,
        "weekday" | "month_day" | "program" | "fingerprint" => FieldKind::Text,
        "random" => FieldKind::Number,
        _ => FieldKind::Json,
    }
}
