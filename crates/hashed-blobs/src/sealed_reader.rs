//! Verified sequential sealed reads with an explicit authenticated-frame bound.
use super::{
    DataKey, Error, Generation, Hash, Result, SealedFooter, Store, VerificationStats, View,
    SEAL_TRAILER,
};
use std::io::{self, Read};
use std::sync::Arc;

/// Evaluation budgets checked before allocating a frame or its index. These are
/// caller choices, not validity restrictions on the encoded sealed format.
#[derive(Clone, Copy, Debug)]
pub struct SealedReadLimits {
    pub frame_bytes: usize,
    pub index_bytes: usize,
}

/// Holds one authenticated frame, never the whole material. The initial proof checks
/// ciphertext, frame authentication, lengths and the complete keyed name before output.
/// A failure fuses the reader. Callers retain authority over key lifetime through `check`.
pub struct SealedReader {
    view: Arc<View>,
    generation: Generation,
    hash: Hash,
    footer: SealedFooter,
    key: DataKey,
    frame: usize,
    offset: u64,
    plain_start: u64,
    bytes: Vec<u8>,
    position: usize,
    check: Box<dyn FnMut() -> Result<()> + Send>,
    stats: VerificationStats,
    failed: bool,
}
impl SealedReader {
    pub(super) fn open(
        store: &Store,
        hash: Hash,
        expected: Option<u64>,
        key: &DataKey,
        limits: SealedReadLimits,
        mut check: Box<dyn FnMut() -> Result<()> + Send>,
    ) -> Result<Self> {
        check()?;
        if limits.frame_bytes == 0 || limits.index_bytes == 0 {
            return Err(Error::Refused(
                "sealed read budgets must be positive".into(),
            ));
        }
        let view = store.view(&hash)?;
        let generation = Generation::of(&view.file)?;
        let tail = view.tail(8)?;
        if tail.len() != 8 || &tail[4..] != SEAL_TRAILER {
            return Err(Error::Refused(
                "sequential sealed reader needs a sealed blob".into(),
            ));
        }
        // The footer is an index, not content; cap malformed length fields before reading it.
        if u32::from_le_bytes(tail[..4].try_into().expect("four bytes")) as usize
            > limits.index_bytes
        {
            return Err(Error::Refused(
                "sealed frame index exceeds the caller's index budget".into(),
            ));
        }
        let (footer, start) = Store::sealed_footer(&view, hash)?;
        if footer.base.is_some() {
            return Err(Error::NotSeekable(hash));
        }
        if footer.key_ref != key.key_ref {
            return Err(Error::Unwrap("sealed key reference differs".into()));
        }
        if expected.is_some_and(|length| length != footer.plain_len) {
            return Err(Error::Corrupt {
                blob: hash,
                what: "sealed length differs from expected length".into(),
            });
        }
        let compressed_limit =
            zstd::zstd_safe::compress_bound(limits.frame_bytes.min(u32::MAX as usize))
                .checked_add(16)
                .ok_or_else(|| Error::Refused("sealed frame limit overflows".into()))?;
        let mut ciphertext_len = 0u64;
        let mut plain_len = 0u64;
        for &(compressed, plain) in &footer.frames {
            if plain as u64 > limits.frame_bytes as u64
                || compressed as u64 > compressed_limit as u64
            {
                return Err(Error::Refused(
                    "sealed frame exceeds the caller's frame limit".into(),
                ));
            }
            ciphertext_len = ciphertext_len
                .checked_add(u64::from(compressed))
                .ok_or_else(|| Error::Refused("sealed extent overflow".into()))?;
            plain_len = plain_len
                .checked_add(u64::from(plain))
                .ok_or_else(|| Error::Refused("sealed length overflow".into()))?;
        }
        if ciphertext_len != start || plain_len != footer.plain_len {
            return Err(Error::Corrupt {
                blob: hash,
                what: "sealed frame lengths differ from body or total".into(),
            });
        }
        let mut proof = blake3::Hasher::new();
        let mut at = 0;
        while at < start {
            check()?;
            let n = (start - at).min(65536);
            proof.update(&view.at(at, n)?);
            at += n;
        }
        proof.update(&footer.body());
        if Hash(*proof.finalize().as_bytes()) != footer.cipher_hash {
            return Err(Error::Corrupt {
                blob: hash,
                what: "sealed ciphertext checksum differs".into(),
            });
        }
        let mut reader = Self {
            view,
            generation,
            hash,
            footer,
            key: key.clone(),
            frame: 0,
            offset: 0,
            plain_start: 0,
            bytes: Vec::new(),
            position: 0,
            check,
            stats: VerificationStats::default(),
            failed: false,
        };
        // Ciphertext hash verification read the body once; authenticated name proof follows.
        reader.stats.compressed_bytes = start;
        reader.stats.metadata_bytes = 16 + reader.view.len - start;
        let mut naming = blake3::Hasher::new_keyed(&Store::name_key(key));
        while reader.frame < reader.footer.frames.len() {
            reader.next_frame()?;
            naming.update(&reader.bytes);
            reader.bytes.clear();
        }
        if Hash(*naming.finalize().as_bytes()) != hash {
            return Err(Error::Corrupt {
                blob: hash,
                what: "sealed content differs from keyed name".into(),
            });
        }
        reader.unchanged()?;
        (reader.check)()?;
        reader.frame = 0;
        reader.offset = 0;
        reader.plain_start = 0;
        reader.position = 0;
        Ok(reader)
    }
    fn unchanged(&self) -> Result<()> {
        if Generation::of(&self.view.file)? != self.generation {
            return Err(Error::Refused(
                "sealed blob changed while its verified reader was open".into(),
            ));
        }
        Ok(())
    }
    fn next_frame(&mut self) -> Result<()> {
        (self.check)()?;
        self.unchanged()?;
        let (compressed, expected) = self.footer.frames[self.frame];
        self.bytes = Store::open_sealed_frame(
            &self.view,
            self.hash,
            &self.footer,
            &self.key.key,
            self.frame,
            self.offset,
            self.plain_start,
            None,
        )?;
        self.unchanged()?;
        (self.check)()?;
        if self.bytes.len() as u64 != u64::from(expected) {
            return Err(Error::Corrupt {
                blob: self.hash,
                what: "decoded sealed frame differs from declared length".into(),
            });
        }
        self.offset += u64::from(compressed);
        self.plain_start += u64::from(expected);
        self.frame += 1;
        self.position = 0;
        self.stats.frames += 1;
        self.stats.compressed_bytes += u64::from(compressed);
        self.stats.decoded_bytes += self.bytes.len() as u64;
        Ok(())
    }
    /// Actual frame decode and ciphertext read work, including the initial proof.
    pub fn stats(&self) -> VerificationStats {
        self.stats
    }
    fn next(&mut self, out: &mut [u8]) -> Result<usize> {
        (self.check)()?;
        self.unchanged()?;
        while self.position == self.bytes.len() {
            if self.frame == self.footer.frames.len() {
                return Ok(0);
            }
            self.next_frame()?;
        }
        let n = out.len().min(self.bytes.len() - self.position);
        out[..n].copy_from_slice(&self.bytes[self.position..self.position + n]);
        self.position += n;
        Ok(n)
    }
}
impl Read for SealedReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.failed {
            return Err(io::Error::other(
                "sealed reader stopped after a failed read",
            ));
        }
        match self.next(out) {
            Ok(n) => Ok(n),
            Err(error) => {
                self.failed = true;
                self.bytes.clear();
                Err(io::Error::other(error))
            }
        }
    }
}

#[cfg(test)]
mod tests;
