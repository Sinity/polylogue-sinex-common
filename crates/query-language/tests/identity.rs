use query_language::catalog::{Catalog, Field, FieldKind, Unit};
use query_language::{ast::*, identity::*};
use std::collections::BTreeMap;
fn catalog() -> Catalog {
    let mut c = Catalog::new("events");
    c.units.push(Unit {
        name: "events".into(),
        fields: vec![
            Field {
                name: "source".into(),
                kind: FieldKind::Text,
                ..Field::default()
            },
            Field {
                name: "host".into(),
                kind: FieldKind::Text,
                ..Field::default()
            },
            Field {
                name: "cost".into(),
                kind: FieldKind::Money,
                ..Field::default()
            },
            Field {
                name: "started".into(),
                kind: FieldKind::Time,
                ..Field::default()
            },
            Field {
                name: "ended".into(),
                kind: FieldKind::Time,
                ..Field::default()
            },
            Field {
                name: "duration".into(),
                kind: FieldKind::Duration,
                ..Field::default()
            },
        ],
        ..Unit::default()
    });
    c
}
fn condition(field: &str, value: &str, span: usize) -> Condition {
    Condition {
        span: Span {
            start: span,
            end: span + 1,
        },
        kind: ConditionKind::Match {
            field: field.into(),
            value: Value::scalar(value),
        },
    }
}
fn query(conditions: Vec<Condition>) -> Query {
    Query {
        source: Source::Unit {
            unit: "events".into(),
            implicit: false,
            condition: Some(Condition {
                span: Span::default(),
                kind: ConditionKind::And { conditions },
            }),
        },
        attach: vec![],
        transforms: vec![],
    }
}
#[test]
fn normal_form_ignores_spans_and_commutative_order() {
    let a = query(vec![
        condition("source", "alpha", 0),
        condition("host", "beta", 20),
    ]);
    let b = query(vec![
        condition("host", "beta", 0),
        condition("source", "alpha", 40),
    ]);
    assert_eq!(
        question_id(&a, &catalog(), &BTreeMap::new()).unwrap(),
        question_id(&b, &catalog(), &BTreeMap::new()).unwrap()
    );
    assert!(
        !normal_form(&a, &catalog())
            .unwrap()
            .to_string()
            .contains("span")
    );
}
#[test]
fn selection_and_parameter_types_change_identity() {
    let a = query(vec![condition("source", "alpha", 0)]);
    let mut b = a.clone();
    b.transforms.push(Transform {
        span: Span::default(),
        kind: TransformKind::Limit { count: 2 },
    });
    assert_ne!(
        question_id(&a, &catalog(), &BTreeMap::new()).unwrap(),
        question_id(&b, &catalog(), &BTreeMap::new()).unwrap()
    );
    assert_ne!(
        question_id(
            &a,
            &catalog(),
            &BTreeMap::from([("p".into(), "text".into())])
        )
        .unwrap(),
        question_id(
            &a,
            &catalog(),
            &BTreeMap::from([("p".into(), "duration".into())])
        )
        .unwrap()
    );
}
#[test]
fn normal_form_keeps_relative_time_and_namespace_literals() {
    let a = query(vec![
        condition("since", "7d", 0),
        condition("text", "sx.events", 20),
    ]);
    let text = normal_form(&a, &catalog()).unwrap().to_string();
    assert!(text.contains("7d"));
    assert!(text.contains("sx.events"));
    assert!(a.namespaces().is_empty());
}

#[test]
fn associative_and_idempotent_conditions_share_identity() {
    let a = condition("source", "alpha", 0);
    let b = condition("host", "beta", 1);
    let nested = Condition {
        span: Span::default(),
        kind: ConditionKind::And {
            conditions: vec![a.clone(), b.clone()],
        },
    };
    assert_eq!(
        normal_form(&query(vec![a.clone(), nested]), &catalog()).unwrap(),
        normal_form(&query(vec![a, b]), &catalog()).unwrap()
    );
}

#[test]
fn scalar_spelling_is_canonical_only_for_declared_numeric_types() {
    for (field, a, b) in [("cost", "2.90", "2.9usd"), ("duration", "1.5s", "1500ms")] {
        assert_eq!(
            normal_form(&query(vec![condition(field, a, 0)]), &catalog()).unwrap(),
            normal_form(&query(vec![condition(field, b, 0)]), &catalog()).unwrap()
        );
    }
    assert_ne!(
        normal_form(&query(vec![condition("source", "001", 0)]), &catalog()).unwrap(),
        normal_form(&query(vec![condition("source", "1", 0)]), &catalog()).unwrap()
    );
}

