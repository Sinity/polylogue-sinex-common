//! Development commands shared by Polylogue and Sinex.
//!
//! A project's xtask starts a [`Run`], adds its steps (cargo commands, a
//! nextest run, its own checks) and finishes it. The run prints a short
//! summary and, with the `history` feature, records its stages, test outcomes
//! and compiler diagnostics in one machine-wide SQLite store. The `status` and
//! `history` commands in [`cli`] read that store, so an agent starting a
//! session can see what failed last time without rerunning anything.
//!
//! History is data, never a gate: a store that cannot be opened is reported
//! and the run continues.

mod junit;
mod run;
pub mod select;

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "history")]
pub mod history;
#[cfg(feature = "pg")]
pub mod pg;

pub use devkit_macros::test;
pub use run::{Diagnostic, Failed, Nextest, Run, Stage, TestOutcome, TestStatus, Tree};

/// The result of one step of a run.
pub type Result<T = ()> = std::result::Result<T, Failed>;
