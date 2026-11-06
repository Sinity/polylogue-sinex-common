use query_language::{
    ast::*,
    bind::bind,
    format::format_query,
    identity::{normal_form, question_id},
    parse::parse,
    semantics::*,
};
use std::collections::BTreeMap;
mod support;

#[test]
fn explicit_witness_keys_bind_inner_output_and_local_candidate_metadata() {
    for (catalog, unit, time) in [
        (support::polylogue(), "tool_calls", "at"),
        (support::sinex(), "events", "when"),
    ] {
        let nearest = parse(&format!("{unit} where near({unit}) within 5m"), &catalog).unwrap();
        let mut explicit = nearest.clone();
        let Source::Unit {
            condition: Some(condition),
            ..
        } = &mut explicit.source
        else {
            panic!()
        };
        let ConditionKind::Order { pick, .. } = &mut condition.kind else {
            panic!()
        };
        *pick = WitnessChoice::FirstBy {
            keys: vec![key("match.id", false)],
        };
        assert_ne!(
            question_id(&nearest, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&explicit, &catalog, &BTreeMap::new()).unwrap()
        );
        let wire = serde_json::to_value(&nearest).unwrap();
        assert!(wire["source"]["condition"]["kind"].get("pick").is_none());
        assert_eq!(serde_json::from_value::<Query>(wire).unwrap(), nearest);
        let text = format!(
            "{unit} where near({unit} | fields id, {time} as selected) within 5m pick first by abs(match.distance), selected desc, match.id"
        );
        let query = parse(&text, &catalog).unwrap();
        let bound = bind(&query, &catalog).unwrap();
        let rendered = format_query(&query, &catalog).unwrap();
        assert_eq!(
            normal_form(&query, &catalog).unwrap(),
            normal_form(&parse(&rendered, &catalog).unwrap(), &catalog).unwrap()
        );
        assert_ne!(
            question_id(&nearest, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&query, &catalog, &BTreeMap::new()).unwrap()
        );
        assert!(bound.typing().operands.iter().any(|operand| {
            operand.result.field.kind == query_language::catalog::FieldKind::Duration
                && operand.path.0.iter().any(|element| {
                    matches!(element,
                    query_language::scope::PathElement::Field(name) if name == "pick")
                })
        }));
        let bad = format!("{unit} where near({unit} | fields id) within 5m pick first by {time}");
        assert!(parse(&bad, &catalog).is_err());
        assert!(
            parse(
                &format!("{unit} where near({unit}) within 5m pick first by"),
                &catalog
            )
            .is_err()
        );
        assert!(parse(&format!("{unit} | fields abs({time})"), &catalog).is_err());
    }
}
fn point(tick: i64) -> Extent {
    Extent {
        start: tick,
        end: tick,
        coarse: false,
    }
}
fn ordered(pick: WitnessChoice) -> Condition {
    Condition {
        span: Span::default(),
        kind: ConditionKind::Order {
            relation: OrderRelation::Near,
            query: Box::new(Query {
                source: Source::Unit {
                    unit: "inner".into(),
                    implicit: false,
                    condition: None,
                },
                attach: vec![],
                transforms: vec![],
            }),
            within: Some(Distance::Time {
                duration: Value::scalar("18446744073709551615"),
            }),
            at: TimePoint::When,
            scope: None,
            pick,
        },
    }
}
fn key(name: &str, descending: bool) -> SortKey {
    SortKey {
        expression: Expression::field(name),
        descending,
    }
}
fn outer_query(condition: Condition) -> Query {
    Query {
        source: Source::Unit {
            unit: "outer".into(),
            implicit: false,
            condition: Some(condition),
        },
        attach: vec![],
        transforms: vec![],
    }
}
#[test]
fn witness_choice_keeps_outer_grain_and_truth_while_changing_only_selected_match() {
    let mut first = Row::new("outer:a");
    first.time = Some(point(10));
    first.parent = Some("scope".into());
    let mut second = Row::new("outer:b");
    second.time = Some(point(20));
    let mut z = Row::new("z").with("score", 1.);
    z.time = Some(point(9));
    let mut a = Row::new("a").with("score", 2.);
    a.time = Some(point(11));
    let mut coarse = Row::new("coarse");
    coarse.time = Some(Extent {
        start: 8,
        end: 12,
        coarse: true,
    });
    let data = BTreeMap::from([
        ("outer".into(), vec![second.clone(), first.clone()]),
        (
            "inner".into(),
            vec![coarse, z, a, Row::new("unknown").with("score", 0.)],
        ),
    ]);
    let evaluator = ReferenceEvaluator::new(&data);
    let nearest = evaluator
        .execute(&outer_query(ordered(WitnessChoice::Nearest)))
        .unwrap();
    assert_eq!(
        nearest
            .rows
            .iter()
            .map(|row| row.value("match.id"))
            .collect::<Vec<_>>(),
        [Scalar::from("a"), Scalar::from("a")]
    );
    let selected = evaluator
        .execute(&outer_query(ordered(WitnessChoice::FirstBy {
            keys: vec![key("score", false)],
        })))
        .unwrap();
    assert_eq!(
        selected
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["outer:a", "outer:b"]
    );
    assert!(
        selected
            .rows
            .iter()
            .all(|row| row.value("match.id") == Scalar::from("z"))
    );
    assert_eq!(selected.rows[0].time, first.time);
    assert_eq!(selected.rows[0].parent, first.parent);
    // Missing score sorts last in both directions; a real candidate outranks an unknown relation.
    let descending = evaluator
        .execute(&outer_query(ordered(WitnessChoice::FirstBy {
            keys: vec![key("score", true)],
        })))
        .unwrap();
    assert!(
        descending
            .rows
            .iter()
            .all(|row| row.value("match.id") == Scalar::from("a"))
    );
    let unknowns = BTreeMap::from([("inner".into(), vec![Row::new("unknown")])]);
    let condition = ordered(WitnessChoice::FirstBy {
        keys: vec![key("match.id", false)],
    });
    assert_eq!(
        ReferenceEvaluator::new(&unknowns)
            .condition(&first, &condition)
            .unwrap(),
        Truth::Unknown
    );
    let empty = ordered(WitnessChoice::FirstBy { keys: vec![] });
    assert!(evaluator.execute(&outer_query(empty)).is_err());
}
#[test]
fn absolute_candidate_distance_sort_retains_exact_ticks_beyond_f64_integer_precision() {
    let mut outer = Row::new("outer");
    outer.time = Some(point(0));
    let mut closest = Row::new("z");
    closest.time = Some(point(18_014_398_509_481_984));
    let mut farther = Row::new("a");
    farther.time = Some(point(18_014_398_509_481_985));
    let data = BTreeMap::from([
        ("outer".into(), vec![outer]),
        ("inner".into(), vec![farther, closest]),
    ]);
    let choice = WitnessChoice::FirstBy {
        keys: vec![SortKey {
            expression: Expression::Function {
                name: "abs".into(),
                arguments: vec![Expression::field("match.distance")],
            },
            descending: false,
        }],
    };
    let answer = ReferenceEvaluator::new(&data)
        .execute(&outer_query(ordered(choice)))
        .unwrap();
    assert_eq!(answer.rows[0].value("match.id"), Scalar::from("z"));
}

