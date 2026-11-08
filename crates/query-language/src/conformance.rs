//! Conformance fixtures and a pure reference evaluator over caller-owned rows.
//!
//! The evaluator is an executable oracle for the shared relational mechanics:
//! conditions, natural matches, expressions, order relations by time and by
//! steps, joins, set algebra, fusion, `diff`, aggregates including
//! `covered`, and the generic `matches` shape. Product storage, text search,
//! ranking lists, civil-time functions, saved definitions, coverage and
//! extensions need product adapters and are refused here, at a position.
//!
//! Missing fields are unknown. A row names its parent (the scope instance its
//! steps are counted in), its order within that parent and its time extent;
//! enumeration order has no meaning.
use crate::ast::*;
use crate::catalog::{Catalog, NaturalMatch};
use crate::coverage::CoverageState;
use crate::semantics::Truth;
use crate::{Error, ErrorKind};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq)]
pub enum Scalar {
    Unknown,
    Bool(bool),
    Number(f64),
    Text(String),
    List(Vec<Scalar>),
}
impl From<&str> for Scalar {
    fn from(s: &str) -> Self {
        Self::Text(s.into())
    }
}
impl From<f64> for Scalar {
    fn from(n: f64) -> Self {
        Self::Number(n)
    }
}
impl Scalar {
    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown)
    }
    fn order(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => a.partial_cmp(b),
            (Self::Text(a), Self::Text(b)) => Some(a.cmp(b)),
            (Self::Bool(a), Self::Bool(b)) => Some(a.cmp(b)),
            _ => None,
        }
    }
}
/// A half-open span, or a point when start equals end. Units are caller-chosen
/// exact ticks, consistent with numeric `within` values. Coarse ranks after precise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extent {
    pub start: i64,
    pub end: i64,
    pub coarse: bool,
}
/// Declared state lifetime, independent of occurrence uncertainty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeclaredSpan {
    pub start: i64,
    /// None is a known open end, rather than missing span evidence.
    pub end: Option<i64>,
    pub end_inclusive: bool,
}
impl DeclaredSpan {
    fn contains(self, evidence: Extent) -> bool {
        evidence.start >= self.start
            && self.end.is_none_or(|end| {
                if evidence.coarse && evidence.end > evidence.start {
                    evidence.end <= end
                        || (self.end_inclusive && evidence.end.saturating_sub(1) <= end)
                } else if self.end_inclusive {
                    evidence.start <= end
                } else {
                    evidence.start < end
                }
            })
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub fields: BTreeMap<String, Scalar>,
    /// The row this one belongs to: an `exists` child's parent, and the scope
    /// instance (a session, a shell session) whose order its steps count in.
    pub parent: Option<String>,
    /// The row's place in its parent's order. Rows of every relation sharing
    /// a parent interleave in one order; ties go to identity.
    pub order: i64,
    /// Occurrence evidence; a coarse extent bounds when the occurrence happened.
    pub time: Option<Extent>,
    /// An independently declared state interval. Its endpoints are not inferred
    /// from the uncertainty bounds of `time`.
    pub span: Option<DeclaredSpan>,
    /// Selected relation identities, including sequence steps in occurrence order.
    pub matches: Vec<String>,
}
impl Row {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            fields: BTreeMap::new(),
            parent: None,
            order: 0,
            time: None,
            span: None,
            matches: vec![],
        }
    }
    pub fn with(mut self, field: impl Into<String>, value: impl Into<Scalar>) -> Self {
        self.fields.insert(field.into(), value.into());
        self
    }
    pub fn value(&self, field: &str) -> Scalar {
        if field == "id" {
            Scalar::Text(self.id.clone())
        } else {
            self.fields.get(field).cloned().unwrap_or(Scalar::Unknown)
        }
    }
}
pub type Dataset = BTreeMap<String, Vec<Row>>;
#[derive(Clone, Debug, PartialEq)]
pub struct Answer {
    pub rows: Vec<Row>,
    pub total: usize,
}
/// Transport slicing is deliberately outside Query and execution. Products use
/// their sealed keyset cursors; this helper only slices an immutable oracle answer.
pub fn page(answer: &Answer, after: Option<&str>, size: usize) -> Result<Answer, Error> {
    if size == 0 {
        return Err(refusal("page size must be positive", 0));
    }
    let start = match after {
        None => 0,
        Some(id) => {
            answer
                .rows
                .iter()
                .position(|r| r.id == id)
                .ok_or_else(|| refusal("continuation row is absent", 0))?
                + 1
        }
    };
    Ok(Answer {
        rows: answer.rows.iter().skip(start).take(size).cloned().collect(),
        total: answer.total,
    })
}
fn refusal(message: impl Into<String>, position: usize) -> Error {
    Error::new(ErrorKind::InvalidQuery, message, position)
}
fn unsupported(feature: &str, position: usize) -> Error {
    refusal(
        format!("reference evaluator requires a product adapter for {feature}"),
        position,
    )
}
fn bad(message: &str) -> Error {
    Error::new(ErrorKind::BadValue, message, 0)
}
fn literal(value: &Value, like: &Scalar) -> Result<Scalar, Error> {
    if let Scalar::List(items) = like {
        return literal(
            value,
            items
                .iter()
                .find(|v| !v.is_unknown())
                .unwrap_or(&Scalar::Unknown),
        );
    }
    match value {
        Value::Unknown => Ok(Scalar::Unknown),
        Value::Scalar { text, .. } => Ok(match like {
            Scalar::Number(_) => Scalar::Number(
                text.parse::<f64>()
                    .ok()
                    .filter(|n| n.is_finite())
                    .ok_or_else(|| bad("expected a finite number"))?,
            ),
            Scalar::Bool(_) => Scalar::Bool(text.parse().map_err(|_| bad("expected a boolean"))?),
            _ => Scalar::Text(text.clone()),
        }),
        Value::Set { values } => Ok(Scalar::List(
            values
                .iter()
                .map(|v| literal(v, like))
                .collect::<Result<_, _>>()?,
        )),
        _ => Err(unsupported("unbound parameter or wildcard comparison", 0)),
    }
}
/// A literal standing alone in an expression: a number when it reads as one.
fn standalone(value: &Value) -> Result<Scalar, Error> {
    match value {
        Value::Scalar {
            text,
            quoted: false,
        } => Ok(text
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .map_or_else(|| Scalar::Text(text.clone()), Scalar::Number)),
        Value::Scalar { text, .. } => Ok(Scalar::Text(text.clone())),
        other => literal(other, &Scalar::Unknown),
    }
}
/// Lists match any member; inequality excludes every equal member. Unknown
/// operands retain unknown, including under inequality and negation. `~` is
/// a case-insensitive substring and `^` a prefix.
pub fn compare(left: &Scalar, op: Op, right: &Scalar) -> Result<Truth, Error> {
    validate_scalar(left)?;
    validate_scalar(right)?;
    if left.is_unknown() || right.is_unknown() {
        return Ok(Truth::Unknown);
    }
    if let Scalar::List(items) = left {
        return compare_list(items, op, right);
    }
    if let Scalar::List(items) = right {
        let mut truth = Truth::False;
        for item in items {
            truth = truth.or(compare(left, if op == Op::Ne { Op::Eq } else { op }, item)?);
        }
        return Ok(if op == Op::Ne { truth.not() } else { truth });
    }
    if let (Scalar::Text(a), Scalar::Text(b)) = (left, right) {
        if op == Op::Contains {
            return Ok(a.to_lowercase().contains(&b.to_lowercase()).into());
        }
        if op == Op::Prefix {
            return Ok(a.starts_with(b.as_str()).into());
        }
    }
    let order = left
        .order(right)
        .ok_or_else(|| bad("comparison requires compatible scalar types"))?;
    Ok(match op {
        Op::Eq => order == Ordering::Equal,
        Op::Ne => order != Ordering::Equal,
        Op::Lt => order == Ordering::Less,
        Op::Le => order != Ordering::Greater,
        Op::Gt => order == Ordering::Greater,
        Op::Ge => order != Ordering::Less,
        _ => return Err(bad("text operator requires text")),
    }
    .into())
}
fn compare_list(items: &[Scalar], op: Op, right: &Scalar) -> Result<Truth, Error> {
    let mut truth = Truth::False;
    for item in items {
        truth = truth.or(compare(
            item,
            if op == Op::Ne { Op::Eq } else { op },
            right,
        )?);
    }
    Ok(if op == Op::Ne { truth.not() } else { truth })
}
/// The natural match `field:value` for one declared match kind.
pub fn natural(left: &Scalar, kind: NaturalMatch, right: &Scalar) -> Result<Truth, Error> {
    if left.is_unknown() || right.is_unknown() {
        return Ok(Truth::Unknown);
    }
    if let Scalar::List(items) = right {
        let mut truth = Truth::False;
        for item in items {
            truth = truth.or(natural(left, kind, item)?);
        }
        return Ok(truth);
    }
    if let Scalar::List(items) = left {
        let mut truth = Truth::False;
        for item in items {
            truth = truth.or(natural(item, kind, right)?);
        }
        return Ok(truth);
    }
    match (kind, left, right) {
        (NaturalMatch::Equal | NaturalMatch::Same, _, _) => compare(left, Op::Eq, right),
        (NaturalMatch::Contains, Scalar::Text(_), Scalar::Text(_)) => {
            compare(left, Op::Contains, right)
        }
        (NaturalMatch::Words, Scalar::Text(a), Scalar::Text(b)) => {
            let wanted = b.to_lowercase();
            Ok(a.split(|c: char| !c.is_alphanumeric())
                .any(|word| word.to_lowercase() == wanted)
                .into())
        }
        (NaturalMatch::Subtree, Scalar::Text(a), Scalar::Text(b)) => Ok(under(a, b).into()),
        (NaturalMatch::Period, _, _) => Err(unsupported("a time period match", 0)),
        _ => Err(bad("this natural match needs text")),
    }
}
/// A path is itself or lies under another by whole components.
fn under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches('/');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/') || root.is_empty())
}
/// Nearest-rank uses ceil(p*n/100), with percentile zero selecting the minimum.
/// Unknown observations are omitted; no known observations yields unknown.
pub fn nearest_rank(values: &[Scalar], percentile: u8) -> Result<Scalar, Error> {
    if percentile > 100 {
        return Err(bad("percentile must be between 0 and 100"));
    }
    if values
        .iter()
        .any(|v| matches!(v,Scalar::Number(n) if !n.is_finite()))
    {
        return Err(bad("percentile requires finite numbers"));
    }
    let mut known: Vec<_> = values.iter().filter(|v| !v.is_unknown()).cloned().collect();
    for pair in known.windows(2) {
        if pair[0].order(&pair[1]).is_none() {
            return Err(bad("percentile requires compatible ordered scalars"));
        }
    }
    known.sort_by(|a, b| a.order(b).unwrap_or(Ordering::Equal));
    if known.is_empty() {
        return Ok(Scalar::Unknown);
    }
    let rank = (usize::from(percentile) * known.len()).div_ceil(100).max(1);
    Ok(known[rank - 1].clone())
}
/// The length of the union of half-open intervals `[start, end)`. An interval
/// whose end is not after its start covers nothing.
pub fn covered(intervals: &[(i64, i64)]) -> i64 {
    let mut sorted: Vec<_> = intervals.iter().filter(|(s, e)| e > s).copied().collect();
    sorted.sort();
    let mut total: i128 = 0;
    let mut current: Option<(i64, i64)> = None;
    for (start, end) in sorted {
        match &mut current {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => {
                if let Some((s, e)) = current {
                    total += i128::from(e) - i128::from(s);
                }
                current = Some((start, end));
            }
        }
    }
    if let Some((s, e)) = current {
        total += i128::from(e) - i128::from(s);
    }
    i64::try_from(total).unwrap_or(i64::MAX)
}
/// Complete a grouped answer over a finite list of time buckets: every
/// bucket appears, in the given order, labelled with its coverage. A bucket
/// with no rows is a known zero for counting aggregates only when its period
/// was completely observed; otherwise its values are unknown, never zero.
/// Groups outside the buckets follow in their own order.
pub fn fill(
    groups: &[Row],
    key: &str,
    aggregates: &[Aggregate],
    buckets: &[Scalar],
    coverage: impl Fn(&Scalar) -> CoverageState,
) -> Vec<Row> {
    let label = |row: &mut Row, state: &CoverageState| {
        row.fields.insert(
            crate::coverage::STATE_FIELD.into(),
            Scalar::Text(state.state().into()),
        );
        row.fields.insert(
            crate::coverage::DETAIL_FIELD.into(),
            state
                .detail()
                .map_or(Scalar::Unknown, |detail| Scalar::Text(detail.into())),
        );
    };
    let mut used = BTreeSet::new();
    let mut out = Vec::new();
    for bucket in buckets {
        let state = coverage(bucket);
        let mut row = match groups
            .iter()
            .enumerate()
            .find(|(_, row)| row.value(key) == *bucket)
        {
            Some((index, row)) => {
                used.insert(index);
                row.clone()
            }
            None => {
                let zero = |counting: bool| {
                    if counting && state.is_complete() {
                        Scalar::Number(0.)
                    } else {
                        Scalar::Unknown
                    }
                };
                let mut row = Row::new(format!("bucket:{bucket:?}"));
                row.fields.insert(key.into(), bucket.clone());
                row.fields.insert("count".into(), zero(true));
                for aggregate in aggregates {
                    let counting = matches!(
                        aggregate.function,
                        AggregateFunction::Count
                            | AggregateFunction::CountDistinct
                            | AggregateFunction::Sum
                            | AggregateFunction::Covered
                    );
                    row.fields.insert(aggregate.column(), zero(counting));
                }
                row
            }
        };
        label(&mut row, &state);
        out.push(row);
    }
    for (index, row) in groups.iter().enumerate() {
        if !used.contains(&index) {
            let state = coverage(&row.value(key));
            let mut row = row.clone();
            label(&mut row, &state);
            out.push(row);
        }
    }
    out
}

