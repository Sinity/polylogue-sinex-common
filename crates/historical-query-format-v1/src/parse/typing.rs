//! Field kinds and origins used to check historical query suffixes.
use super::*;
use crate::scope::{FieldInfo, NodePath, Origin, TypeRule};

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
}
