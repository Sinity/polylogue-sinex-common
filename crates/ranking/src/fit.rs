//! The fit a product reads: scores, Laplace intervals, rank ranges, support,
//! components and fit status, for one judge set.

use crate::judgment::{JudgeSet, Judgment, RankError, validate};
use crate::model::{Model, maximize, variances};
use std::collections::{BTreeMap, BTreeSet};

/// The method and its prior, stated in every fit.
pub const METHOD: &str = "plackett-luce, minorization-maximization (Hunter 2004), \
    Laplace intervals from observed information; weak prior of one virtual win \
    and one virtual loss against a reference of strength 1; tie groups by uniform \
    random tie-break";

/// z for a 95 percent Gaussian interval.
const Z_95: f64 = 1.959_963_984_540_054;

#[derive(Clone, Debug, PartialEq)]
pub struct FitOptions {
    pub judges: JudgeSet,
    /// The largest change in any score at which the iteration stops.
    pub tolerance: f64,
    /// The iteration allowance. An iteration that does not converge within it
    /// returns `FitStatus::Stopped`, never a completed-looking result.
    pub max_iterations: u32,
}

impl Default for FitOptions {
    fn default() -> Self {
        Self {
            judges: JudgeSet::All,
            tolerance: 1e-10,
            max_iterations: 10_000,
        }
    }
}

/// A 1-based rank with the range of positions the interval allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RankRange {
    /// The best position consistent with the intervals.
    pub best: usize,
    /// The worst position consistent with the intervals.
    pub worst: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FitItem {
    pub item: String,
    /// Log-worth relative to the prior's reference (strength 1). Comparable
    /// within a component; components share only the prior.
    pub score: f64,
    pub variance: f64,
    /// 95 percent Laplace interval on the score.
    pub interval: (f64, f64),
    /// Position in the order by score (ties by item).
    pub rank: usize,
    pub rank_range: RankRange,
    /// Judgments in the judge set that name this item.
    pub support: usize,
    /// Connected group of items linked by judgments, numbered by first item.
    pub component: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FitStatus {
    Converged { iterations: u32 },
    Stopped { iterations: u32, change: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    /// Strongest first; equal scores ordered by item.
    pub items: Vec<FitItem>,
    pub status: FitStatus,
    /// The judge set the fit counted.
    pub judges: JudgeSet,
    /// Judgments in the judge set that the fit used, as submitted.
    pub judgment_count: usize,
    pub method: &'static str,
}

impl Fit {
    pub fn item(&self, item: &str) -> Option<&FitItem> {
        self.items.iter().find(|candidate| candidate.item == item)
    }
}

/// Fits the judgments counted by `options.judges` over `items` and every item
/// they name. Input order does not matter: the result is the same for any
/// arrangement of the same judgments and items.
pub fn fit(
    items: &[String],
    judgments: &[Judgment],
    options: &FitOptions,
) -> Result<Fit, RankError> {
    run(items, judgments, options, None)
}

/// Fits as `fit` does, starting the iteration from a previous fit's scores.
/// Items the previous fit did not contain start at strength 1. The optimum is
/// unique, so the warm start changes only the iteration count and the last
/// digits, within the tolerance.
pub fn fit_warm(
    start: &Fit,
    items: &[String],
    judgments: &[Judgment],
    options: &FitOptions,
) -> Result<Fit, RankError> {
    run(items, judgments, options, Some(start))
}

fn run(
    items: &[String],
    judgments: &[Judgment],
    options: &FitOptions,
    start: Option<&Fit>,
) -> Result<Fit, RankError> {
    validate(judgments)?;
    let mut counted: Vec<Judgment> = judgments
        .iter()
        .filter(|judgment| options.judges.counts(&judgment.judge))
        .map(|judgment| {
            let mut sorted = judgment.clone();
            for group in &mut sorted.order {
                group.sort();
            }
            sorted
        })
        .collect();
    counted.sort();
    let mut ids: BTreeSet<String> = items.iter().cloned().collect();
    for judgment in &counted {
        ids.extend(judgment.items().map(str::to_owned));
    }
    let ids: Vec<String> = ids.into_iter().collect();
    let model = Model::build(ids, &counted)?;
    let previous: BTreeMap<&str, f64> = start
        .map(|fit| {
            fit.items
                .iter()
                .map(|item| (item.item.as_str(), item.score.exp()))
                .collect()
        })
        .unwrap_or_default();
    let initial: Vec<f64> = model
        .ids
        .iter()
        .map(|id| previous.get(id.as_str()).copied().unwrap_or(1.0))
        .collect();
    let maximum = maximize(&model, &initial, options.tolerance, options.max_iterations);
    let status = if maximum.converged {
        FitStatus::Converged {
            iterations: maximum.iterations,
        }
    } else {
        FitStatus::Stopped {
            iterations: maximum.iterations,
            change: maximum.change,
        }
    };
    let variance = variances(&model, &maximum.gamma)?;
    let mut rows: Vec<FitItem> = model
        .ids
        .iter()
        .enumerate()
        .map(|(item, id)| {
            let score = maximum.gamma[item].ln();
            let half_width = Z_95 * variance[item].sqrt();
            FitItem {
                item: id.clone(),
                score,
                variance: variance[item],
                interval: (score - half_width, score + half_width),
                rank: 0,
                rank_range: RankRange { best: 0, worst: 0 },
                support: model.support[item],
                component: model.component[item],
            }
        })
        .collect();
    rows.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.item.cmp(&right.item))
    });
    let count = rows.len();
    let mut lows: Vec<f64> = rows.iter().map(|row| row.interval.0).collect();
    let mut highs: Vec<f64> = rows.iter().map(|row| row.interval.1).collect();
    lows.sort_by(f64::total_cmp);
    highs.sort_by(f64::total_cmp);
    for (position, row) in rows.iter_mut().enumerate() {
        row.rank = position + 1;
        // Items whose whole interval lies above this one's top, and below its bottom.
        let above = lows.len() - lows.partition_point(|low| *low <= row.interval.1);
        let below = highs.partition_point(|high| *high < row.interval.0);
        row.rank_range = RankRange {
            best: 1 + above,
            worst: count - below,
        };
    }
    Ok(Fit {
        items: rows,
        status,
        judges: options.judges.clone(),
        judgment_count: counted.len(),
        method: METHOD,
    })
}
