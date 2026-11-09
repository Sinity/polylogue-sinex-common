//! One typed set of operation declarations drives the command line, socket, HTTP
//! and MCP,
//! and one answer contract travels on all of them.
//!
//! Products own operations, authentication, durable transactions and rendered
//! views. The declarations hold the operations, schemas, rendering and MCP
//! grouping. A runtime table binds real handlers to them; every
//! transport calls the same bound handlers. The answer envelope, failures, value states, section
//! status, the budget fitter, caller identity and seen marks are the shared
//! vocabulary both products answer in.

#[cfg(feature = "runtime")]
mod arguments;
mod authorship;
#[cfg(feature = "runtime")]
pub mod checks;
mod envelope;
mod event;
mod failure;
#[cfg(feature = "runtime")]
mod operation;
mod pack;
pub mod seen;
#[cfg(feature = "runtime")]
mod status;
#[cfg(feature = "runtime")]
mod tools;
#[cfg(feature = "runtime")]
mod transport;
mod values;

#[cfg(feature = "runtime")]
pub use arguments::{CliArgument, ParsedCommand, ValueKind};
pub use authorship::*;
pub use envelope::*;
pub use event::*;
pub use failure::*;
#[cfg(feature = "runtime")]
pub use operation::*;
pub use pack::*;
pub use query_language::coverage::CoverageState;
pub use schemars::JsonSchema;
pub use seen::*;
#[cfg(feature = "runtime")]
pub use status::*;
#[cfg(feature = "runtime")]
pub use tokio_util::sync::CancellationToken;
#[cfg(feature = "runtime")]
pub use tools::RenderedAnswer;
#[cfg(feature = "runtime")]
pub use transport::*;
pub use values::*;
