//! Capture the parser's scopes. Walking the returned tree assigns addresses;
//! it never reconstructs group, projection, or aggregate transitions.
use super::*;
use crate::scope::{
    FieldInfo, NodePath, Origin, QueryScope, TransformScope, TypeRule, TypedOperand, Typing,
};

pub(super) struct Capture {
    pub source: Scope,
    pub transforms: Vec<(Scope, Scope)>,
}
impl Parser<'_> {
    pub(super) fn transform_path(&self) -> NodePath {
        let (query, transform) = self
            .query_stack
            .last()
            .copied()
            .expect("transform belongs to query");
        NodePath::default()
            .field("$query")
            .index(query)
            .field("transforms")
            .index(transform)
            .field("kind")
    }
    pub(super) fn field_info(&self, scope: &Scope, name: &str) -> FieldInfo {
        let field = self
            .scope_field(scope, name)
            .cloned()
            .unwrap_or_else(|| Field {
                name: name.into(),
                kind: FieldKind::Json,
                ..Field::default()
            });
        let origin = scope.origins.get(name).cloned().unwrap_or_else(|| {
            scope.output_source.as_ref().map_or_else(
                || Origin::Catalog {
                    unit: scope.unit.clone(),
                    field: name.into(),
                },
                |source| Origin::Output {
                    source: source.clone(),
                    field: name.into(),
                },
            )
        });
        FieldInfo { field, origin }
    }
    pub(super) fn expression_info(&self, scope: &Scope, expression: &Expression) -> FieldInfo {
        match expression {
            Expression::Field { name } => self.field_info(scope, name),
            Expression::Value { value } => FieldInfo {
                field: Field {
                    name: expression.column(),
                    kind: literal_kind(value),
                    ..Field::default()
                },
                origin: Origin::Generated,
            },
            Expression::Function { name, arguments } => {
                let arguments: Vec<_> = if name == "random" {
                    Vec::new()
                } else {
                    arguments
                        .iter()
                        .map(|argument| self.expression_info(scope, argument))
                        .collect()
                };
                let rule = if name == "case_eq" {
                    TypeRule::Input(2)
                } else if name == "abs" {
                    TypeRule::Input(0)
                } else {
                    TypeRule::Fixed
                };
                let mut field = match rule {
                    TypeRule::Input(index) => arguments
                        .get(index)
                        .map(|input| input.field.clone())
                        .unwrap_or_default(),
                    TypeRule::Fixed => Field::default(),
                };
                field.name = expression.column();
                if rule == TypeRule::Fixed {
                    field.kind = if name == "random" {
                        FieldKind::Number
                    } else {
                        self.catalog
                            .function(name)
                            .ok()
                            .flatten()
                            .map_or(FieldKind::Json, |function| function.result)
                    };
                }
                FieldInfo {
                    field,
                    origin: Origin::Function {
                        name: name.clone(),
                        arguments,
                        rule,
                    },
                }
            }
            Expression::Arithmetic {
                operator,
                left,
                right,
            } => {
                let left_expression = left;
                let right_expression = right;
                let left = self.expression_info(scope, left_expression);
                let right = self.expression_info(scope, right_expression);
                let kind = crate::scope::arithmetic_kind(
                    *operator,
                    &operand_kind_in(
                        *operator,
                        left_expression,
                        &left.field.kind,
                        &right.field.kind,
                    ),
                    &operand_kind_in(
                        *operator,
                        right_expression,
                        &right.field.kind,
                        &left.field.kind,
                    ),
                )
                .unwrap_or(FieldKind::Json);
                FieldInfo {
                    field: Field {
                        name: expression.column(),
                        kind,
                        ..Field::default()
                    },
                    origin: Origin::Function {
                        name: operator.symbol().into(),
                        arguments: vec![left, right],
                        rule: TypeRule::Fixed,
                    },
                }
            }
        }
    }
    pub(super) fn aggregate_info(
        &self,
        aggregate: &Aggregate,
        inputs: Vec<FieldInfo>,
        at: NodePath,
    ) -> FieldInfo {
        let (kind, rule) = crate::scope::aggregate_kind(
            &aggregate.function,
            inputs.first().map(|input| &input.field.kind),
        );
        let mut field = if let TypeRule::Input(index) = rule {
            inputs
                .get(index)
                .map(|input| input.field.clone())
                .unwrap_or_default()
        } else {
            Field::default()
        };
        field.name = aggregate.column();
        field.kind = kind;
        // Sum does not retain an input's finite vocabulary.
        if matches!(
            aggregate.function,
            AggregateFunction::Sum | AggregateFunction::Avg
        ) {
            field.values.clear();
        }
        FieldInfo {
            field,
            origin: Origin::Aggregate {
                at,
                function: aggregate.function.clone(),
                inputs,
                rule,
            },
        }
    }
    /// The fields a scope shows, each with its declaration and origin.
    pub(super) fn visible(&self, scope: &Scope) -> Vec<FieldInfo> {
        let mut names: BTreeSet<_> = if let Some(columns) = &scope.columns {
            columns.clone()
        } else if let Some(output) = &scope.output {
            output
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect()
        } else if scope.columns_unknown {
            BTreeSet::new()
        } else {
            self.catalog
                .unit(&scope.unit)
                .into_iter()
                .flat_map(|unit| unit.fields.iter().map(|field| field.name.clone()))
                .collect()
        };
        names.extend(scope.extra.iter().cloned());
        if scope.columns.is_none() {
            names.extend(scope.typed_extra.iter().map(|field| field.name.clone()));
        }
        names
            .iter()
            .map(|name| self.field_info(scope, name))
            .collect()
    }
    fn snapshot(&self, scope: &Scope) -> crate::scope::Scope {
        crate::scope::Scope {
            unit: scope
                .row_domain
                .as_deref()
                .and_then(|unit| self.catalog.unit(unit))
                .map(|unit| unit.name.clone()),
            fields: self.visible(scope).into(),
            match_units: scope.patterns.clone(),
        }
    }
}
/// The kind of one operand of a comparison or an addition, read against the
/// other operand: a bare number beside money is money (`cost - 2.90`), so
/// money needs no unit suffix in an expression.
pub(super) fn operand_kind(expression: &Expression, other: &FieldKind) -> FieldKind {
    // The formatter quotes every scalar it renders, so a comparison's literal
    // is typed by its text whether or not it was quoted.
    let kind = match expression {
        Expression::Value {
            value: Value::Scalar { text, .. },
        } => literal_kind(&Value::Scalar {
            text: text.clone(),
            quoted: false,
        }),
        Expression::Value { value } => literal_kind(value),
        _ => return FieldKind::Json,
    };
    if *other == FieldKind::Money && matches!(kind, FieldKind::Integer | FieldKind::Number) {
        FieldKind::Money
    } else {
        kind
    }
}
/// `operand_kind` inside arithmetic: only `+` and `-` read a bare number as
/// money; `cost * 2` and `cost / 2` scale money by a plain number.
pub(super) fn operand_kind_in(
    operator: Arithmetic,
    expression: &Expression,
    kind: &FieldKind,
    other: &FieldKind,
) -> FieldKind {
    if matches!(expression, Expression::Value { .. })
        && matches!(operator, Arithmetic::Add | Arithmetic::Subtract)
    {
        operand_kind(expression, other)
    } else {
        kind.clone()
    }
}
/// Whether values of the two kinds can be compared. Kinds without a
/// dimension (text, documents, references, booleans) compare with any
/// literal, which the catalog's own value checks govern; numbers, money,
/// durations and times compare only within their dimension. A time compares
/// with text, the spelling of dates and relative times.
pub(super) fn comparable(left: &FieldKind, right: &FieldKind) -> bool {
    use FieldKind::*;
    let dimension = |kind: &FieldKind| match kind {
        Integer | Number => Some(0),
        Money => Some(1),
        Duration => Some(2),
        Time => Some(3),
        _ => None,
    };
    match (dimension(left), dimension(right)) {
        (Some(3), None) | (None, Some(3)) => true,
        (Some(l), Some(r)) => l == r,
        (Some(_), None) | (None, Some(_)) => matches!(left, Json) || matches!(right, Json),
        (None, None) => true,
    }
}
/// The kind of an unquoted or quoted literal in an expression.
pub(super) fn literal_kind(value: &Value) -> FieldKind {
    match value {
        Value::Scalar {
            text,
            quoted: false,
        } => {
            if crate::number::is_integer(text) {
                FieldKind::Integer
            } else if crate::number::is_decimal(text) {
                FieldKind::Number
            } else if is_duration(text) {
                FieldKind::Duration
            } else if is_money(text) {
                FieldKind::Money
            } else {
                FieldKind::Text
            }
        }
        _ => FieldKind::Text,
    }
}

