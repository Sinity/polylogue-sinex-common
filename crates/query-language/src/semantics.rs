//! Three-valued logic and a pure reference interpreter over caller-owned rows.
//!
//! This interpreter is an executable oracle for the shared relational mechanics.
//! Product storage, text ranking, civil-time functions, saved definitions, coverage,
//! grants and extensions require product adapters and are refused here. Missing
//! fields are unknown. Related rows explicitly name their parent, sequence order
//! and temporal extent; enumeration order has no meaning.
use crate::ast::*;
use crate::{Error, ErrorKind};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Truth {
    False,
    Unknown,
    True,
}
impl Truth {
    pub fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::False, _) | (_, Self::False) => Self::False,
            (Self::True, Self::True) => Self::True,
            _ => Self::Unknown,
        }
    }
    pub fn or(self, other: Self) -> Self {
        match (self, other) {
            (Self::True, _) | (_, Self::True) => Self::True,
            (Self::False, Self::False) => Self::False,
            _ => Self::Unknown,
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn not(self) -> Self {
        match self {
            Self::True => Self::False,
            Self::False => Self::True,
            Self::Unknown => Self::Unknown,
        }
    }
    pub fn matches(self) -> bool {
        self == Self::True
    }
}
impl From<bool> for Truth {
    fn from(value: bool) -> Self {
        if value { Self::True } else { Self::False }
    }
}
impl std::ops::Not for Truth {
    type Output = Self;
    fn not(self) -> Self {
        Truth::not(self)
    }
}

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
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: String,
    pub fields: BTreeMap<String, Scalar>,
    pub parent: Option<String>,
    pub order: i64,
    pub time: Option<Extent>,
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
fn operand(row: &Row, o: &Operand) -> Result<Scalar, Error> {
    match o {
        Operand::Field { name } => Ok(row.value(name)),
        Operand::Function { name, .. } => Err(unsupported(name, 0)),
    }
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
                    .ok_or_else(|| {
                        Error::new(ErrorKind::BadValue, "expected a finite number", 0)
                    })?,
            ),
            Scalar::Bool(_) => Scalar::Bool(
                text.parse()
                    .map_err(|_| Error::new(ErrorKind::BadValue, "expected a boolean", 0))?,
            ),
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
/// Lists match any member; inequality excludes every equal member. Unknown
/// operands retain unknown, including under inequality and negation.
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
            return Ok(a.contains(b).into());
        }
        if op == Op::Prefix {
            return Ok(a.starts_with(b).into());
        }
    }
    let order = left.order(right).ok_or_else(|| {
        Error::new(
            ErrorKind::BadValue,
            "comparison requires compatible scalar types",
            0,
        )
    })?;
    Ok(match op {
        Op::Eq => order == Ordering::Equal,
        Op::Ne => order != Ordering::Equal,
        Op::Lt => order == Ordering::Less,
        Op::Le => order != Ordering::Greater,
        Op::Gt => order == Ordering::Greater,
        Op::Ge => order != Ordering::Less,
        _ => {
            return Err(Error::new(
                ErrorKind::BadValue,
                "text operator requires text",
                0,
            ));
        }
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
/// Nearest-rank uses ceil(p*n/100), with percentile zero selecting the minimum.
/// Unknown observations are omitted; no known observations yields unknown.
pub fn nearest_rank(values: &[Scalar], percentile: u8) -> Result<Scalar, Error> {
    if percentile > 100 {
        return Err(Error::new(
            ErrorKind::BadValue,
            "percentile must be between 0 and 100",
            0,
        ));
    }
    if values
        .iter()
        .any(|v| matches!(v,Scalar::Number(n) if !n.is_finite()))
    {
        return Err(Error::new(
            ErrorKind::BadValue,
            "percentile requires finite numbers",
            0,
        ));
    }
    let mut known: Vec<_> = values.iter().filter(|v| !v.is_unknown()).cloned().collect();
    for pair in known.windows(2) {
        if pair[0].order(&pair[1]).is_none() {
            return Err(Error::new(
                ErrorKind::BadValue,
                "percentile requires compatible ordered scalars",
                0,
            ));
        }
    }
    known.sort_by(|a, b| a.order(b).unwrap_or(Ordering::Equal));
    if known.is_empty() {
        return Ok(Scalar::Unknown);
    }
    let rank = (usize::from(percentile) * known.len()).div_ceil(100).max(1);
    Ok(known[rank - 1].clone())
}

pub struct ReferenceEvaluator<'a> {
    pub data: &'a Dataset,
}
impl<'a> ReferenceEvaluator<'a> {
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
                let left = row.value(field);
                match value {
                    Value::Any => Truth::True,
                    Value::Unknown => left.is_unknown().into(),
                    _ => compare(&left, Op::Eq, &literal(value, &left)?)?,
                }
            }
            ConditionKind::Compare {
                operand: o,
                op,
                value,
            } => {
                let left = operand(row, o)?;
                compare(&left, *op, &literal(value, &left)?)?
            }
            ConditionKind::Between {
                operand: o,
                from,
                to,
            } => {
                let left = operand(row, o)?;
                compare(&left, Op::Ge, &literal(from, &left)?)?.and(compare(
                    &left,
                    Op::Le,
                    &literal(to, &left)?,
                )?)
            }
            ConditionKind::FieldExists { operand: o } => (!operand(row, o)?.is_unknown()).into(),
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
            ConditionKind::In { operand: o, query } => {
                let left = operand(row, o)?;
                if left.is_unknown() {
                    Truth::Unknown
                } else {
                    let answer = self.execute(query)?;
                    let column = query
                        .stages
                        .iter()
                        .rev()
                        .find_map(|s| match &s.kind {
                            StageKind::Group { keys } if keys.len() == 1 => Some(keys[0].column()),
                            _ => None,
                        })
                        .ok_or_else(|| {
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
            ConditionKind::Sequence {
                unit,
                all,
                steps,
                gaps,
            } => (!self.sequence(row, unit, *all, steps, gaps)?.is_empty()).into(),
            other => {
                return Err(unsupported(
                    &format!("condition {other:?}"),
                    condition.span.start,
                ));
            }
        };
        Ok(truth)
    }
    /// Select a witness independently of input enumeration. Unknown children do
    /// not become witnesses; existence itself still preserves their truth value.
    pub fn exists_witness(
        &self,
        row: &Row,
        unit: &str,
        condition: Option<&Condition>,
    ) -> Result<Option<String>, Error> {
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
        Ok(selected.into_iter().next())
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
                    Gap::Within { duration } => {
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
        let mut groups: Option<Vec<Vec<Row>>> = None;
        for stage in &query.stages {
            match &stage.kind {
                StageKind::Where { condition } => {
                    rows = self.filter(rows, condition)?;
                    groups = None;
                }
                StageKind::Sort { keys } => {
                    let decorated: Vec<_> = rows
                        .iter()
                        .map(|r| {
                            keys.iter()
                                .map(|k| operand(r, &k.operand))
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
                                    stage.span.start,
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
                    groups = None;
                }
                StageKind::First { .. } if groups.is_some() => {
                    return Err(unsupported("per-group first", stage.span.start));
                }
                StageKind::Limit { count } | StageKind::First { count } => {
                    rows.truncate(usize::try_from(*count).unwrap_or(usize::MAX));
                    groups = None;
                }
                StageKind::Offset { count } => {
                    rows = rows
                        .into_iter()
                        .skip(usize::try_from(*count).unwrap_or(usize::MAX))
                        .collect();
                    groups = None;
                }
                StageKind::Group { keys } => {
                    let mut grouped: Vec<(Vec<Scalar>, Vec<Row>)> = vec![];
                    for row in rows {
                        let values = keys
                            .iter()
                            .map(|k| operand(&row, k))
                            .collect::<Result<Vec<_>, _>>()?;
                        if let Some((_, members)) = grouped.iter_mut().find(|(v, _)| *v == values) {
                            members.push(row);
                        } else {
                            grouped.push((values, vec![row]));
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
                    groups = Some(members);
                }
                StageKind::Aggregate { aggregates } => {
                    let grouped = groups.is_some();
                    let members = groups.take().unwrap_or_else(|| vec![rows.clone()]);
                    if !grouped {
                        rows = vec![Row::new("aggregate")];
                    }
                    for (row, items) in rows.iter_mut().zip(members) {
                        for aggregate in aggregates {
                            row.fields
                                .insert(aggregate.column(), aggregate_value(&items, aggregate)?);
                            for field in &aggregate.fields {
                                row.fields.insert(
                                    format!("unknown_{}", field.replace(['.', '-'], "_")),
                                    Scalar::Number(
                                        items.iter().filter(|r| r.value(field).is_unknown()).count()
                                            as f64,
                                    ),
                                );
                            }
                        }
                    }
                }
                StageKind::Fields { fields } => {
                    for row in &mut rows {
                        row.fields.retain(|key, _| fields.contains(key));
                    }
                    groups = None;
                }
                StageKind::Distinct => {
                    let mut seen = BTreeSet::new();
                    rows.retain(|r| seen.insert(r.id.clone()));
                    groups = None;
                }
                StageKind::Combine { how, query } => {
                    let other = self.execute(query)?.rows;
                    let ids: BTreeSet<_> = other.iter().map(|r| r.id.clone()).collect();
                    match how {
                        Combination::Union => {
                            rows.extend(other);
                            let mut seen = BTreeSet::new();
                            rows.retain(|r| seen.insert(r.id.clone()));
                            rows.sort_by(|a, b| a.id.cmp(&b.id));
                        }
                        Combination::Intersect => rows.retain(|r| ids.contains(&r.id)),
                        Combination::Except => rows.retain(|r| !ids.contains(&r.id)),
                        Combination::Fuse => {
                            return Err(unsupported("rank fusion", stage.span.start));
                        }
                    }
                    groups = None;
                }
                StageKind::Temporal {
                    relation,
                    query,
                    at,
                    within,
                    all,
                } => {
                    let inner = self.execute(query)?.rows;
                    rows = self.temporal(rows, &inner, *relation, *at, within.as_ref(), *all)?;
                    groups = None;
                }
                StageKind::Matches => {
                    let c = condition.as_ref().and_then(find_sequence).ok_or_else(|| {
                        refusal("matches requires a sequence condition", stage.span.start)
                    })?;
                    let ConditionKind::Sequence {
                        unit,
                        all,
                        steps,
                        gaps,
                    } = &c.kind
                    else {
                        unreachable!()
                    };
                    let mut projected = vec![];
                    for row in &rows {
                        for (index, ids) in self
                            .sequence(row, unit, *all, steps, gaps)?
                            .into_iter()
                            .enumerate()
                        {
                            let mut result = Row::new(format!("{}:match:{}", row.id, index + 1));
                            result.parent = Some(row.id.clone());
                            for (step, id) in ids.iter().enumerate() {
                                result.fields.insert(
                                    format!("step{}_id", step + 1),
                                    Scalar::Text(id.clone()),
                                );
                            }
                            result.matches = ids;
                            projected.push(result);
                        }
                    }
                    rows = projected;
                    groups = None;
                }
                other => return Err(unsupported(&format!("stage {other:?}"), stage.span.start)),
            }
        }
        Ok(Answer {
            total: rows.len(),
            rows,
        })
    }
    fn filter(&self, rows: Vec<Row>, c: &Condition) -> Result<Vec<Row>, Error> {
        let mut selected = vec![];
        for row in rows {
            if self.condition(&row, c)?.matches() {
                selected.push(row);
            }
        }
        Ok(selected)
    }
    fn temporal(
        &self,
        outer: Vec<Row>,
        inner: &[Row],
        relation: TemporalRelation,
        at: TimePoint,
        within: Option<&Value>,
        all: bool,
    ) -> Result<Vec<Row>, Error> {
        let window = within
            .map(|v| {
                v.text()
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| unsupported("resolved exact-tick temporal duration", 0))
            })
            .transpose()?;
        let mut output = vec![];
        for row in outer {
            let Some(a) = row.time else { continue };
            let point = match at {
                TimePoint::End => a.end,
                _ => a.start,
            };
            let mut candidates = vec![];
            for other in inner {
                let Some(b) = other.time else { continue };
                let distance = (i128::from(point) - i128::from(b.start)).unsigned_abs();
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
                if (a.coarse || b.coarse)
                    && window.is_none_or(|w| {
                        u128::from(w)
                            < (i128::from(a.end) - i128::from(a.start))
                                .unsigned_abs()
                                .max((i128::from(b.end) - i128::from(b.start)).unsigned_abs())
                    })
                {
                    continue;
                }
                let matches = match relation {
                    TemporalRelation::Near => true,
                    TemporalRelation::Before => point < b.start,
                    TemporalRelation::After => point > b.start,
                    TemporalRelation::During => {
                        if a.coarse {
                            a.start >= b.start && a.end <= b.end
                        } else {
                            point >= b.start && point < b.end
                        }
                    }
                    TemporalRelation::Containing => b.start >= a.start && b.end <= a.end,
                };
                if matches {
                    candidates.push((b.coarse || a.coarse, distance, &other.id));
                }
            }
            candidates.sort();
            if !all {
                candidates.truncate(1);
            }
            for (_, _, id) in candidates {
                let mut result = row.clone();
                result.matches.push(id.clone());
                if all {
                    result.id = format!("{}:{id}", row.id);
                }
                output.push(result);
            }
        }
        Ok(output)
    }
}
fn aggregate_value(rows: &[Row], aggregate: &Aggregate) -> Result<Scalar, Error> {
    let field = aggregate.fields.first();
    let values: Vec<_> = rows
        .iter()
        .map(|r| field.map_or(Scalar::Unknown, |f| r.value(f)))
        .collect();
    let known: Vec<_> = values.iter().filter(|v| !v.is_unknown()).collect();
    Ok(match aggregate.function {
        AggregateFunction::Count => Scalar::Number(if field.is_some() {
            known.len()
        } else {
            rows.len()
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
                        return Err(Error::new(
                            ErrorKind::BadValue,
                            "aggregate requires numbers",
                            0,
                        ));
                    };
                    total += n;
                    if !total.is_finite() {
                        return Err(Error::new(
                            ErrorKind::BadValue,
                            "aggregate requires a finite result",
                            0,
                        ));
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
                    let order = v.order(&selected).ok_or_else(|| {
                        Error::new(
                            ErrorKind::BadValue,
                            "aggregate requires ordered compatible scalars",
                            0,
                        )
                    })?;
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
        _ => {
            return Err(unsupported(
                "histogram/correlation product representation",
                0,
            ));
        }
    })
}

fn find_sequence(condition: &Condition) -> Option<&Condition> {
    match &condition.kind {
        ConditionKind::Sequence { .. } => Some(condition),
        ConditionKind::And { conditions } => conditions.iter().find_map(find_sequence),
        _ => None,
    }
}

fn validate_operand(o: &Operand) -> Result<(), Error> {
    match o {
        Operand::Field { .. } => Ok(()),
        Operand::Function { name, .. } => Err(unsupported(name, 0)),
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
    match &c.kind {
        ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
            for c in conditions {
                validate_condition(c)?;
            }
        }
        ConditionKind::Not { condition } => validate_condition(condition)?,
        ConditionKind::Match { value, .. } => validate_value(value)?,
        ConditionKind::Compare { operand, value, .. } => {
            validate_operand(operand)?;
            validate_value(value)?;
        }
        ConditionKind::Between { operand, from, to } => {
            validate_operand(operand)?;
            validate_value(from)?;
            validate_value(to)?;
        }
        ConditionKind::FieldExists { operand } => validate_operand(operand)?,
        ConditionKind::In { operand, query } => {
            validate_operand(operand)?;
            validate_query(query)?;
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
        ConditionKind::Reference { .. } => {}
        other => return Err(unsupported(&format!("condition {other:?}"), c.span.start)),
    }
    Ok(())
}

fn validate_scalar(value: &Scalar) -> Result<(), Error> {
    match value {
        Scalar::Number(n) if !n.is_finite() => {
            return Err(Error::new(
                ErrorKind::BadValue,
                "expected a finite number",
                0,
            ));
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
    for stage in &query.stages {
        match &stage.kind {
            StageKind::Where { condition } => validate_condition(condition)?,
            StageKind::Group { keys } => {
                for key in keys {
                    validate_operand(key)?;
                }
            }
            StageKind::Sort { keys } => {
                for key in keys {
                    validate_operand(&key.operand)?;
                }
            }
            StageKind::Aggregate { aggregates } => {
                if aggregates.iter().any(|a| {
                    matches!(
                        a.function,
                        AggregateFunction::Histogram | AggregateFunction::Correlation
                    )
                }) {
                    return Err(unsupported(
                        "histogram/correlation product representation",
                        stage.span.start,
                    ));
                }
            }
            StageKind::Combine { query, .. } | StageKind::Temporal { query, .. } => {
                validate_query(query)?
            }
            StageKind::Limit { .. }
            | StageKind::First { .. }
            | StageKind::Offset { .. }
            | StageKind::Fields { .. }
            | StageKind::Distinct
            | StageKind::Matches => {}
            other => return Err(unsupported(&format!("stage {other:?}"), stage.span.start)),
        }
    }
    Ok(())
}
