//! A serde serializer that writes canonical JSON.
//!
//! Scalars are written by serde_json itself, so strings, integers and floats
//! are spelled exactly as serde_json spells them in the same build. Objects
//! buffer their members, sort them by key and then write them.

use serde::ser::{self, Impossible, Serialize};
use std::fmt;

/// serde_json serializes a number held as text (`arbitrary_precision`) as a
/// struct with this name and one field of the same name holding the text.
const NUMBER_TOKEN: &str = "$serde_json::private::Number";
/// serde_json serializes a `RawValue` as a struct with this name and one
/// field holding the raw JSON text.
const RAW_VALUE_TOKEN: &str = "$serde_json::private::RawValue";

/// The value cannot be written as JSON: an object key that is not a string,
/// a number, a boolean or a unit variant, or an error raised by the value's
/// own `Serialize` implementation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "value has no canonical JSON form: {}", self.0)
    }
}

impl std::error::Error for Error {}

impl ser::Error for Error {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self(error.to_string())
    }
}

pub(crate) fn write<T: Serialize + ?Sized>(out: &mut Vec<u8>, value: &T) -> Result<(), Error> {
    value.serialize(Writer { out })
}

/// Writes one scalar exactly as serde_json does.
fn scalar<T: Serialize + ?Sized>(out: &mut Vec<u8>, value: &T) -> Result<(), Error> {
    serde_json::to_writer(out, value).map_err(Error::from)
}

struct Writer<'a> {
    out: &'a mut Vec<u8>,
}

impl<'a> ser::Serializer for Writer<'a> {
    type Ok = ();
    type Error = Error;
    type SerializeSeq = Sequence<'a>;
    type SerializeTuple = Sequence<'a>;
    type SerializeTupleStruct = Sequence<'a>;
    type SerializeTupleVariant = Sequence<'a>;
    type SerializeMap = Object<'a>;
    type SerializeStruct = Object<'a>;
    type SerializeStructVariant = Object<'a>;

    fn serialize_bool(self, v: bool) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_i8(self, v: i8) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_i16(self, v: i16) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_i32(self, v: i32) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_i64(self, v: i64) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_i128(self, v: i128) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_u8(self, v: u8) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_u16(self, v: u16) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_u32(self, v: u32) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_u64(self, v: u64) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_u128(self, v: u128) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_f32(self, v: f32) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_f64(self, v: f64) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_char(self, v: char) -> Result<(), Error> {
        scalar(self.out, &v)
    }
    fn serialize_str(self, v: &str) -> Result<(), Error> {
        scalar(self.out, v)
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<(), Error> {
        scalar(self.out, v)
    }
    fn serialize_none(self) -> Result<(), Error> {
        self.out.extend_from_slice(b"null");
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), Error> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<(), Error> {
        self.serialize_none()
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), Error> {
        self.serialize_none()
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<(), Error> {
        scalar(self.out, variant)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.out.push(b'{');
        scalar(self.out, variant)?;
        self.out.push(b':');
        value.serialize(Writer { out: self.out })?;
        self.out.push(b'}');
        Ok(())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Sequence<'a>, Error> {
        self.out.push(b'[');
        Ok(Sequence {
            out: self.out,
            first: true,
            variant: false,
        })
    }
    fn serialize_tuple(self, len: usize) -> Result<Sequence<'a>, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Sequence<'a>, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Sequence<'a>, Error> {
        self.out.push(b'{');
        scalar(self.out, variant)?;
        self.out.extend_from_slice(b":[");
        Ok(Sequence {
            out: self.out,
            first: true,
            variant: true,
        })
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Object<'a>, Error> {
        Ok(Object::new(self.out, Shape::Members))
    }
    fn serialize_struct(self, name: &'static str, _len: usize) -> Result<Object<'a>, Error> {
        let shape = match name {
            NUMBER_TOKEN => Shape::Number,
            RAW_VALUE_TOKEN => Shape::Raw,
            _ => Shape::Members,
        };
        Ok(Object::new(self.out, shape))
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Object<'a>, Error> {
        self.out.push(b'{');
        scalar(self.out, variant)?;
        self.out.push(b':');
        Ok(Object::new(self.out, Shape::VariantMembers))
    }
}

pub(crate) struct Sequence<'a> {
    out: &'a mut Vec<u8>,
    first: bool,
    variant: bool,
}

impl Sequence<'_> {
    fn element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        if !self.first {
            self.out.push(b',');
        }
        self.first = false;
        value.serialize(Writer { out: self.out })
    }
    fn finish(self) -> Result<(), Error> {
        self.out.push(b']');
        if self.variant {
            self.out.push(b'}');
        }
        Ok(())
    }
}

impl ser::SerializeSeq for Sequence<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTuple for Sequence<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTupleStruct for Sequence<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeTupleVariant for Sequence<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        self.element(value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// An object: a map or a struct.
    Members,
    /// The object inside `{"variant": {...}}`, which closes one more brace.
    VariantMembers,
    /// serde_json's number held as text: written bare.
    Number,
    /// serde_json's raw JSON text: parsed and written canonically.
    Raw,
}

