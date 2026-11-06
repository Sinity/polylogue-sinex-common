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
