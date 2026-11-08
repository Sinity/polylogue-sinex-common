//! Tests of next-set selection, against the fit's public numbers.

#[allow(dead_code)]
mod common;

use ranking::{Fit, FitOptions, Judgment, RankError, SeatWhy, SelectionRule, fit, next_set};

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn fitted(items: &[&str], judgments: &[Judgment]) -> Fit {
    fit(&names(items), judgments, &FitOptions::default()).unwrap()
}

/// The variance a fit reports for an item, read from its interval.
fn variance_from_interval(fit: &Fit, item: &str) -> f64 {
    let (low, high) = fit.item(item).unwrap().interval;
    ((high - low) / (2.0 * 1.959_963_984_540_054)).powi(2)
}

fn pair_reduction(fit: &Fit, left: &str, right: &str) -> f64 {
    let (a, b) = (fit.item(left).unwrap(), fit.item(right).unwrap());
    let variance = variance_from_interval(fit, left) + variance_from_interval(fit, right);
    let probability = 1.0 / (1.0 + (b.score - a.score).exp());
    let information = probability * (1.0 - probability);
    variance * variance * information / (1.0 + variance * information)
}

const ALL_RULES: [SelectionRule; 3] = [
    SelectionRule::Information,
    SelectionRule::Random { seed: 7 },
    SelectionRule::UncomparedFirst,
];

#[test]
fn rank_next_includes_unseen() {
    let items = ["a", "b", "c", "d", "e", "f", "g"];
    for rule in ALL_RULES {
        let mut judgments: Vec<Judgment> = Vec::new();
        let mut proposals = 0;
        let size = 3;
        let bound = items.len().div_ceil(size);
        loop {
            let current = fitted(&items, &judgments);
            let uncompared: Vec<&str> = items
                .iter()
                .copied()
                .filter(|item| current.item(item).unwrap().support == 0)
                .collect();
            if uncompared.is_empty() {
                break;
            }
            proposals += 1;
            assert!(
                proposals <= bound,
                "{rule:?}: {proposals} proposals for {} items",
                items.len()
            );
            let proposal = next_set(&current, &names(&items), size, rule).unwrap();
            assert_eq!(proposal.seats.len(), size);
            let fresh = proposal
                .seats
                .iter()
                .filter(|seat| uncompared.contains(&seat.item.as_str()))
                .count();
            assert!(
                fresh >= 1.min(uncompared.len()),
                "{rule:?} proposed no uncompared item"
            );
            let mut ordered: Vec<String> = proposal
                .seats
                .iter()
                .map(|seat| seat.item.clone())
                .collect();
            ordered.sort();
            judgments.push(Judgment::ordering("operator", ordered));
        }
        assert!(proposals <= bound);
    }
}

#[test]
fn rank_next_information_max_reduction() {
    let items = ["a", "b", "c", "d", "e"];
    let judgments = vec![
        Judgment::ordering("operator", ["a", "b", "c"]),
        Judgment::ordering("operator", ["d", "a"]),
        Judgment::ordering("operator", ["e", "c", "d"]),
        Judgment::ordering("operator", ["b", "e"]),
    ];
    let current = fitted(&items, &judgments);
    let proposal = next_set(&current, &names(&items), 2, SelectionRule::Information).unwrap();
    let mut best = (f64::NEG_INFINITY, String::new(), String::new());
    for (index, left) in items.iter().enumerate() {
        for right in &items[index + 1..] {
            let reduction = pair_reduction(&current, left, right);
            if reduction > best.0 {
                best = (reduction, (*left).to_owned(), (*right).to_owned());
            }
        }
    }
    let chosen: Vec<&str> = proposal
        .seats
        .iter()
        .map(|seat| seat.item.as_str())
        .collect();
    assert!(chosen.contains(&best.1.as_str()) && chosen.contains(&best.2.as_str()));
    match proposal.seats[0].why {
        SeatWhy::Information { reduction } => assert!((reduction - best.0).abs() < 1e-9),
        ref other => panic!("expected information seat, got {other:?}"),
    }
}

#[test]
fn rank_next_random_seeded() {
    let items = ["a", "b", "c", "d", "e", "f"];
    let judgments = vec![
        Judgment::ordering("operator", ["a", "b"]),
        Judgment::ordering("operator", ["c", "d"]),
    ];
    let current = fitted(&items, &judgments);
    let first = next_set(
        &current,
        &names(&items),
        3,
        SelectionRule::Random { seed: 11 },
    )
    .unwrap();
    let again = next_set(
        &current,
        &names(&items),
        3,
        SelectionRule::Random { seed: 11 },
    )
    .unwrap();
    assert_eq!(first, again);
    let distinct: std::collections::BTreeSet<String> = (0..20)
        .map(|seed| {
            let proposal =
                next_set(&current, &names(&items), 3, SelectionRule::Random { seed }).unwrap();
            proposal
                .seats
                .iter()
                .map(|seat| seat.item.clone())
                .collect::<Vec<_>>()
                .join(",")
        })
        .collect();
    assert!(distinct.len() > 1);
}

#[test]
fn rank_next_uncompared_nearest() {
    let items = ["a", "b", "c", "d", "u"];
    let judgments = vec![
        Judgment::ordering("operator", ["a", "b", "c", "d"]),
        Judgment::ordering("operator", ["a", "c"]),
    ];
    let current = fitted(&items, &judgments);
    let proposal = next_set(&current, &names(&items), 2, SelectionRule::UncomparedFirst).unwrap();
    assert_eq!(proposal.seats[0].item, "u");
    assert_eq!(proposal.seats[0].why, SeatWhy::Uncompared);
    let anchor_score = current.item("u").unwrap().score;
    let expected = ["a", "b", "c", "d"]
        .into_iter()
        .min_by(|left, right| {
            let distance = |item: &str| (current.item(item).unwrap().score - anchor_score).abs();
            distance(left)
                .total_cmp(&distance(right))
                .then(left.cmp(right))
        })
        .unwrap();
    assert_eq!(proposal.seats[1].item, expected);
    assert_eq!(
        proposal.seats[1].why,
        SeatWhy::NearestScore { anchor: "u".into() }
    );
}

#[test]
fn rank_next_refuses_bad_sets() {
    let items = ["a", "b", "c"];
    let current = fitted(&items, &[Judgment::ordering("operator", ["a", "b"])]);
    assert_eq!(
        next_set(&current, &names(&items), 1, SelectionRule::Information),
        Err(RankError::SetBelowTwo { size: 1 })
    );
    assert_eq!(
        next_set(
            &current,
            &names(&["a", "zz"]),
            2,
            SelectionRule::Information
        ),
        Err(RankError::UnknownItem { item: "zz".into() })
    );
    assert_eq!(
        next_set(&current, &names(&["a"]), 2, SelectionRule::Information),
        Err(RankError::TooFewCandidates {
            wanted: 2,
            available: 1
        })
    );
}