pub(crate) struct Object<'a> {
    out: &'a mut Vec<u8>,
    shape: Shape,
    /// Each member's key, as text, and its encoded value.
    members: Vec<(String, Vec<u8>)>,
    pending_key: Option<String>,
}

impl<'a> Object<'a> {
    fn new(out: &'a mut Vec<u8>, shape: Shape) -> Self {
        Self {
            out,
            shape,
            members: Vec::new(),
            pending_key: None,
        }
    }

    fn member<T: Serialize + ?Sized>(&mut self, key: String, value: &T) -> Result<(), Error> {
        match self.shape {
            Shape::Number => {
                // The number's text is a string field; write it unquoted.
                let text = value.serialize(KeyWriter)?;
                self.out.extend_from_slice(text.as_bytes());
                Ok(())
            }
            Shape::Raw => {
                let text = value.serialize(KeyWriter)?;
                let parsed: serde_json::Value = serde_json::from_str(&text)?;
                write(self.out, &parsed)
            }
            Shape::Members | Shape::VariantMembers => {
                let mut encoded = Vec::new();
                write(&mut encoded, value)?;
                self.members.push((key, encoded));
                Ok(())
            }
        }
    }

    fn finish(mut self) -> Result<(), Error> {
        if matches!(self.shape, Shape::Number | Shape::Raw) {
            return Ok(());
        }
        self.members
            .sort_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
        self.out.push(b'{');
        for (index, (key, value)) in self.members.iter().enumerate() {
            if index > 0 {
                self.out.push(b',');
            }
            scalar(self.out, key.as_str())?;
            self.out.push(b':');
            self.out.extend_from_slice(value);
        }
        self.out.push(b'}');
        if self.shape == Shape::VariantMembers {
            self.out.push(b'}');
        }
        Ok(())
    }
}

impl ser::SerializeMap for Object<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Error> {
        self.pending_key = Some(key.serialize(KeyWriter)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), Error> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| Error("map value written before its key".into()))?;
        self.member(key, value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeStruct for Object<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.member(key.to_owned(), value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

impl ser::SerializeStructVariant for Object<'_> {
    type Ok = ();
    type Error = Error;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Error> {
        self.member(key.to_owned(), value)
    }
    fn end(self) -> Result<(), Error> {
        self.finish()
    }
}

/// Turns an object key into its text, accepting what serde_json accepts as a
/// key: strings, characters, integers, finite floats, booleans and unit
/// variants.
struct KeyWriter;

fn key_error() -> Error {
    Error("an object key must be a string, a number, a boolean or a unit variant".into())
}

impl ser::Serializer for KeyWriter {
    type Ok = String;
    type Error = Error;
    type SerializeSeq = Impossible<String, Error>;
    type SerializeTuple = Impossible<String, Error>;
    type SerializeTupleStruct = Impossible<String, Error>;
    type SerializeTupleVariant = Impossible<String, Error>;
    type SerializeMap = Impossible<String, Error>;
    type SerializeStruct = Impossible<String, Error>;
    type SerializeStructVariant = Impossible<String, Error>;

    fn serialize_bool(self, v: bool) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i8(self, v: i8) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i16(self, v: i16) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i32(self, v: i32) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i64(self, v: i64) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_i128(self, v: i128) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u8(self, v: u8) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u16(self, v: u16) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u32(self, v: u32) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u64(self, v: u64) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_u128(self, v: u128) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_f32(self, v: f32) -> Result<String, Error> {
        self.serialize_f64(f64::from(v))
    }
    fn serialize_f64(self, v: f64) -> Result<String, Error> {
        if !v.is_finite() {
            return Err(key_error());
        }
        Ok(serde_json::to_string(&v)?)
    }
    fn serialize_char(self, v: char) -> Result<String, Error> {
        Ok(v.to_string())
    }
    fn serialize_str(self, v: &str) -> Result<String, Error> {
        Ok(v.to_owned())
    }
    fn serialize_bytes(self, _v: &[u8]) -> Result<String, Error> {
        Err(key_error())
    }
    fn serialize_none(self) -> Result<String, Error> {
        Err(key_error())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, _value: &T) -> Result<String, Error> {
        Err(key_error())
    }
    fn serialize_unit(self) -> Result<String, Error> {
        Err(key_error())
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<String, Error> {
        Err(key_error())
    }
    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _index: u32,
        variant: &'static str,
    ) -> Result<String, Error> {
        Ok(variant.to_owned())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<String, Error> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _value: &T,
    ) -> Result<String, Error> {
        Err(key_error())
    }
    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Error> {
        Err(key_error())
    }
    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Error> {
        Err(key_error())
    }
    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Error> {
        Err(key_error())
    }
    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Error> {
        Err(key_error())
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Error> {
        Err(key_error())
    }
    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Error> {
        Err(key_error())
    }
    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Error> {
        Err(key_error())
    }
}
