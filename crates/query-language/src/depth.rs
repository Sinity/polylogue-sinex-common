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

/// The identity normal form is a JSON tree as deep as the query, and
/// serde_json walks a `Value` recursively when it copies, compares,
/// serializes or drops it. These do the same one level at a time through
/// [`deeper`]; callers holding a deep normal form use them instead.
pub mod json {
    use super::deeper;
    use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
    use serde_json::Value;

    pub fn clone(value: &Value) -> Value {
        deeper(|| match value {
            Value::Array(values) => Value::Array(values.iter().map(clone).collect()),
            Value::Object(fields) => Value::Object(
                fields
                    .iter()
                    .map(|(key, value)| (key.clone(), clone(value)))
                    .collect(),
            ),
            scalar => scalar.clone(),
        })
    }

    pub fn equal(left: &Value, right: &Value) -> bool {
        deeper(|| match (left, right) {
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equal(a, b))
            }
            (Value::Object(a), Value::Object(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, a)| b.get(key).is_some_and(|b| equal(a, b)))
            }
            (Value::Array(_) | Value::Object(_), _) | (_, Value::Array(_) | Value::Object(_)) => {
                false
            }
            (a, b) => a == b,
        })
    }

    pub fn drop(value: Value) {
        deeper(move || match value {
            Value::Array(values) => values.into_iter().for_each(drop),
            Value::Object(fields) => fields.into_iter().for_each(|(_, value)| drop(value)),
            _ => {}
        })
    }

    /// Serializes like the `Value` it borrows, one level at a time.
    pub struct Deep<'a>(pub &'a Value);

    impl Serialize for Deep<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            deeper(|| match self.0 {
                Value::Array(values) => {
                    let mut out = serializer.serialize_seq(Some(values.len()))?;
                    for value in values {
                        out.serialize_element(&Deep(value))?;
                    }
                    out.end()
                }
                Value::Object(fields) => {
                    let mut out = serializer.serialize_map(Some(fields.len()))?;
                    for (key, value) in fields {
                        out.serialize_entry(key, &Deep(value))?;
                    }
                    out.end()
                }
                scalar => scalar.serialize(serializer),
            })
        }
    }
}
