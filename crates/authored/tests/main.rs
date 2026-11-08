//! Integration tests for this crate, built as one binary.

mod currency;
#[cfg(feature = "generator")]
mod generated;
mod order;
mod request;
mod uniqueness;