#[test]
fn absolute_payload_values_defer_json_narrowing_to_the_product() {
    use query_language::catalog::{Catalog, Field, FieldKind, Unit};
    let mut catalog = Catalog::new("events");
    catalog.units.push(Unit {
        name: "events".into(),
        fields: vec![Field {
            name: "payload.exit_code".into(),
            kind: FieldKind::Json,
            ..Field::default()
        }],
        ..Unit::default()
    });
    let query = parse("events | fields abs(payload.exit_code)", &catalog).unwrap();
    bind(&query, &catalog).unwrap();
    let expression = Expression::Function {
        name: "abs".into(),
        arguments: vec![Expression::field("payload.exit_code")],
    };
    let data = BTreeMap::new();
    let evaluator = ReferenceEvaluator::new(&data);
    assert_eq!(
        evaluator
            .expression(&Row::new("unknown"), &expression)
            .unwrap(),
        Scalar::Unknown
    );
    assert_eq!(
        evaluator
            .expression(
                &Row::new("number").with("payload.exit_code", -7.),
                &expression
            )
            .unwrap(),
        Scalar::Number(7.)
    );
    assert!(
        evaluator
            .expression(
                &Row::new("text").with("payload.exit_code", "text"),
                &expression
            )
            .is_err()
    );
}

