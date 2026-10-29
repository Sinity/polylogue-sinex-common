//! A prepared encoding can cross a custody boundary without acquiring a segment location.
use crate::{codec, BlobReader, DataKey, Error, Extent, Hash, Kind, ReadLimits, Result};
use std::{fs::File, os::unix::fs::FileExt};

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
    /// Derive the descriptor of a locally owned self-describing encoding.
    /// This does not authenticate an external claim; adoption still verifies
    /// every decoded byte, and remote callers bind the result to their logical identity.
    pub(crate) fn inspect(file: &File, limits: ReadLimits) -> Result<Self> {
        let stored_len = file.metadata()?.len();
        let at = |offset: u64, length: usize| -> Result<Vec<u8>> {
            if offset
                .checked_add(length as u64)
                .is_none_or(|end| end > stored_len)
            {
                return Err(Error::Refused("encoded index lies outside file".into()));
            }
            let mut bytes = Vec::new();
            bytes
                .try_reserve_exact(length)
                .map_err(|_| Error::Refused("cannot allocate encoded index".into()))?;
            bytes.resize(length, 0);
            file.read_exact_at(&mut bytes, offset)?;
            Ok(bytes)
        };
        if stored_len < 24 {
            return Err(Error::Refused("truncated encoded file".into()));
        }
        let mut end = stored_len;
        let tail = at(stored_len - 9, 9)?;
        if tail[4] == 0 && u32::from_le_bytes(tail[5..].try_into().unwrap()) == 0x8F92_EAB1 {
            let count = u32::from_le_bytes(tail[..4].try_into().unwrap()) as u64;
            let bytes = count
                .checked_mul(8)
                .and_then(|length| length.checked_add(17))
                .ok_or_else(|| Error::Refused("seek table length overflows".into()))?;
            if bytes > limits.index_bytes as u64 {
                return Err(Error::Refused(
                    "seek table exceeds caller index budget".into(),
                ));
            }
            end = end
                .checked_sub(bytes)
                .ok_or_else(|| Error::Refused("seek table exceeds encoded file".into()))?;
        }
        let tail = at(
            end.checked_sub(16)
                .ok_or_else(|| Error::Refused("truncated frame index".into()))?,
            16,
        )?;
        if &tail[8..] != codec::TAIL {
            return Err(Error::Refused("invalid encoded frame index trailer".into()));
        }
        let length = u64::from_le_bytes(tail[..8].try_into().unwrap());
        if length > limits.index_bytes as u64 {
            return Err(Error::Refused(
                "frame index exceeds caller index budget".into(),
            ));
        }
        let index_start = end
            .checked_sub(length)
            .and_then(|offset| offset.checked_sub(16))
            .ok_or_else(|| Error::Refused("frame index exceeds encoded file".into()))?;
        let bytes = at(
            index_start,
            usize::try_from(length)
                .map_err(|_| Error::Refused("frame index exceeds address space".into()))?,
        )?;
        let index = codec::Index::decode(&bytes)?;
        let decoded_len = index.frames.iter().try_fold(0u64, |length, frame| {
            length
                .checked_add(frame.decoded)
                .ok_or_else(|| Error::Refused("decoded length overflows".into()))
        })?;
        let mut encoded_hash = blake3::Hasher::new();
        let mut buffer = [0; 65536];
        let mut offset = 0;
        while offset < stored_len {
            let n = (stored_len - offset).min(buffer.len() as u64) as usize;
            file.read_exact_at(&mut buffer[..n], offset)?;
            encoded_hash.update(&buffer[..n]);
            offset += n as u64;
        }
        Ok(Self {
            hash: index.hash,
            decoded_len,
            stored_len,
            kind: index.kind,
            index_hash: Hash::of(&bytes),
            encoded_hash: Hash(*encoded_hash.finalize().as_bytes()),
        })
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
