//! Shared causal signal evaluation and desktop delivery. Product adapters own query
//! execution, stable occurrence keys, persistence and the one host deliverer.
mod calendar;
pub mod evaluator;
mod host;
pub mod notifications;
mod schema;
pub use calendar::DayCalendar;
pub use evaluator::*;
pub use host::*;
pub use schema::*;
