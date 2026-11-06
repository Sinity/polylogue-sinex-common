//! Capture the parser's scopes. Walking the returned tree assigns addresses;
//! it never reconstructs group, projection, or aggregate transitions.
use super::*;
use crate::scope::{
    FieldInfo, NodePath, Origin, QueryScope, StageScope, TypeRule, TypedOperand, Typing,
};

pub(super) struct Capture {
    pub source: Scope,
    pub stages: Vec<(Scope, Scope)>,
}
impl Parser<'_> {
    pub(super) fn stage_path(&self) -> NodePath {
        let (query, stage) = self
            .query_stack
            .last()
            .copied()
            .expect("stage belongs to query");
        NodePath::default()
            .field("$query")
            .index(query)
            .field("stages")
            .index(stage)
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
    pub(super) fn operand_info(&self, scope: &Scope, operand: &Operand) -> FieldInfo {
        match operand {
            Operand::Field { name } => self.field_info(scope, name),
            Operand::Function { name, arguments } => {
                let arguments = if name == "random" {
                    Vec::new()
                } else {
                    arguments
                        .iter()
                        .map(|arg| self.operand_info(scope, arg))
                        .collect()
                };
                let kind = crate::scope::function_kind(name);
                FieldInfo {
                    field: Field {
                        name: operand.column(),
                        kind,
                        ..Field::default()
                    },
                    origin: Origin::Function {
                        name: name.clone(),
                        arguments,
                        rule: TypeRule::Fixed,
                    },
                }
            }
        }
    }
    pub(super) fn generated_field(
        &self,
        scope: &mut Scope,
        name: &str,
        kind: FieldKind,
        origin: Origin,
    ) {
        Arc::make_mut(&mut scope.typed_extra).retain(|field| field.name != name);
        Arc::make_mut(&mut scope.typed_extra).push(Field {
            name: name.into(),
            kind,
            ..Field::default()
        });
        Arc::make_mut(&mut scope.origins).insert(name.into(), origin);
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
        let mut field = if rule == TypeRule::Input {
            inputs
                .first()
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
    fn snapshot(&self, scope: &Scope) -> crate::scope::Scope {
        let mut names: BTreeSet<_> = if let Some(columns) = &scope.columns {
            columns.clone()
        } else if let Some(output) = &scope.output {
            output
                .fields
                .iter()
                .map(|field| field.name.clone())
                .collect()
        } else if scope.deferred {
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
        crate::scope::Scope {
            unit: self.catalog.unit(&scope.unit).map(|unit| unit.name.clone()),
            fields: names
                .iter()
                .map(|name| self.field_info(scope, name))
                .collect::<Vec<_>>()
                .into(),
        }
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
        for stage in &mut query.stages {
            rewrite_scope(&mut stage.before, &paths, &mut shapes);
            rewrite_scope(&mut stage.after, &paths, &mut shapes);
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
        self.operand(&Operand::field(name), scope, path);
    }
    fn operand(&mut self, operand: &Operand, scope: &Scope, path: NodePath) {
        self.typing.operands.push(TypedOperand {
            path: path.clone(),
            unit: self
                .parser
                .catalog
                .unit(&scope.unit)
                .map(|unit| unit.name.clone()),
            result: self.parser.operand_info(scope, operand),
        });
        if let Operand::Function { name, arguments } = operand
            && name != "random"
        {
            for (index, argument) in arguments.iter().enumerate() {
                self.operand(argument, scope, path.field("arguments").index(index));
            }
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
        let stages = capture
            .stages
            .iter()
            .map(|(before, after)| {
                let next = if before == after {
                    previous.clone()
                } else {
                    self.parser.snapshot(after)
                };
                let stage = StageScope {
                    before: previous.clone(),
                    after: next.clone(),
                };
                previous = next;
                stage
            })
            .collect();
        self.typing.queries.push(QueryScope {
            path: path.clone(),
            source,
            stages,
        });
        match &query.source {
            Source::Unit {
                condition: Some(condition),
                ..
            }
            | Source::RecallLast {
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
        for (index, stage) in query.stages.iter().enumerate() {
            let (scope, after) = &capture.stages[index];
            let at = path.field("stages").index(index).field("kind");
            match &stage.kind {
                StageKind::Where { condition } => {
                    self.condition(condition, scope, at.field("condition"))
                }
                StageKind::Sort { keys } => {
                    for (index, key) in keys.iter().enumerate() {
                        let path = at.field("keys").index(index).field("operand");
                        if matches!(&key.operand, Operand::Field { name } if name == "rank") {
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
                            self.operand(&key.operand, scope, path);
                        }
                    }
                }
                StageKind::Group { keys } => {
                    for (index, key) in keys.iter().enumerate() {
                        self.operand(key, scope, at.field("keys").index(index));
                    }
                }
                StageKind::Measure { by, .. } => {
                    for (index, key) in by.iter().enumerate() {
                        self.operand(key, scope, at.field("by").index(index));
                    }
                }
                StageKind::Aggregate { aggregates } => {
                    let rows = scope.group_input.as_deref().unwrap_or(scope);
                    for (index, aggregate) in aggregates.iter().enumerate() {
                        for (field_index, field) in aggregate.fields.iter().enumerate() {
                            self.field(
                                field,
                                rows,
                                at.field("aggregates")
                                    .index(index)
                                    .field("fields")
                                    .index(field_index),
                            );
                        }
                    }
                }
                StageKind::Fields { fields } => {
                    for (index, field) in fields.iter().enumerate() {
                        self.field(field, scope, at.field("fields").index(index));
                    }
                }
                StageKind::Scope {
                    condition: Some(condition),
                    ..
                } => self.condition(condition, after, at.field("condition")),
                StageKind::Combine { query, .. } | StageKind::Temporal { query, .. } => {
                    self.query(query, at.field("query"))
                }
                StageKind::Extension { arguments, .. } => {
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
            ConditionKind::Match { field, .. } => self.field(field, scope, at.field("field")),
            ConditionKind::Compare { operand, .. }
            | ConditionKind::Since { operand, .. }
            | ConditionKind::Until { operand, .. }
            | ConditionKind::Between { operand, .. }
            | ConditionKind::FieldExists { operand }
            | ConditionKind::TimeRange { operand, .. } => {
                self.operand(operand, scope, at.field("operand"))
            }
            ConditionKind::In { operand, query } => {
                self.operand(operand, scope, at.field("operand"));
                self.query(query, at.field("query"));
            }
            ConditionKind::TimeDifference { left, right, .. } => {
                self.operand(left, scope, at.field("left"));
                self.operand(right, scope, at.field("right"));
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
                ExtensionArgument::Condition(condition) => {
                    self.condition(condition, scope, at.field("condition"))
                }
                ExtensionArgument::Query(query) => self.query(query, at.field("query")),
                ExtensionArgument::Optional(arguments) => {
                    self.arguments(arguments, scope, at.field("optional"))
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
        Origin::Temporal { query, .. } => rewrite_path(query, queries),
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
