//! Owned syntax, independent of a product's storage and clocks.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Query {
    pub source: Source,
    pub attach: Vec<Attachment>,
    pub stages: Vec<Stage>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Unit {
        unit: String,
        condition: Option<Condition>,
    },
    Saved {
        name: String,
        arguments: Vec<Argument>,
        output: OutputBinding,
    },
    Result {
        reference: String,
        part: ResultPart,
        output: OutputBinding,
    },
    Walk {
        direction: WalkDirection,
        reference: String,
    },
    Recall {
        time: Option<Value>,
        window: Option<Value>,
    },
    RecallLast {
        unit: String,
        condition: Option<Condition>,
    },
    RecallMark {
        number: u64,
        window: Option<Value>,
    },
}
/// Deferred output scopes are syntax only and must pass shared binding before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputBinding {
    Deferred,
    Known,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultPart {
    Rows,
    Matches,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WalkDirection {
    Trace,
    Descendants,
    Parents,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub span: Span,
    pub kind: ConditionKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConditionKind {
    And {
        conditions: Vec<Condition>,
    },
    Or {
        conditions: Vec<Condition>,
    },
    Not {
        condition: Box<Condition>,
    },
    Match {
        field: String,
        value: Value,
    },
    Compare {
        operand: Operand,
        op: Op,
        value: Value,
    },
    /// Main-time lower bound; span-valued rows are selected by overlap.
    Since {
        operand: Operand,
        value: Value,
    },
    /// Main-time exclusive upper bound. A date resolves to its logical-day
    /// end, while an instant remains that instant.
    Until {
        operand: Operand,
        value: Value,
    },
    Between {
        operand: Operand,
        from: Value,
        to: Value,
    },
    Text {
        text: String,
        mode: TextMode,
    },
    Reference {
        reference: String,
    },
    Similar {
        value: Value,
    },
    Exists {
        unit: String,
        condition: Option<Box<Condition>>,
    },
    FieldExists {
        operand: Operand,
    },
    Count {
        unit: String,
        condition: Option<Box<Condition>>,
        op: Op,
        count: u64,
    },
    In {
        operand: Operand,
        query: Box<Query>,
    },
    TimeRange {
        operand: Operand,
        value: Value,
    },
    Overlaps {
        from: Value,
        to: Option<Value>,
    },
    TimeDifference {
        left: Operand,
        right: Operand,
        op: Op,
        value: Value,
    },
    Saved {
        name: String,
        arguments: Vec<Argument>,
    },
    Sequence {
        unit: String,
        all: bool,
        steps: Vec<Condition>,
        gaps: Vec<Gap>,
    },
    Extension {
        name: String,
        arguments: Vec<ExtensionArgument>,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    Word,
    Phrase,
    Prefix,
    Approximate,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Value {
    Scalar { text: String, quoted: bool },
    Set { values: Vec<Value> },
    Any,
    Unknown,
    Parameter { name: String },
}
impl Value {
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Scalar { text, .. } => Some(text),
            Self::Parameter { name } => Some(name),
            _ => None,
        }
    }
    pub fn scalar(text: impl Into<String>) -> Self {
        Self::Scalar {
            text: text.into(),
            quoted: false,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Argument {
    pub parameter: String,
    pub value: Value,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Operand {
    Field {
        name: String,
    },
    Function {
        name: String,
        arguments: Vec<Operand>,
    },
}
impl Operand {
    pub fn field(name: impl Into<String>) -> Self {
        Self::Field { name: name.into() }
    }
    pub fn column(&self) -> String {
        match self {
            Self::Field { name } => name.replace(['.', '-'], "_"),
            Self::Function { name, .. } => name.clone(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    Prefix,
}
impl Op {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Contains => "~",
            Self::Prefix => "^",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "=" => Self::Eq,
            "!=" => Self::Ne,
            "<" => Self::Lt,
            "<=" => Self::Le,
            ">" => Self::Gt,
            ">=" => Self::Ge,
            "~" => Self::Contains,
            "^" => Self::Prefix,
            _ => return None,
        })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gap {
    Later,
    Next,
    Within { duration: Value },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attachment {
    pub span: Span,
    pub unit: String,
    pub fields: Vec<String>,
    pub windows: Vec<Window>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    First(u64),
    Last(u64),
    Where(Condition),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stage {
    pub span: Span,
    pub kind: StageKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StageKind {
    Where {
        condition: Condition,
    },
    Sort {
        keys: Vec<SortKey>,
    },
    Group {
        keys: Vec<Operand>,
    },
    Aggregate {
        aggregates: Vec<Aggregate>,
    },
    Limit {
        count: u64,
    },
    Offset {
        count: u64,
    },
    First {
        count: u64,
    },
    Distinct,
    Matches,
    Fill,
    Fields {
        fields: Vec<String>,
    },
    Scope {
        unit: String,
        condition: Option<Condition>,
    },
    Combine {
        how: Combination,
        query: Box<Query>,
    },
    Temporal {
        relation: TemporalRelation,
        query: Box<Query>,
        at: TimePoint,
        within: Option<Value>,
        all: bool,
    },
    AsOf {
        value: Value,
        sequence: bool,
    },
    Measure {
        name: String,
        by: Vec<Operand>,
    },
    Extension {
        name: String,
        arguments: Vec<ExtensionArgument>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortKey {
    pub operand: Operand,
    pub descending: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Combination {
    Union,
    Intersect,
    Except,
    Fuse,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalRelation {
    Near,
    Before,
    After,
    During,
    Containing,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimePoint {
    When,
    Start,
    End,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aggregate {
    pub function: AggregateFunction,
    pub fields: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AggregateFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Percentile(u8),
    CountDistinct,
    Histogram,
    Correlation,
}
impl Aggregate {
    pub fn column(&self) -> String {
        let name = match self.function {
            AggregateFunction::Count => "count".into(),
            AggregateFunction::Sum => "sum".into(),
            AggregateFunction::Avg => "avg".into(),
            AggregateFunction::Min => "min".into(),
            AggregateFunction::Max => "max".into(),
            AggregateFunction::Percentile(p) => format!("p{p}"),
            AggregateFunction::CountDistinct => "count_distinct".into(),
            AggregateFunction::Histogram => "histogram".into(),
            AggregateFunction::Correlation => "corr".into(),
        };
        if self.fields.is_empty() {
            name
        } else {
            format!("{name}_{}", self.fields.join("_").replace(['.', '-'], "_"))
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtensionArgument {
    Value(Value),
    Name(String),
    Optional(Vec<ExtensionArgument>),
    Field(String),
    Unit(String),
    Condition(Box<Condition>),
    Query(Box<Query>),
    Keyword(String),
}
impl Query {
    /// Namespaces come exclusively from unit/operator nodes, never literals.
    pub fn namespaces(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        self.visit_units(&mut |unit| {
            if let Some((ns, _)) = unit.split_once('.') {
                out.insert(ns.to_string());
            }
        });
        out
    }
    pub fn visit_units(&self, visitor: &mut impl FnMut(&str)) {
        match &self.source {
            Source::Unit { unit, condition } => {
                visitor(unit);
                if let Some(c) = condition {
                    c.visit_units(visitor);
                }
            }
            Source::RecallLast { unit, condition } => {
                visitor(unit);
                if let Some(c) = condition {
                    c.visit_units(visitor);
                }
            }
            _ => {}
        }
        for a in &self.attach {
            visitor(&a.unit);
            for w in &a.windows {
                if let Window::Where(c) = w {
                    c.visit_units(visitor);
                }
            }
        }
        for s in &self.stages {
            match &s.kind {
                StageKind::Where { condition } => condition.visit_units(visitor),
                StageKind::Scope { unit, condition } => {
                    visitor(unit);
                    if let Some(c) = condition {
                        c.visit_units(visitor);
                    }
                }
                StageKind::Combine { query, .. } | StageKind::Temporal { query, .. } => {
                    query.visit_units(visitor)
                }
                StageKind::Extension { arguments, .. } => visit_arguments(arguments, visitor),
                _ => {}
            }
        }
    }
}
impl Condition {
    pub fn visit_units(&self, visitor: &mut impl FnMut(&str)) {
        match &self.kind {
            ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
                for c in conditions {
                    c.visit_units(visitor)
                }
            }
            ConditionKind::Not { condition } => condition.visit_units(visitor),
            ConditionKind::Exists { unit, condition }
            | ConditionKind::Count {
                unit, condition, ..
            } => {
                visitor(unit);
                if let Some(c) = condition {
                    c.visit_units(visitor);
                }
            }
            ConditionKind::In { query, .. } => query.visit_units(visitor),
            ConditionKind::Sequence { unit, steps, .. } => {
                visitor(unit);
                for c in steps {
                    c.visit_units(visitor)
                }
            }
            ConditionKind::Extension { arguments, .. } => visit_arguments(arguments, visitor),
            // Bounds contain values and an already-bound field, never units.
            ConditionKind::Since { .. } | ConditionKind::Until { .. } => {}
            _ => {}
        }
    }
}
fn visit_arguments(arguments: &[ExtensionArgument], visitor: &mut impl FnMut(&str)) {
    for a in arguments {
        match a {
            ExtensionArgument::Unit(u) => visitor(u),
            ExtensionArgument::Query(q) => q.visit_units(visitor),
            ExtensionArgument::Condition(c) => c.visit_units(visitor),
            ExtensionArgument::Optional(arguments) => visit_arguments(arguments, visitor),
            _ => {}
        }
    }
}
