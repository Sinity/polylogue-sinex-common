//! Uniqueness keys: which records occupy "the same slot", so that a second
//! write revises the first instead of adding another record.
//!
//! A kind declares its key as an ordered list of component names, or none
//! ("many"). The product reads each component's value from a write; this
//! module turns the values into slots. One function serves every
//! cardinality: one title per session, one tag per name and value, one
//! annotation per rubric field and author, one setting per machine, one
//! rating per scale and author, one decision per unordered pair.
use serde::{Deserialize, Serialize};

/// A kind's declared uniqueness key. The kind is always part of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Uniqueness {
    pub components: &'static [&'static str],
    pub when_held: WhenHeld,
}

/// What a write into a slot already held by a current record does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhenHeld {
    /// The write becomes a revision of the record that holds the slot.
    Revise,
    /// The write is answered "already" and writes nothing: the slot's key
    /// says everything the record says (a tag with its name and value).
    Already,
}

/// One component's value in a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// The component has no value; that is itself one slot value.
    Absent,
    One(String),
    /// Several values, one slot each: a record about two targets occupies a
    /// slot for each target. Without the split, two records could give one
    /// target two current values without either being a revision.
    Each(Vec<String>),
    /// Several values taken together as one unordered value (a pair).
    Set(Vec<String>),
}

/// One component's value inside a slot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotValue {
    Absent,
    One(String),
    Set(Vec<String>),
}

/// One slot: the kind and each declared component's value, in declaration
/// order.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UniqueSlot {
    pub kind: String,
    pub parts: Vec<(String, SlotValue)>,
}

impl UniqueSlot {
    /// A stable 32-byte name for the slot, for a product's slot index. The
    /// encoding is length-prefixed, so no two slots share bytes.
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        let mut text = |text: &str| {
            hasher.update(&(text.len() as u64).to_le_bytes());
            hasher.update(text.as_bytes());
        };
        text(&self.kind);
        for (name, value) in &self.parts {
            text(name);
            match value {
                SlotValue::Absent => text("absent"),
                SlotValue::One(value) => {
                    text("one");
                    text(value);
                }
                SlotValue::Set(values) => {
                    text("set");
                    text(&values.len().to_string());
                    for value in values {
                        text(value);
                    }
                }
            }
        }
        *hasher.finalize().as_bytes()
    }
}

/// The slots one write occupies, with what a held slot means for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unique {
    pub slots: Vec<UniqueSlot>,
    pub when_held: WhenHeld,
}

impl Uniqueness {
    pub const fn new(components: &'static [&'static str], when_held: WhenHeld) -> Self {
        Self {
            components,
            when_held,
        }
    }

    /// The slots a write of `kind` occupies. `part` gives each declared
    /// component's value in the write; it is asked once per component.
    pub fn unique(&self, kind: &str, mut part: impl FnMut(&str) -> Part) -> Unique {
        let mut slots = vec![Vec::with_capacity(self.components.len())];
        for name in self.components {
            let values = match part(name) {
                Part::Absent => vec![SlotValue::Absent],
                Part::One(value) => vec![SlotValue::One(value)],
                Part::Each(mut values) => {
                    values.sort();
                    values.dedup();
                    if values.is_empty() {
                        vec![SlotValue::Absent]
                    } else {
                        values.into_iter().map(SlotValue::One).collect()
                    }
                }
                Part::Set(mut values) => {
                    values.sort();
                    values.dedup();
                    vec![SlotValue::Set(values)]
                }
            };
            slots = slots
                .into_iter()
                .flat_map(|prefix: Vec<(String, SlotValue)>| {
                    values.iter().map(move |value| {
                        let mut parts = prefix.clone();
                        parts.push(((*name).to_owned(), value.clone()));
                        parts
                    })
                })
                .collect();
        }
        let mut slots: Vec<UniqueSlot> = slots
            .into_iter()
            .map(|parts| UniqueSlot {
                kind: kind.to_owned(),
                parts,
            })
            .collect();
        slots.sort();
        slots.dedup();
        Unique {
            slots,
            when_held: self.when_held,
        }
    }
}

/// The slot lookup a write needs, beside the currency links. A product
/// answers it from its own index (for example a column holding
/// [`UniqueSlot::digest`]).
pub trait Holders: crate::Links {
    /// Every revision whose content occupies this slot, current or not.
    fn holders(&mut self, slot: &UniqueSlot) -> Result<Vec<crate::Revision>, Self::Error>;
}
