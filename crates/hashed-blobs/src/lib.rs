//! Append-only, product-committed segments of checked plain or sealed frames.
//!
//! Preparation owns no store. Products append and sync a group, commit its
//! extents with their own rows, then acknowledge it. Opening truncates the tail
//! to the product's committed end and finishes its recorded punches.
mod codec;
mod pax;
pub use pax::{PaxArchive, PaxMember, PaxSparseExtent, PaxTime};
mod preparation;
mod reader;
mod writer;
pub use preparation::{Preparation, Prepared};
pub use reader::{BlobReader, Mismatch, ReadLimits, Reader, VerificationStats};
pub use writer::{GroupMetrics, PendingGroup, SegmentWriter, SyncedGroup};

use std::{fmt, io};
pub const DEFAULT_FRAME: usize = 4 << 20;

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Hash(pub [u8; 32]);
impl Hash {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }
    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}
impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "blake3:{}", self.hex())
    }
}
impl fmt::Debug for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}
#[derive(Clone)]
pub struct DataKey {
    pub key_ref: String,
    pub key: [u8; 32],
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Sealed { key_ref: String },
}

/// An immutable byte range. Products persist the complete descriptor with the
/// rows that cite it. Hashes in this descriptor are part of that trusted index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Extent {
    pub segment: String,
    pub offset: u64,
    pub len: u64,
    pub decoded_len: u64,
    pub hash: Hash,
    pub kind: Kind,
    pub index_hash: Hash,
    pub encoded_hash: Hash,
}
impl Extent {
    pub fn end(&self) -> u64 {
        self.offset.saturating_add(self.len)
    }
    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"HBEXT3\0".to_vec();
        codec::put_bytes(&mut out, self.segment.as_bytes());
        for v in [self.offset, self.len, self.decoded_len] {
            out.extend(v.to_le_bytes());
        }
        out.extend(self.hash.0);
        codec::put_kind(&mut out, &self.kind);
        out.extend(self.index_hash.0);
        out.extend(self.encoded_hash.0);
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = codec::Cursor::new(bytes);
        if c.take(7)? != b"HBEXT3\0" {
            return Err(Error::Refused("invalid extent descriptor".into()));
        }
        let value = Self {
            segment: c.string()?,
            offset: c.u64()?,
            len: c.u64()?,
            decoded_len: c.u64()?,
            hash: c.hash()?,
            kind: c.kind()?,
            index_hash: c.hash()?,
            encoded_hash: c.hash()?,
        };
        c.finish()?;
        codec::segment_name(&value.segment)?;
        value
            .offset
            .checked_add(value.len)
            .ok_or_else(|| Error::Refused("extent overflows".into()))?;
        Ok(value)
    }
}
#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    Corrupt {
        segment: String,
        offset: u64,
        len: u64,
        what: String,
    },
    Locked(Hash),
    Refused(String),
    Unwrap(String),
}
impl Error {
    pub(crate) fn corrupt(extent: &Extent, what: impl Into<String>) -> Self {
        Self::Corrupt {
            segment: extent.segment.clone(),
            offset: extent.offset,
            len: extent.len,
            what: what.into(),
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Corrupt {
                segment,
                offset,
                len,
                what,
            } => write!(f, "segment {segment} extent {offset}+{len}: {what}"),
            Self::Locked(hash) => write!(f, "blob {hash} requires a data key"),
            Self::Refused(s) => write!(f, "refused: {s}"),
            Self::Unwrap(s) => write!(f, "key unavailable: {s}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}
pub type Result<T> = std::result::Result<T, Error>;
