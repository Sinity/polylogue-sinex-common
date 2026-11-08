//! Shared signal rules, causal evaluation, the route function and the host
//! policy, desktop notification state, `follow`, `wait` and `backtest`.
//! Product adapters own query execution, occurrence keys, persistence and
//! their outlets; `signal-delivery` owns platform I/O.
mod calendar;
pub mod evaluator;
mod follow;
mod host;
pub mod notifications;
mod outcome;
mod route;
mod schema;
pub use calendar::DayCalendar;
pub use evaluator::*;
pub use follow::*;
pub use host::*;
pub use outcome::*;
pub use route::*;
pub use schema::*;
