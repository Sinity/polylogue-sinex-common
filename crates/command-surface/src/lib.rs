//! One typed operation table drives the command line, socket, HTTP and MCP,
//! and one answer contract travels on all of them.
//!
//! Products own operations, authentication, durable transactions and rendered
//! views. The table owns dispatch, schemas and the MCP grouping; every
//! transport calls it. The answer envelope, failures, value states, section
//! status, the budget fitter, caller identity and seen marks are the shared
//! vocabulary both products answer in.

mod arguments;
mod authorship;
mod envelope;
mod event;
mod failure;
pub mod laws;
mod operation;
mod pack;
pub mod seen;
mod status;
mod tools;
mod transport;
mod values;

pub use arguments::ParsedCommand;
pub use authorship::*;
pub use envelope::*;
pub use event::*;
pub use failure::*;
pub use operation::*;
pub use pack::*;
pub use query_language::coverage::CoverageState;
pub use schemars::JsonSchema;
pub use seen::*;
pub use status::*;
pub use tokio_util::sync::CancellationToken;
pub use transport::*;
pub use values::*;
