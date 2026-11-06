//! Owned syntax, independent of a product's storage and clocks.
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}
/// A relation, its attached child lists and the transforms after each `|`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Query {
    pub source: Source,
    pub attach: Vec<Attachment>,
    pub transforms: Vec<Transform>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    Unit {
        unit: String,
        /// True when the unit was omitted and inherits its enclosing row scope.
        implicit: bool,
        condition: Option<Condition>,
    },
    Saved {
        name: String,
        arguments: Vec<Argument>,
        condition: Option<Condition>,
        output: SourceColumns,
    },
    Result {
        reference: String,
        part: ResultPart,
        condition: Option<Condition>,
        output: SourceColumns,
    },
}
/// Whether a saved query's or kept result's columns were known when the
/// text was read. A source whose columns are unknown is syntax only: binding
/// against the catalog's declared outputs must resolve it before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceColumns {
    Unknown,
    Known,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResultPart {
    Rows,
    Matches,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Condition {
    pub span: Span,
    pub kind: ConditionKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
    /// `field:value`, the field's natural match (see `catalog::NaturalMatch`).
    Match {
        field: String,
        value: Value,
    },
    /// `left OP right`: exact comparison, `~` contains, `^` starts with or lies under.
    Compare {
        left: Expression,
        op: Op,
        right: Expression,
    },
    /// Main-time lower bound; span-valued rows are selected by overlap.
    Since {
        field: String,
        value: Value,
    },
    /// Main-time exclusive upper bound. A date resolves to its logical-day
    /// end, while an instant remains that instant.
    Until {
        field: String,
        value: Value,
    },
    Between {
        left: Expression,
        from: Value,
        to: Value,
    },
    /// `field in RANGE`: a time inside a named range (`when in last week`).
    TimeRange {
        left: Expression,
        value: Value,
    },
    /// `field in (QUERY)`: membership in the inner query's value set.
    In {
        left: Expression,
        query: Box<Query>,
    },
    Text {
        text: String,
        mode: TextMode,
    },
    Reference {
        reference: String,
    },
    /// A ranking atom. It orders rows and never changes which rows are selected.
    Rank {
        list: RankList,
        value: Value,
    },
    Exists {
        unit: String,
        condition: Option<Box<Condition>>,
    },
    Count {
        unit: String,
        condition: Option<Box<Condition>>,
        op: Op,
        count: u64,
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
    /// An order relation used as a condition: a semijoin that keeps an outer
    /// row when at least one inner row stands in the relation.
    Order {
        relation: OrderRelation,
        query: Box<Query>,
        within: Option<Distance>,
        at: TimePoint,
        scope: Option<String>,
        #[serde(default, skip_serializing_if = "WitnessChoice::is_nearest")]
        pick: WitnessChoice,
    },
    Extension {
        name: String,
        arguments: Vec<ExtensionArgument>,
    },
}
/// How an order semijoin chooses the one exposed matching row.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WitnessChoice {
    #[default]
    Nearest,
    FirstBy {
        keys: Vec<SortKey>,
    },
}
impl WitnessChoice {
    pub fn is_nearest(&self) -> bool {
        matches!(self, Self::Nearest)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    Word,
    Phrase,
    Prefix,
    Approximate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RankList {
    /// `similar:` ranks by vector similarity to a text, a reference or rows.
    Similar,
    /// `related:` ranks by the product's derived relatedness to a reference.
    Related,
    /// `around:` ranks by closeness in time to an instant or period.
    Around,
}
impl RankList {
    pub const ALL: [Self; 3] = [Self::Similar, Self::Related, Self::Around];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Similar => "similar",
            Self::Related => "related",
            Self::Around => "around",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Value {
    Scalar {
        text: String,
        quoted: bool,
    },
    Set {
        values: Vec<Value>,
    },
    /// `*`: names the field without restricting it and drops its default.
    Any,
    /// `unknown`: true when the value is unknown.
    Unknown,
    /// `known`: true when the value is known.
    Known,
    Parameter {
        name: String,
    },
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Argument {
    pub parameter: String,
    pub value: Value,
}
/// A typed value computed from a row: a field, a literal, a function call or
/// arithmetic. Dimensions are checked when the query is parsed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Expression {
    Field {
        name: String,
    },
    Value {
        value: Value,
    },
    Function {
        name: String,
        arguments: Vec<Expression>,
    },
    Arithmetic {
        operator: Arithmetic,
        left: Box<Expression>,
        right: Box<Expression>,
    },
}
impl Expression {
    pub fn field(name: impl Into<String>) -> Self {
        Self::Field { name: name.into() }
    }
    pub fn value(value: Value) -> Self {
        Self::Value { value }
    }
    /// The column name this expression produces when no `as` name is given.
    pub fn column(&self) -> String {
        match self {
            Self::Field { name } => name.replace(['.', '-'], "_"),
            Self::Value { value } => value
                .text()
                .unwrap_or("value")
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .collect(),
            Self::Function { name, .. } => name.clone(),
            Self::Arithmetic {
                operator,
                left,
                right,
            } => format!("{}_{}_{}", left.column(), operator.word(), right.column()),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Arithmetic {
    Add,
    Subtract,
    Multiply,
    Divide,
}
impl Arithmetic {
    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Add => "+",
            Self::Subtract => "-",
            Self::Multiply => "*",
            Self::Divide => "/",
        }
    }
    pub const fn word(self) -> &'static str {
        match self {
            Self::Add => "plus",
            Self::Subtract => "minus",
            Self::Multiply => "times",
            Self::Divide => "over",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "+" => Self::Add,
            "-" => Self::Subtract,
            "*" => Self::Multiply,
            "/" => Self::Divide,
            _ => return None,
        })
    }
    /// Multiplication and division bind tighter than addition and subtraction.
    pub const fn binds_tightly(self) -> bool {
        matches!(self, Self::Multiply | Self::Divide)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Gap {
    Later,
    Next,
    Within { distance: Distance },
}
/// How far apart two rows may be: a duration, or a number of positions in
/// the order scope (`1 step` is adjacent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Distance {
    Time { duration: Value },
    Steps { count: u64 },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Attachment {
    pub span: Span,
    pub unit: String,
    pub fields: Vec<String>,
    pub windows: Vec<Window>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    First(u64),
    Last(u64),
    Where(Condition),
}
/// One step after `|`: a relation in, a relation out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Transform {
    pub span: Span,
    pub kind: TransformKind,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransformKind {
    Where {
        condition: Condition,
    },
    Sort {
        keys: Vec<SortKey>,
    },
    Group {
        keys: Vec<Column>,
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
    /// One row per step of each `exists` or `seq` match (see the README).
    Matches,
    /// Every bucket of a time-bucketed group, each with its coverage state.
    Fill,
    Fields {
        columns: Vec<Column>,
    },
    Scope {
        unit: String,
        condition: Option<Condition>,
    },
    Combine {
        how: Combination,
        query: Box<Query>,
    },
    /// The pair relation: one row per matching (outer, inner) pair.
    Join {
        query: Box<Query>,
        on: JoinOn,
    },
    /// Rows that joined, left or moved in rank between the inner relation
    /// (before) and this one (after).
    Diff {
        query: Box<Query>,
    },
    AsOf {
        value: Value,
        sequence: bool,
    },
    Extension {
        name: String,
        arguments: Vec<ExtensionArgument>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SortKey {
    pub expression: Expression,
    pub descending: bool,
}
/// A projected or grouped column: an expression and an optional `as` name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Column {
    pub expression: Expression,
    pub name: Option<String>,
}
impl Column {
    pub fn field(name: impl Into<String>) -> Self {
        Self {
            expression: Expression::field(name),
            name: None,
        }
    }
    /// The `as` name, else a field's own name, else the expression's column.
    pub fn column(&self) -> String {
        match (&self.name, &self.expression) {
            (Some(name), _) => name.clone(),
            (None, Expression::Field { name }) => name.clone(),
            (None, expression) => expression.column(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Combination {
    Union,
    Intersect,
    Except,
    Fuse,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JoinOn {
    /// `join REL (Q) [within D] [at P]`: pairs standing in an order relation.
    Order {
        relation: OrderRelation,
        within: Option<Distance>,
        at: TimePoint,
        scope: Option<String>,
    },
    /// `join (Q) on F = G`: pairs with equal exact keys.
    Key { left: String, right: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrderRelation {
    Near,
    Before,
    After,
    During,
    Containing,
}
impl OrderRelation {
    pub const ALL: [Self; 5] = [
        Self::Near,
        Self::Before,
        Self::After,
        Self::During,
        Self::Containing,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Near => "near",
            Self::Before => "before",
            Self::After => "after",
            Self::During => "during",
            Self::Containing => "containing",
        }
    }
    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|relation| relation.name().eq_ignore_ascii_case(text))
    }
    /// During and containing compare spans in time; positions have no extent.
    pub const fn measures_steps(self) -> bool {
        matches!(self, Self::Near | Self::Before | Self::After)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimePoint {
    When,
    Start,
    End,
    /// The matched row's occurrence time, independent of its actual span.
    MatchWhen,
    /// The start of the matched row's explicitly declared actual span.
    MatchStart,
    /// The end of the matched row's explicitly declared actual span.
    MatchEnd,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Aggregate {
    pub function: AggregateFunction,
    pub arguments: Vec<Expression>,
    pub name: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    /// `covered(start, end)`: the length of the union of the intervals.
    Covered,
}
impl AggregateFunction {
    pub fn name(&self) -> String {
        match self {
            Self::Count => "count".into(),
            Self::Sum => "sum".into(),
            Self::Avg => "avg".into(),
            Self::Min => "min".into(),
            Self::Max => "max".into(),
            Self::Percentile(p) => format!("p{p}"),
            Self::CountDistinct => "count_distinct".into(),
            Self::Histogram => "histogram".into(),
            Self::Correlation => "corr".into(),
            Self::Covered => "covered".into(),
        }
    }
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "count" => Self::Count,
            "sum" => Self::Sum,
            "avg" => Self::Avg,
            "min" => Self::Min,
            "max" => Self::Max,
            "count_distinct" => Self::CountDistinct,
            "histogram" => Self::Histogram,
            "corr" => Self::Correlation,
            "covered" => Self::Covered,
            other => {
                let digits = other.strip_prefix('p')?;
                if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                Self::Percentile(digits.parse().ok().filter(|p| *p <= 100)?)
            }
        })
    }
    /// The number of arguments the function takes; `count` takes zero or one.
    pub fn arguments(&self) -> std::ops::RangeInclusive<usize> {
        match self {
            Self::Count => 0..=1,
            Self::Correlation | Self::Covered => 2..=2,
            _ => 1..=1,
        }
    }
}
impl Aggregate {
    pub fn column(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let name = self.function.name();
        if self.arguments.is_empty() {
            name
        } else {
            format!(
                "{name}_{}",
                self.arguments
                    .iter()
                    .map(Expression::column)
                    .collect::<Vec<_>>()
                    .join("_")
            )
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
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
    Expression(Expression),
    /// A comma-separated repetition of one slot.
    List(Vec<ExtensionArgument>),
}
impl Query {
    /// Namespaces come exclusively from unit nodes, never literals.
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
            Source::Unit {
                unit, condition, ..
            } => {
                visitor(unit);
                if let Some(c) = condition {
                    c.visit_units(visitor);
                }
            }
            Source::Saved { condition, .. } | Source::Result { condition, .. } => {
                if let Some(condition) = condition {
                    condition.visit_units(visitor);
                }
            }
        }
        for a in &self.attach {
            visitor(&a.unit);
            for w in &a.windows {
                if let Window::Where(c) = w {
                    c.visit_units(visitor);
                }
            }
        }
        for t in &self.transforms {
            match &t.kind {
                TransformKind::Where { condition } => condition.visit_units(visitor),
                TransformKind::Scope { unit, condition } => {
                    visitor(unit);
                    if let Some(c) = condition {
                        c.visit_units(visitor);
                    }
                }
                TransformKind::Combine { query, .. }
                | TransformKind::Join { query, .. }
                | TransformKind::Diff { query } => query.visit_units(visitor),
                TransformKind::Extension { arguments, .. } => visit_arguments(arguments, visitor),
                _ => {}
            }
        }
    }
}
impl Condition {
    /// One positive order relation exposes its selected witness after the
    /// complete condition. All predicates read the same incoming row.
    /// Multiple orders and orders beneath OR or NOT expose no new namespace.
    pub fn exposed_order(&self) -> Option<&Condition> {
        fn collect<'a>(condition: &'a Condition, orders: &mut Vec<&'a Condition>) {
            match &condition.kind {
                ConditionKind::And { conditions } => {
                    for condition in conditions {
                        collect(condition, orders);
                    }
                }
                ConditionKind::Order { .. } => orders.push(condition),
                _ => {}
            }
        }
        let mut orders = Vec::new();
        collect(self, &mut orders);
        if orders.len() == 1 {
            orders.pop()
        } else {
            None
        }
    }
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
            ConditionKind::In { query, .. } | ConditionKind::Order { query, .. } => {
                query.visit_units(visitor)
            }
            ConditionKind::Sequence { unit, steps, .. } => {
                visitor(unit);
                for c in steps {
                    c.visit_units(visitor)
                }
            }
            ConditionKind::Extension { arguments, .. } => visit_arguments(arguments, visitor),
            // Bounds contain values and an already-bound field, never units.
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
            ExtensionArgument::Optional(arguments) | ExtensionArgument::List(arguments) => {
                visit_arguments(arguments, visitor)
            }
            _ => {}
        }
    }
}
