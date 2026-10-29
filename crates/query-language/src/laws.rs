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
    pub category: String,
}
impl Default for Vocabulary {
    fn default() -> Self {
        Self {
            outer: "rows".into(),
            children: "children".into(),
            number: "value".into(),
            category: "category".into(),
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
fn s(kind: StageKind) -> Stage {
    Stage {
        span: Span::default(),
        kind,
    }
}
fn query(unit: &str, condition: Option<Condition>, stages: Vec<Stage>) -> Query {
    Query {
        source: Source::Unit {
            unit: unit.into(),
            condition,
        },
        attach: vec![],
        stages,
    }
}
fn comparison(field: &str, op: Op, value: &str) -> Condition {
    c(ConditionKind::Compare {
        operand: Operand::field(field),
        op,
        value: Value::scalar(value),
    })
}
fn answer(rows: Vec<Row>) -> Answer {
    Answer {
        total: rows.len(),
        rows,
    }
}
/// Independent expected answers are literal fixture projections. No call here
/// executes a query or derives the oracle by the mechanism being tested.
pub fn cases(v: &Vocabulary) -> Vec<Case> {
    let rows = vec![
        Row::new("a").with(&v.number, 1.).with(&v.category, "east"),
        Row::new("b").with(&v.number, 3.).with(&v.category, "east"),
        Row::new("c").with(&v.number, 5.).with(&v.category, "west"),
        Row::new("u")
            .with(&v.number, Scalar::Unknown)
            .with(&v.category, "west"),
    ];
    let mut children = vec![];
    for (id, n, order, time) in [("x", 1., 0, 20), ("y", 3., 1, 10), ("z", 3., 2, 30)] {
        let mut row = Row::new(id).with(&v.number, n);
        row.parent = Some("a".into());
        row.order = order;
        row.time = Some(Extent {
            start: time,
            end: time,
            coarse: false,
        });
        children.push(row);
    }
    let mut unknown = Row::new("unknown-child").with(&v.number, Scalar::Unknown);
    unknown.parent = Some("u".into());
    children.push(unknown);
    let fixture = BTreeMap::from([
        (v.outer.clone(), rows.clone()),
        (v.children.clone(), children),
    ]);
    let mut output = vec![];
    let a = comparison(&v.number, Op::Gt, "1");
    let b = comparison(&v.number, Op::Lt, "5");
    for stages in [
        vec![
            s(StageKind::Where {
                condition: a.clone(),
            }),
            s(StageKind::Where {
                condition: b.clone(),
            }),
        ],
        vec![s(StageKind::Where {
            condition: c(ConditionKind::And {
                conditions: vec![a, b],
            }),
        })],
    ] {
        output.push(Case {
            law: "filter-composition".into(),
            query: query(&v.outer, None, stages),
            fixture: fixture.clone(),
            expected: answer(vec![rows[1].clone()]),
        });
    }
    for (op, expected) in [
        (Op::Eq, vec![rows[1].clone()]),
        (Op::Ne, vec![rows[0].clone(), rows[2].clone()]),
        (Op::Gt, vec![rows[2].clone()]),
        (Op::Ge, vec![rows[1].clone(), rows[2].clone()]),
        (Op::Lt, vec![rows[0].clone()]),
        (Op::Le, vec![rows[0].clone(), rows[1].clone()]),
    ] {
        output.push(Case {
            law: "three-valued-filters".into(),
            query: query(&v.outer, Some(comparison(&v.number, op, "3")), vec![]),
            fixture: fixture.clone(),
            expected: answer(expected),
        });
    }
    output.push(Case {
        law: "three-valued-negation".into(),
        query: query(
            &v.outer,
            Some(c(ConditionKind::Not {
                condition: Box::new(comparison(&v.number, Op::Eq, "3")),
            })),
            vec![],
        ),
        fixture: fixture.clone(),
        expected: answer(vec![rows[0].clone(), rows[2].clone()]),
    });
    let aggregate = Aggregate {
        function: AggregateFunction::Sum,
        fields: vec![v.number.clone()],
    };
    let column = aggregate.column();
    let category = Operand::field(&v.category).column();
    let east = Row::new("group:[Text(\"east\")]")
        .with(&category, "east")
        .with("count", 2.)
        .with(&column, 4.)
        .with(format!("unknown_{}", v.number.replace(['.', '-'], "_")), 0.);
    output.push(Case {
        law: "grouped-rows-are-a-relation".into(),
        query: query(
            &v.outer,
            None,
            vec![
                s(StageKind::Group {
                    keys: vec![Operand::field(&v.category)],
                }),
                s(StageKind::Aggregate {
                    aggregates: vec![aggregate],
                }),
                s(StageKind::Where {
                    condition: comparison(&column, Op::Gt, "3"),
                }),
                s(StageKind::Sort {
                    keys: vec![SortKey {
                        operand: Operand::field(&column),
                        descending: false,
                    }],
                }),
                s(StageKind::Limit { count: 1 }),
            ],
        ),
        fixture: fixture.clone(),
        expected: answer(vec![east]),
    });
    let percentile = Aggregate {
        function: AggregateFunction::Percentile(50),
        fields: vec![v.number.clone()],
    };
    output.push(Case {
        law: "nearest-rank".into(),
        query: query(
            &v.outer,
            None,
            vec![s(StageKind::Aggregate {
                aggregates: vec![percentile.clone()],
            })],
        ),
        fixture: fixture.clone(),
        expected: answer(vec![
            Row::new("aggregate")
                .with(percentile.column(), 3.)
                .with(format!("unknown_{}", v.number.replace(['.', '-'], "_")), 1.),
        ]),
    });
    output.push(Case {
        law: "exists-three-valued".into(),
        query: query(
            &v.outer,
            Some(c(ConditionKind::Not {
                condition: Box::new(c(ConditionKind::Exists {
                    unit: v.children.clone(),
                    condition: Some(Box::new(comparison(&v.number, Op::Eq, "3"))),
                })),
            })),
            vec![],
        ),
        fixture: fixture.clone(),
        expected: answer(vec![rows[1].clone(), rows[2].clone()]),
    });
    let seq = c(ConditionKind::Sequence {
        unit: v.children.clone(),
        all: false,
        steps: vec![
            comparison(&v.number, Op::Eq, "1"),
            comparison(&v.number, Op::Eq, "3"),
        ],
        gaps: vec![Gap::Later],
    });
    let mut matched = Row::new("a:match:1")
        .with("step1_id", "x")
        .with("step2_id", "y");
    matched.parent = Some("a".into());
    matched.matches = vec!["x".into(), "y".into()];
    output.push(Case {
        law: "sequence-match-order-independent".into(),
        query: query(&v.outer, Some(seq), vec![s(StageKind::Matches)]),
        fixture: fixture.clone(),
        expected: answer(vec![matched]),
    });
    output.push(Case {
        law: "page-is-not-the-answer".into(),
        query: query(
            &v.outer,
            None,
            vec![
                s(StageKind::Sort {
                    keys: vec![SortKey {
                        operand: Operand::field(&v.number),
                        descending: true,
                    }],
                }),
                s(StageKind::Limit { count: 3 }),
            ],
        ),
        fixture,
        expected: answer(vec![rows[2].clone(), rows[1].clone(), rows[0].clone()]),
    });
    // A nearest precise witness beats an equally near coarse witness; equal
    // precise distances use identity, independently of enumeration order.
    let mut left = Row::new("outer");
    left.time = Some(Extent {
        start: 10,
        end: 10,
        coarse: false,
    });
    let mut near_a = Row::new("near-a");
    near_a.time = Some(Extent {
        start: 9,
        end: 9,
        coarse: false,
    });
    let mut near_b = Row::new("near-b");
    near_b.time = Some(Extent {
        start: 11,
        end: 11,
        coarse: false,
    });
    let mut coarse = Row::new("coarse");
    coarse.time = Some(Extent {
        start: 10,
        end: 11,
        coarse: true,
    });
    let temporal_fixture = BTreeMap::from([
        (v.outer.clone(), vec![left.clone()]),
        (v.children.clone(), vec![near_b, coarse, near_a]),
    ]);
    left.matches = vec!["near-a".into()];
    output.push(Case {
        law: "join-match-order-independent".into(),
        query: query(
            &v.outer,
            None,
            vec![s(StageKind::Temporal {
                relation: TemporalRelation::Near,
                query: Box::new(query(&v.children, None, vec![])),
                at: TimePoint::When,
                within: Some(Value::scalar("5")),
                all: false,
            })],
        ),
        fixture: temporal_fixture,
        expected: answer(vec![left]),
    });
    let count = c(ConditionKind::Count {
        unit: v.children.clone(),
        condition: Some(Box::new(comparison(&v.number, Op::Eq, "3"))),
        op: Op::Eq,
        count: 0,
    });
    output.push(Case {
        law: "count-is-known".into(),
        query: query(&v.outer, Some(count), vec![]),
        fixture: output[0].fixture.clone(),
        expected: answer(vec![rows[1].clone(), rows[2].clone(), rows[3].clone()]),
    });
    let inner = query(
        &v.outer,
        Some(comparison(&v.number, Op::Gt, "3")),
        vec![s(StageKind::Group {
            keys: vec![Operand::field(&v.category)],
        })],
    );
    output.push(Case {
        law: "membership-over-grouped-relation".into(),
        query: query(
            &v.outer,
            Some(c(ConditionKind::In {
                operand: Operand::field(&v.category),
                query: Box::new(inner),
            })),
            vec![],
        ),
        fixture: output[0].fixture.clone(),
        expected: answer(vec![rows[2].clone(), rows[3].clone()]),
    });
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
                law: "execution-mode-parity".into(),
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
