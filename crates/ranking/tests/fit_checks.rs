//! Tests of the fit, checked against independent oracles in `common`.

#[allow(dead_code)]
mod common;

use common::*;
use ranking::{FitOptions, FitStatus, JudgeSet, Judgment, RankError, fit, fit_warm};

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn options() -> FitOptions {
    FitOptions::default()
}

/// Newton's method on the Bradley–Terry log-posterior for pair judgments,
/// written in closed form for pairs (independent of the MM update). The
/// information matrix is the negated Hessian, so each step adds its inverse
/// applied to the gradient.
fn newton_bradley_terry(sorted: &[String], pairs: &[(&str, &str)]) -> Vec<f64> {
    let n = sorted.len();
    let mut theta = vec![0.0f64; n];
    for _ in 0..200 {
        let mut gradient = vec![0.0f64; n];
        let mut information = vec![vec![0.0f64; n]; n];
        for (i, value) in theta.iter().enumerate() {
            let gamma = value.exp();
            gradient[i] += 1.0 - 2.0 * gamma / (gamma + 1.0);
            information[i][i] += 2.0 * gamma / ((gamma + 1.0) * (gamma + 1.0));
        }
        for (winner, loser) in pairs {
            let (w, l) = (position(sorted, winner), position(sorted, loser));
            let (gw, gl) = (theta[w].exp(), theta[l].exp());
            let p = gw / (gw + gl);
            gradient[w] += 1.0 - p;
            gradient[l] -= 1.0 - p;
            let curvature = p * (1.0 - p);
            information[w][w] += curvature;
            information[l][l] += curvature;
            information[w][l] -= curvature;
            information[l][w] -= curvature;
        }
        let step = solve(&information, &gradient);
        for (value, delta) in theta.iter_mut().zip(step) {
            *value += delta;
        }
    }
    theta
}

