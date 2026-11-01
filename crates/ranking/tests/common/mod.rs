//! Independent oracles for the fit: a brute-force objective that enumerates
//! every internal order of each tie group, and dense linear algebra for the
//! Laplace variances. Nothing here calls the crate's own model code.

use ranking::{Fit, Judgment};

/// Item positions in sorted order, the crate's index for each item.
pub fn sorted_ids(items: &[&str]) -> Vec<String> {
    let mut sorted: Vec<String> = items.iter().map(|item| (*item).to_owned()).collect();
    sorted.sort();
    sorted
}

pub fn position(sorted: &[String], item: &str) -> usize {
    sorted
        .iter()
        .position(|candidate| candidate == item)
        .expect("item in index")
}

/// A judgment as tie groups of positions in the sorted index.
pub fn groups_of(sorted: &[String], judgment: &Judgment) -> Vec<Vec<usize>> {
    judgment
        .order
        .iter()
        .map(|group| group.iter().map(|item| position(sorted, item)).collect())
        .collect()
}

fn permutations(items: &[usize]) -> Vec<Vec<usize>> {
    if items.len() <= 1 {
        return vec![items.to_vec()];
    }
    let mut out = Vec::new();
    for (index, &head) in items.iter().enumerate() {
        let mut rest = items.to_vec();
        rest.remove(index);
        for mut tail in permutations(&rest) {
            tail.insert(0, head);
            out.push(tail);
        }
    }
    out
}

/// The log-posterior of the weak-prior Plackett–Luce model, with each tie
/// group's internal order averaged uniformly over all of its orders.
pub fn brute_objective(judgments: &[Vec<Vec<usize>>], theta: &[f64]) -> f64 {
    let gamma: Vec<f64> = theta.iter().map(|value| value.exp()).collect();
    let mut total: f64 = theta
        .iter()
        .map(|value| value - 2.0 * (value.exp() + 1.0).ln())
        .sum();
    for groups in judgments {
        let choices: Vec<Vec<Vec<usize>>> =
            groups.iter().map(|group| permutations(group)).collect();
        let mut odometer = vec![0usize; groups.len()];
        let mut sum = 0.0;
        let mut count = 0usize;
        loop {
            let order: Vec<usize> = odometer
                .iter()
                .enumerate()
                .flat_map(|(group, &choice)| choices[group][choice].clone())
                .collect();
            let mut log_likelihood = 0.0;
            for start in 0..order.len() {
                let denominator: f64 = order[start..].iter().map(|&item| gamma[item]).sum();
                log_likelihood += theta[order[start]] - denominator.ln();
            }
            sum += log_likelihood;
            count += 1;
            let mut position = 0;
            loop {
                if position == odometer.len() {
                    break;
                }
                odometer[position] += 1;
                if odometer[position] < choices[position].len() {
                    break;
                }
                odometer[position] = 0;
                position += 1;
            }
            if position == odometer.len() {
                break;
            }
        }
        total += sum / count as f64;
    }
    total
}

pub fn gradient(judgments: &[Vec<Vec<usize>>], theta: &[f64]) -> Vec<f64> {
    let step = 1e-6;
    (0..theta.len())
        .map(|i| {
            let mut up = theta.to_vec();
            let mut down = theta.to_vec();
            up[i] += step;
            down[i] -= step;
            (brute_objective(judgments, &up) - brute_objective(judgments, &down)) / (2.0 * step)
        })
        .collect()
}

/// Observed information: the negated Hessian by central differences.
pub fn information(judgments: &[Vec<Vec<usize>>], theta: &[f64]) -> Vec<Vec<f64>> {
    let step = 1e-4;
    let n = theta.len();
    let mut matrix = vec![vec![0.0; n]; n];
    for (i, row) in matrix.iter_mut().enumerate() {
        for (j, entry) in row.iter_mut().enumerate() {
            let value = |di: f64, dj: f64| {
                let mut point = theta.to_vec();
                point[i] += di;
                point[j] += dj;
                brute_objective(judgments, &point)
            };
            let second = (value(step, step) - value(step, -step) - value(-step, step)
                + value(-step, -step))
                / (4.0 * step * step);
            *entry = -second;
        }
    }
    matrix
}

/// Diagonal of the inverse, by Gauss–Jordan elimination.
pub fn inverse_diagonal(matrix: &[Vec<f64>]) -> Vec<f64> {
    let n = matrix.len();
    let mut augmented: Vec<Vec<f64>> = matrix
        .iter()
        .enumerate()
        .map(|(row, values)| {
            let mut line = values.clone();
            line.extend((0..n).map(|column| f64::from(u8::from(column == row))));
            line
        })
        .collect();
    for pivot in 0..n {
        let scale = augmented[pivot][pivot];
        for value in augmented[pivot].iter_mut() {
            *value /= scale;
        }
        for row in 0..n {
            if row != pivot {
                let factor = augmented[row][pivot];
                let source = augmented[pivot].clone();
                for (value, other) in augmented[row].iter_mut().zip(source) {
                    *value -= factor * other;
                }
            }
        }
    }
    (0..n).map(|row| augmented[row][n + row]).collect()
}

/// The fit's score of each sorted item, by item id.
pub fn scores_in_order(fit: &Fit, sorted: &[String]) -> Vec<f64> {
    sorted
        .iter()
        .map(|item| fit.item(item).expect("item in fit").score)
        .collect()
}
