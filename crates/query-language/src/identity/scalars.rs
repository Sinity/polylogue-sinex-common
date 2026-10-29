//! Type-aware scalar spelling, preserving numeric-looking text as text.
use crate::{
    ast::*,
    catalog::{Catalog, Field, FieldKind, UnknownRule},
};
use std::collections::BTreeMap;
type Fields = BTreeMap<String, Field>;
fn fields(catalog: &Catalog, unit: &str) -> Fields {
    catalog
        .unit(unit)
        .map(|u| {
            u.fields
                .iter()
                .cloned()
                .map(|f| (f.name.clone(), f))
                .collect()
        })
        .unwrap_or_default()
}
fn field(scope: &Fields, operand: &Operand) -> Option<Field> {
    match operand {
        Operand::Field { name } => scope.get(name).cloned(),
        Operand::Function { name, .. } => Some(Field {
            name: operand.column(),
            kind: crate::scope::function_kind(name),
            ..Field::default()
        }),
    }
}
fn value(value: &mut Value, field: &Field) {
    match value {
        Value::Set { values } => {
            for value in values {
                self::value(value, field);
            }
        }
        Value::Scalar { text, quoted } => {
            if !*quoted
                && matches!(&field.unknown,UnknownRule::Word(word) if word.eq_ignore_ascii_case(text))
            {
                *value = Value::Unknown;
                return;
            }
            if let Some(canonical) = field
                .values
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(text))
            {
                *text = canonical.clone();
            }
            let kind = match &field.kind {
                FieldKind::List(kind) => kind.as_ref(),
                kind => kind,
            };
            let normalized = match kind {
                FieldKind::Integer | FieldKind::Number => crate::number::canonical(text),
                FieldKind::Money => crate::number::canonical(
                    text.strip_suffix("usd")
                        .unwrap_or(text)
                        .strip_prefix('$')
                        .unwrap_or(text.strip_suffix("usd").unwrap_or(text)),
                )
                .map(|s| format!("{s}usd")),
                FieldKind::Duration => {
                    crate::time::duration(text).map(|d| format!("{}ns", d.as_nanos()))
                }
                FieldKind::Bool
                    if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") =>
                {
                    Some(text.to_ascii_lowercase())
                }
                FieldKind::Time => text
                    .parse::<jiff::Timestamp>()
                    .ok()
                    .map(|at| at.to_string()),
                _ => None,
            };
            if let Some(normalized) = normalized {
                *text = normalized;
            }
        }
        _ => {}
    }
}
fn condition(c: &mut Condition, scope: &Fields, catalog: &Catalog) {
    match &mut c.kind {
        ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
            for c in conditions {
                condition(c, scope, catalog);
            }
        }
        ConditionKind::Not { condition: c } => condition(c, scope, catalog),
        ConditionKind::Match {
            field: name,
            value: v,
        } => {
            if let Some(f) = scope.get(name) {
                value(v, f);
            }
        }
        ConditionKind::Compare {
            operand, value: v, ..
        }
        | ConditionKind::Since { operand, value: v }
        | ConditionKind::Until { operand, value: v } => {
            if let Some(f) = field(scope, operand) {
                value(v, &f);
            }
        }
        ConditionKind::Between { operand, from, to } => {
            if let Some(f) = field(scope, operand) {
                value(from, &f);
                value(to, &f);
            }
        }
        ConditionKind::Exists { unit, condition: c }
        | ConditionKind::Count {
            unit, condition: c, ..
        } => {
            if let Some(c) = c {
                condition(c, &fields(catalog, unit), catalog);
            }
        }
        ConditionKind::In { query, .. } => canonicalize(query, catalog),
        ConditionKind::Sequence {
            unit, steps, gaps, ..
        } => {
            for c in steps {
                condition(c, &fields(catalog, unit), catalog);
            }
            for gap in gaps {
                if let Gap::Within { duration } = gap {
                    duration_value(duration);
                }
            }
        }
        ConditionKind::TimeDifference { value, .. } => duration_value(value),
        ConditionKind::Extension { arguments, .. } => arguments
            .iter_mut()
            .for_each(|a| argument(a, scope, catalog)),
        _ => {}
    }
}
fn duration_value(v: &mut Value) {
    value(
        v,
        &Field {
            kind: FieldKind::Duration,
            ..Field::default()
        },
    );
}
fn argument(a: &mut ExtensionArgument, scope: &Fields, catalog: &Catalog) {
    match a {
        ExtensionArgument::Condition(c) => condition(c, scope, catalog),
        ExtensionArgument::Query(q) => canonicalize(q, catalog),
        ExtensionArgument::Optional(arguments) => arguments
            .iter_mut()
            .for_each(|a| argument(a, scope, catalog)),
        _ => {}
    }
}
pub(super) fn canonicalize(query: &mut Query, catalog: &Catalog) {
    let mut scope = match &query.source {
        Source::Unit { unit, .. } | Source::RecallLast { unit, .. } => fields(catalog, unit),
        Source::Saved { .. } | Source::Result { .. } => catalog
            .source_output(&query.source)
            .map(|output| {
                output
                    .fields
                    .iter()
                    .cloned()
                    .map(|f| (f.name.clone(), f))
                    .collect()
            })
            .unwrap_or_default(),
        _ => fields(catalog, &catalog.default_unit),
    };
    match &mut query.source {
        Source::Unit {
            unit,
            implicit,
            condition: c,
        } => {
            if let Some(u) = catalog.unit(unit) {
                *unit = u.name.clone();
                *implicit = false;
            }
            if let Some(c) = c {
                condition(c, &scope, catalog);
            }
        }
        Source::RecallLast {
            condition: Some(c), ..
        }
        | Source::Saved {
            condition: Some(c), ..
        }
        | Source::Result {
            condition: Some(c), ..
        } => condition(c, &scope, catalog),
        Source::Recall {
            window: Some(window),
            ..
        }
        | Source::RecallMark {
            window: Some(window),
            ..
        } => duration_value(window),
        _ => {}
    }
    for attachment in &mut query.attach {
        for window in &mut attachment.windows {
            if let Window::Where(c) = window {
                condition(c, &fields(catalog, &attachment.unit), catalog);
            }
        }
    }
    let mut group_input = None;
    for stage in &mut query.stages {
        match &mut stage.kind {
            StageKind::Where { condition: c } => {
                condition(c, &scope, catalog);
                group_input = None;
            }
            StageKind::Scope { unit, condition: c } => {
                group_input = None;
                scope = fields(catalog, unit);
                if let Some(c) = c {
                    condition(c, &scope, catalog);
                }
            }
            StageKind::Group { keys } => {
                group_input = Some(scope.clone());
                scope = keys
                    .iter()
                    .filter_map(|k| field(&scope, k).map(|f| (k.column(), f)))
                    .collect();
                scope.insert(
                    "count".into(),
                    Field {
                        name: "count".into(),
                        kind: FieldKind::Integer,
                        ..Field::default()
                    },
                );
            }
            StageKind::Aggregate { aggregates } => {
                let grouped = group_input.is_some();
                let input = group_input.take().unwrap_or_else(|| scope.clone());
                if !grouped {
                    scope.clear();
                }
                for aggregate in aggregates {
                    let (kind, _) = crate::scope::aggregate_kind(
                        &aggregate.function,
                        aggregate
                            .fields
                            .first()
                            .and_then(|name| input.get(name))
                            .map(|field| &field.kind),
                    );
                    let name = aggregate.column();
                    scope.insert(
                        name.clone(),
                        Field {
                            name,
                            kind,
                            ..Field::default()
                        },
                    );
                }
            }
            StageKind::Fields { fields } => {
                scope.retain(|name, _| fields.contains(name));
                group_input = None;
            }
            StageKind::Combine { query, .. } => {
                canonicalize(query, catalog);
                group_input = None;
            }
            StageKind::Temporal { query, within, .. } => {
                canonicalize(query, catalog);
                if let Some(within) = within {
                    duration_value(within);
                }
                group_input = None;
            }
            StageKind::Extension { arguments, .. } => arguments
                .iter_mut()
                .for_each(|a| argument(a, &scope, catalog)),
            _ => {
                group_input = None;
            }
        }
    }
}
