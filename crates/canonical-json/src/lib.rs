//! The one JSON encoding that identity hashes read.
//!
//! An identity hash must give the same bytes for the same value in every
//! build of both products. serde_json cannot promise that by itself: its
//! `preserve_order` feature changes the order a `Map` keeps, its
//! `arbitrary_precision` feature changes how numbers are held, and Cargo
//! unifies features across everything linked into one binary. This encoding
//! fixes the bytes:
//!
//! - the domain tag comes first, then one zero byte, then the JSON, so two
//!   kinds of identity never share bytes;
//! - object members are sorted by key, compared as UTF-8 bytes, at every
//!   depth, for maps and for structs alike;
//! - numbers are written as the value holds them: a number parsed with
//!   `arbitrary_precision` keeps its spelling (`1.50` stays `1.50`), and
//!   integers and floats from typed fields are written as serde_json writes
//!   them;
//! - no whitespace; strings are escaped as serde_json escapes them.
//!
//! The encoder serializes the value directly, buffering only the members of
//! each object while it sorts them, so a typed value is never converted into a
//! `serde_json::Value` first. Encoding a value and encoding
//! `serde_json::to_value` of it give the same bytes.

mod serializer;

use serde::Serialize;

pub use serializer::Error;

/// The tag, a zero byte, then the value's canonical JSON.
///
/// The tag names what kind of identity the bytes are for, such as
/// `polylogue-message`. It must not contain a zero byte.
pub fn canonical_bytes<T: Serialize + ?Sized>(tag: &str, value: &T) -> Result<Vec<u8>, Error> {
    debug_assert!(!tag.contains('\0'), "an identity tag has no zero byte");
    let mut out = Vec::with_capacity(tag.len() + 64);
    out.extend_from_slice(tag.as_bytes());
    out.push(0);
    serializer::write(&mut out, value)?;
    Ok(out)
}

/// BLAKE3 of [`canonical_bytes`].
pub fn canonical_hash<T: Serialize + ?Sized>(tag: &str, value: &T) -> Result<blake3::Hash, Error> {
    Ok(blake3::hash(&canonical_bytes(tag, value)?))
}

/// The value's canonical JSON with no tag.
///
/// Only for an identity whose bytes are already cited by stored records and
/// were defined without a tag; every new identity uses [`canonical_bytes`].
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, Error> {
    let mut out = Vec::new();
    serializer::write(&mut out, value)?;
    Ok(out)
}
