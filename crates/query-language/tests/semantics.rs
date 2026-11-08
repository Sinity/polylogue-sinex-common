use crate::support;
use query_language::ast::*;
use query_language::catalog::{Field, FieldKind, NaturalMatch};
use query_language::compiler_cases::{self, Compiler, Executor, Page, Vocabulary};
use query_language::conformance::*;
use query_language::coverage::CoverageState;
use query_language::cursor::{PageRequest, SealedCursor};
use query_language::semantics::Truth;
use query_language::{Error, ErrorKind};
use std::collections::BTreeMap;

fn condition(kind: ConditionKind) -> Condition {
    Condition {
        span: Span::default(),
        kind,
    }
}
fn comparison(field: &str, op: Op, value: &str) -> Condition {
    condition(ConditionKind::Compare {
        left: Expression::field(field),
        op,
        right: Expression::value(Value::scalar(value)),
    })
}
fn query(unit: &str, transforms: Vec<TransformKind>) -> Query {
    Query {
        source: Source::Unit {
            unit: unit.into(),
            implicit: false,
            condition: None,
        },
        attach: vec![],
        transforms: transforms
            .into_iter()
            .map(|kind| Transform {
                span: Span::default(),
                kind,
            })
            .collect(),
    }
}
fn filtered(unit: &str, c: Condition, transforms: Vec<TransformKind>) -> Query {
    let mut q = query(unit, transforms);
    q.source = Source::Unit {
        unit: unit.into(),
        implicit: false,
        condition: Some(c),
    };
    q
}
fn ids(answer: &Answer) -> Vec<&str> {
    answer.rows.iter().map(|row| row.id.as_str()).collect()
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
/// The declarations of the short-text fields the case vocabularies name: the
/// default one, Polylogue's and Sinex's.
fn reference_catalog() -> query_language::catalog::Catalog {
    use query_language::catalog::{Catalog, Unit};
    let mut catalog = Catalog::new("rows");
    catalog.units.push(Unit {
        name: "rows".into(),
        fields: ["title", "host"]
            .into_iter()
            .map(|name| Field {
                name: name.into(),
                kind: FieldKind::Text,
                ..Field::default()
            })
            .collect(),
        ..Unit::default()
    });
    catalog
}
// This compiler binds one fixture tick to one second before handing the
// query to the exact-tick oracle. Product compilers perform their own binding.
fn resolve_fixture_times(query: &mut Query) -> Result<(), Error> {
    fn distance(distance: &mut Distance) -> Result<(), Error> {
        if let Distance::Time { duration } = distance {
            let nanos = duration
                .text()
                .and_then(query_language::time::duration)
                .map(|duration| duration.as_nanos())
                .filter(|nanos| *nanos > 0 && nanos % 1_000_000_000 == 0)
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::BadValue,
                        "fixture durations require whole seconds",
                        0,
                    )
                })?;
            *duration = Value::scalar((nanos / 1_000_000_000).to_string());
        }
        Ok(())
    }
    fn condition(value: &mut Condition) -> Result<(), Error> {
        match &mut value.kind {
            ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
                for value in conditions {
                    condition(value)?;
                }
            }
            ConditionKind::Not { condition: value } => condition(value)?,
            ConditionKind::Exists {
                condition: Some(value),
                ..
            }
            | ConditionKind::Count {
                condition: Some(value),
                ..
            } => condition(value)?,
            ConditionKind::Sequence { steps, gaps, .. } => {
                for value in steps {
                    condition(value)?;
                }
                for gap in gaps {
                    if let Gap::Within { distance: value } = gap {
                        distance(value)?;
                    }
                }
            }
            ConditionKind::Order { query, within, .. } => {
                if let Some(value) = within {
                    distance(value)?;
                }
                resolve_fixture_times(query)?;
            }
            ConditionKind::In { query, .. } => resolve_fixture_times(query)?,
            _ => {}
        }
        Ok(())
    }
    match &mut query.source {
        Source::Unit {
            condition: Some(value),
            ..
        }
        | Source::Saved {
            condition: Some(value),
            ..
        }
        | Source::Result {
            condition: Some(value),
            ..
        } => condition(value)?,
        _ => {}
    }
    for attachment in &mut query.attach {
        for window in &mut attachment.windows {
            if let Window::Where(value) = window {
                condition(value)?;
            }
        }
    }
    for transform in &mut query.transforms {
        match &mut transform.kind {
            TransformKind::Where { condition: value }
            | TransformKind::Scope {
                condition: Some(value),
                ..
            } => condition(value)?,
            TransformKind::Join { query, on } => {
                if let JoinOn::Order {
                    within: Some(value),
                    ..
                } = on
                {
                    distance(value)?;
                }
                resolve_fixture_times(query)?;
            }
            TransformKind::Combine { query, .. } | TransformKind::Diff { query } => {
                resolve_fixture_times(query)?
            }
            _ => {}
        }
    }
    Ok(())
}
struct Reference;
impl Compiler for Reference {
    type Plan = Query;
    fn compile(&self, query: &Query) -> Result<Query, Error> {
        let mut query = query.clone();
        resolve_fixture_times(&mut query)?;
        Ok(query)
    }
}
impl Executor<Query> for Reference {
    fn execute(
        &mut self,
        query: &Query,
        data: &Dataset,
        request: &PageRequest,
    ) -> Result<Page, Error> {
        let answer = ReferenceEvaluator::new(data)
            .with_catalog(&reference_catalog())
            .execute(query)?;
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
fn compiler_cases_run_against_both_product_vocabularies() {
    // These use each product's unit/field spellings. Grammar's catalog tests own
    // closed-set/default binding; this suite owns backend relation semantics.
    for (catalog, vocabulary) in [
        (
            support::polylogue(),
            Vocabulary {
                outer: "sessions".into(),
                children: "tool_calls".into(),
                number: "cost".into(),
                child_number: "exit_code".into(),
                end: "tree_cost".into(),
                category: "origin".into(),
                text: "title".into(),
            },
        ),
        (
            support::sinex(),
            Vocabulary {
                outer: "events".into(),
                children: "commands".into(),
                number: "commit_seq".into(),
                child_number: "exit_code".into(),
                end: "offset".into(),
                category: "source".into(),
                text: "host".into(),
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
        let cases = compiler_cases::cases(&vocabulary);
        assert_eq!(
            ReferenceEvaluator::new(&cases[0].fixture)
                .execute(&parsed)
                .unwrap(),
            cases[0].expected
        );
        let failures = compiler_cases::check(
            &Reference,
            &mut Reference,
            &compiler_cases::cases(&vocabulary),
        );
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
#[test]
fn case_harness_detects_a_backend_that_ignores_filters_and_paging() {
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
    let failures = compiler_cases::check(
        &Reference,
        &mut Broken,
        &compiler_cases::cases(&Vocabulary::default()),
    );
    for expected in [
        "ql.filter.composition",
        "ql.filter.three-valued",
        "ql.group.relation",
        "ql.aggregate.nearest-rank",
        "ql.page-not-answer",
        "ql.match-order",
        "ql.order.join-grain",
        "ql.diff.watch",
        "ql.covered",
    ] {
        assert!(failures.iter().any(|v| v.name == expected), "{expected}");
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
        arguments: vec![Expression::field("value")],
        name: None,
    })
    .collect();
    let answer = ReferenceEvaluator::new(&data)
        .execute(&query(
            "rows",
            vec![TransformKind::Aggregate { aggregates }],
        ))
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
    let evaluator = ReferenceEvaluator::new(&data);
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
        evaluator.exists_match(&parent, "children", None).unwrap(),
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
        let evaluator = ReferenceEvaluator::new(&data);
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
                        distance: Distance::Time {
                            duration: Value::scalar("100")
                        }
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
    let evaluator = ReferenceEvaluator::new(&data);
    for transform in [
        TransformKind::Fill,
        TransformKind::AsOf {
            value: Value::scalar("1"),
            sequence: false,
        },
        TransformKind::Extension {
            name: "extension".into(),
            arguments: vec![],
        },
    ] {
        let mut q = query("rows", vec![transform]);
        q.transforms[0].span.start = 17;
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
    let evaluator = ReferenceEvaluator::new(&data);
    let q = query(
        "rows",
        vec![TransformKind::Group {
            keys: vec![Column::field("key")],
        }],
    );
    let c = condition(ConditionKind::In {
        left: Expression::field("key"),
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
    let failures = compiler_cases::check_modes(
        &Reference,
        &mut [
            ("fixture-cli", &mut reference as &mut dyn Executor<Query>),
            ("fixture-http", &mut corrupt as &mut dyn Executor<Query>),
        ],
        &compiler_cases::cases(&Vocabulary::default()),
    );
    assert!(!failures.is_empty());
    assert!(
        failures
            .iter()
            .all(|v| v.name == "ql.mode-parity" && v.detail.starts_with("fixture-http:"))
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
            vec![TransformKind::Join {
                query: Box::new(query("inner", vec![])),
                on: JoinOn::Order {
                    relation: OrderRelation::Near,
                    within: Some(Distance::Time {
                        duration: Value::scalar(window),
                    }),
                    at: TimePoint::When,
                    scope: None,
                },
            }],
        )
    };
    let evaluator = ReferenceEvaluator::new(&data);
    assert_eq!(evaluator.execute(&temporal("24")).unwrap().total, 0);
    assert_eq!(
        evaluator.execute(&temporal("46")).unwrap().rows[0].matches,
        vec!["span"]
    );
}
#[test]
fn empty_outer_relation_does_not_hide_unsupported_nested_membership() {
    let mut inner = query("inner", vec![TransformKind::Fill]);
    inner.transforms[0].span.start = 23;
    let mut outer = query("outer", vec![]);
    outer.source = Source::Unit {
        unit: "outer".into(),
        implicit: false,
        condition: Some(condition(ConditionKind::In {
            left: Expression::field("key"),
            query: Box::new(inner),
        })),
    };
    let data = BTreeMap::from([("outer".into(), vec![]), ("inner".into(), vec![])]);
    let error = ReferenceEvaluator::new(&data).execute(&outer).unwrap_err();
    assert_eq!(error.position, 23);
    assert!(error.message.contains("product adapter"));
}
#[test]
fn nonfinite_singletons_refuse_in_sort_and_aggregate() {
    let data = BTreeMap::from([(
        "rows".into(),
        vec![Row::new("a").with("value", f64::INFINITY)],
    )]);
    for transform in [
        TransformKind::Sort {
            keys: vec![SortKey {
                expression: Expression::field("value"),
                descending: false,
            }],
        },
        TransformKind::Aggregate {
            aggregates: vec![Aggregate {
                function: AggregateFunction::Sum,
                arguments: vec![Expression::field("value")],
                name: None,
            }],
        },
    ] {
        assert_eq!(
            ReferenceEvaluator::new(&data)
                .execute(&query("rows", vec![transform]))
                .unwrap_err()
                .kind,
            ErrorKind::BadValue
        );
    }
}

macro_rules! compiler_case {
    ($name:ident, $case:literal) => {
        #[test]
        fn $name() {
            let cases: Vec<_> = compiler_cases::cases(&Vocabulary::default())
                .into_iter()
                .filter(|case| case.name == $case)
                .collect();
            assert!(!cases.is_empty());
            let failures = compiler_cases::check(&Reference, &mut Reference, &cases);
            assert!(failures.is_empty(), "{failures:#?}");
        }
    };
}
compiler_case!(filter_composition, "ql.filter.composition");
compiler_case!(filter_three_valued, "ql.filter.three-valued");
compiler_case!(group_relation, "ql.group.relation");
compiler_case!(match_order, "ql.match-order");
compiler_case!(page_not_answer, "ql.page-not-answer");
compiler_case!(order_semijoin, "ql.order.semijoin");
compiler_case!(order_join_grain, "ql.order.join-grain");
compiler_case!(order_steps, "ql.order.steps");
compiler_case!(order_coarse, "ql.order.coarse");
compiler_case!(rank_membership, "ql.rank.membership");
compiler_case!(diff_watch, "ql.diff.watch");
compiler_case!(covered, "ql.covered");
compiler_case!(rank_unranked_kept, "ql.rank.unranked-kept");
compiler_case!(expr_unknown, "ql.expr.unknown");
compiler_case!(match_natural, "ql.match.natural");

#[test]
fn parsed_grouped_implicit_count_can_filter_and_sort_in_both_catalogs() {
    for (catalog, unit, field) in [
        (support::polylogue(), "sessions", "origin"),
        (support::sinex(), "events", "event_type"),
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
        let answer = ReferenceEvaluator::new(&data).execute(&parsed).unwrap();
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

#[test]
fn natural_matches_follow_the_declared_kind() {
    let row = Row::new("r")
        .with("title", "Fix the Parser")
        .with("cwd", "/src/demo/crate")
        .with("text", "the tests pass now");
    let data = Dataset::new();
    let mut catalog = reference_catalog();
    for (name, kind, natural) in [
        ("cwd", FieldKind::Subtree, NaturalMatch::Subtree),
        ("text", FieldKind::Words, NaturalMatch::Words),
    ] {
        let field = Field {
            name: name.into(),
            kind,
            ..Field::default()
        };
        assert_eq!(field.natural_match(), natural);
        catalog.units[0].fields.push(field);
    }
    let evaluator = ReferenceEvaluator::new(&data).with_catalog(&catalog);
    let natural = |field: &str, value: &str| {
        evaluator
            .condition(
                &row,
                &condition(ConditionKind::Match {
                    field: field.into(),
                    value: Value::scalar(value),
                }),
            )
            .unwrap()
    };
    assert_eq!(natural("title", "parser"), Truth::True);
    assert_eq!(natural("cwd", "/src/demo"), Truth::True);
    assert_eq!(natural("cwd", "/src/dem"), Truth::False, "whole components");
    assert_eq!(natural("text", "pass"), Truth::True);
    assert_eq!(natural("text", "pas"), Truth::False, "whole words");
    // `^` on a path field means "is or lies under".
    let under = condition(ConditionKind::Compare {
        left: Expression::field("cwd"),
        op: Op::Prefix,
        right: Expression::value(Value::scalar("/src/dem")),
    });
    assert_eq!(evaluator.condition(&row, &under).unwrap(), Truth::False);
    // `~` is a case-insensitive substring on any text.
    let contains = condition(ConditionKind::Compare {
        left: Expression::field("title"),
        op: Op::Contains,
        right: Expression::value(Value::scalar("PARSER")),
    });
    assert_eq!(evaluator.condition(&row, &contains).unwrap(), Truth::True);
}

#[test]
fn key_joins_pair_equal_exact_keys_and_skip_unknown_keys() {
    let data = BTreeMap::from([
        (
            "calls".to_string(),
            vec![
                Row::new("c1").with("commit", "abc"),
                Row::new("c2").with("commit", Scalar::Unknown),
                Row::new("c3").with("commit", "zzz"),
            ],
        ),
        (
            "commits".to_string(),
            vec![
                Row::new("k2").with("hash", "abc").with("repo", "two"),
                Row::new("k1").with("hash", "abc").with("repo", "one"),
            ],
        ),
    ]);
    let joined = ReferenceEvaluator::new(&data)
        .execute(&query(
            "calls",
            vec![TransformKind::Join {
                query: Box::new(query("commits", vec![])),
                on: JoinOn::Key {
                    left: "commit".into(),
                    right: "hash".into(),
                },
            }],
        ))
        .unwrap();
    assert_eq!(ids(&joined), ["c1:k1", "c1:k2"]);
    assert_eq!(joined.rows[0].value("match.repo"), Scalar::from("one"));
    assert_eq!(joined.rows[0].value("match.distance"), Scalar::Unknown);
}

#[test]
fn matches_expand_exists_into_one_row_per_matched_child() {
    let mut children = vec![];
    for (id, n) in [("x", 1.), ("y", 2.), ("z", 1.)] {
        let mut row = Row::new(id).with("n", n);
        row.parent = Some("p".into());
        children.push(row);
    }
    let data = BTreeMap::from([
        ("rows".to_string(), vec![Row::new("p"), Row::new("q")]),
        ("children".to_string(), children),
    ]);
    let exists = condition(ConditionKind::Exists {
        unit: "children".into(),
        condition: Some(Box::new(comparison("n", Op::Eq, "1"))),
    });
    let answer = ReferenceEvaluator::new(&data)
        .execute(&filtered("rows", exists, vec![TransformKind::Matches]))
        .unwrap();
    assert_eq!(ids(&answer), ["p:match:1:1", "p:match:2:1"]);
    assert_eq!(answer.rows[1].value("step"), Scalar::from("z"));
    assert_eq!(answer.rows[1].value("match"), Scalar::Number(2.));
    assert_eq!(answer.rows[1].value("position"), Scalar::Number(1.));
    assert_eq!(answer.rows[1].value("step.n"), Scalar::Number(1.));
    assert_eq!(answer.rows[1].value("parent"), Scalar::from("p"));
    // A relation with two patterns has no single expansion.
    let two = condition(ConditionKind::And {
        conditions: vec![
            condition(ConditionKind::Exists {
                unit: "children".into(),
                condition: None,
            }),
            condition(ConditionKind::Exists {
                unit: "children".into(),
                condition: None,
            }),
        ],
    });
    assert!(
        ReferenceEvaluator::new(&data)
            .execute(&filtered("rows", two, vec![TransformKind::Matches]))
            .is_err()
    );
}

/// Steps never cross a scope instance, and `within 1
/// step` before a row is the row whose next row matched.
#[test]
fn steps_stay_inside_one_scope_instance() {
    let mut rows = vec![];
    for (id, parent, order, kind) in [
        ("a1", "s1", 0, "edit"),
        ("a2", "s1", 1, "test"),
        ("a3", "s1", 2, "edit"),
        ("b1", "s2", 0, "test"),
        ("b2", "s2", 1, "edit"),
    ] {
        let mut row = Row::new(id).with("kind", kind);
        row.parent = Some(parent.into());
        row.order = order;
        rows.push(row);
    }
    let data = BTreeMap::from([("calls".to_string(), rows)]);
    let evaluator = ReferenceEvaluator::new(&data);
    let before = |count: u64| {
        filtered(
            "calls",
            condition(ConditionKind::Order {
                relation: OrderRelation::Before,
                query: Box::new(filtered(
                    "calls",
                    comparison("kind", Op::Eq, "test"),
                    vec![],
                )),
                within: Some(Distance::Steps { count }),
                at: TimePoint::When,
                scope: None,
                pick: MatchChoice::Nearest,
            }),
            vec![],
        )
    };
    assert_eq!(ids(&evaluator.execute(&before(1)).unwrap()), ["a1"]);
    let next = filtered("calls", comparison("next.kind", Op::Eq, "test"), vec![]);
    assert_eq!(ids(&evaluator.execute(&next).unwrap()), ["a1"]);
    // b1 is a test, but in another session: a3 is never "before" it.
    assert_eq!(ids(&evaluator.execute(&before(5)).unwrap()), ["a1"]);
    let joined = evaluator
        .execute(&query(
            "calls",
            vec![TransformKind::Join {
                query: Box::new(filtered(
                    "calls",
                    comparison("kind", Op::Eq, "test"),
                    vec![],
                )),
                on: JoinOn::Order {
                    relation: OrderRelation::Near,
                    within: Some(Distance::Steps { count: 1 }),
                    at: TimePoint::When,
                    scope: None,
                },
            }],
        ))
        .unwrap();
    assert_eq!(ids(&joined), ["a1:a2", "a3:a2", "b2:b1"]);
    assert_eq!(joined.rows[0].value("match.distance"), Scalar::Number(1.));
    assert_eq!(joined.rows[1].value("match.distance"), Scalar::Number(-1.));
}

#[test]
fn arithmetic_keeps_unknown_and_a_zero_divisor_is_unknown() {
    let data = BTreeMap::from([(
        "rows".to_string(),
        vec![
            Row::new("a").with("n", 6.).with("d", 2.),
            Row::new("b").with("n", 6.).with("d", 0.),
            Row::new("c").with("n", Scalar::Unknown).with("d", 1.),
        ],
    )]);
    let quotient = Expression::Arithmetic {
        operator: Arithmetic::Divide,
        left: Box::new(Expression::field("n")),
        right: Box::new(Expression::field("d")),
    };
    let answer = ReferenceEvaluator::new(&data)
        .execute(&query(
            "rows",
            vec![TransformKind::Fields {
                columns: vec![Column {
                    expression: quotient,
                    name: Some("q".into()),
                }],
            }],
        ))
        .unwrap();
    let values: Vec<_> = answer.rows.iter().map(|row| row.value("q")).collect();
    assert_eq!(
        values,
        [Scalar::Number(3.), Scalar::Unknown, Scalar::Unknown]
    );
}

#[test]
fn fill_labels_every_bucket_and_never_reads_missing_as_zero() {
    let count = Aggregate {
        function: AggregateFunction::Count,
        arguments: vec![],
        name: None,
    };
    let average = Aggregate {
        function: AggregateFunction::Avg,
        arguments: vec![Expression::field("n")],
        name: None,
    };
    let groups = vec![
        Row::new("g2")
            .with("day", "d2")
            .with("count", 4.)
            .with("avg_n", 1.5),
    ];
    let buckets: Vec<Scalar> = ["d1", "d2", "d3"].into_iter().map(Scalar::from).collect();
    let filled = fill(
        &groups,
        "day",
        &[count, average],
        &buckets,
        |bucket| match bucket {
            Scalar::Text(day) if day == "d1" => CoverageState::Complete,
            Scalar::Text(day) if day == "d2" => CoverageState::Complete,
            _ => CoverageState::Gap {
                reason: "collector not running".into(),
            },
        },
    );
    let days: Vec<_> = filled.iter().map(|row| row.value("day")).collect();
    assert_eq!(days, buckets);
    assert_eq!(filled[0].value("count"), Scalar::Number(0.));
    assert_eq!(filled[0].value("avg_n"), Scalar::Unknown);
    assert_eq!(filled[0].value("coverage"), Scalar::from("complete"));
    assert_eq!(filled[1].value("count"), Scalar::Number(4.));
    assert_eq!(filled[2].value("count"), Scalar::Unknown);
    assert_eq!(filled[2].value("coverage"), Scalar::from("gap"));
    assert_eq!(
        filled[2].value("coverage_detail"),
        Scalar::from("collector not running")
    );
    for state in [
        CoverageState::Complete,
        CoverageState::Gap { reason: "r".into() },
        CoverageState::Unknown,
        CoverageState::OutsideSpan,
        CoverageState::NotYet,
        CoverageState::Excluded,
        CoverageState::Unavailable { cause: "c".into() },
    ] {
        assert!(CoverageState::STATES.contains(&state.state()));
        let wire = serde_json::to_value(&state).unwrap();
        assert_eq!(wire["state"], state.state());
        assert_eq!(
            serde_json::from_value::<CoverageState>(wire).unwrap(),
            state
        );
    }
}

#[test]
fn fusion_chain_ranks_each_original_union_operand_in_its_own_order() {
    let data = BTreeMap::from([(
        "rows".to_string(),
        vec![
            Row::new("a").with("n", 0.),
            Row::new("b").with("n", 1.),
            Row::new("c").with("n", 2.),
        ],
    )]);
    let sort = |descending| TransformKind::Sort {
        keys: vec![SortKey {
            expression: Expression::field("n"),
            descending,
        }],
    };
    let mut combined = filtered(
        "rows",
        comparison("n", Op::Lt, "2"),
        vec![
            sort(false),
            TransformKind::Combine {
                how: Combination::Union,
                query: Box::new(filtered(
                    "rows",
                    comparison("n", Op::Gt, "0"),
                    vec![sort(true)],
                )),
            },
        ],
    );
    let ordinary = ReferenceEvaluator::new(&data).execute(&combined).unwrap();
    assert_eq!(ids(&ordinary), ["a", "b", "c"]);
    combined.transforms.push(Transform {
        span: Span::default(),
        kind: TransformKind::Combine {
            how: Combination::Fuse,
            query: Box::new(filtered("rows", comparison("n", Op::Gt, "9"), vec![])),
        },
    });
    let answer = ReferenceEvaluator::new(&data).execute(&combined).unwrap();
    // Original operands [a,b] and [c,b]: b scores 2/62; a and c score
    // 1/61 each, so their identity breaks the tie. Right-only fusion would
    // instead yield c,b,a, and set membership alone would yield a,b,c.
    assert_eq!(ids(&answer), ["b", "a", "c"]);
    assert_eq!(answer.total, 3);
}

/// A chain of fuse and union at one level is one
/// fusion of every list, not a fusion of fusions.
#[test]
fn union_fuses_once() {
    let rows: Vec<_> = ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|id| Row::new(id).with("n", (id.as_bytes()[0] - b'a') as f64))
        .collect();
    let data = BTreeMap::from([("rows".to_string(), rows)]);
    let ranked = |op: Op, value: &str, descending: bool| {
        Box::new(filtered(
            "rows",
            comparison("n", op, value),
            vec![TransformKind::Sort {
                keys: vec![SortKey {
                    expression: Expression::field("n"),
                    descending,
                }],
            }],
        ))
    };
    let chain = filtered(
        "rows",
        comparison("n", Op::Lt, "2"),
        vec![
            TransformKind::Combine {
                how: Combination::Fuse,
                query: ranked(Op::Ge, "0", true),
            },
            TransformKind::Combine {
                how: Combination::Union,
                query: Box::new(filtered("rows", comparison("n", Op::Eq, "4"), vec![])),
            },
            TransformKind::Combine {
                how: Combination::Fuse,
                query: ranked(Op::Ge, "0", false),
            },
        ],
    );
    let answer = ReferenceEvaluator::new(&data).execute(&chain).unwrap();
    // One fusion of the original operands [a b], [e d c b a], [e] and
    // [a b c d e] over the rows {a, b, e}. a and e tie on score
    // (2/61 + 1/65), so identity wins; b scores 2/62 + 1/64, which is less.
    // The union operand contributes its own order, including its singleton.
    assert_eq!(ids(&answer), ["a", "e", "b"]);
    assert_eq!(answer.total, 3);
}

#[test]
fn containing_obeys_literal_half_open_point_and_span_boundaries() {
    for (outer, inner, expected) in [
        ((0, 10), (0, 0), 1),
        ((0, 10), (5, 5), 1),
        ((0, 10), (10, 10), 0),
        ((0, 10), (0, 10), 1),
        ((5, 5), (5, 5), 1),
        ((5, 5), (0, 10), 0),
    ] {
        // The same half-open boundary applies to precise spans and coarse periods.
        for coarse in [false, true] {
            let mut a = Row::new("a");
            a.time = Some(Extent {
                start: outer.0,
                end: outer.1,
                coarse,
            });
            let mut b = Row::new("b");
            b.time = Some(Extent {
                start: inner.0,
                end: inner.1,
                coarse,
            });
            let data = BTreeMap::from([("outer".into(), vec![a]), ("inner".into(), vec![b])]);
            let q = query(
                "outer",
                vec![TransformKind::Join {
                    query: Box::new(query("inner", vec![])),
                    on: JoinOn::Order {
                        relation: OrderRelation::Containing,
                        within: None,
                        at: TimePoint::When,
                        scope: None,
                    },
                }],
            );
            assert_eq!(
                ReferenceEvaluator::new(&data).execute(&q).unwrap().total,
                expected,
                "outer={outer:?}, inner={inner:?}, coarse={coarse}"
            );
        }
    }
}

#[test]
fn matched_time_selectors_keep_declared_spans_separate_from_occurrence_uncertainty() {
    let point = |tick| Extent {
        start: tick,
        end: tick,
        coarse: false,
    };
    let mut outer = Row::new("outer");
    outer.time = Some(point(10));
    outer.span = Some(DeclaredSpan {
        start: 0,
        end: Some(20),
        end_inclusive: false,
    });
    let mut inner = Row::new("inner");
    inner.time = Some(point(30));
    inner.span = Some(DeclaredSpan {
        start: 5,
        end: Some(15),
        end_inclusive: false,
    });
    let truth = |outer: &Row, inner: Row, relation, at| {
        let data = BTreeMap::from([("inner".into(), vec![inner])]);
        ReferenceEvaluator::new(&data)
            .condition(
                outer,
                &condition(ConditionKind::Order {
                    relation,
                    query: Box::new(query("inner", vec![])),
                    within: None,
                    at,
                    scope: None,
                    pick: MatchChoice::Nearest,
                }),
            )
            .unwrap()
    };
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Before,
            TimePoint::MatchWhen
        ),
        Truth::True
    );
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Before,
            TimePoint::MatchStart
        ),
        Truth::False
    );
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Before,
            TimePoint::MatchEnd
        ),
        Truth::True
    );
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Containing,
            TimePoint::MatchWhen
        ),
        Truth::False
    );
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Containing,
            TimePoint::MatchEnd
        ),
        Truth::True
    );
    // During uses the state lifetime, even though its occurrence is later.
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::During,
            TimePoint::MatchWhen
        ),
        Truth::True
    );
    inner.span.as_mut().unwrap().end = None;
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::During,
            TimePoint::MatchEnd
        ),
        Truth::True
    );
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Before,
            TimePoint::MatchEnd
        ),
        Truth::Unknown
    );
    // An uncertain occurrence's upper bound is not a declared state endpoint.
    inner.span = None;
    inner.time = Some(Extent {
        start: 5,
        end: 15,
        coarse: true,
    });
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Before,
            TimePoint::MatchEnd
        ),
        Truth::Unknown
    );
    assert_eq!(
        truth(
            &outer,
            inner.clone(),
            OrderRelation::Containing,
            TimePoint::MatchWhen
        ),
        Truth::True
    );
    outer.span.as_mut().unwrap().end = Some(10);
    assert_eq!(
        truth(
            &outer,
            inner,
            OrderRelation::Containing,
            TimePoint::MatchWhen
        ),
        Truth::False
    );
}

