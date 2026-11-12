//! Authored-record rules shared by Polylogue and Sinex: the record header,
//! currency by the descendant rule, stale-base checks, atomic requests
//! numbered `<request>.<n>`, uniqueness slots, and order keys. Every function
//! is pure over links the product supplies; storage stays in each product.
pub mod currency;
mod history;
pub mod order;
mod record;
pub mod request;
pub mod uniqueness;

pub use currency::{Currency, CurrencyError, Links, RevisionState, RootState, affected_roots};
pub use history::History;
pub use order::{OrderError, OrderKey, SortKey};
pub use record::{Header, IdentityError, Revision, Target};
pub use request::{
    Arrival, OnStale, Placed, Plan, Planned, Record, Refusal, Refused, Replay, RequestDigest,
    Write, replay, write_id,
};
pub use uniqueness::{Holders, Part, SlotValue, Unique, UniqueSlot, Uniqueness, WhenHeld};
