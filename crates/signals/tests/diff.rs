//! Law `ql.diff.watch`: a watch's change report and the query language's
//! `A | diff (B)` agree on membership, ranking and top-k changes.
use query_language::ast::*;
use query_language::semantics::{Dataset, ReferenceEvaluator, Row, Scalar};
use signals::{ChangeKind, Occurrence, difference};
use std::collections::BTreeSet;

fn occurrence(id: &str) -> Occurrence {
    Occurrence {
        id: id.into(),
        period: String::new(),
    }
}
/// Deterministic generated answers: subsets of eight rows in shuffled orders.
fn answers(seed: u64) -> (Vec<String>, Vec<String>) {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut next = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        state >> 33
    };
    let pick = |next: &mut dyn FnMut() -> u64| {
        let mut ids: Vec<String> = (0..8)
            .filter(|_| !next().is_multiple_of(3))
            .map(|i| format!("r{i}"))
            .collect();
        for i in (1..ids.len()).rev() {
            ids.swap(i, (next() as usize) % (i + 1));
        }
        ids
    };
    (pick(&mut next), pick(&mut next))
}
fn ranked(field: &str) -> Query {
    let condition = Condition {
        span: Span::default(),
        kind: ConditionKind::Match {
            field: field.into(),
            value: Value::Known,
        },
    };
    Query {
        source: Source::Unit {
            unit: "rows".into(),
            implicit: false,
            condition: Some(condition),
        },
        attach: vec![],
        transforms: vec![Transform {
            span: Span::default(),
            kind: TransformKind::Sort {
                keys: vec![SortKey {
                    expression: Expression::field(field),
                    descending: false,
                }],
            },
        }],
    }
}
/// The oracle's `diff` rows of one kind, as a sorted set.
fn rows_of(rows: &[Row], kind: &str) -> BTreeSet<String> {
    rows.iter()
        .filter(|row| row.value("change") == Scalar::Text(kind.into()))
        .map(|row| row.id.clone())
        .collect()
}

#[test]
fn diff_watch() {
    for seed in 0..400 {
        let (before, after) = answers(seed);
        let mut rows = Vec::new();
        for i in 0..8 {
            let id = format!("r{i}");
            let mut row = Row::new(id.clone());
            if let Some(rank) = before.iter().position(|b| *b == id) {
                row = row.with("before_rank", rank as f64);
            }
            if let Some(rank) = after.iter().position(|a| *a == id) {
                row = row.with("after_rank", rank as f64);
            }
            rows.push(row);
        }
        let data: Dataset = [("rows".to_string(), rows)].into();
        let mut query = ranked("after_rank");
        query.transforms.push(Transform {
            span: Span::default(),
            kind: TransformKind::Diff {
                query: Box::new(ranked("before_rank")),
            },
        });
        let diffed = ReferenceEvaluator::new(&data).execute(&query).unwrap().rows;
        let old: Vec<_> = before.iter().map(|id| occurrence(id)).collect();
        let new: Vec<_> = after.iter().map(|id| occurrence(id)).collect();
        let ids = |list: &[Occurrence]| list.iter().map(|o| o.id.clone()).collect::<BTreeSet<_>>();

        let membership = difference(&old, &new, ChangeKind::Membership, None);
        let joined = rows_of(&diffed, "joined");
        let left = rows_of(&diffed, "left");
        assert_eq!(
            membership.is_some(),
            !joined.is_empty() || !left.is_empty(),
            "seed {seed}"
        );
        if let Some(change) = membership {
            assert_eq!(ids(&change.joined), joined, "seed {seed}");
            assert_eq!(ids(&change.left), left, "seed {seed}");
        }
        let ranking = difference(&old, &new, ChangeKind::Ranking, None);
        assert_eq!(ranking.is_some(), !diffed.is_empty(), "seed {seed}");

        for k in 0..4 {
            let top = |list: &[String]| list.iter().take(k).cloned().collect::<Vec<_>>();
            let changes = query_language::diff::changes(&top(&before), &top(&after));
            let top_k = difference(&old, &new, ChangeKind::TopK, Some(k));
            assert_eq!(
                top_k.is_some(),
                query_language::diff::membership_changed(&changes),
                "seed {seed}, k {k}"
            );
        }
    }
}