#[test]
fn condition_order_is_commutative_and_witness_publication_happens_after_its_predicate() {
    let catalog = support::sinex();
    let mut rows = vec![];
    for (id, source, at) in [
        ("outer", "outer", 10),
        ("prior", "prior", 9),
        ("a", "A", 11),
        ("b", "B", 12),
    ] {
        let mut row = Row::new(id)
            .with("source", source)
            .with("when", f64::from(at));
        row.time = Some(point(i64::from(at)));
        rows.push(row);
    }
    let data = BTreeMap::from([("events".into(), rows)]);
    let evaluator = ReferenceEvaluator::new(&data);
    let prefix =
        "events where source = outer AND near(events where source = prior) within 5m | where ";
    let a = "near(events where source = A) within 5m";
    let b = "near(events where source = B) within 5m";
    for (first, second, witness) in [
        (format!("{a} AND {b}"), format!("{b} AND {a}"), "prior"),
        (format!("{a} OR {b}"), format!("{b} OR {a}"), "prior"),
        (
            format!("{a} AND match.source = prior"),
            format!("match.source = prior AND {a}"),
            "a",
        ),
    ] {
        let left = parse(
            &format!("{prefix}{first} | fields id, match.id as witness"),
            &catalog,
        )
        .unwrap();
        let right = parse(
            &format!("{prefix}{second} | fields id, match.id as witness"),
            &catalog,
        )
        .unwrap();
        bind(&left, &catalog).unwrap();
        bind(&right, &catalog).unwrap();
        assert_eq!(
            question_id(&left, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&right, &catalog, &BTreeMap::new()).unwrap()
        );
        let mut left = left;
        let mut right = right;
        resolve_test_durations(&mut left);
        resolve_test_durations(&mut right);
        let answer = evaluator.execute(&left).unwrap();
        assert_eq!(answer.rows, evaluator.execute(&right).unwrap().rows);
        assert_eq!(answer.rows.len(), 1);
        assert_eq!(answer.rows[0].value("witness"), Scalar::from(witness));
        let TransformKind::Where { condition } = &left.transforms[0].kind else {
            panic!()
        };
        assert_eq!(condition.exposed_order().is_some(), witness == "a");
    }
}
fn resolve_test_durations(query: &mut Query) {
    fn resolve_condition(node: &mut Condition) {
        match &mut node.kind {
            ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
                for child in conditions {
                    resolve_condition(child);
                }
            }
            ConditionKind::Not { condition } => resolve_condition(condition),
            ConditionKind::Order { query, within, .. } => {
                if let Some(Distance::Time { duration }) = within {
                    *duration = Value::scalar(
                        query_language::time::duration_ms(duration.text().unwrap())
                            .unwrap()
                            .to_string(),
                    );
                }
                resolve_test_durations(query);
            }
            _ => {}
        }
    }
    if let Source::Unit {
        condition: Some(filter),
        ..
    } = &mut query.source
    {
        resolve_condition(filter);
    }
    for transform in &mut query.transforms {
        if let TransformKind::Where { condition: filter } = &mut transform.kind {
            resolve_condition(filter);
        }
    }
}

