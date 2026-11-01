//! Order keys for ordered lists of authored records: page slots, collection
//! entries, workspace panes.
//!
//! A key is a fraction in (0, 1) written as base-62 digits after the point,
//! without trailing zeros (`0-9`, `A-Z`, `a-z`, in that order, which is also
//! their byte order). Comparing keys as strings therefore compares the
//! fractions, and a key always exists strictly between any two keys, so
//! inserting never rewrites a neighbour and there is no precision limit.
//! Each new key is the midpoint plus a random suffix, so two writers
//! inserting into the same gap get distinct keys. Equal keys, which only
//! imports can produce, are ordered by record root ([`SortKey`]).
use serde::{Deserialize, Serialize};
use std::fmt;

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: u32 = 62;
/// Digits a `u32` suffix takes: 62^6 > 2^32.
const SUFFIX_DIGITS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OrderKey(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OrderError {
    #[error("an order key is empty")]
    Empty,
    #[error("order key `{0}` holds a character outside 0-9, A-Z, a-z")]
    Digit(String),
    #[error("order key `{0}` ends in 0; the shorter key without it is the same place")]
    TrailingZero(String),
    #[error("{before} is not before {after}")]
    NotBefore { before: OrderKey, after: OrderKey },
}

fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'Z' => Some(byte - b'A' + 10),
        b'a'..=b'z' => Some(byte - b'a' + 36),
        _ => None,
    }
}

impl OrderKey {
    pub fn parse(text: &str) -> Result<Self, OrderError> {
        if text.is_empty() {
            return Err(OrderError::Empty);
        }
        if text.bytes().any(|byte| digit(byte).is_none()) {
            return Err(OrderError::Digit(text.to_owned()));
        }
        if text.ends_with('0') {
            return Err(OrderError::TrailingZero(text.to_owned()));
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn digits(&self) -> Vec<u8> {
        self.0
            .bytes()
            .map(|byte| digit(byte).expect("checked"))
            .collect()
    }

    fn from_digits(digits: &[u8]) -> Self {
        Self(digits.iter().map(|d| DIGITS[*d as usize] as char).collect())
    }

    /// A new key strictly between `before` and `after` (either end open).
    /// `suffix` should be random, so concurrent writers in one gap differ.
    pub fn between(
        before: Option<&OrderKey>,
        after: Option<&OrderKey>,
        suffix: u32,
    ) -> Result<Self, OrderError> {
        if let (Some(before), Some(after)) = (before, after)
            && before >= after
        {
            return Err(OrderError::NotBefore {
                before: before.clone(),
                after: after.clone(),
            });
        }
        let low = before.map(OrderKey::digits).unwrap_or_default();
        let high = after.map(OrderKey::digits);
        let mut key = midpoint(&low, high.as_deref());
        // The midpoint is never a prefix of `after`, so any extension of it
        // stays below `after`, and any extension stays above `before`.
        let mut rest = suffix;
        let mut tail = [0u8; SUFFIX_DIGITS];
        for place in tail.iter_mut().rev() {
            *place = (rest % BASE) as u8;
            rest /= BASE;
        }
        let used = tail
            .iter()
            .rposition(|d| *d != 0)
            .map_or(0, |last| last + 1);
        if used == 0 {
            key.push(1);
        } else {
            key.extend_from_slice(&tail[..used]);
        }
        Ok(Self::from_digits(&key))
    }

    /// `count` keys in increasing order, evenly spread over the whole range,
    /// for building a new list at once (an import, or converting another
    /// ordering).
    pub fn evenly(count: usize) -> Vec<Self> {
        if count == 0 {
            return Vec::new();
        }
        let mut width = 1u32;
        while (BASE as u128).pow(width) <= count as u128 {
            width += 1;
        }
        let range = (BASE as u128).pow(width);
        let step = range / (count as u128 + 1);
        (1..=count as u128)
            .map(|index| {
                let mut value = index * step;
                let mut digits = vec![0u8; width as usize];
                for place in digits.iter_mut().rev() {
                    *place = (value % BASE as u128) as u8;
                    value /= BASE as u128;
                }
                while digits.last() == Some(&0) {
                    digits.pop();
                }
                Self::from_digits(&digits)
            })
            .collect()
    }
}

/// The midpoint of two digit strings `low < high` (`low` may be empty for 0;
/// no `high` means 1). The result is strictly between them, ends in a nonzero
/// digit and is never a prefix of `high`.
fn midpoint(low: &[u8], high: Option<&[u8]>) -> Vec<u8> {
    let mut key = Vec::new();
    // While `high` still bounds the result, the key so far equals its prefix.
    let mut bounded = high.is_some();
    let mut place = 0;
    loop {
        let low_digit = low.get(place).copied().unwrap_or(0);
        let high_digit = match high {
            Some(high) if bounded => high[place],
            _ => BASE as u8,
        };
        if low_digit == high_digit {
            key.push(low_digit);
        } else if high_digit - low_digit > 1 {
            key.push(((low_digit as u32 + high_digit as u32).div_ceil(2)) as u8);
            return key;
        } else {
            // Adjacent digits: take the lower one, after which `high` no
            // longer bounds the rest.
            key.push(low_digit);
            bounded = false;
        }
        place += 1;
    }
}

impl TryFrom<String> for OrderKey {
    type Error = OrderError;
    fn try_from(text: String) -> Result<Self, OrderError> {
        Self::parse(&text)
    }
}

impl From<OrderKey> for String {
    fn from(key: OrderKey) -> String {
        key.0
    }
}

impl fmt::Display for OrderKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An ordered record's place: its key, then its root for equal keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SortKey {
    pub key: OrderKey,
    pub root: String,
}
