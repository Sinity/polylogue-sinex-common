//! Plackett–Luce ranking from judgments: a fit with Laplace intervals, judge
//! sets, components, warm starts, and next-set selection under three rules.
//!
//! A judgment orders k targets, best first, with ties as groups (see
//! [`Judgment`]). The fit is the maximum of a weak-prior Plackett–Luce
//! likelihood, reached by Hunter's minorization–maximization. Its documentation
//! and the derivation are in `model.rs`; the crate README states the design.

mod fit;
mod judgment;
mod linalg;
mod model;
mod select;

pub use fit::{Fit, FitItem, FitOptions, FitStatus, METHOD, RankRange, fit, fit_warm};
pub use judgment::{JudgeSet, Judgment, RankError};
pub use select::{Proposal, Seat, SeatWhy, SelectionRule, next_set};
