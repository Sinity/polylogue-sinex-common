//! Authenticated keyset continuations, with transport page size outside identity.
use crate::{
    Error, ErrorKind,
    identity::QuestionId,
    time::{Clock, ResolvedTime},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, num::NonZeroUsize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PageRequest {
    pub page_size: NonZeroUsize,
    pub cursor: Option<SealedCursor>,
}

/// Context fixed on the first page. Products bind grants, schema/definition/ranking
/// versions and every read position here, and check their continued availability.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CursorContext {
    pub question: QuestionId,
    pub clock: Clock,
    pub resolved_times: Vec<ResolvedTime>,
    pub random_seed: u64,
    pub read_positions: BTreeMap<String, String>,
    pub versions: BTreeMap<String, String>,
}

/// The last row's explicit sort values, followed by its unique row identity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderPosition {
    pub values: Vec<Value>,
    pub row_identity: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Continuation {
    pub context: CursorContext,
    pub after: OrderPosition,
}

/// Wire representation is opaque; consumers open it through the owning seal.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SealedCursor(String);
impl SealedCursor {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn from_wire(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

/// Caller-provided secret. No randomness, environment or I/O is read by this crate.
/// Authentication seals integrity; the payload is not encrypted.
pub struct CursorSeal {
    key: [u8; 32],
}
impl std::fmt::Debug for CursorSeal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CursorSeal { key: [redacted] }")
    }
}
impl CursorSeal {
    pub fn new(key: [u8; 32]) -> Self {
        Self { key }
    }
    pub fn seal(&self, continuation: &Continuation) -> Result<SealedCursor, Error> {
        let payload = serde_json::to_vec(continuation)
            .map_err(|e| invalid(format!("cursor cannot be encoded: {e}")))?;
        let mut bytes = Vec::with_capacity(payload.len() + 1);
        bytes.push(1);
        bytes.extend_from_slice(&payload);
        let signature = self.signature(&bytes);
        bytes.extend_from_slice(signature.as_bytes());
        Ok(SealedCursor(hex(&bytes)))
    }
    fn signature(&self, payload: &[u8]) -> blake3::Hash {
        let mut digest = blake3::Hasher::new_keyed(&self.key);
        digest.update(b"query-language.cursor\0");
        digest.update(payload);
        digest.finalize()
    }
    /// Authenticate before decoding. The product's callback checks retained read
    /// positions and compatible versions against its own authoritative state.
    pub fn open(
        &self,
        cursor: &SealedCursor,
        question: QuestionId,
        available: impl FnOnce(&CursorContext) -> bool,
    ) -> Result<Continuation, Error> {
        let bytes =
            unhex(cursor.as_str()).ok_or_else(|| invalid("the cursor encoding is invalid"))?;
        if bytes.len() < 33 {
            return Err(invalid("the cursor is incomplete"));
        }
        let (payload, supplied) = bytes.split_at(bytes.len() - 32);
        let expected = self.signature(payload);
        let supplied: [u8; 32] = supplied.try_into().expect("split signature length");
        if expected != blake3::Hash::from(supplied) {
            return Err(invalid("the cursor seal is invalid"));
        }
        if payload[0] != 1 {
            return Err(invalid("the cursor version is unsupported"));
        }
        let continuation: Continuation = serde_json::from_slice(&payload[1..])
            .map_err(|_| invalid("the cursor payload is invalid"))?;
        if continuation.context.question != question {
            return Err(invalid("the cursor belongs to a different question"));
        }
        if !available(&continuation.context) {
            return Err(Error::new(
                ErrorKind::StaleCursor,
                "the cursor read position or query context is no longer available",
                0,
            )
            .hint("restart the question without its cursor"));
        }
        Ok(continuation)
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidQuery, message, 0)
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}
fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect()
}
fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Unknown values sort last in either direction; row identity is always the
/// final ascending key. Products use this ordering when compiling keyset SQL.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Ascending,
    Descending,
}

pub fn compare_positions(
    left: &OrderPosition,
    right: &OrderPosition,
    directions: &[Direction],
) -> Result<std::cmp::Ordering, Error> {
    use std::cmp::Ordering;
    if left.values.len() != directions.len() || right.values.len() != directions.len() {
        return Err(invalid(
            "order position does not match the question's sort keys",
        ));
    }
    for ((left, right), direction) in left.values.iter().zip(&right.values).zip(directions) {
        let order = match (left, right) {
            (Value::Null, Value::Null) => Ordering::Equal,
            (Value::Null, _) => Ordering::Greater,
            (_, Value::Null) => Ordering::Less,
            _ => {
                let order = match (left, right) {
                    (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
                    (Value::String(a), Value::String(b)) => a.cmp(b),
                    (Value::Number(a), Value::Number(b)) => {
                        crate::number::compare(&a.to_string(), &b.to_string())
                            .ok_or_else(|| invalid("order key is not a representable decimal"))?
                    }
                    _ => return Err(invalid("order keys require compatible scalar types")),
                };
                if *direction == Direction::Descending {
                    order.reverse()
                } else {
                    order
                }
            }
        };
        if order != Ordering::Equal {
            return Ok(order);
        }
    }
    Ok(left.row_identity.cmp(&right.row_identity))
}
