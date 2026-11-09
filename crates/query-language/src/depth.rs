//! A query nests as deeply as its text does: `NOT NOT … x`, `(a OR (b OR …))`
//! and `1 + 1 + …` each add a level for a few bytes. Code that recurses over
//! the tree runs its next level through [`deeper`], which moves onto a fresh
//! stack segment when the current one runs low, so nesting is bounded by
//! memory rather than by the thread's stack size.

/// Stack that must remain before a level runs on the current segment.
const RED_ZONE: usize = 256 * 1024;
/// Size of each new segment.
const SEGMENT: usize = 4 * 1024 * 1024;

/// Runs one level of a recursion over a query, growing the stack first if
/// fewer than [`RED_ZONE`] bytes remain.
#[inline]
pub fn deeper<R>(level: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(RED_ZONE, SEGMENT, level)
}