pub(super) fn project(parser: &Parser<'_>, query: &Query) -> Typing {
    let mut walker = Walker {
        parser,
        next_query: 0,
        typing: Typing::default(),
    };
    walker.query(query, NodePath::default());
    debug_assert_eq!(walker.next_query, parser.captures.len());
    let paths: Vec<_> = walker
        .typing
        .queries
        .iter()
        .map(|query| query.path.clone())
        .collect();
    let mut shapes = BTreeMap::new();
    for query in &mut walker.typing.queries {
        rewrite_scope(&mut query.source, &paths, &mut shapes);
        for transform in &mut query.transforms {
            rewrite_scope(&mut transform.before, &paths, &mut shapes);
            rewrite_scope(&mut transform.after, &paths, &mut shapes);
        }
    }
    for operand in &mut walker.typing.operands {
        rewrite_info(&mut operand.result, &paths);
    }
    walker.typing
}
struct Walker<'a, 'b> {
    parser: &'a Parser<'b>,
    next_query: usize,
    typing: Typing,
}
impl Walker<'_, '_> {
    fn field(&mut self, name: &str, scope: &Scope, path: NodePath) {
        self.expression(&Expression::field(name), scope, path);
    }
    fn expression(&mut self, expression: &Expression, scope: &Scope, path: NodePath) {
        if matches!(expression, Expression::Value { .. }) {
            return;
        }
        self.typing.operands.push(TypedOperand {
            path: path.clone(),
            unit: self
                .parser
                .catalog
                .unit(&scope.unit)
                .map(|unit| unit.name.clone()),
            result: self.parser.expression_info(scope, expression),
        });
        match expression {
            Expression::Function { name, arguments } if name != "random" => {
                for (index, argument) in arguments.iter().enumerate() {
                    self.expression(argument, scope, path.field("arguments").index(index));
                }
            }
            Expression::Arithmetic { left, right, .. } => {
                self.expression(left, scope, path.field("left"));
                self.expression(right, scope, path.field("right"));
            }
            _ => {}
        }
    }
    fn query(&mut self, query: &Query, path: NodePath) {
        let id = self.next_query;
        self.next_query += 1;
        let parser = self.parser;
        let capture = parser.captures[id]
            .as_ref()
            .expect("successful parser filled each scope capture");
        let source = self.parser.snapshot(&capture.source);
        let mut previous = source.clone();
        let transforms = capture
            .transforms
            .iter()
            .map(|(before, after)| {
                let next = if before == after {
                    previous.clone()
                } else {
                    self.parser.snapshot(after)
                };
                let transform = TransformScope {
                    before: previous.clone(),
                    after: next.clone(),
                };
                previous = next;
                transform
            })
            .collect();
        self.typing.queries.push(QueryScope {
            path: path.clone(),
            source,
            transforms,
        });
        match &query.source {
            Source::Unit {
                condition: Some(condition),
                ..
            }
            | Source::Saved {
                condition: Some(condition),
                ..
            }
            | Source::Result {
                condition: Some(condition),
                ..
            } => self.condition(
                condition,
                &capture.source,
                path.field("source").field("condition"),
            ),
            _ => {}
        }
        for (index, attachment) in query.attach.iter().enumerate() {
            let scope = Scope::new(attachment.unit.clone());
            let at = path.field("attach").index(index);
            for (index, field) in attachment.fields.iter().enumerate() {
                self.field(field, &scope, at.field("fields").index(index));
            }
            for (index, window) in attachment.windows.iter().enumerate() {
                if let Window::Where(condition) = window {
                    self.condition(
                        condition,
                        &scope,
                        at.field("windows").index(index).field("where"),
                    );
                }
            }
        }
        for (index, transform) in query.transforms.iter().enumerate() {
            let (scope, after) = &capture.transforms[index];
            let at = path.field("transforms").index(index).field("kind");
            match &transform.kind {
                TransformKind::Where { condition } => {
                    self.condition(condition, scope, at.field("condition"))
                }
                TransformKind::Sort { keys } => {
                    for (index, key) in keys.iter().enumerate() {
                        let path = at.field("keys").index(index).field("expression");
                        if matches!(&key.expression, Expression::Field { name } if name == "rank")
                            && !self.parser.field_known(scope, "rank")
                        {
                            self.typing.operands.push(TypedOperand {
                                path,
                                unit: self
                                    .parser
                                    .catalog
                                    .unit(&scope.unit)
                                    .map(|unit| unit.name.clone()),
                                result: FieldInfo {
                                    field: Field {
                                        name: "rank".into(),
                                        kind: FieldKind::Number,
                                        ..Field::default()
                                    },
                                    origin: Origin::Generated,
                                },
                            });
                        } else {
                            self.expression(&key.expression, scope, path);
                        }
                    }
                }
                TransformKind::Group { keys } => {
                    for (index, key) in keys.iter().enumerate() {
                        self.expression(
                            &key.expression,
                            scope,
                            at.field("keys").index(index).field("expression"),
                        );
                    }
                }
                TransformKind::Aggregate { aggregates } => {
                    let rows = scope.group_input.as_deref().unwrap_or(scope);
                    for (index, aggregate) in aggregates.iter().enumerate() {
                        for (argument_index, argument) in aggregate.arguments.iter().enumerate() {
                            self.expression(
                                argument,
                                rows,
                                at.field("aggregates")
                                    .index(index)
                                    .field("arguments")
                                    .index(argument_index),
                            );
                        }
                    }
                }
                TransformKind::Fields { columns } => {
                    for (index, column) in columns.iter().enumerate() {
                        self.expression(
                            &column.expression,
                            scope,
                            at.field("columns").index(index).field("expression"),
                        );
                    }
                }
                TransformKind::Scope {
                    condition: Some(condition),
                    ..
                } => self.condition(condition, after, at.field("condition")),
                TransformKind::Combine { query, .. }
                | TransformKind::Join { query, .. }
                | TransformKind::Diff { query } => self.query(query, at.field("query")),
                TransformKind::Extension { arguments, .. } => {
                    self.arguments(arguments, scope, at.field("arguments"))
                }
                _ => {}
            }
        }
    }
    fn condition(&mut self, condition: &Condition, scope: &Scope, path: NodePath) {
        let at = path.field("kind");
        match &condition.kind {
            ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
                for (index, condition) in conditions.iter().enumerate() {
                    self.condition(condition, scope, at.field("conditions").index(index));
                }
            }
            ConditionKind::Not { condition } => {
                self.condition(condition, scope, at.field("condition"))
            }
            ConditionKind::Match { field, .. }
            | ConditionKind::Since { field, .. }
            | ConditionKind::Until { field, .. } => self.field(field, scope, at.field("field")),
            ConditionKind::Compare { left, right, .. } => {
                self.expression(left, scope, at.field("left"));
                self.expression(right, scope, at.field("right"));
            }
            ConditionKind::Between { left, .. } | ConditionKind::TimeRange { left, .. } => {
                self.expression(left, scope, at.field("left"))
            }
            ConditionKind::In { left, query } => {
                self.expression(left, scope, at.field("left"));
                self.query(query, at.field("query"));
            }
            ConditionKind::Order {
                query,
                within,
                pick,
                ..
            } => {
                self.query(query, at.field("query"));
                if let WitnessChoice::FirstBy { keys } = pick {
                    let inner = &self.parser.inner_scopes[&condition.span.start];
                    let candidate = self.parser.witness_scope(inner, within.as_ref());
                    for (index, key) in keys.iter().enumerate() {
                        self.expression(
                            &key.expression,
                            &candidate,
                            at.field("pick")
                                .field("keys")
                                .index(index)
                                .field("expression"),
                        );
                    }
                }
            }
            ConditionKind::Exists {
                unit,
                condition: Some(condition),
            }
            | ConditionKind::Count {
                unit,
                condition: Some(condition),
                ..
            } => self.condition(condition, &Scope::new(unit.clone()), at.field("condition")),
            ConditionKind::Sequence { unit, steps, .. } => {
                for (index, condition) in steps.iter().enumerate() {
                    self.condition(
                        condition,
                        &Scope::new(unit.clone()),
                        at.field("steps").index(index),
                    );
                }
            }
            ConditionKind::Extension { arguments, .. } => {
                self.arguments(arguments, scope, at.field("arguments"))
            }
            _ => {}
        }
    }
    fn arguments(&mut self, arguments: &[ExtensionArgument], scope: &Scope, path: NodePath) {
        for (index, argument) in arguments.iter().enumerate() {
            let at = path.index(index);
            match argument {
                ExtensionArgument::Field(name) => self.field(name, scope, at.field("field")),
                ExtensionArgument::Expression(expression) => {
                    self.expression(expression, scope, at.field("expression"))
                }
                ExtensionArgument::Condition(condition) => {
                    self.condition(condition, scope, at.field("condition"))
                }
                ExtensionArgument::Query(query) => self.query(query, at.field("query")),
                ExtensionArgument::Optional(arguments) => {
                    self.arguments(arguments, scope, at.field("optional"))
                }
                ExtensionArgument::List(arguments) => {
                    self.arguments(arguments, scope, at.field("list"))
                }
                _ => {}
            }
        }
    }
}

