//! Next-set selection: which items a judge should compare next.
//!
//! Every rule seats the items that no judgment in the fit's judge set names
//! yet first, in the rule's own order, so an uncompared item appears in the
//! first proposal and every proposal takes `min(k, uncompared)` of them. The
//! rule fills the remaining seats:
//!
//! - `Information`: the pair with the largest expected reduction in the
//!   variance of its score difference, then greedily the item whose summed
//!   reduction against the seats so far is largest. The reduction for a pair
//!   with difference variance `v` and fitted win probability `p` is
//!   `v² q / (1 + v q)` with `q = p (1 − p)`, the drop in variance from one
//!   more Plackett–Luce comparison of the pair. Covariances are ignored: the
//!   variance of each difference is the sum of the two items' variances. This
//!   is the active-sampling criterion of Chen et al. (2013), approximated.
//! - `Random { seed }`: a seeded shuffle (SplitMix64, Fisher–Yates), so the
//!   same seed gives the same proposal.
//! - `UncomparedFirst`: the nearest scores to an anchor, the first uncompared
//!   item or, when none is uncompared, the least-supported item.

use crate::fit::{Fit, FitItem};
use crate::judgment::RankError;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionRule {
    Information,
    Random { seed: u64 },
    UncomparedFirst,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SeatWhy {
    /// No judgment in the judge set names this item yet.
    Uncompared,
    /// Chosen for its expected reduction, summed over the seats chosen before
    /// it (the first seat of a pair carries the pair's reduction).
    Information {
        reduction: f64,
    },
    Random,
    /// Anchors the set when nothing is uncompared: the least-supported item.
    Anchor,
    /// Among the nearest scores to the anchor.
    NearestScore {
        anchor: String,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Seat {
    pub item: String,
    pub why: SeatWhy,
}

/// A proposed set, seats in selection order.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub rule: SelectionRule,
    pub seats: Vec<Seat>,
}

/// Proposes `size` items from `candidates`, which must all be in `fit`.
pub fn next_set(
    fit: &Fit,
    candidates: &[String],
    size: usize,
    rule: SelectionRule,
) -> Result<Proposal, RankError> {
    if size < 2 {
        return Err(RankError::SetBelowTwo { size });
    }
    let by_id: BTreeMap<&str, &FitItem> = fit
        .items
        .iter()
        .map(|item| (item.item.as_str(), item))
        .collect();
    let distinct: BTreeSet<&str> = candidates.iter().map(String::as_str).collect();
    let mut pool: Vec<&FitItem> = Vec::with_capacity(distinct.len());
    for id in distinct {
        pool.push(
            by_id
                .get(id)
                .copied()
                .ok_or_else(|| RankError::UnknownItem {
                    item: id.to_owned(),
                })?,
        );
    }
    if pool.len() < size {
        return Err(RankError::TooFewCandidates {
            wanted: size,
            available: pool.len(),
        });
    }
    let seats = match rule {
        SelectionRule::Information => information(&pool, size),
        SelectionRule::Random { seed } => random(&pool, size, seed),
        SelectionRule::UncomparedFirst => uncompared_first(&pool, size),
    };
    Ok(Proposal { rule, seats })
}

fn uncompared<'a>(pool: &[&'a FitItem]) -> Vec<&'a FitItem> {
    pool.iter()
        .copied()
        .filter(|item| item.support == 0)
        .collect()
}

