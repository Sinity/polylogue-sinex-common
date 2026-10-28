//! Shared causal signal evaluation and desktop delivery. Product adapters own query
//! execution, stable occurrence keys, persistence and the one host deliverer.
pub mod evaluator;
pub mod notifications;
mod schema;
pub use evaluator::*;
pub use schema::*;
