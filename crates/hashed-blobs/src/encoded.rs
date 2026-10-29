//! A prepared encoding can cross a custody boundary without acquiring a segment location.
use crate::{BlobReader, DataKey, Error, Extent, Hash, Kind, ReadLimits, Result, codec};
use std::fs::File;

/// Description of exactly one encoded blob. The codec version, frame index,
/// encoded bytes and decoded identity are bound independently of any path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedBlob {
    pub hash: Hash,
    pub decoded_len: u64,
    pub stored_len: u64,
    pub kind: Kind,
    pub index_hash: Hash,
    pub encoded_hash: Hash,
}
impl EncodedBlob {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = b"HBBLOB3\0".to_vec();
        bytes.extend(self.hash.0);
        bytes.extend(self.decoded_len.to_le_bytes());
        bytes.extend(self.stored_len.to_le_bytes());
        codec::put_kind(&mut bytes, &self.kind);
        bytes.extend(self.index_hash.0);
        bytes.extend(self.encoded_hash.0);
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut cursor = codec::Cursor::new(bytes);
        if cursor.take(8)? != b"HBBLOB3\0" {
            return Err(Error::Refused("invalid encoded blob descriptor".into()));
        }
        let value = Self {
            hash: cursor.hash()?,
            decoded_len: cursor.u64()?,
            stored_len: cursor.u64()?,
            kind: cursor.kind()?,
            index_hash: cursor.hash()?,
            encoded_hash: cursor.hash()?,
        };
        cursor.finish()?;
        Ok(value)
    }
    /// The caller keeps this encoded inode immutable for the reader's lifetime.
    /// This checks the index and each frame read. Adoption additionally verifies
    /// the complete encoded and decoded identities before authoritative storage.
    pub fn open(
        &self,
        file: File,
        key: Option<&DataKey>,
        limits: ReadLimits,
    ) -> Result<BlobReader> {
        crate::reader::open_encoded(file, self, key, limits)
    }
    pub(crate) fn extent(&self) -> Extent {
        Extent {
            segment: "prepared".into(),
            offset: 0,
            len: self.stored_len,
            decoded_len: self.decoded_len,
            hash: self.hash,
            kind: self.kind.clone(),
            index_hash: self.index_hash,
            encoded_hash: self.encoded_hash,
        }
    }
}
