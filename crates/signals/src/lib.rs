//! Shared causal signal evaluation and desktop delivery. Product adapters own query
//! execution, stable occurrence keys, persistence and the one host deliverer.
pub mod evaluator;
mod host;
pub mod notifications;
mod schema;
pub use evaluator::*;
pub use host::*;
pub use schema::*;
