//! The impls that walk [`Query`], [`Condition`] and [`Expression`] one level
//! at a time through [`deeper`]. Each delegates to the field types' own
//! impls, so a level costs one stack check; the output matches what
//! `#[derive]` would produce.
//!
//! Dropping is not among them: a `Drop` impl would forbid moving fields out
//! of these types, which callers do throughout. Dropping a level costs a few
//! dozen bytes of stack, and parsing keeps the cheapest nesting shallow: a
//! run of negations collapses (see `Parser::negation`) and a set inside a set
//! adds its values.
use super::*;
use crate::depth::deeper;
use serde::ser::SerializeStruct;
use std::fmt;

impl Clone for Query {
    fn clone(&self) -> Self {
        deeper(|| Query {
            source: self.source.clone(),
            attach: self.attach.clone(),
            transforms: self.transforms.clone(),
        })
    }
}

impl PartialEq for Query {
    fn eq(&self, other: &Self) -> bool {
        deeper(|| {
            self.source == other.source
                && self.attach == other.attach
                && self.transforms == other.transforms
        })
    }
}

impl fmt::Debug for Query {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        deeper(|| {
            f.debug_struct("Query")
                .field("source", &self.source)
                .field("attach", &self.attach)
                .field("transforms", &self.transforms)
                .finish()
        })
    }
}

impl Serialize for Query {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        deeper(|| {
            let mut out = serializer.serialize_struct("Query", 3)?;
            out.serialize_field("source", &self.source)?;
            out.serialize_field("attach", &self.attach)?;
            out.serialize_field("transforms", &self.transforms)?;
            out.end()
        })
    }
}

impl Clone for Condition {
    fn clone(&self) -> Self {
        deeper(|| Condition {
            span: self.span,
            kind: self.kind.clone(),
        })
    }
}

impl PartialEq for Condition {
    fn eq(&self, other: &Self) -> bool {
        deeper(|| self.span == other.span && self.kind == other.kind)
    }
}

impl fmt::Debug for Condition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        deeper(|| {
            f.debug_struct("Condition")
                .field("span", &self.span)
                .field("kind", &self.kind)
                .finish()
        })
    }
}

impl Serialize for Condition {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        deeper(|| {
            let mut out = serializer.serialize_struct("Condition", 2)?;
            out.serialize_field("span", &self.span)?;
            out.serialize_field("kind", &self.kind)?;
            out.end()
        })
    }
}

/// Borrowed spelling of [`Expression`] whose derived impls print and
/// serialize exactly as derives on `Expression` itself would.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Spelled<'a> {
    Field {
        name: &'a String,
    },
    Value {
        value: &'a Value,
    },
    Function {
        name: &'a String,
        arguments: &'a Vec<Expression>,
    },
    Arithmetic {
        operator: &'a Arithmetic,
        left: &'a Expression,
        right: &'a Expression,
    },
}

impl Expression {
    fn spelled(&self) -> Spelled<'_> {
        match self {
            Self::Field { name } => Spelled::Field { name },
            Self::Value { value } => Spelled::Value { value },
            Self::Function { name, arguments } => Spelled::Function { name, arguments },
            Self::Arithmetic {
                operator,
                left,
                right,
            } => Spelled::Arithmetic {
                operator,
                left,
                right,
            },
        }
    }
}

impl Clone for Expression {
    fn clone(&self) -> Self {
        deeper(|| match self {
            Self::Field { name } => Self::Field { name: name.clone() },
            Self::Value { value } => Self::Value {
                value: value.clone(),
            },
            Self::Function { name, arguments } => Self::Function {
                name: name.clone(),
                arguments: arguments.clone(),
            },
            Self::Arithmetic {
                operator,
                left,
                right,
            } => Self::Arithmetic {
                operator: *operator,
                left: left.clone(),
                right: right.clone(),
            },
        })
    }
}

impl PartialEq for Expression {
    fn eq(&self, other: &Self) -> bool {
        deeper(|| match (self, other) {
            (Self::Field { name: a }, Self::Field { name: b }) => a == b,
            (Self::Value { value: a }, Self::Value { value: b }) => a == b,
            (
                Self::Function {
                    name: a,
                    arguments: x,
                },
                Self::Function {
                    name: b,
                    arguments: y,
                },
            ) => a == b && x == y,
            (
                Self::Arithmetic {
                    operator: a,
                    left: l1,
                    right: r1,
                },
                Self::Arithmetic {
                    operator: b,
                    left: l2,
                    right: r2,
                },
            ) => a == b && l1 == l2 && r1 == r2,
            _ => false,
        })
    }
}

impl Eq for Expression {}

impl fmt::Debug for Expression {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        deeper(|| fmt::Debug::fmt(&self.spelled(), f))
    }
}

impl Serialize for Expression {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        deeper(|| self.spelled().serialize(serializer))
    }
}
