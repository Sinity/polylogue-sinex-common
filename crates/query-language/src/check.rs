//! The one type checker over the syntax tree. It owns the row scope a query
//! reads at each point, the rules that check a field, a value, a comparison,
//! a function call or a transform against that scope, and the scope each
//! transform leaves behind.
//!
//! The parser calls these rules on each node as soon as it has built it,
//! passing the byte position of the token the node came from, so its
//! refusals keep their place in the text. Binding walks a finished tree with
//! the same rules (`walk.rs`), placing a refusal at the start of the
//! condition, transform or attachment that holds it, since a tree carries
//! spans only on those nodes.
use crate::ast::*;
use crate::catalog::{Catalog, Field, FieldKind, OutputSchema};
use crate::parse::{closest, is_duration, is_literal, kind_name, patterns};
use crate::scope::Origin;
use crate::{Error, ErrorKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub(crate) mod ranking;
pub(crate) mod typing;
mod walk;

/// The rows a query reads at one point, and the fields it shows there.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    pub unit: String,
    // Native field ownership survives grouping; the resulting row domain does not.
    pub row_domain: Option<String>,
    /// The visible columns after a grouping, projection or aggregate; None
    /// means every field of the unit or output, plus `extra`.
    pub columns: Option<BTreeSet<String>>,
    /// Generated names visible beside the unit's own fields.
    pub extra: BTreeSet<String>,
    pub bucketed: bool,
    pub output: Option<Arc<OutputSchema>>,
    pub columns_unknown: bool,
    pub typed_extra: Arc<Vec<Field>>,
    pub origins: Arc<BTreeMap<String, Origin>>,
    pub group_input: Option<Arc<Scope>>,
    pub output_source: Option<crate::catalog::OutputSource>,
    /// The step units of the relation's top-level `exists` and `seq`
    /// patterns, which `matches` expands; None when a saved or kept source
    /// hides them.
    pub patterns: Option<Vec<String>>,
}
impl Scope {
    pub fn new(unit: String) -> Self {
        Self {
            row_domain: Some(unit.clone()),
            unit,
            columns: None,
            extra: BTreeSet::new(),
            bucketed: false,
            output: None,
            columns_unknown: false,
            typed_extra: Arc::new(Vec::new()),
            origins: Arc::new(BTreeMap::new()),
            group_input: None,
            output_source: None,
            patterns: Some(Vec::new()),
        }
    }
}
/// Where an expression stands: the left side of a condition holds fields and
/// functions; a column or aggregate argument may also hold literals; inside a
/// function call a word that is not a field is a value (`month_day(today)`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    Condition,
    Column,
    Argument,
}

pub(crate) struct Checker<'a> {
    pub catalog: &'a Catalog,
    /// The text being parsed; a refusal of a tree without text carries no
    /// corrected query.
    text: Option<&'a str>,
    pub captures: Vec<Option<typing::Capture>>,
    pub query_stack: Vec<(usize, usize)>,
    pub capture_types: bool,
    /// The final scope of each order relation's inner query, by
    /// [`Checker::order_key`].
    pub inner_scopes: BTreeMap<usize, Scope>,
    /// A tree built elsewhere may repeat spans, so binding keys order
    /// relations by node address; the tree stays in place from the walk to
    /// the typing projection.
    key_by_address: bool,
}

impl<'a> Checker<'a> {
    pub fn for_text(catalog: &'a Catalog, text: &'a str, capture_types: bool) -> Self {
        Self {
            catalog,
            text: Some(text),
            captures: Vec::new(),
            query_stack: Vec::new(),
            capture_types,
            inner_scopes: BTreeMap::new(),
            key_by_address: false,
        }
    }
    pub fn for_tree(catalog: &'a Catalog) -> Self {
        Self {
            catalog,
            text: None,
            captures: Vec::new(),
            query_stack: Vec::new(),
            capture_types: true,
            inner_scopes: BTreeMap::new(),
            key_by_address: true,
        }
    }
    /// The key of an order relation's inner scope: its byte offset in parsed
    /// text, or its address in a bound tree.
    pub fn order_key(&self, condition: &Condition) -> usize {
        if self.key_by_address {
            std::ptr::from_ref(condition) as usize
        } else {
            condition.span.start
        }
    }
    /// The text with `span` replaced, when there is text.
    pub fn replace(&self, span: Span, value: &str) -> Option<String> {
        let text = self.text?;
        Some(format!(
            "{}{}{}",
            &text[..span.start],
            value,
            &text[span.end..]
        ))
    }
    /// A refusal naming the known names nearest to an unknown one; the nearest
    /// becomes the suggestion.
    pub fn near_miss(
        &self,
        word: &str,
        span: Span,
        message: String,
        kind: ErrorKind,
        names: Vec<&str>,
    ) -> Error {
        let nearest = closest(word, names.into_iter());
        let mut e = Error::new(kind, message, span.start).length(span.end - span.start);
        if let Some(fix) = nearest.first()
            && let Some(corrected) = self.replace(span, fix)
        {
            e = e.suggestion(corrected);
        }
        e.candidates(nearest)
    }
    pub fn unknown_unit(&self, word: &str, span: Span) -> Error {
        let names: Vec<&str> = self
            .catalog
            .units
            .iter()
            .map(|unit| unit.name.as_str())
            .collect();
        self.near_miss(
            word,
            span,
            format!("`{word}` is not a unit"),
            ErrorKind::InvalidQuery,
            names,
        )
    }

