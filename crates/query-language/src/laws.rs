//! Reusable conformance cases for caller compilers and executors.
//!
//! A product adapter installs each synthetic Dataset through its fixture API,
//! compiles the supplied shared AST with its real compiler, and executes pages
//! through its real query path. The runner compares independently specified
//! row identities, values, witnesses and totals, not the reference interpreter.
//! Fixture unit/field names are caller supplied so both product catalogs can
//! run the same laws. Product-specific transport parity and refusals add cases.
use crate::Error;
use crate::ast::*;
use crate::cursor::{PageRequest, SealedCursor};
use crate::semantics::{Answer, Dataset, Extent, Row, Scalar};
use std::collections::BTreeMap;
use std::num::NonZeroUsize;

pub trait Compiler {
    type Plan;
    fn compile(&self, query: &Query) -> Result<Self::Plan, Error>;
}
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub answer: Answer,
    pub next: Option<SealedCursor>,
}
/// Dataset is fixture input, not a required product execution representation.
/// An adapter may load it into a disposable store before executing the plan.
pub trait Executor<P> {
    fn execute(&mut self, plan: &P, fixture: &Dataset, page: &PageRequest) -> Result<Page, Error>;
}
#[derive(Clone, Debug)]
pub struct Vocabulary {
    pub outer: String,
    pub children: String,
    pub number: String,
    /// A number of `children`, independently named from the outer number.
    pub child_number: String,
    /// A second number of the same kind, the end of `number`'s interval.
    pub end: String,
    pub category: String,
    /// A short-text field of `outer`, whose natural match is a
    /// case-insensitive substring.
    pub text: String,
}
impl Default for Vocabulary {
    fn default() -> Self {
        Self {
            outer: "rows".into(),
            children: "children".into(),
            number: "value".into(),
            child_number: "value".into(),
            end: "end".into(),
            category: "category".into(),
            text: "title".into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub law: String,
    pub detail: String,
}
#[derive(Clone, Debug)]
pub struct Case {
    pub law: String,
    pub query: Query,
    pub fixture: Dataset,
    pub expected: Answer,
}
fn c(kind: ConditionKind) -> Condition {
    Condition {
        span: Span::default(),
        kind,
    }
}
fn t(kind: TransformKind) -> Transform {
    Transform {
        span: Span::default(),
        kind,
    }
}
fn query(
    v: &Vocabulary,
    unit: &str,
    condition: Option<Condition>,
    transforms: Vec<Transform>,
) -> Query {
    // Fixture order is explicit: a product's ordinary recency order is not a
    // property of these language laws. Subsequent transforms may replace it.
    let number = if unit == v.children {
        &v.child_number
    } else {
        &v.number
    };
    let mut ordered = vec![sort(number, false)];
    ordered.extend(transforms);
    Query {
        source: Source::Unit {
            unit: unit.into(),
            implicit: false,
            condition,
        },
        attach: vec![],
        transforms: ordered,
    }
}
fn comparison(field: &str, op: Op, value: &str) -> Condition {
    c(ConditionKind::Compare {
        left: Expression::field(field),
        op,
        right: Expression::value(Value::scalar(value)),
    })
}
fn not(condition: Condition) -> Condition {
    c(ConditionKind::Not {
        condition: Box::new(condition),
    })
}
fn sort(field: &str, descending: bool) -> Transform {
    t(TransformKind::Sort {
        keys: vec![SortKey {
            expression: Expression::field(field),
            descending,
        }],
    })
}
fn answer(rows: Vec<Row>) -> Answer {
    Answer {
        total: rows.len(),
        rows,
    }
}
// Fixture instants and periods use exact seconds, as their query durations do.
fn at(id: &str, time: i64) -> Row {
    let mut row = Row::new(id);
    row.time = Some(Extent {
        start: time,
        end: time,
        coarse: false,
    });
    row
}
/// Independent expected answers are literal fixture projections. No call here
/// executes a query or derives the oracle by the mechanism being tested.
pub fn cases(v: &Vocabulary) -> Vec<Case> {
    let rows = vec![
        Row::new("a")
            .with(&v.number, 1.)
            .with(&v.end, 4.)
            .with(&v.category, "east"),
        Row::new("b")
            .with(&v.number, 3.)
            .with(&v.end, 6.)
            .with(&v.category, "east"),
        Row::new("c")
            .with(&v.number, 5.)
            .with(&v.end, 5.)
            .with(&v.category, "west"),
        Row::new("u")
            .with(&v.number, Scalar::Unknown)
            .with(&v.category, "west"),
    ];
    let mut children = vec![];
    for (id, n, order, time) in [("x", 1., 0, 20), ("y", 3., 1, 10), ("z", 3., 2, 30)] {
        let mut row = Row::new(id).with(&v.child_number, n);
        row.parent = Some("a".into());
        row.order = order;
        row.time = Some(Extent {
            start: time,
            end: time,
            coarse: false,
        });
        children.push(row);
    }
    let mut unknown = Row::new("unknown-child").with(&v.child_number, Scalar::Unknown);
    unknown.parent = Some("u".into());
    children.push(unknown);
    let fixture = BTreeMap::from([
        (v.outer.clone(), rows.clone()),
        (v.children.clone(), children.clone()),
    ]);
    let case = |law: &str, query: Query, expected: Vec<Row>| Case {
        law: law.into(),
        query,
        fixture: fixture.clone(),
        expected: answer(expected),
    };
    let mut output = vec![];
    let a = comparison(&v.number, Op::Gt, "1");
    let b = comparison(&v.number, Op::Lt, "5");
    for transforms in [
        vec![
            t(TransformKind::Where {
                condition: a.clone(),
            }),
            t(TransformKind::Where {
                condition: b.clone(),
            }),
        ],
        vec![t(TransformKind::Where {
            condition: c(ConditionKind::And {
                conditions: vec![a, b],
            }),
        })],
    ] {
        output.push(case(
            "ql.filter.composition",
            query(v, &v.outer, None, transforms),
            vec![rows[1].clone()],
        ));
    }
    for (op, expected) in [
        (Op::Eq, vec![rows[1].clone()]),
        (Op::Ne, vec![rows[0].clone(), rows[2].clone()]),
        (Op::Gt, vec![rows[2].clone()]),
        (Op::Ge, vec![rows[1].clone(), rows[2].clone()]),
        (Op::Lt, vec![rows[0].clone()]),
        (Op::Le, vec![rows[0].clone(), rows[1].clone()]),
    ] {
        output.push(case(
            "ql.filter.three-valued",
            query(v, &v.outer, Some(comparison(&v.number, op, "3")), vec![]),
            expected,
        ));
    }
    output.push(case(
        "ql.filter.three-valued",
        query(
            v,
            &v.outer,
            Some(not(comparison(&v.number, Op::Eq, "3"))),
            vec![],
        ),
        vec![rows[0].clone(), rows[2].clone()],
    ));
    for (value, expected) in [
        (Value::Unknown, vec![rows[3].clone()]),
        (
            Value::Known,
            vec![rows[0].clone(), rows[1].clone(), rows[2].clone()],
        ),
    ] {
        output.push(case(
            "ql.filter.three-valued",
            query(
                v,
                &v.outer,
                Some(c(ConditionKind::Match {
                    field: v.number.clone(),
                    value,
                })),
                vec![],
            ),
            expected,
        ));
    }
    output.push(case(
        "ql.match.natural",
        query(
            v,
            &v.outer,
            Some(c(ConditionKind::Match {
                field: v.category.clone(),
                value: Value::scalar("east"),
            })),
            vec![],
        ),
        vec![rows[0].clone(), rows[1].clone()],
    ));
    // `field:value` on short text is a case-insensitive substring, not
    // equality.
    let titled = |id: &str, title: &str| Row::new(id).with(&v.text, title);
    output.push(Case {
        law: "ql.match.natural".into(),
        query: query(
            v,
            &v.outer,
            Some(c(ConditionKind::Match {
                field: v.text.clone(),
                value: Value::scalar("parser"),
            })),
            vec![],
        ),
        fixture: BTreeMap::from([(
            v.outer.clone(),
            vec![
                titled("fix", "Fix the Parser"),
                titled("plan", "parse later"),
                titled("same", "parser"),
                Row::new("untitled"),
            ],
        )]),
        expected: answer(vec![
            titled("fix", "Fix the Parser"),
            titled("same", "parser"),
        ]),
    });
    let doubled = Expression::Arithmetic {
        operator: Arithmetic::Multiply,
        left: Box::new(Expression::field(&v.number)),
        right: Box::new(Expression::value(Value::scalar("2"))),
    };
    let more_than_five = c(ConditionKind::Compare {
        left: doubled,
        op: Op::Gt,
        right: Expression::value(Value::scalar("5")),
    });
    output.push(case(
        "ql.expr.unknown",
        query(v, &v.outer, Some(more_than_five.clone()), vec![]),
        vec![rows[1].clone(), rows[2].clone()],
    ));
    output.push(case(
        "ql.expr.unknown",
        query(v, &v.outer, Some(not(more_than_five)), vec![]),
        vec![rows[0].clone()],
    ));
    let aggregate = Aggregate {
        function: AggregateFunction::Sum,
        arguments: vec![Expression::field(&v.number)],
        name: None,
    };
    let column = aggregate.column();
    let category = Column::field(&v.category).column();
    let unknown_number = format!("unknown_{}", Expression::field(&v.number).column());
    let east = Row::new("group:[Text(\"east\")]")
        .with(&category, "east")
        .with("count", 2.)
        .with(&column, 4.)
        .with(&unknown_number, 0.);
    output.push(case(
        "ql.group.relation",
        query(
            v,
            &v.outer,
            None,
            vec![
                t(TransformKind::Group {
                    keys: vec![Column::field(&v.category)],
                }),
                t(TransformKind::Aggregate {
                    aggregates: vec![aggregate],
                }),
                t(TransformKind::Where {
                    condition: comparison(&column, Op::Gt, "3"),
                }),
                sort(&column, false),
                t(TransformKind::Limit { count: 1 }),
            ],
        ),
        vec![east],
    ));
    let percentile = Aggregate {
        function: AggregateFunction::Percentile(50),
        arguments: vec![Expression::field(&v.number)],
        name: None,
    };
    output.push(case(
        "ql.aggregate.nearest-rank",
        query(
            v,
            &v.outer,
            None,
            vec![t(TransformKind::Aggregate {
                aggregates: vec![percentile.clone()],
            })],
        ),
        vec![
            Row::new("aggregate")
                .with(percentile.column(), 3.)
                .with(&unknown_number, 1.),
        ],
    ));
    // [1, 4) and [3, 6) overlap; [5, 5) is empty; an unknown start is skipped.
    let covered = Aggregate {
        function: AggregateFunction::Covered,
        arguments: vec![Expression::field(&v.number), Expression::field(&v.end)],
        name: Some("covered".into()),
    };
    output.push(case(
        "ql.covered",
        query(
            v,
            &v.outer,
            None,
            vec![t(TransformKind::Aggregate {
                aggregates: vec![covered],
            })],
        ),
        vec![
            Row::new("aggregate")
                .with("covered", 5.)
                .with(&unknown_number, 1.)
                .with(
                    format!("unknown_{}", Expression::field(&v.end).column()),
                    1.,
                ),
        ],
    ));
    output.push(case(
        "ql.filter.three-valued",
        query(
            v,
            &v.outer,
            Some(not(c(ConditionKind::Exists {
                unit: v.children.clone(),
                condition: Some(Box::new(comparison(&v.child_number, Op::Eq, "3"))),
            }))),
            vec![],
        ),
        vec![rows[1].clone(), rows[2].clone()],
    ));
    let seq = c(ConditionKind::Sequence {
        unit: v.children.clone(),
        all: false,
        steps: vec![
            comparison(&v.child_number, Op::Eq, "1"),
            comparison(&v.child_number, Op::Eq, "3"),
        ],
        gaps: vec![Gap::Later],
    });
    let step = |position: usize, id: &str, n: f64| {
        let mut row = Row::new(format!("a:match:1:{position}"))
            .with("parent", "a")
            .with("match", 1.)
            .with("position", position as f64)
            .with("step", id)
            .with(format!("step.{}", v.child_number), n)
            .with(format!("parent.{}", v.number), 1.)
            .with(format!("parent.{}", v.end), 4.)
            .with(format!("parent.{}", v.category), "east");
        row.parent = Some("a".into());
        row.matches = vec![id.into()];
        row
    };
    output.push(case(
        "ql.match-order",
        query(v, &v.outer, Some(seq), vec![t(TransformKind::Matches)]),
        vec![step(1, "x", 1.), step(2, "y", 3.)],
    ));
    output.push(case(
        "ql.page-not-answer",
        query(
            v,
            &v.outer,
            None,
            vec![sort(&v.number, true), t(TransformKind::Limit { count: 3 })],
        ),
        vec![rows[2].clone(), rows[1].clone(), rows[0].clone()],
    ));
    // Pairs come nearest first: precise before coarse, then distance, then
    // identity, independently of enumeration order.
    let outer = at("outer", 10);
    let mut coarse = Row::new("coarse");
    coarse.time = Some(Extent {
        start: 10,
        end: 11,
        coarse: true,
    });
    let timed = BTreeMap::from([
        (
            v.outer.clone(),
            vec![outer.clone(), at("far", 100), Row::new("untimed")],
        ),
        (
            v.children.clone(),
            vec![at("near-b", 11), coarse, at("near-a", 9)],
        ),
    ]);
    let pair = |inner: &str, offset: f64| {
        let mut row = outer
            .clone()
            .with("match.id", inner)
            .with("match.distance", offset);
        row.id = format!("outer:{inner}");
        row.matches = vec![inner.into()];
        row
    };
    let near = |relation| JoinOn::Order {
        relation,
        within: Some(Distance::Time {
            duration: Value::scalar("5s"),
        }),
        at: TimePoint::When,
        scope: None,
    };
    output.push(Case {
        law: "ql.order.join-grain".into(),
        query: query(
            v,
            &v.outer,
            None,
            vec![t(TransformKind::Join {
                query: Box::new(query(v, &v.children, None, vec![])),
                on: near(OrderRelation::Near),
            })],
        ),
        fixture: timed.clone(),
        expected: answer(vec![
            pair("near-a", -1.),
            pair("near-b", 1.),
            pair("coarse", 0.),
        ]),
    });
    let near_condition = c(ConditionKind::Order {
        relation: OrderRelation::Near,
        query: Box::new(query(v, &v.children, None, vec![])),
        within: Some(Distance::Time {
            duration: Value::scalar("5s"),
        }),
        at: TimePoint::When,
        scope: None,
        pick: WitnessChoice::Nearest,
    });
    // The semijoin keeps each outer row once; an untimed row is unknown, so
    // neither the relation nor its negation keeps it.
    output.push(Case {
        law: "ql.order.semijoin".into(),
        query: query(v, &v.outer, Some(near_condition.clone()), vec![]),
        fixture: timed.clone(),
        expected: answer(vec![
            outer
                .clone()
                .with("match.id", "near-a")
                .with("match.distance", -1.),
        ]),
    });
    output.push(Case {
        law: "ql.order.semijoin".into(),
        query: query(v, &v.outer, Some(not(near_condition)), vec![]),
        fixture: timed,
        expected: answer(vec![at("far", 100)]),
    });
    // `within 1 step` after a row is the row whose previous row matched.
    let after_one = c(ConditionKind::Order {
        relation: OrderRelation::After,
        query: Box::new(query(
            v,
            &v.children,
            Some(comparison(&v.child_number, Op::Eq, "1")),
            vec![],
        )),
        within: Some(Distance::Steps { count: 1 }),
        at: TimePoint::When,
        scope: None,
        pick: WitnessChoice::Nearest,
    });
    output.push(case(
        "ql.order.steps",
        query(v, &v.children, Some(after_one), vec![]),
        vec![
            children[1]
                .clone()
                .with("match.id", "x")
                .with(format!("match.{}", v.child_number), 1.)
                .with("match.distance", -1.),
        ],
    ));
    // A coarse time takes part by its whole period: a day that contains an
    // event is neither before nor after it, and a window narrower than the
    // day leaves the day out.
    let mut day = Row::new("day");
    day.time = Some(Extent {
        start: 0,
        end: 10,
        coarse: true,
    });
    let point = at("point", 5);
    let coarse_fixture = BTreeMap::from([
        (v.outer.clone(), vec![day.clone(), point.clone()]),
        (v.children.clone(), vec![at("early", 3), at("late", 15)]),
    ]);
    let order = |relation, within: Option<u64>| {
        c(ConditionKind::Order {
            relation,
            query: Box::new(query(v, &v.children, None, vec![])),
            within: within.map(|w| Distance::Time {
                duration: Value::scalar(format!("{w}s")),
            }),
            at: TimePoint::When,
            scope: None,
            pick: WitnessChoice::Nearest,
        })
    };
    let matched = |row: &Row, inner: &str, offset: Option<f64>| {
        let row = row.clone().with("match.id", inner);
        match offset {
            Some(offset) => row.with("match.distance", offset),
            None => row,
        }
    };
    for (relation, within, expected) in [
        (
            OrderRelation::Before,
            None,
            vec![matched(&day, "late", None), matched(&point, "late", None)],
        ),
        (
            OrderRelation::Before,
            Some(20),
            vec![
                matched(&day, "late", Some(15.)),
                matched(&point, "late", Some(10.)),
            ],
        ),
        (
            OrderRelation::After,
            None,
            vec![matched(&point, "early", None)],
        ),
        (
            OrderRelation::Near,
            Some(5),
            vec![matched(&point, "early", Some(-2.))],
        ),
    ] {
        output.push(Case {
            law: "ql.order.coarse".into(),
            query: query(v, &v.outer, Some(order(relation, within)), vec![]),
            fixture: coarse_fixture.clone(),
            expected: answer(expected),
        });
    }
    // Ranking orders the selected rows and never changes which are selected.
    let east = || Some(comparison(&v.category, Op::Eq, "east"));
    output.push(case(
        "ql.rank.membership",
        query(v, &v.outer, east(), vec![]),
        vec![rows[0].clone(), rows[1].clone()],
    ));
    // Extra rows in the right operand contribute ranks without entering the
    // answer. Descending ranks are c,b,a,u: a scores 1/61 + 1/63, just above
    // b's 2/62. Ascending orders reinforce a,b.
    // Separate same-members operands make the opposite-order tie exact; no
    // occurrence times exist, so identity breaks it. Aligned descending ranks
    // demonstrate that both operand orders can change the resulting order.
    for (left_descending, right_descending, same_members, expected) in [
        (false, true, false, vec![rows[0].clone(), rows[1].clone()]),
        (false, false, false, vec![rows[0].clone(), rows[1].clone()]),
        (false, true, true, vec![rows[0].clone(), rows[1].clone()]),
        (true, true, true, vec![rows[1].clone(), rows[0].clone()]),
    ] {
        output.push(case(
            "ql.rank.membership",
            query(
                v,
                &v.outer,
                east(),
                vec![
                    sort(&v.number, left_descending),
                    t(TransformKind::Combine {
                        how: Combination::Fuse,
                        query: Box::new(query(
                            v,
                            &v.outer,
                            if same_members { east() } else { None },
                            vec![sort(&v.number, right_descending)],
                        )),
                    }),
                ],
            ),
            expected,
        ));
    }
    output.push(case(
        "ql.order.steps",
        query(
            v,
            &v.children,
            Some(comparison(
                &format!("previous.{}", v.child_number),
                Op::Eq,
                "1",
            )),
            vec![],
        ),
        vec![children[1].clone()],
    ));
    let diffed = |row: &Row, change: &str, old: Scalar, new: Scalar| {
        row.clone()
            .with("change", change)
            .with("old_rank", old)
            .with("new_rank", new)
    };
    output.push(case(
        "ql.diff.watch",
        query(
            v,
            &v.outer,
            Some(comparison(&v.number, Op::Gt, "1")),
            vec![t(TransformKind::Diff {
                query: Box::new(query(
                    v,
                    &v.outer,
                    Some(comparison(&v.number, Op::Lt, "5")),
                    vec![],
                )),
            })],
        ),
        vec![
            diffed(&rows[1], "moved", 2.0.into(), 1.0.into()),
            diffed(&rows[2], "joined", Scalar::Unknown, 2.0.into()),
            diffed(&rows[0], "left", 1.0.into(), Scalar::Unknown),
        ],
    ));
    output.push(case(
        "ql.rank.unranked-kept",
        query(
            v,
            &v.outer,
            None,
            vec![t(TransformKind::Combine {
                how: Combination::Fuse,
                query: Box::new(query(
                    v,
                    &v.outer,
                    Some(comparison(&v.number, Op::Gt, "1")),
                    vec![sort(&v.number, true)],
                )),
            })],
        ),
        vec![
            rows[2].clone(),
            rows[1].clone(),
            rows[0].clone(),
            rows[3].clone(),
        ],
    ));
    output.push(case(
        "ql.filter.count-known",
        query(
            v,
            &v.outer,
            Some(c(ConditionKind::Count {
                unit: v.children.clone(),
                condition: Some(Box::new(comparison(&v.child_number, Op::Eq, "3"))),
                op: Op::Eq,
                count: 0,
            })),
            vec![],
        ),
        vec![rows[1].clone(), rows[2].clone(), rows[3].clone()],
    ));
    let inner = query(
        v,
        &v.outer,
        Some(comparison(&v.number, Op::Gt, "3")),
        vec![t(TransformKind::Group {
            keys: vec![Column::field(&v.category)],
        })],
    );
    output.push(case(
        "ql.filter.membership",
        query(
            v,
            &v.outer,
            Some(c(ConditionKind::In {
                left: Expression::field(&v.category),
                query: Box::new(inner),
            })),
            vec![],
        ),
        vec![rows[2].clone(), rows[3].clone()],
    ));
    output
}
/// Run input rotations and reversals, and independently page
/// every case with page sizes one, two and larger than the answer. An absent or
/// repeated continuation, a wrong count, values or witnesses is a violation.
pub fn check<C, E>(compiler: &C, executor: &mut E, cases: &[Case]) -> Vec<Violation>
where
    C: Compiler,
    E: Executor<C::Plan> + ?Sized,
{
    let mut failures = vec![];
    for case in cases {
        let plan = match compiler.compile(&case.query) {
            Ok(p) => p,
            Err(e) => {
                failures.push(Violation {
                    law: case.law.clone(),
                    detail: format!("compile refused: {e}"),
                });
                continue;
            }
        };
        let max = case
            .fixture
            .values()
            .map(Vec::len)
            .max()
            .unwrap_or(1)
            .max(1);
        for rotation in 0..max {
            for reverse in [false, true] {
                let mut fixture = case.fixture.clone();
                for rows in fixture.values_mut() {
                    if !rows.is_empty() {
                        let n = rotation % rows.len();
                        rows.rotate_left(n);
                    }
                    if reverse {
                        rows.reverse();
                    }
                }
                for size in [1, 2, case.expected.total + 1] {
                    let mut request = PageRequest {
                        page_size: NonZeroUsize::new(size).expect("positive size"),
                        cursor: None,
                    };
                    let mut actual = vec![];
                    let mut seen = std::collections::BTreeSet::new();
                    let result: Result<(), String> = (|| {
                        loop {
                            let page = executor
                                .execute(&plan, &fixture, &request)
                                .map_err(|e| format!("execute refused: {e}"))?;
                            if page.answer.total != case.expected.total {
                                return Err(format!(
                                    "total {} expected {}",
                                    page.answer.total, case.expected.total
                                ));
                            }
                            if page.answer.rows.len() > size {
                                return Err("page exceeds page size".into());
                            }
                            actual.extend(page.answer.rows);
                            match page.next {
                                None => break,
                                Some(next) => {
                                    if !seen.insert(next.as_str().to_owned()) {
                                        return Err("continuation repeats".into());
                                    }
                                    request.cursor = Some(next);
                                }
                            }
                        }
                        if actual != case.expected.rows {
                            return Err(format!(
                                "rows {actual:?} expected {:?}",
                                case.expected.rows
                            ));
                        }
                        Ok(())
                    })();
                    if let Err(detail) = result {
                        failures.push(Violation {
                            law: case.law.clone(),
                            detail: format!(
                                "rotation={rotation}, reverse={reverse}, page_size={size}: {detail}"
                            ),
                        });
                    }
                }
            }
        }
    }
    failures
}

/// Exercise each actual transport adapter against the same independent cases.
/// Product Tier 1 tests supply their CLI, socket, HTTP, MCP and brief adapters;
/// naming reference adapters here does not establish product transport parity.
pub fn check_modes<C: Compiler>(
    compiler: &C,
    executors: &mut [(&str, &mut dyn Executor<C::Plan>)],
    cases: &[Case],
) -> Vec<Violation> {
    let mut failures = vec![];
    for (mode, executor) in executors {
        for failure in check(compiler, *executor, cases) {
            failures.push(Violation {
                law: "ql.mode-parity".into(),
                detail: format!("{mode}: {}: {}", failure.law, failure.detail),
            });
        }
    }
    failures
}

/// Product/catalog refusal cases run through the same compiler and executor.
/// Required message fragments let a catalog assert the named set/unit/hint
/// without freezing incidental diagnostic prose.
#[derive(Clone, Debug)]
pub struct RefusalCase {
    pub law: String,
    pub query: Query,
    pub fixture: Dataset,
    pub kind: crate::ErrorKind,
    pub message_contains: Vec<String>,
}
pub fn check_refusals<C, E>(compiler: &C, executor: &mut E, cases: &[RefusalCase]) -> Vec<Violation>
where
    C: Compiler,
    E: Executor<C::Plan> + ?Sized,
{
    let mut failures = vec![];
    for case in cases {
        let refusal = match compiler.compile(&case.query) {
            Err(error) => Some(error),
            Ok(plan) => executor
                .execute(
                    &plan,
                    &case.fixture,
                    &PageRequest {
                        page_size: NonZeroUsize::new(1).expect("positive"),
                        cursor: None,
                    },
                )
                .err(),
        };
        let detail = match refusal {
            None => Some("query was accepted instead of refused".into()),
            Some(error) if error.kind != case.kind => Some(format!(
                "refusal kind {:?} expected {:?}",
                error.kind, case.kind
            )),
            Some(error) => case
                .message_contains
                .iter()
                .find(|text| !error.message.contains(text.as_str()))
                .map(|text| format!("refusal did not name {text:?}: {}", error.message)),
        };
        if let Some(detail) = detail {
            failures.push(Violation {
                law: case.law.clone(),
                detail,
            });
        }
    }
    failures
}