fn rewrite_path(path: &mut NodePath, queries: &[NodePath]) {
    if let [
        crate::scope::PathElement::Field(marker),
        crate::scope::PathElement::Index(index),
        tail @ ..,
    ] = path.0.as_slice()
        && marker == "$query"
    {
        let mut result = queries[*index].clone();
        result.0.extend_from_slice(tail);
        *path = result;
    }
}
fn rewrite_info(info: &mut FieldInfo, queries: &[NodePath]) {
    match &mut info.origin {
        Origin::Group { operand, input } => {
            rewrite_path(operand, queries);
            rewrite_info(input, queries);
        }
        Origin::Aggregate { at, inputs, .. } => {
            rewrite_path(at, queries);
            for input in inputs {
                rewrite_info(input, queries);
            }
        }
        Origin::Function { arguments, .. } => {
            for input in arguments {
                rewrite_info(input, queries);
            }
        }
        Origin::Matched { input } => rewrite_info(input, queries),
        _ => {}
    }
}
fn rewrite_scope(
    scope: &mut crate::scope::Scope,
    queries: &[NodePath],
    shapes: &mut BTreeMap<usize, Arc<[FieldInfo]>>,
) {
    let key = scope.fields.as_ptr() as usize;
    if let Some(fields) = shapes.get(&key) {
        scope.fields = fields.clone();
        return;
    }
    let mut fields = scope.fields.to_vec();
    for info in &mut fields {
        rewrite_info(info, queries);
    }
    let fields: Arc<[FieldInfo]> = fields.into();
    shapes.insert(key, fields.clone());
    scope.fields = fields;
}
