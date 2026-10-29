mod support;
use query_language::{
    ast::*,
    bind::{BoundQuery, bind},
    catalog::*,
    identity::question_id,
    parse::parse,
    scope::{NodePath, Origin, PathElement, TypeRule},
};
use std::{collections::BTreeMap, sync::Arc};

fn field(name: &str, kind: FieldKind) -> Field {
    Field {
        name: name.into(),
        kind,
        ..Field::default()
    }
}
fn catalog(unit: &str) -> Catalog {
    let mut catalog = Catalog::new(unit);
    catalog.units.push(Unit {
        name: unit.into(),
        main_time: Some("when".into()),
        fields: vec![
            field("id", FieldKind::Reference),
            field("cwd", FieldKind::Text),
            field("command", FieldKind::Text),
            field("n", FieldKind::Integer),
            field("cost", FieldKind::Money),
            field("elapsed", FieldKind::Duration),
            field("when", FieldKind::Time),
            field("payload.key", FieldKind::Json),
            Field {
                name: "state".into(),
                kind: FieldKind::Text,
                values: vec!["open".into(), "closed".into()],
                ..Field::default()
            },
        ],
        ..Unit::default()
    });
    catalog.units.push(Unit {
        name: "children".into(),
        fields: vec![field("command", FieldKind::Integer)],
        ..Unit::default()
    });
    catalog
}
fn checked(text: &str, catalog: &Catalog) -> BoundQuery {
    bind(&parse(text, catalog).unwrap(), catalog).unwrap()
}
fn stage(index: usize) -> NodePath {
    NodePath::default()
        .field("stages")
        .index(index)
        .field("kind")
}
fn value_at<'a>(root: &'a serde_json::Value, path: &NodePath) -> &'a serde_json::Value {
    path.0.iter().fold(root, |value, part| match part {
        PathElement::Field(name) => &value[name],
        PathElement::Index(index) => &value[*index],
    })
}
#[test]
fn group_scopes_keep_types_closed_values_lineage_and_raw_text() {
    for unit in ["sessions", "events"] {
        let catalog = catalog(unit);
        let text = format!(
            "{unit} | group by cwd,state | agg count,min(command),avg(n),avg(cost),avg(elapsed),histogram(state),corr(n,n) | where cwd:001 | group by cwd | sum(count)"
        );
        let bound = checked(&text, &catalog);
        let scopes = &bound.typing().queries[0];
        let grouped = &scopes.stages[0].after;
        assert_eq!(grouped.unit.as_deref(), Some(unit));
        assert_eq!(
            grouped
                .fields
                .iter()
                .map(|field| field.field.name.as_str())
                .collect::<Vec<_>>(),
            ["count", "cwd", "state"]
        );
        assert_eq!(grouped.field("cwd").unwrap().field.kind, FieldKind::Text);
        assert_eq!(
            grouped.field("state").unwrap().field.values,
            ["open", "closed"]
        );
        assert!(matches!(&grouped.field("cwd").unwrap().origin,
            Origin::Group { operand, input } if operand == &stage(0).field("keys").index(0)
                && matches!(&input.origin, Origin::Catalog { unit: actual, field } if actual == unit && field == "cwd")));
        let aggregates = &scopes.stages[1].after;
        for (name, kind) in [
            ("count", FieldKind::Integer),
            ("min_command", FieldKind::Text),
            ("avg_n", FieldKind::Number),
            ("avg_cost", FieldKind::Money),
            ("avg_elapsed", FieldKind::Duration),
            ("histogram_state", FieldKind::Json),
            ("corr_n_n", FieldKind::Number),
        ] {
            assert_eq!(aggregates.field(name).unwrap().field.kind, kind, "{name}");
        }
        let final_scope = &scopes.stages.last().unwrap().after;
        assert_eq!(
            final_scope.field("sum_count").unwrap().field.kind,
            FieldKind::Integer
        );
        assert!(matches!(&final_scope.field("sum_count").unwrap().origin,
            Origin::Aggregate { inputs, rule: TypeRule::Input, .. }
                if inputs[0].field.name == "count" && inputs[0].field.kind == FieldKind::Integer));
        let tree = serde_json::to_value(bound.query()).unwrap();
        assert_eq!(
            tree["stages"][2]["kind"]["condition"]["kind"]["value"]["text"],
            "001"
        );
        for operand in &bound.typing().operands {
            assert!(
                !value_at(&tree, &operand.path).is_null(),
                "{:?}",
                operand.path
            );
        }
        let one = checked(&format!("{unit} | group by cwd | where cwd:1"), &catalog);
        let padded = checked(&format!("{unit} | group by cwd | where cwd:001"), &catalog);
        assert_ne!(
            question_id(one.query(), &catalog, &BTreeMap::new()).unwrap(),
            question_id(padded.query(), &catalog, &BTreeMap::new()).unwrap()
        );
    }
}
#[test]
fn repeated_groups_have_distinct_structural_origins_and_json_stays_json() {
    let catalog = catalog("events");
    let bound = checked(
        "events | group by payload.key | min(payload.key) | group by min_payload_key | where min_payload_key:001",
        &catalog,
    );
    let scopes = &bound.typing().queries[0];
    let final_field = scopes.stages[2].after.field("min_payload_key").unwrap();
    assert_eq!(final_field.field.kind, FieldKind::Json);
    let Origin::Group { operand, input } = &final_field.origin else {
        panic!("group origin")
    };
    assert_eq!(operand, &stage(2).field("keys").index(0));
    let Origin::Aggregate {
        at, inputs, rule, ..
    } = &input.origin
    else {
        panic!("aggregate origin")
    };
    assert_eq!(at, &stage(1).field("aggregates").index(0));
    assert_eq!(*rule, TypeRule::Input);
    assert_eq!(inputs[0].field.name, "payload.key");
    assert!(
        matches!(&inputs[0].origin, Origin::Catalog { unit, field } if unit == "events" && field == "payload.key")
    );
    let (_, retained) = bound.into_parts();
    assert_eq!(retained.queries.len(), 1);
}
#[test]
fn nested_and_related_uses_have_their_actual_unit_and_distinct_addresses() {
    let catalog = catalog("events");
    let bound = checked(
        "events where exists children(command:001) AND count(children where command:002) >= 1 | group by cwd | where cwd in (command:003)",
        &catalog,
    );
    let typing = bound.typing();
    assert_eq!(typing.queries.len(), 2);
    let nested_path = stage(1).field("condition").field("kind").field("query");
    assert_eq!(
        typing.query(&nested_path).unwrap().source.unit.as_deref(),
        Some("events")
    );
    assert!(
        typing
            .query(&nested_path)
            .unwrap()
            .source
            .field("command")
            .is_some()
    );
    let children: Vec<_> = typing
        .operands
        .iter()
        .filter(|operand| operand.unit.as_deref() == Some("children"))
        .collect();
    assert_eq!(children.len(), 2);
    assert!(
        children
            .iter()
            .all(|operand| operand.result.field.kind == FieldKind::Integer)
    );
    assert_ne!(children[0].path, children[1].path);
    let nested = typing
        .operands
        .iter()
        .find(|operand| {
            operand.path
                == nested_path
                    .field("source")
                    .field("condition")
                    .field("kind")
                    .field("field")
        })
        .unwrap();
    assert_eq!(nested.result.field.kind, FieldKind::Text);
    let tree = serde_json::to_value(bound.query()).unwrap();
    for operand in &typing.operands {
        assert!(!value_at(&tree, &operand.path).is_null());
    }
    assert_eq!(
        value_at(&tree, &nested_path)["source"]["condition"]["kind"]["value"]["text"],
        "003"
    );
}
#[test]
fn saved_and_result_scopes_retain_snapshot_fields_and_closed_values() {
    let mut catalog = catalog("events");
    let schema = OutputSchema {
        unit: Some("children".into()),
        fields: vec![Field {
            name: "promoted".into(),
            kind: FieldKind::Text,
            values: vec!["yes".into(), "no".into()],
            ..Field::default()
        }],
        main_time: None,
    };
    catalog.source_outputs = vec![
        SourceOutput {
            source: OutputSource::Saved {
                name: "named".into(),
                arguments: vec![],
            },
            schema: schema.clone(),
        },
        SourceOutput {
            source: OutputSource::Result {
                reference: "result:r".into(),
                part: ResultPart::Rows,
            },
            schema: schema.clone(),
        },
        SourceOutput {
            source: OutputSource::Result {
                reference: "result:r".into(),
                part: ResultPart::Matches,
            },
            schema: OutputSchema {
                fields: vec![field("match_only", FieldKind::Integer)],
                ..schema
            },
        },
    ];
    for source in ["query:named", "result:r"] {
        let bound = checked(
            &format!("{source} | where promoted:YES | sort by promoted"),
            &catalog,
        );
        let scope = &bound.typing().queries[0].source;
        assert_eq!(scope.unit.as_deref(), Some("children"));
        assert_eq!(scope.fields.len(), 1);
        assert_eq!(scope.field("promoted").unwrap().field.values, ["yes", "no"]);
        assert!(matches!(
            scope.field("promoted").unwrap().origin,
            Origin::Output { .. }
        ));
        assert!(Arc::ptr_eq(
            &scope.fields,
            &bound.typing().queries[0].stages[1].after.fields
        ));
        assert!(parse(&format!("{source} | where cwd:001"), &catalog).is_err());
    }
    let matches = checked("result:r.matches | where match_only:001", &catalog);
    assert!(
        matches.typing().queries[0]
            .source
            .field("promoted")
            .is_none()
    );
    assert_eq!(
        matches.typing().queries[0]
            .source
            .field("match_only")
            .unwrap()
            .field
            .kind,
        FieldKind::Integer
    );
}
#[test]
fn promoted_join_fields_and_nested_group_types_come_from_the_checker() {
    let catalog = catalog("events");
    let bound = checked(
        "events | near (events | group by cwd) within 1h | where offset < 5m | group by near | count",
        &catalog,
    );
    let scope = &bound.typing().queries[0].stages[0].after;
    assert_eq!(
        scope.field("near").unwrap().field.kind,
        FieldKind::Reference
    );
    assert_eq!(
        scope.field("offset").unwrap().field.kind,
        FieldKind::Duration
    );
    assert!(
        matches!(&scope.field("near").unwrap().origin, Origin::Temporal { query, all: false, .. } if query == &stage(0).field("query"))
    );
    let nested = bound.typing().query(&stage(0).field("query")).unwrap();
    assert_eq!(
        nested.stages[0].after.field("cwd").unwrap().field.kind,
        FieldKind::Text
    );
}
#[test]
fn spans_and_presentation_do_not_identify_scope_or_operand_addresses() {
    let catalog = catalog("events");
    let a = checked("events | group by cwd | where cwd:001", &catalog);
    let mut tree = a.query().clone();
    for stage in &mut tree.stages {
        stage.span = Span {
            start: 999,
            end: 1000,
        };
    }
    let b = bind(&tree, &catalog).unwrap();
    assert_eq!(a.typing(), b.typing());
    assert_eq!(
        question_id(a.query(), &catalog, &BTreeMap::new()).unwrap(),
        question_id(b.query(), &catalog, &BTreeMap::new()).unwrap()
    );
}