#[test]
fn numeric_group_columns_canonicalize_in_the_result_relation() {
    let mut a = query(vec![]);
    a.transforms = vec![
        Transform {
            span: Span::default(),
            kind: TransformKind::Group {
                keys: vec![Column::field("source")],
            },
        },
        Transform {
            span: Span::default(),
            kind: TransformKind::Aggregate {
                aggregates: vec![Aggregate {
                    function: AggregateFunction::Sum,
                    arguments: vec![Expression::field("cost")],
                    name: None,
                }],
            },
        },
        Transform {
            span: Span::default(),
            kind: TransformKind::Where {
                condition: condition("sum_cost", "2.90", 0),
            },
        },
    ];
    let mut b = a.clone();
    if let TransformKind::Where { condition: c } = &mut b.transforms[2].kind {
        c.kind = ConditionKind::Match {
            field: "sum_cost".into(),
            value: Value::scalar("2.9usd"),
        };
    }
    assert_eq!(
        normal_form(&a, &catalog()).unwrap(),
        normal_form(&b, &catalog()).unwrap()
    );
}

#[test]
fn duration_gap_spellings_and_catalog_unknown_words_share_identity() {
    let sequence = |duration| Query {
        source: Source::Unit {
            unit: "events".into(),
            implicit: false,
            condition: Some(Condition {
                span: Span::default(),
                kind: ConditionKind::Sequence {
                    unit: "events".into(),
                    all: false,
                    steps: vec![
                        condition("source", "alpha", 0),
                        condition("source", "beta", 1),
                    ],
                    gaps: vec![Gap::Within {
                        distance: Distance::Time {
                            duration: Value::scalar(duration),
                        },
                    }],
                },
            }),
        },
        attach: vec![],
        transforms: vec![],
    };
    assert_eq!(
        normal_form(&sequence("1.5s"), &catalog()).unwrap(),
        normal_form(&sequence("1500ms"), &catalog()).unwrap()
    );
    let mut c = catalog();
    c.units[0].fields[0].unknown = query_language::catalog::UnknownRule::Word("missing".into());
    let a = query(vec![condition("source", "missing", 0)]);
    let b = query(vec![Condition {
        span: Span::default(),
        kind: ConditionKind::Match {
            field: "source".into(),
            value: Value::Unknown,
        },
    }]);
    assert_eq!(normal_form(&a, &c).unwrap(), normal_form(&b, &c).unwrap());
}

#[test]
fn stored_normal_form_has_feature_independent_canonical_bytes() {
    let q = query(vec![condition("source", "alpha", 42)]);
    let form = normal_form(&q, &catalog()).unwrap();
    let expected = r#"{"attach":[],"source":{"condition":{"kind":{"field":"source","kind":"match","value":{"kind":"scalar","text":"alpha"}}},"kind":"unit","unit":"events"},"stages":[]}"#;
    assert_eq!(serde_json::to_string(&form).unwrap(), expected);
    // This exact independent byte oracle is also exercised with preserve_order
    // enabled, as it is in product feature unions.
    let bytes = format!("[{expected},{{}}]");
    let expected_id = QuestionId(*blake3::hash(bytes.as_bytes()).as_bytes());
    assert_eq!(
        question_id(&q, &catalog(), &BTreeMap::new()).unwrap(),
        expected_id
    );
    let mut reversed = form.clone();
    fn reverse(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                for value in fields.values_mut() {
                    reverse(value);
                }
                let entries: Vec<_> = std::mem::take(fields).into_iter().rev().collect();
                fields.extend(entries);
            }
            serde_json::Value::Array(values) => values.iter_mut().for_each(reverse),
            _ => {}
        }
    }
    reverse(&mut reversed);
    assert_ne!(reversed.to_string(), expected);
    assert_eq!(
        question_id_from_normal_form(&reversed, &BTreeMap::new()).unwrap(),
        expected_id
    );
}

#[test]
fn stored_identity_validates_normalization_and_parameter_types() {
    let form = normal_form(&query(vec![condition("source", "alpha", 0)]), &catalog()).unwrap();
    let declarations = BTreeMap::from([("input".into(), "text".into())]);
    assert_eq!(
        question_id_from_normal_form(&form, &declarations).unwrap(),
        question_id(
            &query(vec![condition("source", "alpha", 99)]),
            &catalog(),
            &declarations
        )
        .unwrap()
    );
    assert_ne!(
        question_id_from_normal_form(&form, &declarations).unwrap(),
        question_id_from_normal_form(&form, &BTreeMap::new()).unwrap()
    );
    let mut noncanonical = form.clone();
    noncanonical["span"] = serde_json::json!({"start":0,"end":1});
    assert!(question_id_from_normal_form(&noncanonical, &declarations).is_err());
    assert!(question_id_from_normal_form(&serde_json::json!(null), &declarations).is_err());
}

