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
fn field(scope: &Fields, expression: &Expression, catalog: &Catalog) -> Option<Field> {
    match expression {
        Expression::Field { name } => scope.get(name).cloned(),
        Expression::Function { name, arguments } if name == "case_eq" || name == "abs" => arguments
            .get(if name == "abs" { 0 } else { 2 })
            .and_then(|argument| field(scope, argument, catalog)),
        Expression::Function { name, .. } => Some(Field {
            name: expression.column(),
            kind: catalog
                .function(name)
                .ok()
                .flatten()
                .map_or(FieldKind::Json, |function| function.result),
            ..Field::default()
        }),
        Expression::Arithmetic {
            operator,
            left,
            right,
        } => Some(Field {
            name: expression.column(),
            kind: crate::scope::arithmetic_kind(
                *operator,
                &field(scope, left, catalog)?.kind,
                &field(scope, right, catalog)?.kind,
            )?,
            ..Field::default()
        }),
        _ => None,
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
fn duration_value(v: &mut Value) {
    value(
        v,
        &Field {
            kind: FieldKind::Duration,
            ..Field::default()
        },
    );
}
fn distance(d: &mut Distance) {
    if let Distance::Time { duration } = d {
        duration_value(duration);
    }
}
/// Literals inside expressions take the spelling of their own literal kind.
fn expression(e: &mut Expression, scope: &Fields, catalog: &Catalog) {
    match e {
        Expression::Value {
            value:
                Value::Scalar {
                    text,
                    quoted: false,
                },
        } if crate::parse::is_duration(text) => {
            if let Some(d) = crate::time::duration(text) {
                *text = format!("{}ns", d.as_nanos());
            }
        }
        Expression::Value {
            value:
                Value::Scalar {
                    text,
                    quoted: false,
                },
        } => {
            if let Some(canonical) = crate::number::canonical(text) {
                *text = canonical;
            }
        }
        Expression::Function { name, arguments } if name == "case_eq" && arguments.len() == 3 => {
            let left = field(scope, &arguments[0], catalog);
            let right = field(scope, &arguments[1], catalog);
            for (index, argument) in arguments.iter_mut().enumerate() {
                let contextual = match index {
                    0 => right.as_ref(),
                    1 => left.as_ref(),
                    _ => None,
                };
                if let (Some(declared), Expression::Value { value: v }) =
                    (contextual, &mut *argument)
                {
                    value(v, declared);
                } else {
                    expression(argument, scope, catalog);
                }
            }
        }
        Expression::Function { arguments, .. } => {
            for argument in arguments {
                expression(argument, scope, catalog);
            }
        }
        Expression::Arithmetic { left, right, .. } => {
            expression(left, scope, catalog);
            expression(right, scope, catalog);
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
        ConditionKind::Compare { left, right, .. } => {
            let declared = field(scope, left, catalog);
            expression(left, scope, catalog);
            match right {
                Expression::Value { value: v } => {
                    if let Some(f) = declared {
                        value(v, &f);
                    }
                }
                other => expression(other, scope, catalog),
            }
        }
        ConditionKind::Since {
            field: name,
            value: v,
        }
        | ConditionKind::Until {
            field: name,
            value: v,
        } => {
            if let Some(f) = scope.get(name) {
                value(v, f);
            }
        }
        ConditionKind::Between { left, from, to } => {
            if let Some(f) = field(scope, left, catalog) {
                value(from, &f);
                value(to, &f);
            }
            expression(left, scope, catalog);
        }
        ConditionKind::TimeRange { left, .. } => expression(left, scope, catalog),
        ConditionKind::Exists { unit, condition: c }
        | ConditionKind::Count {
            unit, condition: c, ..
        } => {
            if let Some(c) = c {
                condition(c, &fields(catalog, unit), catalog);
            }
        }
        ConditionKind::In { left, query } => {
            expression(left, scope, catalog);
            canonicalize(query, catalog);
        }
        ConditionKind::Order {
            query,
            within,
            pick,
            ..
        } => {
            let mut candidate = canonicalize_fields(query, catalog);
            if let WitnessChoice::FirstBy { keys } = pick {
                let distance_kind = if matches!(within, Some(Distance::Steps { .. })) {
                    FieldKind::Integer
                } else {
                    FieldKind::Duration
                };
                for (name, kind) in [
                    ("match.distance", distance_kind),
                    ("match.id", FieldKind::Reference),
                    ("match.coarse", FieldKind::Bool),
                ] {
                    candidate.insert(
                        name.into(),
                        Field {
                            name: name.into(),
                            kind,
                            ..Field::default()
                        },
                    );
                }
                for key in keys {
                    expression(&mut key.expression, &candidate, catalog);
                }
            }
            if let Some(within) = within {
                distance(within);
            }
        }
        ConditionKind::Sequence {
            unit, steps, gaps, ..
        } => {
            for c in steps {
                condition(c, &fields(catalog, unit), catalog);
            }
            for gap in gaps {
                if let Gap::Within { distance: d } = gap {
                    distance(d);
                }
            }
        }
        ConditionKind::Extension { arguments, .. } => arguments
            .iter_mut()
            .for_each(|a| argument(a, scope, catalog)),
        _ => {}
    }
}
fn argument(a: &mut ExtensionArgument, scope: &Fields, catalog: &Catalog) {
    match a {
        ExtensionArgument::Condition(c) => condition(c, scope, catalog),
        ExtensionArgument::Query(q) => canonicalize(q, catalog),
        ExtensionArgument::Expression(e) => expression(e, scope, catalog),
        ExtensionArgument::Optional(arguments) | ExtensionArgument::List(arguments) => arguments
            .iter_mut()
            .for_each(|a| argument(a, scope, catalog)),
        _ => {}
    }
}
pub(super) fn canonicalize(query: &mut Query, catalog: &Catalog) {
    canonicalize_fields(query, catalog);
}
fn canonicalize_fields(query: &mut Query, catalog: &Catalog) -> Fields {
    let mut scope = match &query.source {
        Source::Unit { unit, .. } => fields(catalog, unit),
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
        Source::Saved {
            condition: Some(c), ..
        }
        | Source::Result {
            condition: Some(c), ..
        } => condition(c, &scope, catalog),
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
    for transform in &mut query.transforms {
        match &mut transform.kind {
            TransformKind::Where { condition: c } => {
                condition(c, &scope, catalog);
                group_input = None;
            }
            TransformKind::Scope { unit, condition: c } => {
                group_input = None;
                scope = fields(catalog, unit);
                if let Some(c) = c {
                    condition(c, &scope, catalog);
                }
            }
            TransformKind::Matches => {
                scope.clear();
                group_input = None;
            }
            TransformKind::Sort { keys } => {
                for key in keys {
                    expression(&mut key.expression, &scope, catalog);
                }
                group_input = None;
            }
            TransformKind::Group { keys } => {
                group_input = Some(scope.clone());
                let mut grouped: Fields = keys
                    .iter()
                    .filter_map(|k| {
                        field(&scope, &k.expression, catalog).map(|mut f| {
                            f.name = k.column();
                            (k.column(), f)
                        })
                    })
                    .collect();
                for key in keys.iter_mut() {
                    expression(&mut key.expression, &scope, catalog);
                }
                grouped.insert(
                    "count".into(),
                    Field {
                        name: "count".into(),
                        kind: FieldKind::Integer,
                        ..Field::default()
                    },
                );
                scope = grouped;
            }
            TransformKind::Aggregate { aggregates } => {
                let grouped = group_input.is_some();
                let input = group_input.take().unwrap_or_else(|| scope.clone());
                if !grouped {
                    scope.clear();
                }
                for aggregate in aggregates {
                    let (kind, _) = crate::scope::aggregate_kind(
                        &aggregate.function,
                        aggregate
                            .arguments
                            .first()
                            .and_then(|argument| field(&input, argument, catalog))
                            .map(|field| field.kind)
                            .as_ref(),
                    );
                    for argument in &mut aggregate.arguments {
                        expression(argument, &input, catalog);
                    }
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
            TransformKind::Fields { columns } => {
                let projected: Fields = columns
                    .iter()
                    .filter_map(|column| {
                        field(&scope, &column.expression, catalog).map(|mut f| {
                            f.name = column.column();
                            (column.column(), f)
                        })
                    })
                    .collect();
                for column in columns.iter_mut() {
                    expression(&mut column.expression, &scope, catalog);
                }
                scope = projected;
                group_input = None;
            }
            TransformKind::Combine { query, .. } | TransformKind::Diff { query } => {
                canonicalize(query, catalog);
                group_input = None;
            }
            TransformKind::Join { query, on } => {
                canonicalize(query, catalog);
                if let JoinOn::Order {
                    within: Some(within),
                    ..
                } = on
                {
                    distance(within);
                }
                group_input = None;
            }
            TransformKind::Extension { arguments, .. } => arguments
                .iter_mut()
                .for_each(|a| argument(a, &scope, catalog)),
            _ => {
                group_input = None;
            }
        }
    }
    scope
}