fn information(pool: &[&FitItem], size: usize) -> Vec<Seat> {
    let mut chosen: Vec<(usize, SeatWhy)> = Vec::with_capacity(size);
    let mut taken = vec![false; pool.len()];
    for (index, item) in pool.iter().enumerate() {
        if chosen.len() == size {
            break;
        }
        if item.support == 0 {
            taken[index] = true;
            chosen.push((index, SeatWhy::Uncompared));
        }
    }
    if chosen.is_empty() {
        let mut best: Option<(usize, usize, f64)> = None;
        for left in 0..pool.len() {
            for right in left + 1..pool.len() {
                let reduction = reduction(pool[left], pool[right]);
                if best.is_none_or(|(_, _, value)| reduction > value) {
                    best = Some((left, right, reduction));
                }
            }
        }
        if let Some((left, right, value)) = best {
            taken[left] = true;
            taken[right] = true;
            chosen.push((left, SeatWhy::Information { reduction: value }));
            chosen.push((right, SeatWhy::Information { reduction: value }));
        }
    }
    let mut accumulated = vec![0.0f64; pool.len()];
    for (index, item) in pool.iter().enumerate() {
        accumulated[index] = chosen
            .iter()
            .map(|(seat, _)| reduction(item, pool[*seat]))
            .sum();
    }
    while chosen.len() < size {
        let mut best: Option<usize> = None;
        for index in 0..pool.len() {
            if !taken[index] && best.is_none_or(|current| accumulated[index] > accumulated[current])
            {
                best = Some(index);
            }
        }
        let Some(next) = best else { break };
        taken[next] = true;
        chosen.push((
            next,
            SeatWhy::Information {
                reduction: accumulated[next],
            },
        ));
        for index in 0..pool.len() {
            accumulated[index] += reduction(pool[index], pool[next]);
        }
    }
    chosen
        .into_iter()
        .map(|(index, why)| Seat {
            item: pool[index].item.clone(),
            why,
        })
        .collect()
}

fn random(pool: &[&FitItem], size: usize, seed: u64) -> Vec<Seat> {
    let mut rng = SplitMix64(seed);
    let mut order: Vec<&FitItem> = pool.to_vec();
    for index in (1..order.len()).rev() {
        order.swap(index, rng.below(index + 1));
    }
    let mut seats: Vec<Seat> = Vec::with_capacity(size);
    for item in order.iter().filter(|item| item.support == 0).take(size) {
        seats.push(Seat {
            item: item.item.clone(),
            why: SeatWhy::Uncompared,
        });
    }
    for item in order.iter().filter(|item| item.support > 0) {
        if seats.len() == size {
            break;
        }
        seats.push(Seat {
            item: item.item.clone(),
            why: SeatWhy::Random,
        });
    }
    seats
}

fn uncompared_first(pool: &[&FitItem], size: usize) -> Vec<Seat> {
    let unseen = uncompared(pool);
    let mut seats: Vec<Seat> = unseen
        .iter()
        .take(size)
        .map(|item| Seat {
            item: item.item.clone(),
            why: SeatWhy::Uncompared,
        })
        .collect();
    if seats.len() == size {
        return seats;
    }
    let anchor: &FitItem = match unseen.first() {
        Some(item) => item,
        None => {
            let least = pool
                .iter()
                .copied()
                .min_by(|left, right| {
                    left.support
                        .cmp(&right.support)
                        .then_with(|| left.item.cmp(&right.item))
                })
                .expect("pool holds at least two items");
            seats.push(Seat {
                item: least.item.clone(),
                why: SeatWhy::Anchor,
            });
            least
        }
    };
    let mut neighbours: Vec<&FitItem> = pool
        .iter()
        .copied()
        .filter(|item| !seats.iter().any(|seat| seat.item == item.item))
        .collect();
    neighbours.sort_by(|left, right| {
        let distance = |item: &FitItem| (item.score - anchor.score).abs();
        distance(left)
            .total_cmp(&distance(right))
            .then_with(|| left.item.cmp(&right.item))
    });
    for item in neighbours.into_iter().take(size - seats.len()) {
        seats.push(Seat {
            item: item.item.clone(),
            why: SeatWhy::NearestScore {
                anchor: anchor.item.clone(),
            },
        });
    }
    seats
}

/// Expected reduction in the variance of the score difference of two items
/// from one more comparison, at the fitted win probability.
fn reduction(left: &FitItem, right: &FitItem) -> f64 {
    let variance = left.variance + right.variance;
    let probability = 1.0 / (1.0 + (right.score - left.score).exp());
    let information = probability * (1.0 - probability);
    variance * variance * information / (1.0 + variance * information)
}

/// SplitMix64: a small, fixed generator, so a seed names one proposal.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }

    /// A value in `0..bound`, by multiplicative mapping of the high bits.
    fn below(&mut self, bound: usize) -> usize {
        ((u128::from(self.next_u64()) * bound as u128) >> 64) as usize
    }
}