#[test]
fn sequence_and_attachment_conditions_keep_each_declared_scope() {
    let mut catalog = catalog("events");
    catalog.units[0].sequence_unit = Some("children".into());
    let bound = checked(
        "events where seq(command:001 -> command:002) with children(command)[command:003]",
        &catalog,
    );
    assert_eq!(bound.typing().operands.len(), 4);
    assert!(
        bound
            .typing()
            .operands
            .iter()
            .all(|operand| operand.unit.as_deref() == Some("children")
                && operand.result.field.kind == FieldKind::Integer)
    );
    let tree = serde_json::to_value(bound.query()).unwrap();
    for operand in &bound.typing().operands {
        assert!(!value_at(&tree, &operand.path).is_null());
    }
}

#[test]
fn fixed_results_discard_discrete_values_and_min_preserves_them() {
    let catalog = catalog("events");
    let bound = checked(
        "events | group by cwd | agg min(state),max(state),histogram(state),avg(n),sum(n)",
        &catalog,
    );
    let scope = &bound.typing().queries[0].stages[1].after;
    for name in ["min_state", "max_state"] {
        assert_eq!(scope.field(name).unwrap().field.values, ["open", "closed"]);
    }
    for name in ["histogram_state", "avg_n", "sum_n"] {
        assert!(scope.field(name).unwrap().field.values.is_empty());
    }
}

