//! The Plackett–Luce model over judgments, fitted by minorization–maximization.
//!
//! Notation. An item `i` has worth `gamma_i > 0`; its log-worth is the score
//! `theta_i = ln gamma_i`. The reference item of the weak prior has worth 1
//! and is never updated.
//!
//! A judgment is a sequence of tie groups `B_0, B_1, …`. For group `B` with
//! lower items `L` (every item of the later groups), one stage is the chance
//! that the group's members come next. Strict groups (one item each) give the
//! ordinary Plackett–Luce stage `gamma_w / (gamma_w + sum over L)`.
//!
//! A tie group of size `g` is modelled by a uniform random tie-break: each of
//! its `g!` internal orders is equally likely. The stage with `U` already
//! placed members has risk set `(B \ U) ∪ L`, and the group contributes the
//! expectation over orders of its stage log-likelihoods:
//!
//! `Q_B(theta) = Σ_{i∈B} theta_i − Σ_{U ⊊ B} w_U · ln S_U`,
//! with `w_U = 1 / C(g, |U|)` and `S_U = Σ_{(B\U)∪L} gamma`.
//!
//! The weak prior adds, per item, one virtual win against the reference and
//! one virtual loss: `theta_i − 2 ln(gamma_i + 1)`. The total objective is
//! strictly concave in `theta`, so the maximizer is unique.
//!
//! The update is Hunter's (2004) minorization–maximization, applied to the
//! weighted sum above. Using `ln S ≤ ln S_old + (S − S_old)/S_old` gives
//! `gamma_i ← W_i / D_i`, where `W_i = 1 + support_i` (every judgment that
//! names `i` contributes one unit of winner weight) and
//! `D_i = 2 / (gamma_i + 1) + Σ_{stages containing i} w_U / S_U` evaluated at
//! the current worths. Each step cannot decrease the objective.
//!
//! The observed information `−∂²Q/∂theta²` is `Σ w_U (diag(pi) − pi pi ᵀ)`
//! over the stages, with `pi_r = gamma_r / S_U`, plus `2 pi (1 − pi)` on each
//! item's diagonal for the prior. Its inverse gives the Laplace variances.

use crate::judgment::{Judgment, RankError};
use std::collections::BTreeMap;

/// The largest tie group the uniform tie-break model accepts. Its cost is
/// `2^g` subsets per iteration; judgments from the judging interface have
/// `k ≤ 6`, so this bound is a cost bound, not an input rule.
pub(crate) const MAX_TIE_GROUP: usize = 20;

#[derive(Clone, Debug)]
pub(crate) struct Subset {
    /// `w_U`: the probability that this exact set is the placed prefix.
    pub weight: f64,
    /// The group members not yet placed, in ascending item index.
    pub remaining: Vec<usize>,
}

#[derive(Clone, Debug)]
pub(crate) struct Stage {
    /// The tie group, ascending item index.
    pub group: Vec<usize>,
    /// Items of every later group of the same judgment.
    pub lower: Vec<usize>,
    pub subsets: Vec<Subset>,
}

#[derive(Debug)]
pub(crate) struct Model {
    /// Every item in ascending order; an item's index is its position here.
    pub ids: Vec<String>,
    /// Judgments naming each item.
    pub support: Vec<usize>,
    pub stages: Vec<Stage>,
    /// Connected component of each item (judgment graph, numbered by first item).
    pub component: Vec<usize>,
    /// Items of each component, ascending.
    pub members: Vec<Vec<usize>>,
    /// Position of each item within its component.
    pub local: Vec<usize>,
}

impl Model {
    /// Builds the model from validated judgments already in canonical order.
    pub(crate) fn build(ids: Vec<String>, judgments: &[Judgment]) -> Result<Self, RankError> {
        let index: BTreeMap<&str, usize> = ids
            .iter()
            .enumerate()
            .map(|(position, id)| (id.as_str(), position))
            .collect();
        let mut support = vec![0usize; ids.len()];
        let mut stages = Vec::new();
        let mut parent: Vec<usize> = (0..ids.len()).collect();
        for judgment in judgments {
            let groups: Vec<Vec<usize>> = judgment
                .order
                .iter()
                .map(|group| group.iter().map(|item| index[item.as_str()]).collect())
                .collect();
            let mut lower: Vec<usize> = Vec::new();
            let mut built = Vec::with_capacity(groups.len());
            for group in groups.iter().rev() {
                let mut members = group.clone();
                members.sort_unstable();
                if members.len() > MAX_TIE_GROUP {
                    return Err(RankError::TieGroupTooLarge {
                        judge: judgment.judge.clone(),
                        size: members.len(),
                    });
                }
                built.push(Stage {
                    subsets: subsets(&members),
                    group: members,
                    lower: lower.clone(),
                });
                lower.extend(group.iter().copied());
            }
            built.reverse();
            let first = groups[0][0];
            for group in &groups {
                for &item in group {
                    support[item] += 1;
                    union(&mut parent, first, item);
                }
            }
            stages.extend(built);
        }
        let mut component_of_root: BTreeMap<usize, usize> = BTreeMap::new();
        let mut component = vec![0usize; ids.len()];
        let mut members: Vec<Vec<usize>> = Vec::new();
        for (item, slot) in component.iter_mut().enumerate() {
            let root = find(&mut parent, item);
            let next = members.len();
            let id = *component_of_root.entry(root).or_insert(next);
            if id == members.len() {
                members.push(Vec::new());
            }
            *slot = id;
            members[id].push(item);
        }
        let mut local = vec![0usize; ids.len()];
        for group in &members {
            for (position, &item) in group.iter().enumerate() {
                local[item] = position;
            }
        }
        Ok(Self {
            ids,
            support,
            stages,
            component,
            members,
            local,
        })
    }
}

