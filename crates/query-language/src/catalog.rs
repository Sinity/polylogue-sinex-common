//! Product-owned vocabulary and structural extension declarations.
use crate::ast::{Argument, ResultPart, Source, Value};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Catalog {
    pub default_unit: String,
    pub recall_last_unit: Option<String>,
    pub units: Vec<Unit>,
    pub commands: Vec<String>,
    pub extensions: Vec<Extension>,
    pub references: Vec<ReferenceSyntax>,
    /// Immutable product-derived outputs of expanded saved questions and kept results.
    #[serde(default)]
    pub source_outputs: Vec<SourceOutput>,
    /// Both products accept pipes; products may additionally expose clauses without pipes.
    pub unpiped_stages: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputSource {
    Saved {
        name: String,
        arguments: Vec<Argument>,
    },
    Result {
        reference: String,
        part: ResultPart,
    },
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutputSchema {
    /// Actual row domain supplied by the product, used for nested implicit sources.
    pub unit: Option<String>,
    pub fields: Vec<Field>,
    pub main_time: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SourceOutput {
    pub source: OutputSource,
    pub schema: OutputSchema,
}
impl Catalog {
    pub fn source_output(&self, source: &Source) -> Option<&OutputSchema> {
        self.source_outputs.iter().find_map(|output| {
            let matches = match (&output.source, source) {
                (
                    OutputSource::Saved { name, arguments },
                    Source::Saved {
                        name: other,
                        arguments: actual,
                        ..
                    },
                ) => name == other && arguments_equal(arguments, actual),
                (
                    OutputSource::Result { reference, part },
                    Source::Result {
                        reference: other,
                        part: actual,
                        ..
                    },
                ) => reference == other && part == actual,
                _ => false,
            };
            matches.then_some(&output.schema)
        })
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Unit {
    pub name: String,
    /// The row domain of sequence steps; None means this unit itself.
    pub sequence_unit: Option<String>,
    pub aliases: Vec<String>,
    pub fields: Vec<Field>,
    pub defaults: Vec<DefaultFilter>,
    pub default_order: Vec<String>,
    pub identity: Vec<String>,
    pub main_time: Option<String>,
    pub help: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Field {
    pub name: String,
    pub kind: FieldKind,
    pub values: Vec<String>,
    pub unknown: UnknownRule,
    pub help: String,
    pub read_as: String,
    pub unit: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    #[default]
    Text,
    Match,
    Substring,
    Words,
    Subtree,
    Integer,
    Number,
    Money,
    Time,
    Duration,
    Bool,
    Reference,
    Json,
    List(Box<FieldKind>),
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnknownRule {
    #[default]
    Unknown,
    Never,
    Word(String),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DefaultFilter {
    pub field: String,
    pub value: Value,
}
/// An extension declares its syntax as typed slots; no product parses an opaque remainder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Extension {
    pub name: String,
    pub position: ExtensionPosition,
    pub arguments: Vec<ExtensionSlot>,
    pub help: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionPosition {
    Atom,
    Stage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionSlot {
    Keyword(String),
    Value,
    Name,
    Optional(Vec<ExtensionSlot>),
    Field,
    Unit,
    Condition,
    Query,
}
impl Catalog {
    pub fn new(default_unit: impl Into<String>) -> Self {
        Self {
            default_unit: default_unit.into(),
            ..Self::default()
        }
    }
    pub fn unit(&self, name: &str) -> Option<&Unit> {
        self.units.iter().find(|u| {
            u.name.eq_ignore_ascii_case(name)
                || u.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
        })
    }
    pub fn field(&self, unit: &str, name: &str) -> Option<&Field> {
        self.unit(unit)?.fields.iter().find(|f| f.name == name)
    }
    pub fn units_accepting(&self, value: &str) -> Vec<String> {
        self.units
            .iter()
            .flat_map(|u| {
                u.fields
                    .iter()
                    .filter(move |f| f.values.iter().any(|v| v.eq_ignore_ascii_case(value)))
                    .map(move |f| format!("{}.{}", u.name, f.name))
            })
            .collect()
    }
    pub fn command_words(&self) -> impl Iterator<Item = &str> {
        self.commands
            .iter()
            .map(String::as_str)
            .chain(self.units.iter().flat_map(|u| {
                std::iter::once(u.name.as_str()).chain(u.aliases.iter().map(String::as_str))
            }))
    }
}

/// A product reference is one literal node. A required marker disambiguates
/// reference forms from a field with the same prefix (for example tool:Bash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ReferenceSyntax {
    pub prefix: String,
    pub marker: Option<String>,
}
impl Catalog {
    pub fn is_reference(&self, text: &str) -> bool {
        self.references.iter().any(|form| {
            text.strip_prefix(&form.prefix).is_some_and(|rest| {
                !rest.is_empty()
                    && form
                        .marker
                        .as_ref()
                        .is_none_or(|marker| rest.contains(marker))
            })
        })
    }
}

/// Shared stage vocabulary for parsing, completion and product discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StageSyntax {
    pub name: &'static str,
    pub completion: &'static str,
    pub form: StageForm,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageForm {
    Plain,
    Function,
    Relation,
    Count,
}
pub const STAGES: &[StageSyntax] = &[
    StageSyntax {
        name: "where",
        completion: "where",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "since",
        completion: "since",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "until",
        completion: "until",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "sort",
        completion: "sort by",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "group",
        completion: "group by",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "agg",
        completion: "agg",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "count",
        completion: "count",
        form: StageForm::Count,
    },
    StageSyntax {
        name: "sum",
        completion: "sum",
        form: StageForm::Function,
    },
    StageSyntax {
        name: "avg",
        completion: "avg",
        form: StageForm::Function,
    },
    StageSyntax {
        name: "min",
        completion: "min",
        form: StageForm::Function,
    },
    StageSyntax {
        name: "max",
        completion: "max",
        form: StageForm::Function,
    },
    StageSyntax {
        name: "count_distinct",
        completion: "count_distinct",
        form: StageForm::Function,
    },
    StageSyntax {
        name: "histogram",
        completion: "histogram",
        form: StageForm::Function,
    },
    StageSyntax {
        name: "corr",
        completion: "corr",
        form: StageForm::Function,
    },
    StageSyntax {
        name: "limit",
        completion: "limit",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "offset",
        completion: "offset",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "first",
        completion: "first",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "distinct",
        completion: "distinct",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "matches",
        completion: "matches",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "fields",
        completion: "fields",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "fill",
        completion: "fill",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "measure",
        completion: "measure",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "as",
        completion: "as of",
        form: StageForm::Plain,
    },
    StageSyntax {
        name: "union",
        completion: "union",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "intersect",
        completion: "intersect",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "except",
        completion: "except",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "fuse",
        completion: "fuse",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "near",
        completion: "near",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "before",
        completion: "before",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "after",
        completion: "after",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "during",
        completion: "during",
        form: StageForm::Relation,
    },
    StageSyntax {
        name: "containing",
        completion: "containing",
        form: StageForm::Relation,
    },
];
pub const AGGREGATES: &[&str] = &[
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "count_distinct",
    "histogram",
    "corr",
];
pub const KEY_FUNCTIONS: &[&str] = &[
    "day",
    "hour",
    "week",
    "weekday",
    "month",
    "month_day",
    "program",
    "fingerprint",
];

fn arguments_equal(a: &[Argument], b: &[Argument]) -> bool {
    a.len() == b.len()
        && a.iter().all(|arg| {
            b.iter().any(|other| {
                arg.parameter == other.parameter && values_equal(&arg.value, &other.value)
            })
        })
}
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Scalar { text: a, .. }, Value::Scalar { text: b, .. }) => a == b,
        (Value::Set { values: a }, Value::Set { values: b }) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| values_equal(a, b))
        }
        _ => a == b,
    }
}

pub(crate) fn same_output_source(a: &OutputSource, b: &OutputSource) -> bool {
    match (a, b) {
        (
            OutputSource::Saved {
                name: a,
                arguments: av,
            },
            OutputSource::Saved {
                name: b,
                arguments: bv,
            },
        ) => a == b && arguments_equal(av, bv),
        (
            OutputSource::Result {
                reference: a,
                part: ap,
            },
            OutputSource::Result {
                reference: b,
                part: bp,
            },
        ) => a == b && ap == bp,
        _ => false,
    }
}