pub struct ReferenceEvaluator<'a> {
    pub data: &'a Dataset,
    /// The natural match of each field written `field:value`, read from a
    /// catalog's declarations; a field the catalog does not declare matches
    /// by equality.
    natural: BTreeMap<String, NaturalMatch>,
}
impl<'a> ReferenceEvaluator<'a> {
    pub fn new(data: &'a Dataset) -> Self {
        Self {
            data,
            natural: BTreeMap::new(),
        }
    }
    /// Read each field's natural match from the catalog's declarations, by
    /// field name; the oracle's fixtures name fields, not units.
    pub fn with_catalog(mut self, catalog: &Catalog) -> Self {
        for field in catalog.units.iter().flat_map(|unit| &unit.fields) {
            self.natural
                .insert(field.name.clone(), field.natural_match());
        }
        self
    }
    fn relation(&self, unit: &str) -> Result<Vec<Row>, Error> {
        self.data
            .get(unit)
            .cloned()
            .ok_or_else(|| refusal(format!("unknown fixture relation {unit}"), 0))
    }
    fn children(&self, row: &Row, unit: &str) -> Result<Vec<Row>, Error> {
        Ok(self
            .relation(unit)?
            .into_iter()
            .filter(|child| child.parent.as_deref() == Some(&row.id))
            .collect())
    }
    /// Every row sharing a parent, in that parent's order.
    fn timeline(&self, parent: &str) -> Vec<&Row> {
        let mut seen = BTreeSet::new();
        let mut rows: Vec<&Row> = self
            .data
            .values()
            .flatten()
            .filter(|row| row.parent.as_deref() == Some(parent))
            .filter(|row| seen.insert(row.id.clone()))
            .collect();
        rows.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        rows
    }
    /// A row's position in its parent's order; unknown without a parent.
    fn position(&self, row: &Row) -> Option<usize> {
        let parent = row.parent.as_deref()?;
        self.timeline(parent)
            .iter()
            .position(|other| other.id == row.id)
    }
    /// A field, including `next.F` and `previous.F`: the field of the row one
    /// step later or earlier in the parent's order.
    fn value(&self, row: &Row, name: &str) -> Scalar {
        if let Some(value) = row.fields.get(name) {
            return value.clone();
        }
        for (prefix, step) in [("next.", 1isize), ("previous.", -1)] {
            if let Some(field) = name.strip_prefix(prefix) {
                let (Some(parent), Some(at)) = (row.parent.as_deref(), self.position(row)) else {
                    return Scalar::Unknown;
                };
                let timeline = self.timeline(parent);
                return at
                    .checked_add_signed(step)
                    .and_then(|i| timeline.get(i))
                    .map_or(Scalar::Unknown, |neighbour| neighbour.value(field));
            }
        }
        row.value(name)
    }
    pub fn expression(&self, row: &Row, expression: &Expression) -> Result<Scalar, Error> {
        match expression {
            Expression::Field { name } => Ok(self.value(row, name)),
            Expression::Value { value } => standalone(value),
            Expression::Function { name, arguments } if name == "len" => {
                Ok(match self.expression(row, &arguments[0])? {
                    Scalar::Text(text) => Scalar::Number(text.chars().count() as f64),
                    Scalar::List(items) => Scalar::Number(items.len() as f64),
                    Scalar::Unknown => Scalar::Unknown,
                    _ => return Err(bad("len takes text or a list")),
                })
            }
            Expression::Function { name, arguments } if name == "text" => {
                let [argument] = arguments.as_slice() else {
                    return Err(bad("text takes one argument"));
                };
                match self.expression(row, argument)? {
                    value @ (Scalar::Text(_) | Scalar::Unknown) => Ok(value),
                    _ => Err(bad("text takes a string value")),
                }
            }
            Expression::Function { name, arguments } if name == "abs" => {
                let [argument] = arguments.as_slice() else {
                    return Err(bad("abs takes one argument"));
                };
                match self.expression(row, argument)? {
                    Scalar::Number(value) => Ok(Scalar::Number(value.abs())),
                    Scalar::Unknown => Ok(Scalar::Unknown),
                    _ => Err(bad("abs takes a linear number")),
                }
            }
            Expression::Function { name, arguments } if name == "glob" => {
                let [text, pattern] = arguments.as_slice() else {
                    return Err(bad("glob takes two arguments"));
                };
                match (self.expression(row, text)?, self.expression(row, pattern)?) {
                    (Scalar::Unknown, _) | (_, Scalar::Unknown) => Ok(Scalar::Unknown),
                    (Scalar::Text(text), Scalar::Text(pattern)) => {
                        Ok(Scalar::Bool(crate::glob::matches(&text, &pattern)))
                    }
                    _ => Err(bad("glob takes text values")),
                }
            }
            Expression::Function { name, arguments } if name == "case_eq" => {
                let [left, right, selected] = arguments.as_slice() else {
                    return Err(bad("case_eq takes three arguments"));
                };
                let mut left_value = self.expression(row, left)?;
                let right_value = match right {
                    Expression::Value { value } => literal(value, &left_value)?,
                    other => self.expression(row, other)?,
                };
                if let Expression::Value { value } = left {
                    left_value = literal(value, &right_value)?;
                }
                if left_value != Scalar::Unknown
                    && right_value != Scalar::Unknown
                    && left_value == right_value
                {
                    self.expression(row, selected)
                } else {
                    Ok(Scalar::Unknown)
                }
            }
            Expression::Function { name, .. } => Err(unsupported(name, 0)),
            Expression::Arithmetic {
                operator,
                left,
                right,
            } => {
                let (l, r) = (self.expression(row, left)?, self.expression(row, right)?);
                let (Scalar::Number(l), Scalar::Number(r)) = (&l, &r) else {
                    if l.is_unknown() || r.is_unknown() {
                        return Ok(Scalar::Unknown);
                    }
                    return Err(bad("arithmetic requires numbers"));
                };
                let result = match operator {
                    Arithmetic::Add => l + r,
                    Arithmetic::Subtract => l - r,
                    Arithmetic::Multiply => l * r,
                    // A quotient by zero does not exist: unknown, never infinite.
                    Arithmetic::Divide if *r == 0. => return Ok(Scalar::Unknown),
                    Arithmetic::Divide => l / r,
                };
                if !result.is_finite() {
                    return Err(bad("arithmetic requires a finite result"));
                }
                Ok(Scalar::Number(result))
            }
        }
    }
    pub fn condition(&self, row: &Row, condition: &Condition) -> Result<Truth, Error> {
        let truth = match &condition.kind {
            ConditionKind::And { conditions } => {
                let mut v = Truth::True;
                for c in conditions {
                    v = v.and(self.condition(row, c)?);
                }
                v
            }
            ConditionKind::Or { conditions } => {
                let mut v = Truth::False;
                for c in conditions {
                    v = v.or(self.condition(row, c)?);
                }
                v
            }
            ConditionKind::Not { condition } => self.condition(row, condition)?.not(),
            ConditionKind::Match { field, value } => {
                let left = self.value(row, field);
                match value {
                    Value::Any => Truth::True,
                    Value::Unknown => left.is_unknown().into(),
                    Value::Known => (!left.is_unknown()).into(),
                    _ => natural(
                        &left,
                        self.natural
                            .get(field)
                            .copied()
                            .unwrap_or(NaturalMatch::Equal),
                        &literal(value, &left)?,
                    )?,
                }
            }
            ConditionKind::Compare { left, op, right } => {
                let left_value = self.expression(row, left)?;
                let right_value = match right {
                    Expression::Value { value } => literal(value, &left_value)?,
                    other => self.expression(row, other)?,
                };
                if *op == Op::Prefix
                    && let Expression::Field { name } = left
                    && self.natural.get(name) == Some(&NaturalMatch::Subtree)
                    && let (Scalar::Text(a), Scalar::Text(b)) = (&left_value, &right_value)
                {
                    under(a, b).into()
                } else {
                    compare(&left_value, *op, &right_value)?
                }
            }
            ConditionKind::Between { left, from, to } => {
                let left = self.expression(row, left)?;
                compare(&left, Op::Ge, &literal(from, &left)?)?.and(compare(
                    &left,
                    Op::Le,
                    &literal(to, &left)?,
                )?)
            }
            ConditionKind::Exists { unit, condition } => {
                let mut v = Truth::False;
                for child in self.children(row, unit)? {
                    v = v.or(match condition {
                        Some(c) => self.condition(&child, c)?,
                        None => Truth::True,
                    });
                }
                v
            }
            ConditionKind::Count {
                unit,
                condition,
                op,
                count,
            } => {
                let mut n = 0;
                for child in self.children(row, unit)? {
                    if condition
                        .as_ref()
                        .map_or(Ok(Truth::True), |c| self.condition(&child, c))?
                        .matches()
                    {
                        n += 1;
                    }
                }
                compare(
                    &Scalar::Number(n as f64),
                    *op,
                    &Scalar::Number(*count as f64),
                )?
            }
            ConditionKind::In { left, query } => {
                let left = self.expression(row, left)?;
                if left.is_unknown() {
                    Truth::Unknown
                } else {
                    let answer = self.execute(query)?;
                    let column = membership_column(query).ok_or_else(|| {
                        refusal("membership requires one grouped key", condition.span.start)
                    })?;
                    let mut v = Truth::False;
                    for r in answer.rows {
                        let right = r.value(&column);
                        if !right.is_unknown() {
                            v = v.or(compare(&left, Op::Eq, &right)?);
                        }
                    }
                    v
                }
            }
            ConditionKind::Reference { reference } => (row.id == *reference).into(),
            // Ranking atoms order rows and never select them.
            ConditionKind::Rank { .. } => Truth::True,
            ConditionKind::Sequence {
                unit,
                all,
                steps,
                gaps,
            } => (!self.sequence(row, unit, *all, steps, gaps)?.is_empty()).into(),
            ConditionKind::Order {
                relation,
                query,
                within,
                at,
                ..
            } => {
                let inner = self.execute(query)?.rows;
                match self.related(row, &inner, *relation, within.as_ref(), *at)? {
                    None => Truth::Unknown,
                    Some(pairs) => (!pairs.is_empty()).into(),
                }
            }
            other => {
                return Err(unsupported(
                    &format!("condition {other:?}"),
                    condition.span.start,
                ));
            }
        };
        Ok(truth)
    }
    /// The inner rows standing in an order relation to an outer row, nearest
    /// first (precise before coarse, then distance, then identity), each with
    /// its signed offset. None when the outer row's time or position is unknown.
    fn related<'r>(
        &self,
        row: &Row,
        inner: &'r [Row],
        relation: OrderRelation,
        within: Option<&Distance>,
        at: TimePoint,
    ) -> Result<Option<Vec<(&'r Row, i128)>>, Error> {
        match within {
            Some(Distance::Steps { count }) => {
                let Some(here) = self.position(row) else {
                    return Ok(None);
                };
                let mut pairs = Vec::new();
                for other in inner {
                    if other.id == row.id || other.parent != row.parent {
                        continue;
                    }
                    let Some(there) = self.position(other) else {
                        continue;
                    };
                    let offset = there as i128 - here as i128;
                    let fits = offset.unsigned_abs() <= u128::from(*count)
                        && match relation {
                            OrderRelation::Before => offset > 0,
                            OrderRelation::After => offset < 0,
                            _ => true,
                        };
                    if fits {
                        pairs.push((other, offset));
                    }
                }
                pairs.sort_by(|a, b| {
                    (a.1.unsigned_abs(), &a.0.id).cmp(&(b.1.unsigned_abs(), &b.0.id))
                });
                Ok(Some(pairs))
            }
            Some(Distance::Time { duration }) => self.related_in_time(
                row,
                inner,
                relation,
                Some(
                    duration
                        .text()
                        .and_then(|v| v.parse::<u64>().ok())
                        .ok_or_else(|| unsupported("resolved exact-tick duration", 0))?,
                ),
                at,
            ),
            None => self.related_in_time(row, inner, relation, None, at),
        }
    }
    fn related_in_time<'r>(
        &self,
        row: &Row,
        inner: &'r [Row],
        relation: OrderRelation,
        window: Option<u64>,
        at: TimePoint,
    ) -> Result<Option<Vec<(&'r Row, i128)>>, Error> {
        if matches!(
            at,
            TimePoint::MatchWhen | TimePoint::MatchStart | TimePoint::MatchEnd
        ) {
            return self.related_at_match(row, inner, relation, window, at);
        }
        let Some(a) = row.time else {
            return Ok(None);
        };
        let point = match at {
            TimePoint::End => a.end,
            _ => a.start,
        };
        let mut candidates = vec![];
        let mut unknown = false;
        for other in inner {
            let Some(b) = other.time else {
                unknown = true;
                continue;
            };
            let offset = i128::from(b.start) - i128::from(point);
            let distance = offset.unsigned_abs();
            let last = |v: Extent| if v.end > v.start { v.end - 1 } else { v.start };
            let greatest_distance = [
                (a.start, b.start),
                (a.start, last(b)),
                (last(a), b.start),
                (last(a), last(b)),
            ]
            .into_iter()
            .map(|(x, y)| (i128::from(x) - i128::from(y)).unsigned_abs())
            .max()
            .unwrap_or(0);
            if window.is_some_and(|w| {
                if a.coarse || b.coarse {
                    greatest_distance > u128::from(w)
                } else {
                    distance > u128::from(w)
                }
            }) {
                continue;
            }
            // A coarse time joins only a window at least as wide as its period;
            // no window is wider than every period.
            if (a.coarse || b.coarse)
                && window.is_some_and(|w| {
                    u128::from(w)
                        < (i128::from(a.end) - i128::from(a.start))
                            .unsigned_abs()
                            .max((i128::from(b.end) - i128::from(b.start)).unsigned_abs())
                })
            {
                continue;
            }
            // A coarse time takes part by its whole period: the half-open
            // stretch each side occupies, a precise time being one tick.
            let tick_after = |at: i64| at.saturating_add(1);
            let (a_from, a_to) = if a.coarse {
                (a.start, a.end.max(tick_after(a.start)))
            } else {
                (point, tick_after(point))
            };
            let b_to = if b.coarse {
                b.end.max(tick_after(b.start))
            } else {
                tick_after(b.start)
            };
            let matches = match relation {
                OrderRelation::Near => true,
                OrderRelation::Before => a_to <= b.start,
                OrderRelation::After => a_from >= b_to,
                OrderRelation::During => {
                    if a.coarse {
                        a.start >= b.start && a.end <= b.end
                    } else {
                        point >= b.start && point < b.end
                    }
                }
                OrderRelation::Containing => {
                    if b.end > b.start {
                        a.end > a.start && b.start >= a.start && b.end <= a.end
                    } else if a.end > a.start {
                        b.start >= a.start && b.start < a.end
                    } else {
                        b.start == a.start
                    }
                }
            };
            if matches {
                candidates.push((
                    b.coarse || a.coarse,
                    distance,
                    other.id.clone(),
                    other,
                    offset,
                ));
            }
        }
        if candidates.is_empty() && unknown {
            return Ok(None);
        }
        candidates.sort_by(|x, y| (x.0, x.1, &x.2).cmp(&(y.0, y.1, &y.2)));
        Ok(Some(
            candidates
                .into_iter()
                .map(|(_, _, _, other, offset)| (other, offset))
                .collect(),
        ))
    }
    fn related_at_match<'r>(
        &self,
        row: &Row,
        inner: &'r [Row],
        relation: OrderRelation,
        window: Option<u64>,
        at: TimePoint,
    ) -> Result<Option<Vec<(&'r Row, i128)>>, Error> {
        let Some(a) = row.time else { return Ok(None) };
        if relation == OrderRelation::Containing && row.span.is_none() {
            return Ok(None);
        }
        let point = |tick| Extent {
            start: tick,
            end: tick,
            coarse: false,
        };
        let last = |v: Extent| {
            if v.coarse && v.end > v.start {
                v.end - 1
            } else {
                v.start
            }
        };
        let mut pairs = vec![];
        let mut unknown = false;
        for other in inner {
            // During selects a state by its declared lifetime. The occurrence
            // time may precede its start, and is not its ordering anchor.
            let selected = if relation == OrderRelation::During {
                other.span.map(|span| point(span.start))
            } else {
                match at {
                    TimePoint::MatchWhen => other.time,
                    TimePoint::MatchStart => other.span.map(|span| point(span.start)),
                    TimePoint::MatchEnd => other.span.and_then(|span| span.end.map(point)),
                    _ => unreachable!(),
                }
            };
            let Some(b) = selected else {
                unknown = true;
                continue;
            };
            let offset = i128::from(b.start) - i128::from(a.start);
            let distance = offset.unsigned_abs();
            let greatest = [
                (a.start, b.start),
                (a.start, last(b)),
                (last(a), b.start),
                (last(a), last(b)),
            ]
            .into_iter()
            .map(|(x, y)| (i128::from(y) - i128::from(x)).unsigned_abs())
            .max()
            .unwrap();
            if window.is_some_and(|w| {
                if a.coarse || b.coarse {
                    greatest > u128::from(w)
                } else {
                    distance > u128::from(w)
                }
            }) {
                continue;
            }
            let fits = match relation {
                OrderRelation::Near => true,
                OrderRelation::Before => last(a) < b.start,
                OrderRelation::After => a.start > last(b),
                OrderRelation::During => other.span.unwrap().contains(a),
                OrderRelation::Containing => row.span.unwrap().contains(b),
            };
            if fits {
                pairs.push((a.coarse || b.coarse, distance, &other.id, other, offset))
            }
        }
        pairs.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
        if pairs.is_empty() && unknown {
            return Ok(None);
        }
        Ok(Some(
            pairs
                .into_iter()
                .map(|(_, _, _, row, offset)| (row, offset))
                .collect(),
        ))
    }
    /// Select a witness independently of input enumeration. Unknown children do
    /// not become witnesses; existence itself still preserves their truth value.
    pub fn exists_witness(
        &self,
        row: &Row,
        unit: &str,
        condition: Option<&Condition>,
    ) -> Result<Option<String>, Error> {
        Ok(self
            .exists_matches(row, unit, condition)?
            .into_iter()
            .next())
    }
    fn exists_matches(
        &self,
        row: &Row,
        unit: &str,
        condition: Option<&Condition>,
    ) -> Result<Vec<String>, Error> {
        let mut selected = vec![];
        for child in self.children(row, unit)? {
            if condition
                .map_or(Ok(Truth::True), |c| self.condition(&child, c))?
                .matches()
            {
                selected.push(child.id);
            }
        }
        selected.sort();
        Ok(selected)
    }
    /// Sequence order is explicit occurrence order, with identity breaking ties.
    /// Default matches are earliest non-overlapping; `all` returns every tuple.
    pub fn sequence(
        &self,
        parent: &Row,
        unit: &str,
        all: bool,
        steps: &[Condition],
        gaps: &[Gap],
    ) -> Result<Vec<Vec<String>>, Error> {
        if steps.is_empty() || gaps.len() + 1 != steps.len() {
            return Err(refusal("sequence needs a gap between each step", 0));
        }
        let related = self.children(parent, unit)?;
        let mut rows: Vec<_> = related.iter().collect();
        rows.sort_by(|a, b| (a.order, &a.id).cmp(&(b.order, &b.id)));
        let mut matches = vec![];
        let mut start = 0;
        while start < rows.len() {
            let mut tuples = vec![];
            self.sequence_step(&rows, steps, gaps, 0, start, &mut vec![], &mut tuples, !all)?;
            if all {
                matches.extend(
                    tuples
                        .into_iter()
                        .map(|indices| indices.into_iter().map(|i| rows[i].id.clone()).collect()),
                );
                break;
            }
            if let Some(indices) = tuples.first() {
                start = indices[indices.len() - 1] + 1;
                matches.push(indices.iter().map(|&i| rows[i].id.clone()).collect());
            } else {
                break;
            }
        }
        Ok(matches)
    }
    #[allow(clippy::too_many_arguments)]
    fn sequence_step(
        &self,
        rows: &[&Row],
        steps: &[Condition],
        gaps: &[Gap],
        step: usize,
        start: usize,
        picked: &mut Vec<usize>,
        out: &mut Vec<Vec<usize>>,
        first_only: bool,
    ) -> Result<(), Error> {
        if step == steps.len() {
            out.push(picked.clone());
            return Ok(());
        }
        for i in start..rows.len() {
            if let Some(&prev) = picked.last() {
                match &gaps[step - 1] {
                    Gap::Next if i != prev + 1 => continue,
                    Gap::Within {
                        distance: Distance::Steps { count },
                    } => {
                        let (Some(a), Some(b)) =
                            (self.position(rows[prev]), self.position(rows[i]))
                        else {
                            continue;
                        };
                        if b <= a || (b - a) as u64 > *count {
                            continue;
                        }
                    }
                    Gap::Within {
                        distance: Distance::Time { duration },
                    } => {
                        let max = duration
                            .text()
                            .and_then(|v| v.parse::<i64>().ok())
                            .ok_or_else(|| {
                                unsupported("resolved exact-tick sequence duration", 0)
                            })?;
                        let (Some(a), Some(b)) = (rows[prev].time, rows[i].time) else {
                            continue;
                        };
                        if b.start <= a.start
                            || i128::from(b.start) - i128::from(a.start) > i128::from(max)
                        {
                            continue;
                        }
                    }
                    _ => {}
                }
            }
            if self.condition(rows[i], &steps[step])?.matches() {
                picked.push(i);
                self.sequence_step(rows, steps, gaps, step + 1, i + 1, picked, out, first_only)?;
                picked.pop();
                if first_only && !out.is_empty() {
                    return Ok(());
                }
            }
        }
        Ok(())
    }
    pub fn execute(&self, query: &Query) -> Result<Answer, Error> {
        if !query.attach.is_empty() {
            return Err(unsupported("attachments", query.attach[0].span.start));
        }
        let Source::Unit {
            unit, condition, ..
        } = &query.source
        else {
            return Err(unsupported("non-unit source", 0));
        };
        validate_query(query)?;
        let mut rows = self.relation(unit)?;
        for row in &rows {
            for value in row.fields.values() {
                validate_scalar(value)?;
            }
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        if let Some(c) = condition {
            rows = self.filter(rows, c)?;
        }
        let mut pattern = condition.clone();
        type IndexedRows = Vec<(usize, Row)>;
        let mut groups: Option<(Vec<IndexedRows>, Option<Condition>)> = None;
        // Ranked lists of one level of fuse and union, fused once.
        let mut lists: Vec<Vec<crate::fusion::Entry<String>>> = Vec::new();
        for (index, transform) in query.transforms.iter().enumerate() {
            let at = transform.span.start;
            match &transform.kind {
                TransformKind::Where { condition } => {
                    rows = self.filter(rows, condition)?;
                    groups = None;
                }
                TransformKind::Sort { keys } => {
                    rows = self.sort(rows, keys, at)?;
                    groups = None;
                }
                TransformKind::First { count } if groups.is_some() => {
                    let (members, input_pattern) = groups.take().unwrap();
                    let count = usize::try_from(*count).unwrap_or(usize::MAX);
                    let mut selected = members
                        .into_iter()
                        .flat_map(|members| members.into_iter().take(count))
                        .collect::<Vec<_>>();
                    selected.sort_by_key(|(position, _)| *position);
                    rows = selected.into_iter().map(|(_, row)| row).collect();
                    pattern = input_pattern;
                }
                TransformKind::Limit { count } | TransformKind::First { count } => {
                    rows.truncate(usize::try_from(*count).unwrap_or(usize::MAX));
                    groups = None;
                }
                TransformKind::Offset { count } => {
                    rows = rows
                        .into_iter()
                        .skip(usize::try_from(*count).unwrap_or(usize::MAX))
                        .collect();
                    groups = None;
                }
                TransformKind::Group { keys } => {
                    let mut grouped: Vec<(Vec<Scalar>, IndexedRows)> = vec![];
                    for (position, row) in rows.into_iter().enumerate() {
                        let values = keys
                            .iter()
                            .map(|k| self.expression(&row, &k.expression))
                            .collect::<Result<Vec<_>, _>>()?;
                        if let Some((_, members)) = grouped.iter_mut().find(|(v, _)| *v == values) {
                            members.push((position, row));
                        } else {
                            grouped.push((values, vec![(position, row)]));
                        }
                    }
                    grouped.sort_by_key(|(v, _)| format!("{v:?}"));
                    let mut members = vec![];
                    rows = vec![];
                    for (values, items) in grouped {
                        let mut row = Row::new(format!("group:{values:?}"));
                        for (key, value) in keys.iter().zip(values) {
                            row.fields.insert(key.column(), value);
                        }
                        row.fields
                            .insert("count".into(), Scalar::Number(items.len() as f64));
                        rows.push(row);
                        members.push(items);
                    }
                    groups = Some((members, pattern.take()));
                }
                TransformKind::Aggregate { aggregates } => {
                    let grouped = groups.is_some();
                    let members = groups
                        .take()
                        .map(|(groups, _)| {
                            groups
                                .into_iter()
                                .map(|group| group.into_iter().map(|(_, row)| row).collect())
                                .collect()
                        })
                        .unwrap_or_else(|| vec![rows.clone()]);
                    if !grouped {
                        rows = vec![Row::new("aggregate")];
                    }
                    for (row, items) in rows.iter_mut().zip(members) {
                        for aggregate in aggregates {
                            row.fields.insert(
                                aggregate.column(),
                                self.aggregate_value(&items, aggregate)?,
                            );
                            for argument in &aggregate.arguments {
                                if let Expression::Field { name } = argument {
                                    let mut unknown = 0;
                                    for item in &items {
                                        if self.value(item, name).is_unknown() {
                                            unknown += 1;
                                        }
                                    }
                                    row.fields.insert(
                                        format!("unknown_{}", argument.column()),
                                        Scalar::Number(unknown as f64),
                                    );
                                }
                            }
                        }
                    }
                    pattern = None;
                }
                TransformKind::Fields { columns } => {
                    for row in &mut rows {
                        let mut fields = BTreeMap::new();
                        for column in columns {
                            fields
                                .insert(column.column(), self.expression(row, &column.expression)?);
                        }
                        row.fields = fields;
                    }
                    groups = None;
                }
                TransformKind::Distinct => {
                    let mut seen = BTreeSet::new();
                    rows.retain(|r| seen.insert(r.id.clone()));
                    groups = None;
                }
                TransformKind::Combine {
                    how,
                    query: operand,
                } => {
                    let other = self.execute(operand)?.rows;
                    let ids: BTreeSet<_> = other.iter().map(|r| r.id.clone()).collect();
                    let fusion_chain =
                        !lists.is_empty() || query_contains_fuse(&query.transforms[index..]);
                    if fusion_chain && matches!(how, Combination::Union | Combination::Fuse) {
                        // The left relation is the first operand of this chain;
                        // later stages add operands, never a fused intermediate.
                        if lists.is_empty() {
                            lists.push(ranked_rows(&rows));
                        }
                        lists.push(ranked_rows(&other));
                    }
                    match how {
                        Combination::Union => {
                            rows.extend(other);
                            let mut seen = BTreeSet::new();
                            rows.retain(|r| seen.insert(r.id.clone()));
                            if !fusion_chain {
                                rows.sort_by(|a, b| a.id.cmp(&b.id));
                            }
                        }
                        Combination::Intersect => rows.retain(|r| ids.contains(&r.id)),
                        Combination::Except => rows.retain(|r| !ids.contains(&r.id)),
                        Combination::Fuse => {}
                    }
                    groups = None;
                }
                TransformKind::Join { query, on } => {
                    let inner = self.execute(query)?.rows;
                    rows = self.join(rows, &inner, on)?;
                    groups = None;
                    pattern = None;
                }
                TransformKind::Diff { query } => {
                    let before = self.execute(query)?.rows;
                    rows = diff_rows(&before, rows);
                    groups = None;
                    pattern = None;
                }
                TransformKind::Matches => {
                    rows = self.matches(&rows, pattern.as_ref(), at)?;
                    groups = None;
                    pattern = None;
                }
                other => return Err(unsupported(&format!("transform {other:?}"), at)),
            }
            let chain_continues = matches!(
                query.transforms.get(index + 1).map(|t| &t.kind),
                Some(TransformKind::Combine {
                    how: Combination::Fuse | Combination::Union,
                    ..
                })
            );
            if !lists.is_empty() && !chain_continues {
                rows = apply_fusion(rows, std::mem::take(&mut lists));
            }
        }
        Ok(Answer {
            total: rows.len(),
            rows,
        })
    }
    fn sort(&self, mut rows: Vec<Row>, keys: &[SortKey], at: usize) -> Result<Vec<Row>, Error> {
        for key in keys {
            if matches!(&key.expression, Expression::Function { name, .. } if name == "random")
                || matches!(&key.expression, Expression::Field { name } if name == "rank")
            {
                return Err(unsupported("random or ranked order", at));
            }
        }
        let decorated: Vec<_> = rows
            .iter()
            .map(|r| {
                keys.iter()
                    .map(|k| self.expression(r, &k.expression))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<_, _>>()?;
        for index in 0..keys.len() {
            let known: Vec<_> = decorated
                .iter()
                .map(|v| &v[index])
                .filter(|v| !v.is_unknown())
                .collect();
            for pair in known.windows(2) {
                if pair[0].order(pair[1]).is_none() {
                    return Err(Error::new(
                        ErrorKind::BadValue,
                        "sort requires compatible ordered scalars",
                        at,
                    ));
                }
            }
        }
        let by_id: BTreeMap<_, _> = rows
            .iter()
            .zip(decorated)
            .map(|(r, v)| (r.id.clone(), v))
            .collect();
        rows.sort_by(|a, b| {
            for (i, key) in keys.iter().enumerate() {
                let l = &by_id[&a.id][i];
                let r = &by_id[&b.id][i];
                let order = match (l.is_unknown(), r.is_unknown()) {
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    (true, true) => Ordering::Equal,
                    _ => {
                        let o = l.order(r).unwrap_or(Ordering::Equal);
                        if key.descending { o.reverse() } else { o }
                    }
                };
                if order != Ordering::Equal {
                    return order;
                }
            }
            a.id.cmp(&b.id)
        });
        Ok(rows)
    }
    fn witness<'r>(
        &self,
        outer: &Row,
        mut pairs: Vec<(&'r Row, i128)>,
        pick: &WitnessChoice,
        order: (OrderRelation, Option<&Distance>, TimePoint),
    ) -> Result<Option<(&'r Row, i128)>, Error> {
        let (relation, within, at) = order;
        if let WitnessChoice::FirstBy { keys } = pick {
            if keys.is_empty() {
                return Err(bad("pick first needs a sort key"));
            }
            let mut values = BTreeMap::new();
            for (inner, offset) in &pairs {
                let mut candidate = (**inner).clone();
                candidate
                    .fields
                    .insert("match.id".into(), Scalar::Text(inner.id.clone()));
                candidate
                    .fields
                    .insert("match.distance".into(), Scalar::Number(*offset as f64));
                let coarse = !matches!(within, Some(Distance::Steps { .. }))
                    && (outer.time.is_some_and(|time| time.coarse)
                        || (!matches!(at, TimePoint::MatchStart | TimePoint::MatchEnd)
                            && !(relation == OrderRelation::During && at == TimePoint::MatchWhen)
                            && inner.time.is_some_and(|time| time.coarse)));
                candidate
                    .fields
                    .insert("match.coarse".into(), Scalar::Bool(coarse));
                let row_values = keys
                    .iter()
                    .map(|key| self.expression(&candidate, &key.expression))
                    .collect::<Result<Vec<_>, _>>()?;
                values.insert(inner.id.clone(), row_values);
            }
            for index in 0..keys.len() {
                let known = values
                    .values()
                    .map(|row| &row[index])
                    .filter(|value| !value.is_unknown())
                    .collect::<Vec<_>>();
                for pair in known.windows(2) {
                    if pair[0].order(pair[1]).is_none() {
                        return Err(bad("sort requires compatible ordered scalars"));
                    }
                }
            }
            pairs.sort_by(|(a, a_offset), (b, b_offset)| {
                for (index, key) in keys.iter().enumerate() {
                    let left = &values[&a.id][index];
                    let right = &values[&b.id][index];
                    let known_order = match &key.expression {
                        Expression::Field { name } if name == "match.distance" => a_offset.cmp(b_offset),
                        Expression::Function { name, arguments } if name == "abs" && matches!(arguments.as_slice(), [Expression::Field { name }] if name == "match.distance") => a_offset.unsigned_abs().cmp(&b_offset.unsigned_abs()),
                        _ => left.order(right).unwrap_or(Ordering::Equal),
                    };
                    let order = match (left.is_unknown(), right.is_unknown()) {
                        (true,false) => Ordering::Greater,
                        (false,true) => Ordering::Less,
                        (true,true) => Ordering::Equal,
                        _ if key.descending => known_order.reverse(),
                        _ => known_order,
                    };
                    if order != Ordering::Equal { return order; }
                }
                a.id.cmp(&b.id)
            });
        }
        Ok(pairs.into_iter().next())
    }
    fn filter(&self, rows: Vec<Row>, c: &Condition) -> Result<Vec<Row>, Error> {
        let exposed = match c.exposed_order().map(|order| &order.kind) {
            Some(ConditionKind::Order {
                relation,
                query,
                within,
                at,
                pick,
                ..
            }) => Some((
                self.execute(query)?.rows,
                *relation,
                within.as_ref(),
                *at,
                pick,
            )),
            _ => None,
        };
        let mut selected = vec![];
        for mut row in rows {
            if !self.condition(&row, c)?.matches() {
                continue;
            }
            if let Some((inner, relation, within, at, pick)) = &exposed
                && let Some(pairs) = self.related(&row, inner, *relation, *within, *at)?
                && let Some((nearest, offset)) =
                    self.witness(&row, pairs, pick, (*relation, *within, *at))?
            {
                row.fields
                    .insert("match.id".into(), Scalar::Text(nearest.id.clone()));
                for (name, value) in &nearest.fields {
                    row.fields.insert(format!("match.{name}"), value.clone());
                }
                if within.is_some() {
                    row.fields.insert(
                        crate::catalog::MATCH_DISTANCE.into(),
                        Scalar::Number(offset as f64),
                    );
                }
            }
            selected.push(row);
        }
        Ok(selected)
    }
    /// One row per pair. A pair carries the outer row's fields, the inner
    /// row's as `match.F`, and for an order relation the signed `match.distance`.
    fn join(&self, outer: Vec<Row>, inner: &[Row], on: &JoinOn) -> Result<Vec<Row>, Error> {
        let mut out = Vec::new();
        for row in outer {
            let pairs: Vec<(&Row, Option<i128>)> = match on {
                JoinOn::Order {
                    relation,
                    within,
                    at,
                    ..
                } => self
                    .related(&row, inner, *relation, within.as_ref(), *at)?
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(other, offset)| (other, Some(offset)))
                    .collect(),
                JoinOn::Key { left, right } => {
                    let key = self.value(&row, left);
                    let mut matched = Vec::new();
                    if !key.is_unknown() {
                        for other in inner {
                            if compare(&key, Op::Eq, &self.value(other, right))?.matches() {
                                matched.push((other, None));
                            }
                        }
                    }
                    matched.sort_by(|a: &(&Row, Option<i128>), b| a.0.id.cmp(&b.0.id));
                    matched
                }
            };
            for (other, offset) in pairs {
                let mut pair = row.clone();
                pair.id = format!("{}:{}", row.id, other.id);
                pair.fields
                    .insert("match.id".into(), Scalar::Text(other.id.clone()));
                for (name, value) in &other.fields {
                    pair.fields.insert(format!("match.{name}"), value.clone());
                }
                if let Some(offset) = offset {
                    pair.fields.insert(
                        crate::catalog::MATCH_DISTANCE.into(),
                        Scalar::Number(offset as f64),
                    );
                }
                pair.matches = vec![other.id.clone()];
                out.push(pair);
            }
        }
        Ok(out)
    }
    /// The generic `matches` shape: one row per step of each match.
    fn matches(
        &self,
        rows: &[Row],
        pattern: Option<&Condition>,
        at: usize,
    ) -> Result<Vec<Row>, Error> {
        let mut found = Vec::new();
        if let Some(condition) = pattern {
            collect_patterns(condition, &mut found);
        }
        let [pattern] = found.as_slice() else {
            return Err(refusal(
                "matches needs exactly one top-level exists or seq",
                at,
            ));
        };
        let mut out = Vec::new();
        for row in rows {
            let (unit, matches): (&str, Vec<Vec<String>>) = match &pattern.kind {
                ConditionKind::Exists { unit, condition } => (
                    unit,
                    self.exists_matches(row, unit, condition.as_deref())?
                        .into_iter()
                        .map(|id| vec![id])
                        .collect(),
                ),
                ConditionKind::Sequence {
                    unit,
                    all,
                    steps,
                    gaps,
                } => (unit, self.sequence(row, unit, *all, steps, gaps)?),
                _ => unreachable!("collected patterns are exists or seq"),
            };
            let steps: BTreeMap<String, Row> = self
                .children(row, unit)?
                .into_iter()
                .map(|child| (child.id.clone(), child))
                .collect();
            for (number, ids) in matches.into_iter().enumerate() {
                for (position, id) in ids.into_iter().enumerate() {
                    let mut result =
                        Row::new(format!("{}:match:{}:{}", row.id, number + 1, position + 1));
                    result.parent = Some(row.id.clone());
                    for (name, value) in &row.fields {
                        result
                            .fields
                            .insert(format!("parent.{name}"), value.clone());
                    }
                    result
                        .fields
                        .insert("parent".into(), Scalar::Text(row.id.clone()));
                    result
                        .fields
                        .insert("match".into(), Scalar::Number((number + 1) as f64));
                    result
                        .fields
                        .insert("position".into(), Scalar::Number((position + 1) as f64));
                    result
                        .fields
                        .insert("step".into(), Scalar::Text(id.clone()));
                    if let Some(step) = steps.get(&id) {
                        for (name, value) in &step.fields {
                            result.fields.insert(format!("step.{name}"), value.clone());
                        }
                    }
                    result.matches = vec![id];
                    out.push(result);
                }
            }
        }
        Ok(out)
    }
    fn aggregate_value(&self, rows: &[Row], aggregate: &Aggregate) -> Result<Scalar, Error> {
        let argument = |index: usize| -> Result<Vec<Scalar>, Error> {
            match aggregate.arguments.get(index) {
                None => Ok(vec![Scalar::Unknown; rows.len()]),
                Some(e) => rows.iter().map(|r| self.expression(r, e)).collect(),
            }
        };
        let values = argument(0)?;
        let known: Vec<_> = values.iter().filter(|v| !v.is_unknown()).collect();
        Ok(match aggregate.function {
            AggregateFunction::Count => Scalar::Number(if aggregate.arguments.is_empty() {
                rows.len()
            } else {
                known.len()
            } as f64),
            AggregateFunction::Percentile(p) => nearest_rank(&values, p)?,
            AggregateFunction::CountDistinct => {
                let mut unique = vec![];
                for v in known {
                    if !unique.contains(&v) {
                        unique.push(v);
                    }
                }
                Scalar::Number(unique.len() as f64)
            }
            AggregateFunction::Sum | AggregateFunction::Avg => {
                if known.is_empty() {
                    Scalar::Unknown
                } else {
                    let mut total = 0.;
                    for v in &known {
                        let Scalar::Number(n) = v else {
                            return Err(bad("aggregate requires numbers"));
                        };
                        total += n;
                        if !total.is_finite() {
                            return Err(bad("aggregate requires a finite result"));
                        }
                    }
                    Scalar::Number(if aggregate.function == AggregateFunction::Avg {
                        total / known.len() as f64
                    } else {
                        total
                    })
                }
            }
            AggregateFunction::Min | AggregateFunction::Max => {
                let mut selected = Scalar::Unknown;
                for v in known {
                    if selected.is_unknown() {
                        selected = v.clone();
                    } else {
                        let order = v
                            .order(&selected)
                            .ok_or_else(|| bad("aggregate requires ordered compatible scalars"))?;
                        if (aggregate.function == AggregateFunction::Min && order == Ordering::Less)
                            || (aggregate.function == AggregateFunction::Max
                                && order == Ordering::Greater)
                        {
                            selected = v.clone();
                        }
                    }
                }
                selected
            }
            AggregateFunction::Covered => {
                let ends = argument(1)?;
                let mut intervals = Vec::new();
                for (start, end) in values.iter().zip(&ends) {
                    match (start, end) {
                        (Scalar::Number(s), Scalar::Number(e)) => {
                            if s.fract() != 0. || e.fract() != 0. {
                                return Err(bad("covered takes whole ticks"));
                            }
                            intervals.push((*s as i64, *e as i64));
                        }
                        (Scalar::Unknown, _) | (_, Scalar::Unknown) => {}
                        _ => return Err(bad("covered requires numbers")),
                    }
                }
                if intervals.is_empty() {
                    Scalar::Unknown
                } else {
                    Scalar::Number(covered(&intervals) as f64)
                }
            }
            AggregateFunction::Histogram | AggregateFunction::Correlation => {
                return Err(unsupported(
                    "histogram/correlation product representation",
                    0,
                ));
            }
        })
    }
}
/// The rows of `A | diff (B)`: rows that joined (from A) or moved (from A,
/// with both ranks) in A's order, then rows that left (from B) in B's order.
fn diff_rows(before: &[Row], after: Vec<Row>) -> Vec<Row> {
    let old: Vec<String> = before.iter().map(|r| r.id.clone()).collect();
    let new: Vec<String> = after.iter().map(|r| r.id.clone()).collect();
    let rows: BTreeMap<&str, &Row> = after
        .iter()
        .chain(before)
        .rev()
        .map(|row| (row.id.as_str(), row))
        .collect();
    crate::diff::changes(&old, &new)
        .into_iter()
        .map(|change| {
            let mut row = rows[change.key.as_str()].clone();
            row.fields
                .insert("change".into(), Scalar::Text(change.kind.name().into()));
            let rank =
                |rank: Option<usize>| rank.map_or(Scalar::Unknown, |r| Scalar::Number(r as f64));
            row.fields.insert("old_rank".into(), rank(change.old_rank));
            row.fields.insert("new_rank".into(), rank(change.new_rank));
            row
        })
        .collect()
}
fn query_contains_fuse(transforms: &[Transform]) -> bool {
    transforms
        .iter()
        .take_while(|transform| {
            matches!(
                transform.kind,
                TransformKind::Combine {
                    how: Combination::Union | Combination::Fuse,
                    ..
                }
            )
        })
        .any(|transform| {
            matches!(
                transform.kind,
                TransformKind::Combine {
                    how: Combination::Fuse,
                    ..
                }
            )
        })
}
fn ranked_rows(rows: &[Row]) -> Vec<crate::fusion::Entry<String>> {
    rows.iter()
        .map(|row| crate::fusion::Entry {
            key: row.id.clone(),
            recency: row.time.map(|time| time.start),
        })
        .collect()
}
/// Order rows by one fusion of the lists: ranked rows first, then the rows
/// no list ranks, in their existing order. Membership never changes.
fn apply_fusion(rows: Vec<Row>, lists: Vec<Vec<crate::fusion::Entry<String>>>) -> Vec<Row> {
    let present: BTreeSet<String> = rows.iter().map(|r| r.id.clone()).collect();
    let order: Vec<String> = crate::fusion::fuse(lists)
        .into_iter()
        .map(|fused| fused.key)
        .filter(|key| present.contains(key))
        .collect();
    let ranked: BTreeSet<&String> = order.iter().collect();
    let mut by_id: BTreeMap<String, Row> = BTreeMap::new();
    let mut unranked = Vec::new();
    for row in rows {
        if ranked.contains(&row.id) {
            by_id.insert(row.id.clone(), row);
        } else {
            unranked.push(row);
        }
    }
    order
        .iter()
        .filter_map(|key| by_id.remove(key))
        .chain(unranked)
        .collect()
}
fn collect_patterns<'a>(condition: &'a Condition, out: &mut Vec<&'a Condition>) {
    match &condition.kind {
        ConditionKind::And { conditions } => {
            conditions.iter().for_each(|c| collect_patterns(c, out))
        }
        ConditionKind::Exists { .. } | ConditionKind::Sequence { .. } => out.push(condition),
        _ => {}
    }
}
/// The column a membership subquery offers: its last single group key or projected column.
fn membership_column(query: &Query) -> Option<String> {
    query.transforms.iter().rev().find_map(|t| match &t.kind {
        TransformKind::Group { keys } if keys.len() == 1 => Some(keys[0].column()),
        TransformKind::Fields { columns } if columns.len() == 1 => Some(columns[0].column()),
        _ => None,
    })
}