#[test]
fn both_catalogs_keep_calendar_labels_and_hour_numbers_through_binding() {
    for catalog in [support::polylogue(), support::sinex()] {
        let unit = catalog
            .units
            .iter()
            .find(|unit| unit.main_time.is_some())
            .unwrap();
        let time = unit.main_time.as_ref().unwrap();
        for (function, label, kind) in [
            ("weekday", "mon", FieldKind::Text),
            ("month_day", "09-14", FieldKind::Text),
            ("hour", "10", FieldKind::Integer),
        ] {
            let text = format!(
                "{} | group by {function}({time}) | where {function}:{label}",
                unit.name
            );
            let bound = checked(&text, &catalog);
            let output = bound.typing().query(&NodePath::default()).unwrap().stages[0]
                .after
                .field(function)
                .unwrap();
            assert_eq!(output.field.kind, kind, "{text}");
            let StageKind::Where { condition } = &bound.query().stages[1].kind else {
                panic!()
            };
            assert!(matches!(&condition.kind, ConditionKind::Match {
                value: Value::Scalar { text, .. }, ..
            } if text == label));
            let rebound = bind(bound.query(), &catalog).unwrap();
            assert_eq!(
                question_id(bound.query(), &catalog, &BTreeMap::new()).unwrap(),
                question_id(rebound.query(), &catalog, &BTreeMap::new()).unwrap()
            );
        }
    }
}
