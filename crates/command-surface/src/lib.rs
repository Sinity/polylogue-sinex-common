//! One typed operation table drives the command line, socket, HTTP and MCP.
//!
//! Products own operations, authentication, durable transactions and rendered
//! views. The table owns dispatch and schemas; every transport calls it.

mod envelope;
mod event;
mod failure;
mod operation;
mod status;
mod transport;

pub use envelope::*;
pub use event::*;
pub use failure::*;
pub use operation::*;
pub use schemars::JsonSchema;
pub use status::*;
pub use tokio_util::sync::CancellationToken;
pub use transport::*;