/// The frozen format decoder is an independent oracle for constructs whose
/// meanings survived the syntax representation change.
#[test]
fn isomorphic_constructs_keep_original_question_ids() {
    let native = catalog();
    let old = historical_query_format_v1::catalog::Catalog {
        default_unit: native.default_unit.clone(),
        units: native
            .units
            .iter()
            .map(|unit| historical_query_format_v1::catalog::Unit {
                name: unit.name.clone(),
                fields: serde_json::from_value(serde_json::to_value(&unit.fields).unwrap())
                    .unwrap(),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    for (current_text, old_text) in [
        ("events | fields source as source", "events | fields source"),
        (
            "events | group by source as source | agg sum(cost) as sum_cost",
            "events | group by source | agg sum(cost)",
        ),
    ] {
        let current = query_language::parse::parse(current_text, &native).unwrap();
        let original = historical_query_format_v1::parse::parse(old_text, &old).unwrap();
        assert_eq!(
            normal_form(&current, &native).unwrap(),
            historical_query_format_v1::identity::normal_form(&original, &old).unwrap(),
            "{current_text} / {old_text}"
        );
    }
    for text in [
        "events where source:alpha",
        "events where source = alpha",
        "events where duration > 1.5s",
        "events where cost between 1usd and 2usd",
        "events where ended - started > 5s",
        "events where ended - started > 0.005s",
        "events where ended - started > $window",
        "events | group by ended,started | where ended - started > $window",
        "events where NOT host:beta OR source:alpha",
        "events | sort by source desc | offset 2 | limit 4 | fields source,host",
        "events | group by source | agg count,sum(cost)",
        "events where source in (events | fields source)",
    ] {
        let current = query_language::parse::parse(text, &native)
            .unwrap_or_else(|e| panic!("current {text}: {e}"));
        let original = historical_query_format_v1::parse::parse(text, &old)
            .unwrap_or_else(|e| panic!("original {text}: {e}"));
        let original_form =
            historical_query_format_v1::identity::normal_form(&original, &old).unwrap();
        assert_eq!(
            normal_form(&current, &native).unwrap(),
            original_form,
            "{text}"
        );
        let parameters = BTreeMap::from([("synthetic".into(), "text".into())]);
        let original_id =
            historical_query_format_v1::identity::question_id(&original, &old, &parameters)
                .unwrap();
        assert_eq!(
            question_id(&current, &native, &parameters).unwrap().0,
            original_id.0,
            "{text}"
        );
    }
}

#[test]
fn new_match_steps_do_not_reuse_original_tuple_identity() {
    let mut current = query(vec![]);
    current.transforms.push(Transform {
        span: Span::default(),
        kind: TransformKind::Matches,
    });
    let current_form = normal_form(&current, &catalog()).unwrap();
    let mut original_form = current_form.clone();
    original_form["stages"][0]["kind"]["kind"] = serde_json::json!("matches");
    assert_ne!(
        question_id_from_normal_form(&current_form, &BTreeMap::new()).unwrap(),
        question_id_from_normal_form(&original_form, &BTreeMap::new()).unwrap()
    );
}

#[test]
fn intermediate_projection_is_distinct_from_original_final_fields() {
    let mut current = query(vec![]);
    current.transforms = vec![
        Transform {
            span: Span::default(),
            kind: TransformKind::Fields {
                columns: vec![Column::field("source")],
            },
        },
        Transform {
            span: Span::default(),
            kind: TransformKind::Limit { count: 1 },
        },
    ];
    let form = normal_form(&current, &catalog()).unwrap();
    assert_eq!(form["stages"][0]["kind"]["kind"], "projection");
    current.transforms.swap(0, 1);
    let terminal = normal_form(&current, &catalog()).unwrap();
    assert_eq!(terminal["stages"][1]["kind"]["kind"], "fields");
}

#[test]
fn time_difference_identity_uses_each_conditions_actual_field_types() {
    let mut native = catalog();
    native.units.push(Unit {
        name: "numbers".into(),
        fields: ["ended", "started"]
            .into_iter()
            .map(|name| Field {
                name: name.into(),
                kind: FieldKind::Number,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    let subtraction = |threshold| Condition {
        span: Span::default(),
        kind: ConditionKind::Compare {
            left: Expression::Arithmetic {
                operator: Arithmetic::Subtract,
                left: Box::new(Expression::field("ended")),
                right: Box::new(Expression::field("started")),
            },
            op: Op::Gt,
            right: Expression::value(threshold),
        },
    };
    let q = query(vec![
        subtraction(Value::Parameter {
            name: "window".into(),
        }),
        Condition {
            span: Span::default(),
            kind: ConditionKind::Exists {
                unit: "numbers".into(),
                condition: Some(Box::new(subtraction(Value::scalar("5")))),
            },
        },
    ]);
    let form = normal_form(&q, &native).unwrap();
    let conditions = form["source"]["condition"]["kind"]["conditions"]
        .as_array()
        .unwrap();
    assert!(
        conditions
            .iter()
            .any(|condition| condition["kind"]["kind"] == "time_difference"
                && condition["kind"]["value"]["kind"] == "parameter")
    );
    let numeric = conditions
        .iter()
        .find(|condition| condition["kind"]["kind"] == "exists")
        .unwrap();
    assert_eq!(
        numeric["kind"]["condition"]["kind"]["kind"],
        "expression_compare"
    );
    let bytes = serde_json::to_vec(&q).unwrap();
    let copy = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(normal_form(&copy, &native).unwrap(), form);
}