/// Every proper subset of the group that may already be placed, with its
/// uniform-tie-break weight.
fn subsets(group: &[usize]) -> Vec<Subset> {
    let size = group.len();
    let full = (1usize << size) - 1;
    (0..full)
        .map(|mask: usize| {
            let placed = mask.count_ones() as usize;
            Subset {
                weight: 1.0 / binomial(size, placed),
                remaining: (0..size)
                    .filter(|position| mask & (1 << position) == 0)
                    .map(|position| group[position])
                    .collect(),
            }
        })
        .collect()
}

fn binomial(n: usize, k: usize) -> f64 {
    let mut value = 1.0f64;
    for step in 0..k {
        value = value * (n - step) as f64 / (step + 1) as f64;
    }
    value
}

fn find(parent: &mut [usize], mut item: usize) -> usize {
    while parent[item] != item {
        parent[item] = parent[parent[item]];
        item = parent[item];
    }
    item
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let (a, b) = (find(parent, left), find(parent, right));
    if a != b {
        let (low, high) = if a < b { (a, b) } else { (b, a) };
        parent[high] = low;
    }
}

/// Result of the minorization–maximization iteration.
pub(crate) struct Maximum {
    pub gamma: Vec<f64>,
    pub iterations: u32,
    /// Largest change of any score in the last iteration.
    pub change: f64,
    pub converged: bool,
}

/// Iterates the MM update from `start` until every score moves by at most
/// `tolerance`, or `max_iterations` iterations have run.
pub(crate) fn maximize(
    model: &Model,
    start: &[f64],
    tolerance: f64,
    max_iterations: u32,
) -> Maximum {
    let mut gamma = start.to_vec();
    let mut change = f64::INFINITY;
    let mut iterations = 0;
    let mut denominators = vec![0.0f64; gamma.len()];
    while iterations < max_iterations {
        iterations += 1;
        for (item, value) in denominators.iter_mut().enumerate() {
            *value = 2.0 / (gamma[item] + 1.0);
        }
        for stage in &model.stages {
            let lower_sum: f64 = stage.lower.iter().map(|&item| gamma[item]).sum();
            let mut lower_coefficient = 0.0;
            for subset in &stage.subsets {
                let total = lower_sum
                    + subset
                        .remaining
                        .iter()
                        .map(|&item| gamma[item])
                        .sum::<f64>();
                let share = subset.weight / total;
                for &item in &subset.remaining {
                    denominators[item] += share;
                }
                lower_coefficient += share;
            }
            for &item in &stage.lower {
                denominators[item] += lower_coefficient;
            }
        }
        change = 0.0;
        for item in 0..gamma.len() {
            let next = (1.0 + model.support[item] as f64) / denominators[item];
            change = change.max((next.ln() - gamma[item].ln()).abs());
            gamma[item] = next;
        }
        if change <= tolerance {
            return Maximum {
                gamma,
                iterations,
                change,
                converged: true,
            };
        }
    }
    Maximum {
        gamma,
        iterations,
        change,
        converged: false,
    }
}

/// Laplace variance of each score: the diagonal of the inverse of the observed
/// information, computed per component (the information is block diagonal
/// across components because the reference is fixed).
pub(crate) fn variances(model: &Model, gamma: &[f64]) -> Result<Vec<f64>, RankError> {
    let mut matrices: Vec<Vec<f64>> = model
        .members
        .iter()
        .map(|group| vec![0.0f64; group.len() * group.len()])
        .collect();
    for (item, &value) in gamma.iter().enumerate() {
        let component = model.component[item];
        let size = model.members[component].len();
        let probability = value / (value + 1.0);
        matrices[component][model.local[item] * size + model.local[item]] +=
            2.0 * probability * (1.0 - probability);
    }
    for stage in &model.stages {
        let component = model.component[stage.group[0]];
        let size = model.members[component].len();
        let lower_sum: f64 = stage.lower.iter().map(|&item| gamma[item]).sum();
        let matrix = &mut matrices[component];
        for subset in &stage.subsets {
            let total = lower_sum
                + subset
                    .remaining
                    .iter()
                    .map(|&item| gamma[item])
                    .sum::<f64>();
            let members: Vec<usize> = subset
                .remaining
                .iter()
                .chain(stage.lower.iter())
                .copied()
                .collect();
            for (position, &row) in members.iter().enumerate() {
                let row_probability = gamma[row] / total;
                let row_local = model.local[row];
                matrix[row_local * size + row_local] +=
                    subset.weight * (row_probability - row_probability * row_probability);
                for &column in &members[position + 1..] {
                    let column_probability = gamma[column] / total;
                    let column_local = model.local[column];
                    let cross = subset.weight * row_probability * column_probability;
                    matrix[row_local * size + column_local] -= cross;
                    matrix[column_local * size + row_local] -= cross;
                }
            }
        }
    }
    let mut variances = vec![0.0f64; gamma.len()];
    for (component, matrix) in matrices.iter().enumerate() {
        let size = model.members[component].len();
        let diagonal =
            crate::linalg::inverse_diagonal(matrix, size).ok_or(RankError::Numerical {
                reason: "information matrix is not positive definite",
            })?;
        for (position, &item) in model.members[component].iter().enumerate() {
            variances[item] = diagonal[position];
        }
    }
    Ok(variances)
}