fn validate_expression(e: &Expression, at: usize) -> Result<(), Error> {
    match e {
        Expression::Field { .. } => Ok(()),
        Expression::Value { value } => validate_value(value),
        Expression::Function { name, arguments }
            if matches!(name.as_str(), "len" | "abs" | "text") && arguments.len() == 1 =>
        {
            validate_expression(&arguments[0], at)
        }
        Expression::Function { name, arguments }
            if (name == "case_eq" && arguments.len() == 3)
                || (name == "glob" && arguments.len() == 2) =>
        {
            for argument in arguments {
                validate_expression(argument, at)?;
            }
            Ok(())
        }
        Expression::Function { name, .. } if name == "random" => Ok(()),
        Expression::Function { name, .. } => Err(unsupported(name, at)),
        Expression::Arithmetic { left, right, .. } => {
            validate_expression(left, at)?;
            validate_expression(right, at)
        }
    }
}
fn validate_value(v: &Value) -> Result<(), Error> {
    match v {
        Value::Parameter { .. } => Err(unsupported("unbound parameter", 0)),
        Value::Set { values } => {
            for value in values {
                validate_value(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}
fn validate_condition(c: &Condition) -> Result<(), Error> {
    let at = c.span.start;
    match &c.kind {
        ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
            for c in conditions {
                validate_condition(c)?;
            }
        }
        ConditionKind::Not { condition } => validate_condition(condition)?,
        ConditionKind::Match { value, .. } => validate_value(value)?,
        ConditionKind::Compare { left, right, .. } => {
            validate_expression(left, at)?;
            validate_expression(right, at)?;
        }
        ConditionKind::Between { left, from, to } => {
            validate_expression(left, at)?;
            validate_value(from)?;
            validate_value(to)?;
        }
        ConditionKind::In { left, query } => {
            validate_expression(left, at)?;
            validate_query(query)?;
        }
        ConditionKind::Order { query, pick, .. } => {
            validate_query(query)?;
            if let WitnessChoice::FirstBy { keys } = pick {
                if keys.is_empty() {
                    return Err(bad("pick first needs a sort key"));
                }
                for key in keys {
                    validate_expression(&key.expression, at)?;
                }
            }
        }
        ConditionKind::Exists { condition, .. } | ConditionKind::Count { condition, .. } => {
            if let Some(c) = condition {
                validate_condition(c)?;
            }
        }
        ConditionKind::Sequence { steps, .. } => {
            for c in steps {
                validate_condition(c)?;
            }
        }
        ConditionKind::Reference { .. } | ConditionKind::Rank { .. } => {}
        other => return Err(unsupported(&format!("condition {other:?}"), at)),
    }
    Ok(())
}

fn validate_scalar(value: &Scalar) -> Result<(), Error> {
    match value {
        Scalar::Number(n) if !n.is_finite() => {
            return Err(bad("expected a finite number"));
        }
        Scalar::List(values) => {
            for value in values {
                validate_scalar(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// Refuse every node a product must execute before reading any data, so an
/// empty relation never hides an unsupported nested query.
fn validate_query(query: &Query) -> Result<(), Error> {
    if !query.attach.is_empty() {
        return Err(unsupported("attachments", query.attach[0].span.start));
    }
    match &query.source {
        Source::Unit { condition, .. } => {
            if let Some(c) = condition {
                validate_condition(c)?;
            }
        }
        _ => return Err(unsupported("non-unit source", 0)),
    }
    for transform in &query.transforms {
        let at = transform.span.start;
        match &transform.kind {
            TransformKind::Where { condition } => validate_condition(condition)?,
            TransformKind::Group { keys } => {
                for key in keys {
                    validate_expression(&key.expression, at)?;
                }
            }
            TransformKind::Fields { columns } => {
                for column in columns {
                    validate_expression(&column.expression, at)?;
                }
            }
            TransformKind::Sort { keys } => {
                for key in keys {
                    validate_expression(&key.expression, at)?;
                }
            }
            TransformKind::Aggregate { aggregates } => {
                for aggregate in aggregates {
                    if matches!(
                        aggregate.function,
                        AggregateFunction::Histogram | AggregateFunction::Correlation
                    ) {
                        return Err(unsupported(
                            "histogram/correlation product representation",
                            at,
                        ));
                    }
                    for argument in &aggregate.arguments {
                        validate_expression(argument, at)?;
                    }
                }
            }
            TransformKind::Combine { query, .. }
            | TransformKind::Join { query, .. }
            | TransformKind::Diff { query } => validate_query(query)?,
            TransformKind::Limit { .. }
            | TransformKind::First { .. }
            | TransformKind::Offset { .. }
            | TransformKind::Distinct
            | TransformKind::Matches => {}
            other => return Err(unsupported(&format!("transform {other:?}"), at)),
        }
    }
    Ok(())
}
