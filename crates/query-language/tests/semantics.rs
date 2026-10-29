use query_language::ast::*;
use query_language::cursor::{PageRequest, SealedCursor};
use query_language::laws::{self, Compiler, Executor, Page, Vocabulary};
use query_language::semantics::*;
use query_language::{Error, ErrorKind};
#[path = "support/mod.rs"]
mod support;
use std::collections::BTreeMap;

fn condition(kind: ConditionKind) -> Condition {
    Condition {
        span: Span::default(),
        kind,
    }
}
fn comparison(field: &str, op: Op, value: &str) -> Condition {
    condition(ConditionKind::Compare {
        operand: Operand::field(field),
        op,
        value: Value::scalar(value),
    })
}
fn query(unit: &str, stages: Vec<StageKind>) -> Query {
    Query {
        source: Source::Unit {
            unit: unit.into(),
            implicit: false,
            condition: None,
        },
        attach: vec![],
        stages: stages
            .into_iter()
            .map(|kind| Stage {
                span: Span::default(),
                kind,
            })
            .collect(),
    }
}

#[test]
fn truth_tables_cover_every_pair_and_negation() {
    use Truth::{False as F, True as T, Unknown as U};
    for (a, b, and, or) in [
        (F, F, F, F),
        (F, U, F, U),
        (F, T, F, T),
        (U, F, F, U),
        (U, U, U, U),
        (U, T, U, T),
        (T, F, F, T),
        (T, U, U, T),
        (T, T, T, T),
    ] {
        assert_eq!(a.and(b), and);
        assert_eq!(a.or(b), or);
    }
    for (a, expected) in [(T, F), (F, T), (U, U)] {
        assert_eq!(!a, expected);
        assert_eq!(a.not(), expected);
        assert_eq!(a.matches(), a == T);
    }
}
#[test]
fn unknown_is_not_zero_or_empty_and_lists_keep_unknown() {
    for op in [Op::Eq, Op::Ne, Op::Lt, Op::Le, Op::Gt, Op::Ge] {
        for value in [Scalar::Number(0.), Scalar::Text(String::new())] {
            let truth = compare(&Scalar::Unknown, op, &value).unwrap();
            assert_eq!(truth, Truth::Unknown);
            assert_eq!(!truth, Truth::Unknown);
        }
    }
    assert_eq!(
        compare(&Scalar::List(vec![]), Op::Eq, &Scalar::from("x")).unwrap(),
        Truth::False
    );
    assert_eq!(
        compare(&Scalar::List(vec![]), Op::Ne, &Scalar::from("x")).unwrap(),
        Truth::True
    );
    assert_eq!(
        compare(
            &Scalar::List(vec![Scalar::Unknown]),
            Op::Ne,
            &Scalar::from("x")
        )
        .unwrap(),
        Truth::Unknown
    );
    assert_eq!(
        compare(
            &Scalar::List(vec![Scalar::Unknown, Scalar::from("x")]),
            Op::Eq,
            &Scalar::from("x")
        )
        .unwrap(),
        Truth::True
    );
}
#[test]
fn nearest_rank_uses_ceiling_and_omits_unknown_not_zero() {
    let values = [
        Scalar::Number(100.),
        Scalar::Unknown,
        Scalar::Number(2.),
        Scalar::Number(8.),
        Scalar::Number(3.),
    ];
    for (p, expected) in [
        (0, 2.),
        (1, 2.),
        (25, 2.),
        (26, 3.),
        (50, 3.),
        (51, 8.),
        (75, 8.),
        (76, 100.),
        (100, 100.),
    ] {
        assert_eq!(nearest_rank(&values, p).unwrap(), Scalar::Number(expected));
    }
    assert_eq!(nearest_rank(&[], 50).unwrap(), Scalar::Unknown);
    assert_eq!(
        nearest_rank(&[Scalar::Unknown], 50).unwrap(),
        Scalar::Unknown
    );
    assert!(nearest_rank(&values, 101).is_err());
    assert!(nearest_rank(&[Scalar::Number(f64::NAN)], 50).is_err());
}
struct Reference;
impl Compiler for Reference {
    type Plan = Query;
    fn compile(&self, query: &Query) -> Result<Query, Error> {
        Ok(query.clone())
    }
}
impl Executor<Query> for Reference {
    fn execute(
        &mut self,
        query: &Query,
        data: &Dataset,
        request: &PageRequest,
    ) -> Result<Page, Error> {
        let answer = ReferenceEvaluator { data }.execute(query)?;
        let sliced = page(
            &answer,
            request.cursor.as_ref().map(SealedCursor::as_str),
            request.page_size.get(),
        )?;
        let next = sliced
            .rows
            .last()
            .filter(|last| answer.rows.last().is_some_and(|r| r.id != last.id))
            .map(|r| SealedCursor::from_wire(r.id.clone()));
        Ok(Page {
            answer: sliced,
            next,
        })
    }
}
#[test]
fn compiler_laws_run_against_both_product_vocabularies() {
    // These use each product's unit/field spellings. Grammar's catalog tests own
    // closed-set/default binding; this suite owns backend relation semantics.
    for (catalog, vocabulary) in [
        (
            support::polylogue(),
            Vocabulary {
                outer: "sessions".into(),
                children: "tool_calls".into(),
                number: "cost".into(),
                category: "origin".into(),
            },
        ),
        (
            support::sinex(),
            Vocabulary {
                outer: "events".into(),
                children: "commands".into(),
                number: "commit_seq".into(),
                category: "source".into(),
            },
        ),
    ] {
        assert!(catalog.unit(&vocabulary.outer).is_some());
        assert!(catalog.unit(&vocabulary.children).is_some());
        let text = format!(
            "{} where {} > 1 | where {} < 5",
            vocabulary.outer, vocabulary.number, vocabulary.number
        );
        let parsed = query_language::parse::parse(&text, &catalog).unwrap();
        let cases = laws::cases(&vocabulary);
        assert_eq!(
            ReferenceEvaluator {
                data: &cases[0].fixture
            }
            .execute(&parsed)
            .unwrap(),
            cases[0].expected
        );
        let failures = laws::check(&Reference, &mut Reference, &laws::cases(&vocabulary));
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
#[test]
fn law_harness_detects_a_backend_that_ignores_filters_and_paging() {
    struct Broken;
    impl Executor<Query> for Broken {
        fn execute(&mut self, _: &Query, data: &Dataset, _: &PageRequest) -> Result<Page, Error> {
            let rows = data["rows"].clone();
            Ok(Page {
                answer: Answer {
                    total: rows.len(),
                    rows,
                },
                next: None,
            })
        }
    }
    let failures = laws::check(
        &Reference,
        &mut Broken,
        &laws::cases(&Vocabulary::default()),
    );
    for expected in [
        "filter-composition",
        "three-valued-filters",
        "grouped-rows-are-a-relation",
        "nearest-rank",
        "page-is-not-the-answer",
        "sequence-match-order-independent",
        "join-match-order-independent",
    ] {
        assert!(failures.iter().any(|v| v.law == expected), "{expected}");
    }
}
#[test]
fn unknown_only_aggregate_stays_unknown_with_known_count_zero() {
    let data = BTreeMap::from([(
        "rows".into(),
        vec![Row::new("u").with("value", Scalar::Unknown)],
    )]);
    let aggregates = [
        AggregateFunction::Sum,
        AggregateFunction::Avg,
        AggregateFunction::Min,
        AggregateFunction::Max,
        AggregateFunction::Percentile(90),
        AggregateFunction::Count,
    ]
    .into_iter()
    .map(|function| Aggregate {
        function,
        fields: vec!["value".into()],
    })
    .collect();
    let answer = ReferenceEvaluator { data: &data }
        .execute(&query("rows", vec![StageKind::Aggregate { aggregates }]))
        .unwrap();
    for column in [
        "sum_value",
        "avg_value",
        "min_value",
        "max_value",
        "p90_value",
    ] {
        assert_eq!(answer.rows[0].value(column), Scalar::Unknown);
    }
    assert_eq!(answer.rows[0].value("count_value"), Scalar::Number(0.));
    assert_eq!(answer.rows[0].value("unknown_value"), Scalar::Number(1.));
}
#[test]
fn exists_has_three_values_while_count_is_always_known() {
    let parent = Row::new("p");
    let mut unknown = Row::new("u");
    unknown.parent = Some("p".into());
    let data = BTreeMap::from([("children".into(), vec![unknown])]);
    let evaluator = ReferenceEvaluator { data: &data };
    let test = comparison("n", Op::Eq, "0");
    let exists = condition(ConditionKind::Exists {
        unit: "children".into(),
        condition: Some(Box::new(test.clone())),
    });
    assert_eq!(
        evaluator.condition(&parent, &exists).unwrap(),
        Truth::Unknown
    );
    let count = condition(ConditionKind::Count {
        unit: "children".into(),
        condition: Some(Box::new(test)),
        op: Op::Eq,
        count: 0,
    });
    assert_eq!(evaluator.condition(&parent, &count).unwrap(), Truth::True);
    assert_eq!(
        evaluator.exists_witness(&parent, "children", None).unwrap(),
        Some("u".into())
    );
}
#[test]
fn sequence_all_and_next_use_occurrence_order_not_provider_time() {
    let parent = Row::new("p");
    let mut children = vec![];
    for (id, n, order, time) in [("a", 1., 0, 30), ("b", 2., 1, 20), ("c", 2., 2, 10)] {
        let mut row = Row::new(id).with("n", n);
        row.parent = Some("p".into());
        row.order = order;
        row.time = Some(Extent {
            start: time,
            end: time,
            coarse: false,
        });
        children.push(row);
    }
    for reverse in [false, true] {
        if reverse {
            children.reverse();
        }
        let data = BTreeMap::from([("children".into(), children.clone())]);
        let evaluator = ReferenceEvaluator { data: &data };
        let steps = [comparison("n", Op::Eq, "1"), comparison("n", Op::Eq, "2")];
        assert_eq!(
            evaluator
                .sequence(&parent, "children", true, &steps, &[Gap::Later])
                .unwrap(),
            vec![
                vec!["a".to_string(), "b".into()],
                vec!["a".into(), "c".into()]
            ]
        );
        assert_eq!(
            evaluator
                .sequence(&parent, "children", true, &steps, &[Gap::Next])
                .unwrap(),
            vec![vec!["a".to_string(), "b".into()]]
        );
        assert!(
            evaluator
                .sequence(
                    &parent,
                    "children",
                    true,
                    &steps,
                    &[Gap::Within {
                        duration: Value::scalar("100")
                    }]
                )
                .unwrap()
                .is_empty()
        );
    }
}
#[test]
fn unsupported_product_mechanics_are_positioned_refusals() {
    let data = BTreeMap::from([("rows".into(), vec![Row::new("a")])]);
    let evaluator = ReferenceEvaluator { data: &data };
    for stage in [
        StageKind::Fill,
        StageKind::AsOf {
            value: Value::scalar("1"),
            sequence: false,
        },
        StageKind::Extension {
            name: "extension".into(),
            arguments: vec![],
        },
    ] {
        let mut q = query("rows", vec![stage]);
        q.stages[0].span.start = 17;
        let error = evaluator.execute(&q).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidQuery);
        assert_eq!(error.position, 17);
        assert!(error.message.contains("product adapter"));
    }
}
#[test]
fn membership_ignores_unknown_right_and_keeps_unknown_left_unknown() {
    let data = BTreeMap::from([(
        "rows".into(),
        vec![Row::new("u").with("key", Scalar::Unknown)],
    )]);
    let evaluator = ReferenceEvaluator { data: &data };
    let q = query(
        "rows",
        vec![StageKind::Group {
            keys: vec![Operand::field("key")],
        }],
    );
    let c = condition(ConditionKind::In {
        operand: Operand::field("key"),
        query: Box::new(q),
    });
    assert_eq!(
        evaluator
            .condition(&Row::new("known").with("key", "known"), &c)
            .unwrap(),
        Truth::False
    );
    assert_eq!(
        evaluator.condition(&Row::new("unknown"), &c).unwrap(),
        Truth::Unknown
    );
}

#[test]
fn execution_mode_parity_checks_each_supplied_adapter() {
    struct Corrupt;
    impl Executor<Query> for Corrupt {
        fn execute(
            &mut self,
            query: &Query,
            data: &Dataset,
            page: &PageRequest,
        ) -> Result<Page, Error> {
            let mut result = Reference.execute(query, data, page)?;
            result.answer.total += 1;
            Ok(result)
        }
    }
    let mut reference = Reference;
    let mut corrupt = Corrupt;
    let failures = laws::check_modes(
        &Reference,
        &mut [
            ("fixture-cli", &mut reference as &mut dyn Executor<Query>),
            ("fixture-http", &mut corrupt as &mut dyn Executor<Query>),
        ],
        &laws::cases(&Vocabulary::default()),
    );
    assert!(!failures.is_empty());
    assert!(
        failures
            .iter()
            .all(|v| v.law == "execution-mode-parity" && v.detail.starts_with("fixture-http:"))
    );
}

#[test]
fn coarse_near_requires_the_whole_period_inside_the_window() {
    let mut outer = Row::new("point");
    outer.time = Some(Extent {
        start: 0,
        end: 0,
        coarse: false,
    });
    let mut inner = Row::new("span");
    inner.time = Some(Extent {
        start: 23,
        end: 47,
        coarse: true,
    });
    let data = BTreeMap::from([("outer".into(), vec![outer]), ("inner".into(), vec![inner])]);
    let temporal = |window: &str| {
        query(
            "outer",
            vec![StageKind::Temporal {
                relation: TemporalRelation::Near,
                query: Box::new(query("inner", vec![])),
                at: TimePoint::When,
                within: Some(Value::scalar(window)),
                all: false,
            }],
        )
    };
    let evaluator = ReferenceEvaluator { data: &data };
    assert_eq!(evaluator.execute(&temporal("24")).unwrap().total, 0);
    assert_eq!(
        evaluator.execute(&temporal("46")).unwrap().rows[0].matches,
        vec!["span"]
    );
}
#[test]
fn empty_outer_relation_does_not_hide_unsupported_nested_membership() {
    let mut inner = query("inner", vec![StageKind::Fill]);
    inner.stages[0].span.start = 23;
    let mut outer = query("outer", vec![]);
    outer.source = Source::Unit {
        unit: "outer".into(),
        implicit: false,
        condition: Some(condition(ConditionKind::In {
            operand: Operand::field("key"),
            query: Box::new(inner),
        })),
    };
    let data = BTreeMap::from([("outer".into(), vec![]), ("inner".into(), vec![])]);
    let error = ReferenceEvaluator { data: &data }
        .execute(&outer)
        .unwrap_err();
    assert_eq!(error.position, 23);
    assert!(error.message.contains("product adapter"));
}
#[test]
fn nonfinite_singletons_refuse_in_sort_and_aggregate() {
    let data = BTreeMap::from([(
        "rows".into(),
        vec![Row::new("a").with("value", f64::INFINITY)],
    )]);
    for stage in [
        StageKind::Sort {
            keys: vec![SortKey {
                operand: Operand::field("value"),
                descending: false,
            }],
        },
        StageKind::Aggregate {
            aggregates: vec![Aggregate {
                function: AggregateFunction::Sum,
                fields: vec!["value".into()],
            }],
        },
    ] {
        assert_eq!(
            ReferenceEvaluator { data: &data }
                .execute(&query("rows", vec![stage]))
                .unwrap_err()
                .kind,
            ErrorKind::BadValue
        );
    }
}

macro_rules! compiler_law {
    ($name:ident, $law:literal) => {
        #[test]
        fn $name() {
            let cases: Vec<_> = laws::cases(&Vocabulary::default())
                .into_iter()
                .filter(|case| case.law == $law)
                .collect();
            assert!(!cases.is_empty());
            let failures = laws::check(&Reference, &mut Reference, &cases);
            assert!(failures.is_empty(), "{failures:#?}");
        }
    };
}
compiler_law!(filter_composition, "filter-composition");
compiler_law!(three_valued_filters, "three-valued-filters");
compiler_law!(grouped_rows_are_a_relation, "grouped-rows-are-a-relation");
compiler_law!(match_order_independent, "join-match-order-independent");
compiler_law!(page_is_not_the_answer, "page-is-not-the-answer");

#[test]
fn parsed_grouped_implicit_count_can_filter_and_sort_in_both_catalogs() {
    for (catalog, unit, field) in [
        (support::polylogue(), "sessions", "origin"),
        (support::sinex(), "events", "kind"),
    ] {
        let data = BTreeMap::from([(
            unit.into(),
            vec![
                Row::new("a").with(field, "same"),
                Row::new("b").with(field, "same"),
                Row::new("c").with(field, "other"),
            ],
        )]);
        let parsed = query_language::parse::parse(
            &format!("{unit} | group by {field} | where count > 1 | sort by count desc"),
            &catalog,
        )
        .unwrap();
        let answer = ReferenceEvaluator { data: &data }.execute(&parsed).unwrap();
        assert_eq!(answer.rows.len(), 1);
        assert_eq!(answer.rows[0].value(field), Scalar::Text("same".into()));
        assert_eq!(answer.rows[0].value("count"), Scalar::Number(2.));
        assert!(
            query_language::parse::parse(
                &format!("{unit} | group by {field} | where absent > 1"),
                &catalog
            )
            .is_err()
        );
    }
}