/// Solves `matrix · x = rhs` by Gaussian elimination with partial pivoting.
fn solve(matrix: &[Vec<f64>], rhs: &[f64]) -> Vec<f64> {
    let n = rhs.len();
    let mut rows: Vec<Vec<f64>> = matrix
        .iter()
        .zip(rhs)
        .map(|(row, value)| {
            let mut line = row.clone();
            line.push(*value);
            line
        })
        .collect();
    for pivot in 0..n {
        let best = (pivot..n)
            .max_by(|a, b| rows[*a][pivot].abs().total_cmp(&rows[*b][pivot].abs()))
            .unwrap();
        rows.swap(pivot, best);
        for row in pivot + 1..n {
            let factor = rows[row][pivot] / rows[pivot][pivot];
            let source = rows[pivot].clone();
            for (value, other) in rows[row].iter_mut().zip(source) {
                *value -= factor * other;
            }
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let tail: f64 = (row + 1..n)
            .map(|column| rows[row][column] * x[column])
            .sum();
        x[row] = (rows[row][n] - tail) / rows[row][row];
    }
    x
}

#[test]
fn rank_pl_reduces_to_bt() {
    let items = ["a", "b", "c", "d"];
    let pairs = [
        ("a", "b"),
        ("a", "c"),
        ("b", "c"),
        ("c", "d"),
        ("b", "d"),
        ("a", "b"),
        ("d", "a"),
    ];
    let judgments: Vec<Judgment> = pairs
        .iter()
        .map(|(w, l)| Judgment::ordering("operator", [*w, *l]))
        .collect();
    let fitted = fit(&names(&items), &judgments, &options()).unwrap();
    let sorted = sorted_ids(&items);
    let oracle = newton_bradley_terry(&sorted, &pairs);
    for (item, expected) in sorted.iter().zip(&oracle) {
        let got = fitted.item(item).unwrap().score;
        assert!((got - expected).abs() < 1e-7, "{item}: {got} vs {expected}");
    }
}

#[test]
fn rank_k_not_expanded() {
    let four = vec![Judgment::ordering("operator", ["a", "b", "c", "d"])];
    let mut pairs = Vec::new();
    for (index, left) in ["a", "b", "c", "d"].iter().enumerate() {
        for right in ["a", "b", "c", "d"].iter().skip(index + 1) {
            pairs.push(Judgment::ordering("operator", [*left, *right]));
        }
    }
    let items = names(&["a", "b", "c", "d"]);
    let ordered = fit(&items, &four, &options()).unwrap();
    let expanded = fit(&items, &pairs, &options()).unwrap();
    assert_eq!(ordered.judgment_count, 1);
    assert_eq!(expanded.judgment_count, 6);
    assert_eq!(ordered.item("a").unwrap().support, 1);
    assert_eq!(expanded.item("a").unwrap().support, 3);
    assert_ne!(
        ordered.item("a").unwrap().score,
        expanded.item("a").unwrap().score
    );
}

#[test]
fn rank_judge_isolation() {
    let items = names(&["a", "b", "c"]);
    let operator = vec![
        Judgment::ordering("operator", ["a", "b", "c"]),
        Judgment::ordering("operator", ["c", "a"]),
    ];
    let mut with_agents = operator.clone();
    with_agents.push(Judgment::ordering("agent-7", ["b", "a"]));
    with_agents.push(Judgment::ordering("agent-7", ["b", "c"]));
    let set = FitOptions {
        judges: JudgeSet::named(["operator"]),
        ..FitOptions::default()
    };
    let alone = fit(&items, &operator, &set).unwrap();
    let mixed = fit(&items, &with_agents, &set).unwrap();
    assert_eq!(alone, mixed);
    let everyone = fit(&items, &with_agents, &options()).unwrap();
    assert_ne!(alone.item("b"), everyone.item("b"));
}

#[test]
fn rank_deterministic() {
    let judgments = vec![
        Judgment::ordering("operator", ["a", "b", "c"]),
        Judgment {
            judge: "operator".into(),
            order: vec![vec!["d".into()], vec!["b".into(), "a".into()]],
        },
        Judgment::ordering("operator", ["c", "d"]),
        Judgment::ordering("agent", ["d", "a"]),
    ];
    let items = names(&["a", "b", "c", "d"]);
    let first = fit(&items, &judgments, &options()).unwrap();
    let mut reversed_items = items.clone();
    reversed_items.reverse();
    let mut reversed = judgments.clone();
    reversed.reverse();
    let second = fit(&reversed_items, &reversed, &options()).unwrap();
    assert_eq!(first, second);
}

#[test]
fn rank_ties_pull_together() {
    let judgments = vec![
        Judgment {
            judge: "operator".into(),
            order: vec![vec!["c".into()], vec!["a".into(), "b".into()]],
        },
        Judgment {
            judge: "operator".into(),
            order: vec![vec!["c".into()], vec!["a".into(), "b".into()]],
        },
        Judgment::ordering("operator", ["a", "b"]),
        Judgment::ordering("operator", ["b", "a"]),
    ];
    let fitted = fit(&names(&["a", "b", "c"]), &judgments, &options()).unwrap();
    let a = fitted.item("a").unwrap().score;
    let b = fitted.item("b").unwrap().score;
    assert!((a - b).abs() < 1e-9);
    assert!(fitted.item("c").unwrap().score > a);
    // A judgment that is one tie group is informative: it pulls its items
    // together and says nothing about items outside it.
    let pair_tie = vec![
        Judgment {
            judge: "operator".into(),
            order: vec![vec!["x".into(), "y".into()]],
        },
        Judgment::ordering("operator", ["x", "z"]),
    ];
    let tied = fit(&names(&["x", "y", "z"]), &pair_tie, &options()).unwrap();
    assert!(tied.item("x").unwrap().score > tied.item("y").unwrap().score - 1e-9);
    assert!(tied.item("x").unwrap().score > tied.item("z").unwrap().score);
}

#[test]
fn rank_stationary_and_laplace() {
    let items = ["a", "b", "c", "d"];
    let judgments = vec![
        Judgment {
            judge: "operator".into(),
            order: vec![
                vec!["a".into()],
                vec!["b".into(), "c".into()],
                vec!["d".into()],
            ],
        },
        Judgment::ordering("operator", ["c", "a", "d"]),
        Judgment::ordering("operator", ["b", "d"]),
        Judgment {
            judge: "operator".into(),
            order: vec![vec!["d".into(), "a".into()], vec!["c".into()]],
        },
    ];
    let fitted = fit(&names(&items), &judgments, &options()).unwrap();
    let sorted = sorted_ids(&items);
    let groups: Vec<Vec<Vec<usize>>> = judgments.iter().map(|j| groups_of(&sorted, j)).collect();
    let theta = scores_in_order(&fitted, &sorted);
    for slope in gradient(&groups, &theta) {
        assert!(slope.abs() < 1e-6, "gradient {slope}");
    }
    let variances = inverse_diagonal(&information(&groups, &theta));
    for (position, item) in sorted.iter().enumerate() {
        let got = fitted.item(item).unwrap().variance;
        let relative = (got - variances[position]).abs() / variances[position];
        assert!(relative < 1e-4, "{item}: {got} vs {}", variances[position]);
        let (low, high) = fitted.item(item).unwrap().interval;
        assert!(((high - low) / 2.0 - 1.959_963_984_540_054 * got.sqrt()).abs() < 1e-12);
    }
}

#[test]
fn rank_unbeaten_finite() {
    let judgments = vec![
        Judgment::ordering("operator", ["a", "b"]),
        Judgment::ordering("operator", ["a", "b"]),
        Judgment::ordering("operator", ["x", "y"]),
    ];
    let fitted = fit(&names(&["a", "b", "c", "x", "y"]), &judgments, &options()).unwrap();
    for item in &fitted.items {
        assert!(item.score.is_finite() && item.variance.is_finite() && item.variance > 0.0);
    }
    let a = fitted.item("a").unwrap();
    let c = fitted.item("c").unwrap();
    assert!(a.score > 0.0 && c.score.abs() < 1e-9 && c.support == 0);
    assert!(c.variance > a.variance);
    let component = |item: &str| fitted.item(item).unwrap().component;
    assert_eq!(component("a"), component("b"));
    assert_ne!(component("a"), component("x"));
    assert_ne!(component("c"), component("a"));
    assert_eq!(component("x"), component("y"));
}

#[test]
fn rank_rank_range_brackets_position() {
    // Consistent orderings push the scores apart, but the Laplace variance of
    // a near-certain comparison is large, so intervals can still overlap. The
    // ranges are therefore checked against pairwise counts over the intervals
    // rather than assumed to be single positions.
    let mut judgments = Vec::new();
    for _ in 0..200 {
        judgments.push(Judgment::ordering("operator", ["a", "b", "c", "d"]));
    }
    judgments.push(Judgment::ordering("operator", ["d", "a"]));
    let fitted = fit(&names(&["a", "b", "c", "d", "e"]), &judgments, &options()).unwrap();
    let count = fitted.items.len();
    for (position, row) in fitted.items.iter().enumerate() {
        assert_eq!(row.rank, position + 1);
        assert!(row.rank_range.best <= row.rank && row.rank <= row.rank_range.worst);
        if position > 0 {
            assert!(fitted.items[position - 1].score >= row.score);
        }
        let above = fitted
            .items
            .iter()
            .filter(|other| other.interval.0 > row.interval.1)
            .count();
        let below = fitted
            .items
            .iter()
            .filter(|other| other.interval.1 < row.interval.0)
            .count();
        assert_eq!(row.rank_range.best, 1 + above, "{}", row.item);
        assert_eq!(row.rank_range.worst, count - below, "{}", row.item);
    }
}

#[test]
fn rank_warm_start_agrees() {
    // A well-determined base, so one further judgment moves the optimum
    // little and the previous scores are a close start.
    let items = names(&["a", "b", "c", "d", "e"]);
    let mut base: Vec<Judgment> = (0..30)
        .map(|_| Judgment::ordering("operator", ["a", "b", "c", "d", "e"]))
        .collect();
    base.push(Judgment::ordering("operator", ["e", "a"]));
    let previous = fit(&items, &base, &options()).unwrap();
    let mut extended = base.clone();
    extended.push(Judgment::ordering("operator", ["b", "d"]));
    let cold = fit(&items, &extended, &options()).unwrap();
    let warm = fit_warm(&previous, &items, &extended, &options()).unwrap();
    for (left, right) in cold.items.iter().zip(&warm.items) {
        assert_eq!(left.item, right.item);
        assert!((left.score - right.score).abs() < 1e-7);
    }
    let (
        FitStatus::Converged {
            iterations: cold_steps,
        },
        FitStatus::Converged {
            iterations: warm_steps,
        },
    ) = (&cold.status, &warm.status)
    else {
        panic!("both fits converge");
    };
    assert!(
        warm_steps < cold_steps,
        "warm {warm_steps} cold {cold_steps}"
    );
}

#[test]
fn rank_stopped_is_reported() {
    let judgments = vec![Judgment::ordering("operator", ["a", "b", "c"])];
    let stopped = FitOptions {
        max_iterations: 1,
        ..FitOptions::default()
    };
    let fitted = fit(&names(&["a", "b", "c"]), &judgments, &stopped).unwrap();
    match fitted.status {
        FitStatus::Stopped { iterations, change } => {
            assert_eq!(iterations, 1);
            assert!(change > 0.0);
        }
        FitStatus::Converged { .. } => panic!("one iteration cannot converge"),
    }
}

#[test]
fn rank_invalid_refused() {
    let items = names(&["a", "b"]);
    let single = vec![Judgment::ordering("operator", ["a"])];
    assert_eq!(
        fit(&items, &single, &options()),
        Err(RankError::TooFewTargets {
            judge: "operator".into()
        })
    );
    let repeated = vec![Judgment::ordering("operator", ["a", "a"])];
    assert!(matches!(
        fit(&items, &repeated, &options()),
        Err(RankError::RepeatedTarget { .. })
    ));
    let empty = vec![Judgment {
        judge: "operator".into(),
        order: vec![vec![], vec!["b".into()]],
    }];
    assert_eq!(
        fit(&items, &empty, &options()),
        Err(RankError::EmptyTieGroup {
            judge: "operator".into()
        })
    );
    let huge: Vec<String> = (0..21).map(|n| format!("i{n:02}")).collect();
    let tie = vec![Judgment {
        judge: "operator".into(),
        order: vec![huge.clone(), vec!["z".into()]],
    }];
    assert!(matches!(
        fit(&huge, &tie, &options()),
        Err(RankError::TieGroupTooLarge { size: 21, .. })
    ));
}