#[test]
fn question_identity_keeps_order_occurrences_and_nonpublishing_boolean_wrappers() {
    let catalog = support::sinex();
    let mut rows = vec![];
    for (id, source, at) in [
        ("outer", "outer", 10),
        ("prior", "prior", 9),
        ("chosen", "chosen", 11),
    ] {
        let mut row = Row::new(id).with("source", source);
        row.time = Some(point(at));
        rows.push(row);
    }
    let data = BTreeMap::from([("events".into(), rows)]);
    let evaluator = ReferenceEvaluator::new(&data);
    let prefix =
        "events where source = outer AND near(events where source = prior) within 5m | where ";
    let order = "near(events where source = chosen) within 5m";
    let mut single_id = None;
    for (predicate, witness) in [
        (order.into(), "chosen"),
        (format!("{order} AND {order}"), "prior"),
        (format!("{order} OR {order}"), "prior"),
        (format!("NOT (NOT ({order}))"), "prior"),
    ] {
        let query = parse(
            &format!("{prefix}{predicate} | fields id, match.id as witness"),
            &catalog,
        )
        .unwrap();
        let id = question_id(&query, &catalog, &BTreeMap::new()).unwrap();
        if let Some(single_id) = single_id {
            assert_ne!(id, single_id);
        } else {
            single_id = Some(id);
        }
        let bound = bind(&query, &catalog).unwrap().into_query();
        assert_eq!(id, question_id(&bound, &catalog, &BTreeMap::new()).unwrap());
        let rendered = format_query(&query, &catalog).unwrap();
        let reparsed = parse(&rendered, &catalog).unwrap();
        assert_eq!(
            normal_form(&query, &catalog).unwrap(),
            normal_form(&reparsed, &catalog).unwrap()
        );
        let mut resolved = bound;
        resolve_test_durations(&mut resolved);
        let answer = evaluator.execute(&resolved).unwrap();
        assert_eq!(answer.rows.len(), 1);
        assert_eq!(answer.rows[0].value("witness"), Scalar::from(witness));
    }
    let mut singleton = parse(
        &format!("{prefix}{order} | fields id, match.id as witness"),
        &catalog,
    )
    .unwrap();
    let TransformKind::Where { condition } = &mut singleton.transforms[0].kind else {
        panic!()
    };
    *condition = Condition {
        span: Span::default(),
        kind: ConditionKind::Or {
            conditions: vec![condition.clone()],
        },
    };
    let rendered = format_query(&singleton, &catalog).unwrap();
    let reparsed = parse(&rendered, &catalog).unwrap();
    assert_eq!(
        normal_form(&singleton, &catalog).unwrap(),
        normal_form(&reparsed, &catalog).unwrap()
    );
    let mut resolved = bind(&singleton, &catalog).unwrap().into_query();
    resolve_test_durations(&mut resolved);
    assert_eq!(
        evaluator.execute(&resolved).unwrap().rows[0].value("witness"),
        Scalar::from("prior")
    );
    // An order inside a membership query does not produce its caller's match.
    let membership = "source in (events where near(events) within 5m | fields source)";
    let one = parse(&format!("events where {membership}"), &catalog).unwrap();
    let duplicate = parse(
        &format!("events where ({membership}) AND ({membership})"),
        &catalog,
    )
    .unwrap();
    assert_eq!(
        question_id(&one, &catalog, &BTreeMap::new()).unwrap(),
        question_id(&duplicate, &catalog, &BTreeMap::new()).unwrap()
    );
    let pure = parse("events where source = outer", &catalog).unwrap();
    for predicate in [
        "source = outer AND source = outer",
        "source = outer OR source = outer",
    ] {
        let duplicate = parse(&format!("events where {predicate}"), &catalog).unwrap();
        assert_eq!(
            question_id(&pure, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&duplicate, &catalog, &BTreeMap::new()).unwrap()
        );
    }
}