    /// The scope a relation starts in: an omitted nested source inherits the
    /// row domain, rather than the enclosing grouping/projection's visible
    /// output columns; a top-level one reads the default unit.
    pub fn relation_scope(&self, inherited: Option<&Scope>) -> Scope {
        if let Some(parent) = inherited {
            let mut rows = Scope::new(parent.unit.clone());
            rows.columns_unknown = self.catalog.unit(&parent.unit).is_none();
            rows
        } else {
            Scope::new(self.catalog.default_unit.clone())
        }
    }
    pub fn output_scope(&self, source: &mut Source, scope: &mut Scope) {
        scope.unit = match source {
            Source::Saved { name, .. } => format!("query:{name}"),
            Source::Result { reference, .. } => reference.clone(),
            _ => return,
        };
        scope.output_source = match source {
            Source::Saved {
                name, arguments, ..
            } => Some(crate::catalog::OutputSource::Saved {
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            Source::Result {
                reference, part, ..
            } => Some(crate::catalog::OutputSource::Result {
                reference: reference.clone(),
                part: *part,
            }),
            _ => None,
        };
        scope.output = self.catalog.source_output(source).cloned().map(Arc::new);
        scope.row_domain = scope.output.as_ref().and_then(|output| output.unit.clone());
        if let Some(unit) = scope
            .output
            .as_ref()
            .and_then(|output| output.unit.as_ref())
        {
            scope.unit = unit.clone();
        }
        scope.columns_unknown = scope.output.is_none();
        scope.columns = scope
            .output
            .as_ref()
            .map(|output| output.fields.iter().map(|f| f.name.clone()).collect());
        if let Source::Saved { output, .. } | Source::Result { output, .. } = source {
            *output = if scope.columns_unknown {
                SourceColumns::Unknown
            } else {
                SourceColumns::Known
            };
        }
    }
    /// The step patterns a relation's source exposes to `matches`.
    pub fn source_patterns(&self, source: &Source) -> Option<Vec<String>> {
        match source {
            Source::Unit { condition, .. } => Some(patterns(condition.as_ref())),
            Source::Saved { condition, .. } => self
                .catalog
                .source_output(source)
                .and_then(|output| output.match_units.clone())
                .map(|mut inherited| {
                    inherited.extend(patterns(condition.as_ref()));
                    inherited
                }),
            Source::Result { .. } => None,
        }
    }
    pub fn scope_field<'s>(&'s self, scope: &'s Scope, name: &str) -> Option<&'s Field> {
        scope
            .typed_extra
            .iter()
            .find(|field| field.name == name)
            .or_else(|| {
                if let Some(output) = &scope.output {
                    output.fields.iter().find(|field| field.name == name)
                } else if scope.columns_unknown {
                    None
                } else {
                    self.catalog.field(&scope.unit, name)
                }
            })
    }
    pub fn main_time(&self, scope: &Scope) -> Option<String> {
        if let Some(output) = &scope.output {
            output.main_time.clone()
        } else if scope.columns_unknown {
            Some("$unknown_main_time".into())
        } else if scope.row_domain.is_none() {
            None
        } else {
            self.catalog
                .unit(&scope.unit)
                .and_then(|unit| unit.main_time.clone())
        }
    }
    /// `since:` and `until:` bound the scope's main time.
    pub fn time_bound(
        &self,
        scope: &Scope,
        until: bool,
        value: Value,
        position: usize,
    ) -> Result<ConditionKind, Error> {
        let field = self.main_time(scope).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidQuery,
                "this unit has no main time",
                position,
            )
        })?;
        Ok(if until {
            ConditionKind::Until { field, value }
        } else {
            ConditionKind::Since { field, value }
        })
    }
    /// The unit a `seq` inside `scope` steps through.
    pub fn sequence_unit(&self, scope: &Scope) -> String {
        self.catalog
            .unit(&scope.unit)
            .and_then(|unit| unit.sequence_unit.clone())
            .unwrap_or_else(|| scope.unit.clone())
    }
    pub fn field_known(&self, scope: &Scope, field: &str) -> bool {
        if (scope.columns_unknown && scope.columns.is_none()) || scope.extra.contains(field) {
            return true;
        }
        scope.columns.as_ref().map_or_else(
            || self.scope_field(scope, field).is_some(),
            |c| c.contains(field),
        )
    }
    /// A word that is not a field reads as a value inside a function call,
    /// and anywhere when it is a literal.
    pub fn reads_as_value(&self, scope: &Scope, word: &str, place: Place) -> bool {
        !self.field_known(scope, word) && (place == Place::Argument || is_literal(word))
    }
    /// A word the catalog declares a reference reads as that reference;
    /// `field:value` does so only when the scope has no such field or the
    /// reference form overrides it.
    pub fn reads_as_reference(&self, scope: &Scope, word: &str) -> bool {
        self.catalog.is_reference(word)
            && word.split_once(':').is_none_or(|(field, _)| {
                self.scope_field(scope, field).is_none()
                    || self.catalog.reference_overrides_field(word)
            })
    }
    pub fn check_field(&self, scope: &Scope, field: &str, position: usize) -> Result<(), Error> {
        if self.field_known(scope, field) {
            return Ok(());
        }
        let names: Vec<String> = if let Some(columns) = &scope.columns {
            columns.iter().cloned().collect()
        } else {
            self.catalog
                .unit(&scope.unit)
                .into_iter()
                .flat_map(|u| u.fields.iter().map(|f| f.name.clone()))
                .chain(scope.extra.iter().cloned())
                .collect()
        };
        let message = format!(
            "unknown {}field `{field}` on `{}`; fields: {}",
            if scope.columns.is_some() {
                "grouped "
            } else {
                ""
            },
            scope.unit,
            names.join(", ")
        );
        Err(self.near_miss(
            field,
            Span {
                start: position,
                end: position + field.len(),
            },
            message,
            ErrorKind::UnknownField,
            names.iter().map(String::as_str).collect(),
        ))
    }
    pub fn normalize_expression_value(&self, scope: &Scope, left: &Expression, value: &mut Value) {
        if let Expression::Field { name } = left {
            self.normalize_field_value(scope, name, value);
        }
    }
    pub fn normalize_field_value(&self, scope: &Scope, field: &str, value: &mut Value) {
        crate::depth::deeper(|| {
            if let Value::Set { values } = value {
                for value in values {
                    self.normalize_field_value(scope, field, value);
                }
                return;
            }
            if let Some(spec) = self.scope_field(scope, field)
                && let crate::catalog::UnknownRule::Word(word) = &spec.unknown
                && matches!(value,Value::Scalar{text,quoted:false} if text.eq_ignore_ascii_case(word))
            {
                *value = Value::Unknown;
            }
            if let Some(spec) = self.scope_field(scope, field)
                && let Value::Scalar { text, .. } = value
                && let Some(canonical) = spec
                    .values
                    .iter()
                    .find(|candidate| candidate.eq_ignore_ascii_case(text))
            {
                *text = canonical.clone();
            }
        })
    }
    pub fn check_value(
        &self,
        scope: &Scope,
        field: &str,
        value: &Value,
        position: usize,
    ) -> Result<(), Error> {
        crate::depth::deeper(|| {
            if let Value::Set { values } = value {
                for v in values {
                    self.check_value(scope, field, v, position)?;
                }
                return Ok(());
            }
            let Value::Scalar { text, .. } = value else {
                return Ok(());
            };
            let Some(spec) = self.scope_field(scope, field) else {
                return Ok(());
            };
            if !spec.values.is_empty() && !spec.values.iter().any(|v| v.eq_ignore_ascii_case(text))
            {
                let elsewhere = self.catalog.units_accepting(text);
                let mut message = format!(
                    "`{field}` on `{}` is one of: {}",
                    scope.unit,
                    spec.values.join(", ")
                );
                if !elsewhere.is_empty() {
                    message.push_str(&format!(
                        "; `{text}` is a value of {}",
                        elsewhere.join(", ")
                    ));
                }
                return Err(Error::new(ErrorKind::BadValue, message, position)
                    .candidates(spec.values.clone()));
            }
            let valid = match spec.kind {
                FieldKind::Integer => crate::number::is_integer(text),
                FieldKind::Number => crate::number::is_decimal(text),
                FieldKind::Bool => matches!(text.as_str(), "true" | "false"),
                _ => true,
            };
            if valid {
                Ok(())
            } else {
                Err(Error::new(
                    ErrorKind::BadValue,
                    format!("`{text}` is not a {:?} value for `{field}`", spec.kind),
                    position,
                ))
            }
        })
    }
    pub fn check_expression_value(
        &self,
        scope: &Scope,
        left: &Expression,
        value: &Value,
        position: usize,
    ) -> Result<(), Error> {
        if let Expression::Field { name } = left {
            return self.check_value(scope, name, value, position);
        }
        let left_kind = self.expression_info(scope, left).field.kind;
        let values = match value {
            Value::Set { values } => values.as_slice(),
            other => std::slice::from_ref(other),
        };
        for value in values {
            if matches!(value, Value::Scalar { .. }) {
                let right_kind =
                    typing::operand_kind(&Expression::value(value.clone()), &left_kind);
                self.check_comparable(&left_kind, &right_kind, position)?;
            }
        }
        Ok(())
    }
    /// Refuse a comparison whose two sides have different dimensions:
    /// `cost * 2 > 1h` compares money with a duration.
    pub fn check_comparable(
        &self,
        left: &FieldKind,
        right: &FieldKind,
        position: usize,
    ) -> Result<(), Error> {
        if typing::comparable(left, right) {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::InvalidQuery,
                format!(
                    "cannot compare {} with {}",
                    kind_name(left),
                    kind_name(right)
                ),
                position,
            ))
        }
    }
    /// `field:value`: the field is known, the value takes its canonical
    /// spelling and is one the field accepts.
    pub fn check_match(
        &self,
        scope: &Scope,
        field: &str,
        value: &mut Value,
        start: usize,
    ) -> Result<(), Error> {
        self.check_field(scope, field, start)?;
        self.normalize_field_value(scope, field, value);
        self.check_value(scope, field, value, start + field.len() + 1)
    }
    /// `left between from and to`.
    pub fn check_between(
        &self,
        scope: &Scope,
        left: &Expression,
        from: &mut Value,
        to: &mut Value,
        start: usize,
    ) -> Result<(), Error> {
        self.normalize_expression_value(scope, left, from);
        self.normalize_expression_value(scope, left, to);
        self.check_expression_value(scope, left, from, start)?;
        self.check_expression_value(scope, left, to, start)
    }
    /// `left OP value`, the comparison `span` covers.
    pub fn check_compare_value(
        &self,
        scope: &Scope,
        left: &Expression,
        op: Op,
        value: &mut Value,
        span: Span,
    ) -> Result<(), Error> {
        self.normalize_expression_value(scope, left, value);
        if let Expression::Field { name } = left
            && self
                .scope_field(scope, name)
                .is_some_and(|f| f.kind == FieldKind::Time)
            && matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge)
            && matches!(&*value, Value::Scalar { text, quoted: false } if is_duration(text))
        {
            let duration = value.text().unwrap();
            let error = Error::new(
                ErrorKind::BadValue,
                "a bare duration is not a time instant; use since or a relative range",
                span.start,
            );
            return Err(
                match self.replace(span, &format!("{name} in last {duration}")) {
                    Some(corrected) => error.suggestion(corrected),
                    None => error,
                },
            );
        }
        self.check_expression_value(scope, left, value, span.start)
    }
    /// `left OP right` where the right side is an expression.
    pub fn check_compare_expressions(
        &self,
        scope: &Scope,
        left: &Expression,
        right: &Expression,
        position: usize,
    ) -> Result<(), Error> {
        self.check_comparable(
            &self.expression_info(scope, left).field.kind,
            &self.expression_info(scope, right).field.kind,
            position,
        )
    }
    /// `left OPERATOR right` combines kinds the operator accepts.
    pub fn check_arithmetic(
        &self,
        scope: &Scope,
        operator: Arithmetic,
        left: &Expression,
        right: &Expression,
        at: usize,
    ) -> Result<(), Error> {
        let l = self.expression_info(scope, left).field.kind;
        let r = self.expression_info(scope, right).field.kind;
        let (l, r) = (
            typing::operand_kind_in(operator, left, &l, &r),
            typing::operand_kind_in(operator, right, &r, &l),
        );
        if crate::scope::arithmetic_kind(operator, &l, &r).is_none() {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                format!(
                    "`{}` cannot combine {} and {}",
                    operator.symbol(),
                    kind_name(&l),
                    kind_name(&r)
                ),
                at,
            )
            .length(1));
        }
        Ok(())
    }
    /// A call of a declared function: its argument count, the kinds its
    /// arguments take, and for `case_eq` its compared value.
    pub fn check_function(
        &self,
        scope: &Scope,
        function: &crate::catalog::Function,
        arguments: &mut [Expression],
        at: usize,
    ) -> Result<(), Error> {
        if arguments.len() != function.arguments {
            return Err(Error::new(
                ErrorKind::BadValue,
                format!(
                    "`{}` takes {} argument{}",
                    function.name,
                    function.arguments,
                    if function.arguments == 1 { "" } else { "s" }
                ),
                at,
            ));
        }
        if function.name == "abs"
            && !matches!(
                self.expression_info(scope, &arguments[0]).field.kind,
                FieldKind::Integer
                    | FieldKind::Number
                    | FieldKind::Money
                    | FieldKind::Duration
                    | FieldKind::Json
            )
            && !matches!(
                &arguments[0],
                Expression::Value {
                    value: Value::Unknown
                }
            )
        {
            return Err(Error::new(
                ErrorKind::BadValue,
                "abs takes a linear number, money or duration",
                at,
            ));
        }
        if matches!(function.name.as_str(), "glob" | "text") {
            for argument in arguments.iter() {
                if !matches!(
                    self.expression_info(scope, argument).field.kind,
                    FieldKind::Text
                        | FieldKind::Exact
                        | FieldKind::Substring
                        | FieldKind::Words
                        | FieldKind::Subtree
                        | FieldKind::Reference
                        | FieldKind::Json
                ) {
                    return Err(Error::new(
                        ErrorKind::BadValue,
                        format!("{} takes text values", function.name),
                        at,
                    ));
                }
            }
        }
        if function.name == "case_eq" {
            let (first, rest) = arguments.split_at_mut(1);
            let (left, right) = (&mut first[0], &mut rest[0]);
            match (left, right) {
                (left, Expression::Value { value }) => {
                    self.normalize_expression_value(scope, left, value);
                    self.check_expression_value(scope, left, value, at)?;
                }
                (Expression::Value { value }, right) => {
                    self.normalize_expression_value(scope, right, value);
                    self.check_expression_value(scope, right, value, at)?;
                }
                _ => {}
            }
            let left = &arguments[0];
            let right = &arguments[1];
            let left_kind = self.expression_info(scope, left).field.kind;
            let right_kind = self.expression_info(scope, right).field.kind;
            let contextual = |expression: &Expression, own: &FieldKind, other: &FieldKind| {
                if let Expression::Value { value } = expression {
                    if matches!(value, Value::Unknown) {
                        FieldKind::Json
                    } else if matches!(
                        other,
                        FieldKind::Text
                            | FieldKind::Reference
                            | FieldKind::Bool
                            | FieldKind::Json
                            | FieldKind::List(_)
                    ) {
                        other.clone()
                    } else {
                        typing::operand_kind(expression, other)
                    }
                } else {
                    own.clone()
                }
            };
            self.check_comparable(
                &contextual(left, &left_kind, &right_kind),
                &contextual(right, &right_kind, &left_kind),
                at,
            )?;
        }
        Ok(())
    }

    pub fn order_scope(&self, unit: &str) -> Option<String> {
        self.catalog
            .unit(unit)
            .and_then(|unit| unit.order_scope.clone())
    }
    pub fn require_order_scope(&self, unit: &str, at: usize) -> Result<String, Error> {
        if self.catalog.unit(unit).is_none() {
            return Ok(String::new());
        }
        self.order_scope(unit).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidQuery,
                format!("`{unit}` has no order scope; steps count positions in one, so use a time distance"),
                at,
            )
        })
    }
    /// The options of an order relation between the `outer` rows and the
    /// `inner` query's rows: a distance where one is needed, and steps only
    /// within one order scope.
    #[allow(clippy::too_many_arguments)]
    pub fn check_order_options(
        &self,
        relation: OrderRelation,
        outer: &Scope,
        inner: &Scope,
        within: Option<&Distance>,
        at: TimePoint,
        order_scope: Option<&str>,
        start: usize,
    ) -> Result<(), Error> {
        if relation == OrderRelation::Near && within.is_none() {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "near needs a distance: `within 5m` or `within 3 steps`",
                start,
            ));
        }
        if matches!(within, Some(Distance::Steps { .. })) {
            if !relation.measures_steps() {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!(
                        "{} compares times; steps measure positions",
                        relation.name()
                    ),
                    start,
                ));
            }
            if at != TimePoint::When {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "at chooses a time point; steps count positions",
                    start,
                ));
            }
            let declared = self.require_order_scope(&outer.unit, start)?;
            let inner_scope = self.order_scope(&inner.unit);
            if !declared.is_empty()
                && self.catalog.unit(&inner.unit).is_some()
                && inner_scope.as_deref() != Some(declared.as_str())
            {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!(
                        "`{}` steps in `{declared}` and `{}` in {}; steps never cross order scopes",
                        outer.unit,
                        inner.unit,
                        inner_scope.map_or("none".to_string(), |scope| format!("`{scope}`"))
                    ),
                    start,
                ));
            }
            if let Some(named) = order_scope
                && !declared.is_empty()
                && named != declared
            {
                return Err(Error::new(
                    ErrorKind::BadValue,
                    format!("`{}` steps in `{declared}`, not `{named}`", outer.unit),
                    start,
                ));
            }
        }
        Ok(())
    }
    /// The rows an order relation's `pick first by` keys read: the inner
    /// query's rows and the match's own fields.
    pub fn match_scope(&self, inner: &Scope, within: Option<&Distance>) -> Scope {
        let mut candidate = inner.clone();
        for (name, kind) in [
            ("match.id", FieldKind::Reference),
            ("match.coarse", FieldKind::Bool),
            (
                crate::catalog::MATCH_DISTANCE,
                if matches!(within, Some(Distance::Steps { .. })) {
                    FieldKind::Integer
                } else {
                    FieldKind::Duration
                },
            ),
        ] {
            self.add_generated(
                &mut candidate,
                Field {
                    name: name.into(),
                    kind,
                    ..Field::default()
                },
                Origin::Generated,
            );
        }
        candidate
    }
    /// One top-level order relation in a relation's condition exposes its
    /// nearest inner row as `match.F` and the signed distance as `match.distance`.
    pub fn expose_order_match(&self, scope: &mut Scope, condition: &Condition) {
        if let Some(order) = condition.exposed_order()
            && let ConditionKind::Order { within, .. } = &order.kind
            && let Some(inner) = self.inner_scopes.get(&self.order_key(order))
        {
            self.expose_inner(scope, inner, within.as_ref());
        }
    }
    pub fn expose_inner(&self, scope: &mut Scope, inner: &Scope, within: Option<&Distance>) {
        for info in self.visible(inner) {
            let mut field = info.field.clone();
            field.name = format!("match.{}", info.field.name);
            self.add_generated(
                scope,
                field,
                Origin::Matched {
                    input: Box::new(info),
                },
            );
        }
        if let Some(within) = within {
            self.add_generated(
                scope,
                Field {
                    name: crate::catalog::MATCH_DISTANCE.into(),
                    kind: match within {
                        Distance::Steps { .. } => FieldKind::Integer,
                        Distance::Time { .. } => FieldKind::Duration,
                    },
                    ..Field::default()
                },
                Origin::Generated,
            );
        }
    }
    pub fn add_generated(&self, scope: &mut Scope, field: Field, origin: Origin) {
        let name = field.name.clone();
        Arc::make_mut(&mut scope.typed_extra).retain(|f| f.name != name);
        Arc::make_mut(&mut scope.typed_extra).push(field);
        Arc::make_mut(&mut scope.origins).insert(name.clone(), origin);
        if let Some(columns) = &mut scope.columns {
            columns.insert(name);
        } else {
            scope.extra.insert(name);
        }
    }

    /// `| where C`: its top-level patterns and order match join the scope.
    pub fn where_scope(&self, scope: &mut Scope, condition: &Condition) {
        if let Some(known) = &mut scope.patterns {
            known.extend(patterns(Some(condition)));
        }
        self.expose_order_match(scope, condition);
    }
    /// After `| UNIT where C`: the condition's patterns and order match.
    pub fn unit_condition_scope(&self, scope: &mut Scope, condition: Option<&Condition>) {
        scope.patterns = Some(patterns(condition));
        if let Some(condition) = condition {
            self.expose_order_match(scope, condition);
        }
    }
    /// `| group by K, …`: one row per key, showing the keys and `count`.
    pub fn group_scope(&self, scope: &mut Scope, keys: &[Column]) {
        let inputs: Vec<_> = keys
            .iter()
            .map(|key| self.expression_info(scope, &key.expression))
            .collect();
        let rows = scope.clone();
        Arc::make_mut(&mut scope.typed_extra).clear();
        Arc::make_mut(&mut scope.origins).clear();
        scope.extra.clear();
        scope.columns = Some(BTreeSet::new());
        for (index, (key, mut info)) in keys.iter().zip(inputs).enumerate() {
            let input = info.clone();
            info.field.name = key.column();
            self.add_generated(
                scope,
                info.field,
                Origin::Group {
                    operand: self.transform_path().field("keys").index(index),
                    input: Box::new(input),
                },
            );
        }
        self.add_generated(
            scope,
            Field {
                name: "count".into(),
                kind: FieldKind::Integer,
                ..Field::default()
            },
            Origin::Aggregate {
                at: self.transform_path(),
                function: AggregateFunction::Count,
                inputs: Vec::new(),
                rule: crate::scope::TypeRule::Fixed,
            },
        );
        scope.group_input = Some(Arc::new(rows));
        scope.bucketed = keys.iter().any(|key| {
            matches!(&key.expression, Expression::Function { name, .. }
                if crate::catalog::TIME_BUCKETS.contains(&name.as_str()))
        });
        scope.patterns = Some(Vec::new());
    }
    /// `| first N` reads the rows a grouping was built from.
    pub fn first_scope(&self, scope: &mut Scope) {
        if let Some(input) = scope.group_input.take() {
            *scope = (*input).clone();
        }
    }
    /// `| fill` after a time-bucket grouping adds each bucket's coverage.
    pub fn fill_scope(&self, scope: &mut Scope, position: usize) -> Result<(), Error> {
        if !scope.bucketed {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "fill follows a group by a time bucket",
                position,
            ));
        }
        self.add_generated(
            scope,
            Field {
                name: crate::coverage::STATE_FIELD.into(),
                kind: FieldKind::Exact,
                values: crate::coverage::CoverageState::STATES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Field::default()
            },
            Origin::Generated,
        );
        self.add_generated(
            scope,
            Field {
                name: crate::coverage::DETAIL_FIELD.into(),
                kind: FieldKind::Text,
                ..Field::default()
            },
            Origin::Generated,
        );
        Ok(())
    }
    /// `| fields C, …` shows exactly the named columns.
    pub fn fields_scope(
        &self,
        scope: &mut Scope,
        columns: &[Column],
        start: usize,
    ) -> Result<(), Error> {
        let infos: Vec<_> = columns
            .iter()
            .map(|column| {
                let mut info = self.expression_info(scope, &column.expression);
                info.field.name = column.column();
                info
            })
            .collect();
        let mut names = BTreeSet::new();
        for info in &infos {
            if !names.insert(info.field.name.clone()) {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!("two columns are named `{}`", info.field.name),
                    start,
                ));
            }
        }
        *Arc::make_mut(&mut scope.typed_extra) =
            infos.iter().map(|info| info.field.clone()).collect();
        *Arc::make_mut(&mut scope.origins) = infos
            .into_iter()
            .map(|info| (info.field.name, info.origin))
            .collect();
        scope.extra.clear();
        scope.columns = Some(names);
        Ok(())
    }
    /// `| union (Q)` and the other combinations need comparable rows.
    pub fn check_combination(
        &self,
        scope: &Scope,
        inner: &Scope,
        how: Combination,
        start: usize,
    ) -> Result<(), Error> {
        // Membership-only operations keep the left values. Native
        // identity survives projection, so visible columns need not match.
        let same_native_domain = match (&scope.row_domain, &inner.row_domain) {
            (Some(left), Some(right)) => {
                let left = self
                    .catalog
                    .unit(left)
                    .map_or(left.as_str(), |unit| unit.name.as_str());
                let right = self
                    .catalog
                    .unit(right)
                    .map_or(right.as_str(), |unit| unit.name.as_str());
                left == right
            }
            _ => false,
        };
        let native_membership =
            matches!(how, Combination::Intersect | Combination::Except) && same_native_domain;
        if how != Combination::Fuse && !native_membership {
            let word = match how {
                Combination::Union => "union",
                Combination::Intersect => "intersect",
                Combination::Except => "except",
                Combination::Fuse => "fuse",
            };
            self.check_same_rows(scope, inner, word, start)?;
        }
        Ok(())
    }
    /// `| diff (Q)`: rows of one kind, each with its change and ranks.
    pub fn diff_scope(&self, scope: &mut Scope, inner: &Scope, start: usize) -> Result<(), Error> {
        self.check_same_rows(scope, inner, "diff", start)?;
        self.add_generated(
            scope,
            Field {
                name: "change".into(),
                kind: FieldKind::Exact,
                values: crate::diff::ChangeKind::NAMES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Field::default()
            },
            Origin::Generated,
        );
        for name in ["old_rank", "new_rank"] {
            self.add_generated(
                scope,
                Field {
                    name: name.into(),
                    kind: FieldKind::Integer,
                    ..Field::default()
                },
                Origin::Generated,
            );
        }
        Ok(())
    }
    /// Set algebra and diff need rows of one unit, or identical projected types.
    fn check_same_rows(
        &self,
        outer: &Scope,
        inner: &Scope,
        word: &str,
        at: usize,
    ) -> Result<(), Error> {
        if outer.columns_unknown || inner.columns_unknown {
            return Ok(());
        }
        let same = match (&outer.columns, &inner.columns) {
            (Some(a), Some(b)) => {
                a == b
                    && a.iter().all(|name| {
                        let left = self.field_info(outer, name).field;
                        let right = self.field_info(inner, name).field;
                        self.same_projected_type(&left, &right)
                    })
            }
            (None, None) => {
                self.catalog.unit(&outer.unit).map(|u| &u.name)
                    == self.catalog.unit(&inner.unit).map(|u| &u.name)
            }
            _ => false,
        };
        if same {
            return Ok(());
        }
        Err(Error::new(
            ErrorKind::InvalidQuery,
            format!(
                "{word} needs rows of one unit or the same projected fields and types; project both sides with `| fields …`"
            ),
            at,
        ))
    }
    fn same_projected_type(&self, left: &Field, right: &Field) -> bool {
        if left.kind != right.kind
            || left.values.iter().collect::<BTreeSet<_>>()
                != right.values.iter().collect::<BTreeSet<_>>()
        {
            return false;
        }
        let same_unknown = match (&left.unknown, &right.unknown) {
            (crate::catalog::UnknownRule::Word(left), crate::catalog::UnknownRule::Word(right)) => {
                left.eq_ignore_ascii_case(right)
            }
            (left, right) => left == right,
        };
        if !same_unknown {
            return false;
        }
        let mut kind = &left.kind;
        while let FieldKind::List(element) = kind {
            kind = element;
        }
        if *kind == FieldKind::Reference {
            let target = |field: &Field| {
                field.unit.as_deref().map(|name| {
                    self.catalog
                        .unit(name)
                        .map_or_else(|| name.to_string(), |unit| unit.name.clone())
                })
            };
            return target(left) == target(right);
        }
        true
    }
    /// One row per step of each match: `parent`, `match`, `position`, `step`,
    /// and the step row's fields as `step.F`.
    pub fn matches_scope(&self, scope: &mut Scope, start: usize) -> Result<(), Error> {
        let Some(patterns) = scope.patterns.clone() else {
            // Unknown saved outputs are syntax until the caller supplies their
            // immutable schema; binding checks the actual steps.
            if scope.columns_unknown {
                *scope = Scope::new("$matches".into());
                scope.columns_unknown = true;
                scope.patterns = None;
                return Ok(());
            }
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "matches expands an exists or seq written in this query; a saved or kept source hides its own",
                start,
            ));
        };
        let unit = match patterns.as_slice() {
            [unit] => unit.clone(),
            [] => {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "matches needs one top-level exists or seq",
                    start,
                ));
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::Ambiguous,
                    format!(
                        "matches needs exactly one top-level exists or seq; this relation has {}",
                        patterns.len()
                    ),
                    start,
                ));
            }
        };
        let parent = scope.unit.clone();
        let parent_fields = self.visible(scope);
        let step_fields: Vec<Field> = self
            .catalog
            .unit(&unit)
            .map(|u| u.fields.clone())
            .unwrap_or_default();
        *scope = Scope::new("$matches".into());
        scope.columns = Some(BTreeSet::new());
        for (name, kind, unit) in [
            ("parent", FieldKind::Reference, Some(parent)),
            ("match", FieldKind::Integer, None),
            ("position", FieldKind::Integer, None),
            ("step", FieldKind::Reference, Some(unit.clone())),
        ] {
            self.add_generated(
                scope,
                Field {
                    name: name.into(),
                    kind,
                    unit,
                    ..Field::default()
                },
                Origin::Generated,
            );
        }
        for mut info in parent_fields {
            info.field.name = format!("parent.{}", info.field.name);
            self.add_generated(scope, info.field, info.origin);
        }
        for mut field in step_fields {
            let origin = Origin::Catalog {
                unit: unit.clone(),
                field: field.name.clone(),
            };
            field.name = format!("step.{}", field.name);
            self.add_generated(scope, field, origin);
        }
        Ok(())
    }
    /// `| agg A, …` starts: returns the rows the aggregates read, and leaves
    /// an ungrouped scope with no columns until each aggregate adds its own.
    pub fn begin_aggregates(&self, scope: &mut Scope) -> Scope {
        let grouped = scope.group_input.is_some();
        let rows = scope
            .group_input
            .take()
            .map(|rows| (*rows).clone())
            .unwrap_or_else(|| scope.clone());
        if !grouped {
            scope.columns = Some(BTreeSet::new());
            scope.extra.clear();
            Arc::make_mut(&mut scope.typed_extra).clear();
            Arc::make_mut(&mut scope.origins).clear();
        }
        rows
    }
    /// An aggregate's arguments: how many it takes, and for `covered` an
    /// interval of one kind.
    pub fn check_aggregate_arguments(
        &self,
        rows: &Scope,
        function: &AggregateFunction,
        arguments: &[Expression],
        at: usize,
    ) -> Result<(), Error> {
        if !function.arguments().contains(&arguments.len()) {
            let range = function.arguments();
            return Err(Error::new(
                ErrorKind::BadValue,
                format!(
                    "`{}` takes {} argument{}",
                    function.name(),
                    range.end(),
                    if *range.end() == 1 { "" } else { "s" }
                ),
                at,
            ));
        }
        if *function == AggregateFunction::Covered {
            let kinds: Vec<_> = arguments
                .iter()
                .map(|a| self.expression_info(rows, a).field.kind)
                .collect();
            let interval = |k: &FieldKind| {
                matches!(
                    k,
                    FieldKind::Time
                        | FieldKind::Integer
                        | FieldKind::Number
                        | FieldKind::Duration
                        | FieldKind::Json
                )
            };
            if kinds[0] != kinds[1] || !interval(&kinds[0]) {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "covered takes a start and an end of one time or number kind",
                    at,
                ));
            }
        }
        Ok(())
    }
    /// The aggregate at `index` adds its column.
    pub fn add_aggregate(
        &self,
        scope: &mut Scope,
        rows: &Scope,
        aggregate: &Aggregate,
        index: usize,
    ) {
        let inputs = aggregate
            .arguments
            .iter()
            .map(|argument| self.expression_info(rows, argument))
            .collect();
        let info = self.aggregate_info(
            aggregate,
            inputs,
            self.transform_path().field("aggregates").index(index),
        );
        self.add_generated(scope, info.field, info.origin);
    }
    /// What every transform leaves: grouping and aggregation have no native
    /// row domain, and only they keep the rows a grouping was built from.
    pub fn finish_transform(&self, scope: &mut Scope, kind: &TransformKind) {
        if matches!(
            kind,
            TransformKind::Group { .. } | TransformKind::Aggregate { .. }
        ) {
            scope.row_domain = None;
        }
        if matches!(
            kind,
            TransformKind::Group { .. }
                | TransformKind::Aggregate { .. }
                | TransformKind::Join { .. }
                | TransformKind::Diff { .. }
        ) {
            scope.patterns = Some(Vec::new());
        }
        if !matches!(
            kind,
            TransformKind::Group { .. } | TransformKind::Aggregate { .. }
        ) {
            scope.group_input = None;
        }
    }
}
