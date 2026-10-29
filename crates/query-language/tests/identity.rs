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
            condition: Some(Condition {
                span: Span::default(),
                kind: ConditionKind::And { conditions },
            }),
        },
        attach: vec![],
        stages: vec![],
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
    b.stages.push(Stage {
        span: Span::default(),
        kind: StageKind::Limit { count: 2 },
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
    a.stages = vec![
        Stage {
            span: Span::default(),
            kind: StageKind::Group {
                keys: vec![Operand::field("source")],
            },
        },
        Stage {
            span: Span::default(),
            kind: StageKind::Aggregate {
                aggregates: vec![Aggregate {
                    function: AggregateFunction::Sum,
                    fields: vec!["cost".into()],
                }],
            },
        },
        Stage {
            span: Span::default(),
            kind: StageKind::Where {
                condition: condition("sum_cost", "2.90", 0),
            },
        },
    ];
    let mut b = a.clone();
    if let StageKind::Where { condition: c } = &mut b.stages[2].kind {
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
                        duration: Value::scalar(duration),
                    }],
                },
            }),
        },
        attach: vec![],
        stages: vec![],
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
