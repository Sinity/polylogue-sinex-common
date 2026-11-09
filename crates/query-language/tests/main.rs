//! Integration tests for this crate, built as one binary.

mod support;

mod completion;
#[cfg(feature = "conformance")]
mod conditional;
mod corpora;
mod cursors;
mod deep;
mod format;
#[cfg(feature = "conformance")]
mod glob;
mod grammar;
mod identity;
mod native_membership;
mod numbers;
mod one_spelling;
#[cfg(feature = "conformance")]
mod order_match;
mod parser_residuals;
mod projected_sets;
#[cfg(feature = "conformance")]
mod ranking;
mod reference_spacing;
mod schema;
mod schemas;
mod scopes;
#[cfg(feature = "conformance")]
mod semantics;
mod source_scopes;
#[cfg(feature = "conformance")]
mod text;
mod time;
mod time_bounds;
mod truth;
mod wire_schema;
