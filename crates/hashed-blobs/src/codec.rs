//! The extent's frame index and encoding context.
use crate::{DataKey, Error, Hash, Kind, Result};
use std::path::{Component, Path};
pub(crate) const SKIP: u32 = 0x184D_2A51;
pub(crate) const TAIL: &[u8; 8] = b"HBFRAME3";
#[derive(Clone)]
pub(crate) struct Frame {
    pub stored: u64,
    pub decoded: u64,
    pub digest: Hash,
}
#[derive(Clone)]
pub(crate) struct Index {
    pub hash: Hash,
    pub kind: Kind,
    pub salt: [u8; 32],
    pub frames: Vec<Frame>,
}
impl Index {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = b"HBINDEX3".to_vec();
        out.extend(self.hash.0);
        put_kind(&mut out, &self.kind);
        out.extend(self.salt);
        out.extend((self.frames.len() as u64).to_le_bytes());
        for f in &self.frames {
            out.extend(f.stored.to_le_bytes());
            out.extend(f.decoded.to_le_bytes());
            out.extend(f.digest.0);
        }
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes);
        if c.take(8)? != b"HBINDEX3" {
            return Err(Error::Refused("invalid frame index".into()));
        }
        let hash = c.hash()?;
        let kind = c.kind()?;
        let salt = c.take(32)?.try_into().unwrap();
        let count = c.u64()?;
        if count > c.remaining() as u64 / 48 {
            return Err(Error::Refused("truncated frame index".into()));
        }
        let mut frames = Vec::new();
        for _ in 0..count {
            frames.push(Frame {
                stored: c.u64()?,
                decoded: c.u64()?,
                digest: c.hash()?,
            });
        }
        c.finish()?;
        Ok(Self {
            hash,
            kind,
            salt,
            frames,
        })
    }
    /// Lengths and positions are known before encryption. Digests are recorded
    /// afterwards and bound by the trusted extent's index hash.
    pub fn aad(&self, name: Hash, number: usize) -> Vec<u8> {
        let mut out = b"hashed-blobs frame v3".to_vec();
        out.extend(name.0);
        put_kind(&mut out, &self.kind);
        out.extend(self.salt);
        out.extend((number as u64).to_le_bytes());
        out.extend((self.frames.len() as u64).to_le_bytes());
        for f in &self.frames {
            out.extend(f.stored.to_le_bytes());
            out.extend(f.decoded.to_le_bytes());
        }
        out
    }
}
pub(crate) fn name_key(key: &DataKey) -> [u8; 32] {
    derive(&key.key, None, b"hashed-blobs name v1")
}
pub(crate) fn frame_key(key: &DataKey, name: Hash, salt: &[u8; 32]) -> [u8; 32] {
    let mut info = b"hashed-blobs frame v3".to_vec();
    info.extend(name.0);
    derive(&key.key, Some(salt), &info)
}
fn derive(key: &[u8; 32], salt: Option<&[u8]>, info: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(salt, key)
        .expand(info, &mut out)
        .expect("valid HKDF length");
    out
}
pub(crate) fn nonce(index: usize) -> [u8; 12] {
    let mut n = [0; 12];
    n[4..].copy_from_slice(&(index as u64).to_be_bytes());
    n
}
pub(crate) fn segment_name(name: &str) -> Result<()> {
    if name.is_empty()
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::Refused(
            "segment must be a relative normal path".into(),
        ));
    }
    Ok(())
}
pub(crate) fn put_bytes(out: &mut Vec<u8>, value: &[u8]) {
    out.extend((value.len() as u64).to_le_bytes());
    out.extend(value);
}
pub(crate) fn put_kind(out: &mut Vec<u8>, kind: &Kind) {
    match kind {
        Kind::Plain => out.push(0),
        Kind::Sealed { key_ref } => {
            out.push(1);
            put_bytes(out, key_ref.as_bytes());
        }
    }
}
pub(crate) struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    pub fn remaining(&self) -> usize {
        self.bytes.len() - self.at
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&e| e <= self.bytes.len())
            .ok_or_else(|| Error::Refused("truncated encoding".into()))?;
        let b = &self.bytes[self.at..end];
        self.at = end;
        Ok(b)
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn hash(&mut self) -> Result<Hash> {
        Ok(Hash(self.take(32)?.try_into().unwrap()))
    }
    pub fn string(&mut self) -> Result<String> {
        let n = usize::try_from(self.u64()?)
            .map_err(|_| Error::Refused("string length overflows".into()))?;
        String::from_utf8(self.take(n)?.to_vec())
            .map_err(|_| Error::Refused("invalid UTF-8 identifier".into()))
    }
    pub fn kind(&mut self) -> Result<Kind> {
        match self.take(1)?[0] {
            0 => Ok(Kind::Plain),
            1 => Ok(Kind::Sealed {
                key_ref: self.string()?,
            }),
            _ => Err(Error::Refused("invalid encoding kind".into())),
        }
    }
    pub fn finish(&self) -> Result<()> {
        if self.remaining() != 0 {
            return Err(Error::Refused("trailing encoding bytes".into()));
        }
        Ok(())
    }
}
