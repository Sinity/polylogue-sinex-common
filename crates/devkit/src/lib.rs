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

mod cargo;
mod files;
mod junit;
#[cfg(any(feature = "nats", feature = "pg"))]
mod owner;
mod run;
pub mod select;

#[cfg(feature = "bench")]
pub mod bench;
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "history")]
pub mod history;
#[cfg(feature = "nats")]
pub mod nats;
#[cfg(feature = "pg")]
pub mod pg;

pub use cargo::cargo_command;
pub use devkit_macros::test;
pub use run::{Diagnostic, Failed, Nextest, Run, Stage, TestOutcome, TestStatus, Tree};

/// The result of one step of a run.
pub type Result<T = ()> = std::result::Result<T, Failed>;

/// A value [`test`] creates for each test parameter of this type, before the
/// test body runs. An error fails the test with the fixture's type name.
///
/// ```
/// struct Scratch(std::path::PathBuf);
///
/// impl devkit::Fixture for Scratch {
///     fn create() -> std::result::Result<Self, String> {
///         Ok(Scratch(std::env::temp_dir()))
///     }
/// }
///
/// #[devkit::test]
/// fn uses_scratch(scratch: Scratch) {
///     assert!(scratch.0.is_absolute());
/// }
/// ```
pub trait Fixture: Sized {
    fn create() -> std::result::Result<Self, String>;
}

#[cfg(feature = "cli")]
pub mod coverage;
#[cfg(feature = "cli")]
pub mod fuzz;
pub mod process;

#[cfg(feature = "cli")]
pub mod vm;