#[test]
fn declared_span_end_membership_is_explicit_and_unknown_candidates_do_not_hide_matches() {
    let mut outer = Row::new("outer");
    outer.time = Some(Extent {
        start: 0,
        end: 0,
        coarse: false,
    });
    outer.span = Some(DeclaredSpan {
        start: 0,
        end: Some(10),
        end_inclusive: false,
    });
    let mut inner = Row::new("inner");
    inner.time = Some(Extent {
        start: 10,
        end: 10,
        coarse: false,
    });
    let data = BTreeMap::from([("inner".into(), vec![Row::new("unknown"), inner])]);
    let evaluator = ReferenceEvaluator::new(&data);
    let condition = condition(ConditionKind::Order {
        relation: OrderRelation::Containing,
        query: Box::new(query("inner", vec![])),
        within: None,
        at: TimePoint::MatchWhen,
        scope: None,
        pick: MatchChoice::Nearest,
    });
    assert_eq!(
        evaluator.condition(&outer, &condition).unwrap(),
        Truth::Unknown
    );
    outer.span.as_mut().unwrap().end_inclusive = true;
    assert_eq!(
        evaluator.condition(&outer, &condition).unwrap(),
        Truth::True
    );
}

#[test]
fn first_per_group_restores_original_rows_and_global_input_order() {
    let mut input = vec![];
    for (id, rank, kind) in [
        ("a", 5, "A"),
        ("b", 2, "A"),
        ("c", 4, "B"),
        ("d", 1, "B"),
        ("e", 3, "C"),
        ("f", 6, "B"),
    ] {
        let mut row = Row::new(id)
            .with("input_order", f64::from(rank))
            .with("kind", kind);
        row.parent = Some(format!("parent:{id}"));
        row.order = i64::from(rank);
        row.time = Some(Extent {
            start: i64::from(rank),
            end: i64::from(rank),
            coarse: false,
        });
        row.span = Some(DeclaredSpan {
            start: 0,
            end: None,
            end_inclusive: false,
        });
        row.matches = vec![format!("match:{id}")];
        input.push(row);
    }
    let data = BTreeMap::from([("rows".into(), input.clone())]);
    let evaluator = ReferenceEvaluator::new(&data);
    let first = |count| {
        query(
            "rows",
            vec![
                TransformKind::Sort {
                    keys: vec![SortKey {
                        expression: Expression::field("input_order"),
                        descending: false,
                    }],
                },
                TransformKind::Group {
                    keys: vec![Column::field("kind")],
                },
                TransformKind::First { count },
            ],
        )
    };
    let mut expected = input;
    expected.sort_by_key(|row| row.order);
    assert_eq!(evaluator.execute(&first(2)).unwrap().rows, expected[..5]);
    assert_eq!(ids(&evaluator.execute(&first(1)).unwrap()), ["d", "b", "e"]);
    assert!(evaluator.execute(&first(0)).unwrap().rows.is_empty());
    // Limit ends the group boundary: later First now selects grouped rows.
    let grouped = evaluator
        .execute(&query(
            "rows",
            vec![
                TransformKind::Group {
                    keys: vec![Column::field("kind")],
                },
                TransformKind::Limit { count: 2 },
                TransformKind::First { count: 1 },
            ],
        ))
        .unwrap();
    assert_eq!(grouped.rows.len(), 1);
    assert_eq!(grouped.rows[0].value("count"), Scalar::Number(2.));
    assert!(grouped.rows[0].time.is_none());
}

