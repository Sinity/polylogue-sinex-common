//! Product-owned vocabulary and structural extension declarations.
use crate::ast::{Argument, ResultPart, Source, Value};
use serde::{Deserialize, Serialize};

/// One product's units, functions, extensions and reference forms. The
/// grammar is the same for every catalog; only these declarations differ.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Catalog {
    pub default_unit: String,
    /// This product's explicit unit namespace, resolved to canonical local names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    pub units: Vec<Unit>,
    /// Functions the product adds beside the built-in ones (`FUNCTIONS`).
    #[serde(default)]
    pub functions: Vec<Function>,
    pub extensions: Vec<Extension>,
    pub references: Vec<ReferenceForm>,
    /// Immutable product-derived outputs of expanded saved questions and kept results.
    #[serde(default)]
    pub source_outputs: Vec<SourceOutput>,
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
    /// Top-level exists/sequence step units available through a transparent
    /// saved definition. None means the pattern provenance is not supplied.
    /// Kept results remain opaque and use their retained matches part instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub match_units: Option<Vec<String>>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SourceOutput {
    pub source: OutputSource,
    /// The columns the saved question or kept result produces.
    pub schema: OutputSchema,
}
impl Catalog {
    pub fn new(default_unit: impl Into<String>) -> Self {
        Self {
            default_unit: default_unit.into(),
            ..Self::default()
        }
    }
    pub fn unit(&self, name: &str) -> Option<&Unit> {
        let name = name
            .split_once('.')
            .filter(|(prefix, _)| {
                self.namespace
                    .as_ref()
                    .is_some_and(|namespace| prefix.eq_ignore_ascii_case(namespace))
            })
            .map_or(name, |(_, local)| local);
        self.units.iter().find(|unit| {
            unit.name.eq_ignore_ascii_case(name)
                || unit
                    .singular
                    .as_ref()
                    .is_some_and(|singular| singular.eq_ignore_ascii_case(name))
        })
    }
    pub fn field(&self, unit: &str, name: &str) -> Option<&Field> {
        self.unit(unit)?.fields.iter().find(|f| f.name == name)
    }
    /// The output schema a saved question or kept result source reads.
    pub fn source_output(&self, source: &Source) -> Option<&OutputSchema> {
        self.source_outputs
            .iter()
            .find(|output| match (&output.source, source) {
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
            })
            .map(|output| &output.schema)
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
    /// A built-in function or one this catalog declares, looked up without
    /// regard to case. Two declarations that differ only in case are ambiguous.
    pub fn function(&self, name: &str) -> Result<Option<Function>, crate::Error> {
        let mut found = builtin_functions()
            .into_iter()
            .chain(self.functions.iter().cloned())
            .filter(|function| function.name.eq_ignore_ascii_case(name));
        let first = found.next();
        if found.next().is_some() {
            return Err(crate::Error::new(
                crate::ErrorKind::Ambiguous,
                format!("ambiguous function declaration `{name}`"),
                0,
            ));
        }
        Ok(first)
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Unit {
    pub name: String,
    /// The unit's declared singular form; it binds to the canonical name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub singular: Option<String>,
    /// The row domain of `seq` steps written on this unit; None means this unit.
    pub sequence_unit: Option<String>,
    /// The order its rows have steps in (for example a session's timeline).
    /// Units naming the same scope interleave in one order; None means the
    /// unit's rows have no steps and only time distances apply.
    #[serde(default)]
    pub order_scope: Option<String>,
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
    /// Short text; its natural match is a case-insensitive substring.
    #[default]
    Text,
    /// Exact text: an identifier or closed word, whose natural match is
    /// equality.
    Exact,
    Substring,
    /// Prose; its natural match is word search.
    Words,
    /// A path; its natural match is the whole-component subtree.
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
/// What `field:value` means for a field, declared once by its kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NaturalMatch {
    /// Closed sets, identifiers, booleans and numbers: equality.
    Equal,
    /// Short text: case-insensitive substring.
    Contains,
    /// Prose: word search.
    Words,
    /// Paths: the value itself or anything under it, by whole components.
    Subtree,
    /// Times: inside the named period.
    Period,
    /// References: the same object.
    Same,
}
impl Field {
    pub fn natural_match(&self) -> NaturalMatch {
        if !self.values.is_empty() {
            return NaturalMatch::Equal;
        }
        natural_match(&self.kind)
    }
}
fn natural_match(kind: &FieldKind) -> NaturalMatch {
    match kind {
        FieldKind::Text | FieldKind::Substring => NaturalMatch::Contains,
        FieldKind::Words => NaturalMatch::Words,
        FieldKind::Subtree => NaturalMatch::Subtree,
        FieldKind::Time => NaturalMatch::Period,
        FieldKind::Reference => NaturalMatch::Same,
        FieldKind::List(kind) => natural_match(kind),
        FieldKind::Exact
        | FieldKind::Integer
        | FieldKind::Number
        | FieldKind::Money
        | FieldKind::Duration
        | FieldKind::Bool
        | FieldKind::Json => NaturalMatch::Equal,
    }
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
/// A function usable in expressions: a built-in time bucket or `len`, or one
/// a product declares (Polylogue's `program(command)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Function {
    pub name: String,
    pub arguments: usize,
    pub result: FieldKind,
    pub help: String,
}
/// Built-in functions: time buckets on the logical day, and `len`.
pub const FUNCTIONS: &[&str] = &[
    "hour",
    "day",
    "week",
    "weekday",
    "month",
    "month_day",
    "year",
    "len",
];
/// Bucket functions whose groups `fill` can complete.
pub const TIME_BUCKETS: &[&str] = &["hour", "day", "week", "month", "year"];
/// The prefix of an order relation's or join's inner-row fields (`match.F`).
pub const MATCH_PREFIX: &str = "match.";
/// The signed distance from an outer row to its matched row, in the
/// distance's unit (time or steps).
pub const MATCH_DISTANCE: &str = "match.distance";
/// Fields the language adds beside a unit's stored fields: the order
/// relation's distance, `diff`'s change and ranks, and `fill`'s coverage.
/// A generated field never hides a stored one; `Catalog::hidden_fields`
/// finds a catalog that would.
pub const GENERATED_FIELDS: &[&str] = &[
    MATCH_DISTANCE,
    "change",
    "old_rank",
    "new_rank",
    crate::coverage::STATE_FIELD,
    crate::coverage::DETAIL_FIELD,
];
impl Catalog {
    /// Stored fields a generated field would hide, as `unit.field`: a field
    /// named like a generated one, or one whose `match.` form is one.
    pub fn hidden_fields(&self) -> Vec<String> {
        let mut hidden = Vec::new();
        for unit in &self.units {
            for field in &unit.fields {
                let matched = format!("{MATCH_PREFIX}{}", field.name);
                if GENERATED_FIELDS
                    .iter()
                    .any(|name| *name == field.name || *name == matched)
                {
                    hidden.push(format!("{}.{}", unit.name, field.name));
                }
            }
        }
        hidden
    }
}
fn builtin_functions() -> Vec<Function> {
    FUNCTIONS
        .iter()
        .map(|name| Function {
            name: (*name).into(),
            arguments: 1,
            result: match *name {
                "day" | "week" | "month" => FieldKind::Time,
                "hour" | "year" | "len" => FieldKind::Integer,
                _ => FieldKind::Text,
            },
            help: String::new(),
        })
        .collect()
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
    Transform,
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
    Expression,
    /// One or more of the slot, separated by commas.
    List(Box<ExtensionSlot>),
}

/// A reference form a product declares. A reference is one literal node and,
/// used as a condition, means "this row". The language's own forms
/// (`query:`, `result:`, `answer:`, `@N`, `@last`) need no declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReferenceForm {
    /// `<prefix><id>`; a required marker disambiguates a reference from a
    /// field with the same prefix (`tool:Bash` is a field, `tool:x#y` a call).
    Prefix {
        prefix: String,
        marker: Option<String>,
    },
    /// A whole word that names one row (`self`, `last`, `here`).
    Word { word: String },
    /// A bare native identifier: hexadecimal digits and dashes, at least this long.
    Identifier { min_length: usize },
}
impl ReferenceForm {
    pub fn prefix(prefix: impl Into<String>) -> Self {
        Self::Prefix {
            prefix: prefix.into(),
            marker: None,
        }
    }
    pub fn word(word: impl Into<String>) -> Self {
        Self::Word { word: word.into() }
    }
    fn matches(&self, text: &str) -> bool {
        match self {
            Self::Prefix { prefix, marker } => {
                text.strip_prefix(prefix.as_str()).is_some_and(|rest| {
                    !rest.is_empty()
                        && marker
                            .as_ref()
                            .is_none_or(|marker| rest.contains(marker.as_str()))
                })
            }
            Self::Word { word } => text == word,
            Self::Identifier { min_length } => {
                text.len() >= *min_length
                    && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
                    && text.chars().any(|c| c.is_ascii_hexdigit())
            }
        }
    }
}
impl Catalog {
    pub fn is_reference(&self, text: &str) -> bool {
        is_language_reference(text) || self.references.iter().any(|form| form.matches(text))
    }
    pub(crate) fn reference_overrides_field(&self, text: &str) -> bool {
        is_language_reference(text)
            || self.references.iter().any(|form| {
                matches!(
                    form,
                    ReferenceForm::Prefix {
                        marker: Some(_),
                        ..
                    } | ReferenceForm::Word { .. }
                ) && form.matches(text)
            })
    }
}
/// The answer-relative references every product reads the same way: an
/// answer object, the latest answer, and a row of the latest answer.
pub fn is_language_reference(text: &str) -> bool {
    text.strip_prefix("answer:")
        .is_some_and(|id| !id.is_empty())
        || text == "@last"
        || text
            .strip_prefix('@')
            .is_some_and(|row| !row.is_empty() && row.chars().all(|c| c.is_ascii_digit()))
}

/// Shared transform vocabulary for parsing, completion and product discovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransformSyntax {
    pub name: &'static str,
    pub completion: &'static str,
}
pub const TRANSFORMS: &[TransformSyntax] = &[
    TransformSyntax {
        name: "where",
        completion: "where",
    },
    TransformSyntax {
        name: "sort",
        completion: "sort by",
    },
    TransformSyntax {
        name: "group",
        completion: "group by",
    },
    TransformSyntax {
        name: "agg",
        completion: "agg",
    },
    TransformSyntax {
        name: "count",
        completion: "count",
    },
    TransformSyntax {
        name: "limit",
        completion: "limit",
    },
    TransformSyntax {
        name: "offset",
        completion: "offset",
    },
    TransformSyntax {
        name: "first",
        completion: "first",
    },
    TransformSyntax {
        name: "distinct",
        completion: "distinct",
    },
    TransformSyntax {
        name: "matches",
        completion: "matches",
    },
    TransformSyntax {
        name: "fields",
        completion: "fields",
    },
    TransformSyntax {
        name: "fill",
        completion: "fill",
    },
    TransformSyntax {
        name: "as",
        completion: "as of",
    },
    TransformSyntax {
        name: "union",
        completion: "union",
    },
    TransformSyntax {
        name: "intersect",
        completion: "intersect",
    },
    TransformSyntax {
        name: "except",
        completion: "except",
    },
    TransformSyntax {
        name: "fuse",
        completion: "fuse",
    },
    TransformSyntax {
        name: "join",
        completion: "join",
    },
    TransformSyntax {
        name: "diff",
        completion: "diff",
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
    "covered",
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