#[test]
fn matches_copy_actual_parent_fields_without_inventing_a_public_id() {
    let public = Row::new("public").with("id", "public").with("n", 7.);
    let opaque = Row::new("opaque").with("n", 9.);
    let mut child_a = Row::new("a");
    child_a.parent = Some("public".into());
    let mut child_b = Row::new("b");
    child_b.parent = Some("opaque".into());
    let data = BTreeMap::from([
        ("parents".into(), vec![public, opaque]),
        ("children".into(), vec![child_a, child_b]),
    ]);
    let answer = ReferenceEvaluator::new(&data)
        .execute(&filtered(
            "parents",
            condition(ConditionKind::Exists {
                unit: "children".into(),
                condition: None,
            }),
            vec![TransformKind::Matches],
        ))
        .unwrap();
    let public = answer
        .rows
        .iter()
        .find(|row| row.parent.as_deref() == Some("public"))
        .unwrap();
    let opaque = answer
        .rows
        .iter()
        .find(|row| row.parent.as_deref() == Some("opaque"))
        .unwrap();
    assert_eq!(public.value("parent.id"), Scalar::from("public"));
    assert_eq!(public.value("parent.n"), Scalar::Number(7.));
    assert_eq!(opaque.value("parent.id"), Scalar::Unknown);
    assert_eq!(opaque.value("parent.n"), Scalar::Number(9.));
}

#[test]
fn projected_parent_aliases_keep_their_matched_rows() {
    let mut child = Row::new("child").with("n", 3.);
    child.parent = Some("parent".into());
    let data = BTreeMap::from([
        ("parents".into(), vec![Row::new("parent").with("n", 7.)]),
        ("children".into(), vec![child]),
    ]);
    let query = filtered(
        "parents",
        condition(ConditionKind::Exists {
            unit: "children".into(),
            condition: None,
        }),
        vec![
            TransformKind::Fields {
                columns: vec![Column {
                    expression: Expression::field("n"),
                    name: Some("value".into()),
                }],
            },
            TransformKind::Matches,
        ],
    );
    let answer = ReferenceEvaluator::new(&data).execute(&query).unwrap();
    assert_eq!(answer.total, 1);
    let matched = &answer.rows[0];
    assert_eq!(
        matched.fields.get("parent.value"),
        Some(&Scalar::Number(7.))
    );
    assert!(!matched.fields.contains_key("parent.n"));
    assert_eq!(matched.fields.get("step.n"), Some(&Scalar::Number(3.)));
    assert_eq!(matched.parent.as_deref(), Some("parent"));
}
