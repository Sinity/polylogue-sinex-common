//! Content-defined chunking: FastCDC normalised cut points over a stream, and
//! the planner that stores a rewritten stream as chunk blobs, reusing every
//! chunk the product already holds.
use crate::{codec, DataKey, Error, Extent, GroupEntry, Hash, Kind, Preparation, Result};
use std::io::{Cursor, Read};

/// Chunk length bounds. Every chunk but a stream's last lies in `min..=max`;
/// cut points are about `target` apart. Changing them moves every boundary, so
/// a product keeps one setting for all revisions of an input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkSizes {
    min: usize,
    target: usize,
    max: usize,
}
impl Default for ChunkSizes {
    /// 16 KiB minimum, 64 KiB target, 256 KiB maximum.
    fn default() -> Self {
        Self {
            min: 16 << 10,
            target: 64 << 10,
            max: 256 << 10,
        }
    }
}
impl ChunkSizes {
    /// `target` is a power of two of at least 8; `0 < min <= target <= max`.
    pub fn new(min: usize, target: usize, max: usize) -> Result<Self> {
        if min == 0 || min > target || target > max || target < 8 || !target.is_power_of_two() {
            return Err(Error::Refused(
                "chunk sizes need 0 < min <= target <= max, target a power of two of at least 8"
                    .into(),
            ));
        }
        Ok(Self { min, target, max })
    }
    pub fn min(&self) -> usize {
        self.min
    }
    pub fn target(&self) -> usize {
        self.target
    }
    pub fn max(&self) -> usize {
        self.max
    }
}

/// Finds cut points with a gear rolling hash. Plain chunking uses one public
/// gear table; sealed chunking derives the table from the data key, so chunk
/// lengths reveal nothing about content to someone without the key.
#[derive(Clone)]
pub struct Chunker {
    gear: Box<[u64; 256]>,
    sizes: ChunkSizes,
    /// Mask before `target`: two more bits than the target's, so cuts are rarer.
    strict: u64,
    /// Mask after `target`: two fewer bits, so cuts come sooner.
    loose: u64,
}
impl Chunker {
    pub fn plain(sizes: ChunkSizes) -> Self {
        Self::with_seed(sizes, blake3::derive_key("hashed-blobs chunk gear v1", b""))
    }
    pub fn sealed(sizes: ChunkSizes, key: &DataKey) -> Self {
        Self::with_seed(sizes, codec::chunk_key(key))
    }
    fn with_seed(sizes: ChunkSizes, seed: [u8; 32]) -> Self {
        let mut bytes = [0u8; 256 * 8];
        blake3::Hasher::new_keyed(&seed)
            .finalize_xof()
            .fill(&mut bytes);
        let mut gear = Box::new([0u64; 256]);
        for (value, word) in gear.iter_mut().zip(bytes.as_chunks::<8>().0) {
            *value = u64::from_le_bytes(*word);
        }
        let bits = sizes.target.trailing_zeros();
        // The top bits of a left-shifting gear hash depend on the last 64
        // bytes; the low bits only on the last few.
        let top = |n: u32| if n == 0 { 0 } else { !0u64 << (64 - n.min(64)) };
        Self {
            gear,
            sizes,
            strict: top(bits + 2),
            loose: top(bits.saturating_sub(2)),
        }
    }
    pub fn sizes(&self) -> ChunkSizes {
        self.sizes
    }
    /// The length of the first chunk of `bytes`. Unless `bytes` is the rest of
    /// the stream, it must hold at least `max` bytes, since a cut depends on
    /// bytes up to the cut itself.
    pub fn cut(&self, bytes: &[u8]) -> usize {
        let ChunkSizes { min, target, max } = self.sizes;
        if bytes.len() <= min {
            return bytes.len();
        }
        let end = bytes.len().min(max);
        let normal = end.min(target);
        let mut hash = 0u64;
        for (position, &byte) in bytes.iter().enumerate().take(normal).skip(min) {
            hash = (hash << 1).wrapping_add(self.gear[byte as usize]);
            if hash & self.strict == 0 {
                return position + 1;
            }
        }
        for (position, &byte) in bytes.iter().enumerate().take(end).skip(normal) {
            hash = (hash << 1).wrapping_add(self.gear[byte as usize]);
            if hash & self.loose == 0 {
                return position + 1;
            }
        }
        end
    }
    /// The chunks of a stream, in order, holding at most `2 × max` bytes at a
    /// time. An empty stream is one empty chunk, so every stream has a chunk.
    pub fn chunks<R: Read>(&self, reader: R) -> Chunks<'_, R> {
        Chunks {
            chunker: self,
            reader,
            buffer: Vec::with_capacity(self.sizes.max * 2),
            start: 0,
            ended: false,
            emitted: false,
            failed: false,
        }
    }
}

pub struct Chunks<'a, R> {
    chunker: &'a Chunker,
    reader: R,
    buffer: Vec<u8>,
    start: usize,
    ended: bool,
    emitted: bool,
    failed: bool,
}
impl<R: Read> Chunks<'_, R> {
    fn fill(&mut self) -> std::io::Result<()> {
        let max = self.chunker.sizes.max;
        if self.start > 0 && self.buffer.len() - self.start < max {
            self.buffer.drain(..self.start);
            self.start = 0;
        }
        while !self.ended && self.buffer.len() - self.start < max {
            let have = self.buffer.len();
            self.buffer.resize(self.start + max * 2, 0);
            let read = loop {
                match self.reader.read(&mut self.buffer[have..]) {
                    Ok(n) => break Ok(n),
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => break Err(error),
                }
            };
            match read {
                Ok(n) => {
                    self.buffer.truncate(have + n);
                    self.ended = n == 0;
                }
                Err(error) => {
                    self.buffer.truncate(have);
                    return Err(error);
                }
            }
        }
        Ok(())
    }
}
impl<R: Read> Iterator for Chunks<'_, R> {
    type Item = Result<Vec<u8>>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed {
            return None;
        }
        if let Err(error) = self.fill() {
            self.failed = true;
            return Some(Err(error.into()));
        }
        let rest = &self.buffer[self.start..];
        if rest.is_empty() && (self.emitted || !self.ended) {
            return None;
        }
        let n = self.chunker.cut(rest);
        let chunk = rest[..n].to_vec();
        self.start += n;
        self.emitted = true;
        Some(Ok(chunk))
    }
}

/// A stream planned as chunks: its entries, in order, are appended as part of
/// a group, and the extents the group returns for them read back as the stream.
pub struct Chunked {
    pub entries: Vec<GroupEntry>,
    /// The name of the whole stream, computed like a blob's name (plain or
    /// keyed), so a product can tell an unchanged revision without its chunks.
    pub hash: Hash,
    pub len: u64,
}
impl Chunked {
    /// Chunks that must be appended, and the bytes they hold before encoding.
    pub fn new_chunks(&self) -> (usize, u64) {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                GroupEntry::Prepared(prepared) => Some(prepared.decoded_len),
                GroupEntry::Stored(_) => None,
            })
            .fold((0, 0), |(count, bytes), len| (count + 1, bytes + len))
    }
}

impl Preparation {
    /// Plan a stream as content-defined chunks. `key` seals every chunk and
    /// keys the cut points; `None` stores plain chunks. `lookup` answers from
    /// the product's index (and, if wanted, `SegmentWriter::uncommitted`); a
    /// chunk it knows becomes `Stored` and is neither compressed nor written.
    pub fn chunked(
        &self,
        reader: impl Read,
        level: i32,
        sizes: ChunkSizes,
        key: Option<&DataKey>,
        lookup: impl FnMut(&Hash) -> Result<Option<Extent>>,
    ) -> Result<Chunked> {
        self.chunked_checked(reader, level, sizes, key, lookup, || Ok(()))
    }
    pub fn chunked_checked<E: From<Error>>(
        &self,
        reader: impl Read,
        level: i32,
        sizes: ChunkSizes,
        key: Option<&DataKey>,
        mut lookup: impl FnMut(&Hash) -> Result<Option<Extent>>,
        mut check: impl FnMut() -> std::result::Result<(), E>,
    ) -> std::result::Result<Chunked, E> {
        let (chunker, naming_key, kind) = match key {
            None => (Chunker::plain(sizes), None, Kind::Plain),
            Some(key) => (
                Chunker::sealed(sizes, key),
                Some(codec::name_key(key)),
                Kind::Sealed {
                    key_ref: key.key_ref.clone(),
                },
            ),
        };
        let name = |bytes: &[u8]| match &naming_key {
            None => Hash::of(bytes),
            Some(naming) => Hash(*blake3::keyed_hash(naming, bytes).as_bytes()),
        };
        let mut whole = match &naming_key {
            None => blake3::Hasher::new(),
            Some(naming) => blake3::Hasher::new_keyed(naming),
        };
        let mut len = 0u64;
        let mut entries = Vec::new();
        for chunk in chunker.chunks(reader) {
            check()?;
            let chunk = chunk?;
            whole.update(&chunk);
            len += chunk.len() as u64;
            let hash = name(&chunk);
            if let Some(extent) = lookup(&hash)? {
                if extent.hash != hash
                    || extent.decoded_len != chunk.len() as u64
                    || extent.kind != kind
                {
                    return Err(Error::Refused(
                        "hash index returned a different blob descriptor".into(),
                    )
                    .into());
                }
                entries.push(GroupEntry::Stored(extent));
                continue;
            }
            let prepared = match key {
                None => self.plain(Cursor::new(&chunk), level)?,
                Some(key) => self.sealed(Cursor::new(&chunk), level, key)?,
            };
            entries.push(GroupEntry::Prepared(prepared));
        }
        check()?;
        Ok(Chunked {
            entries,
            hash: Hash(*whole.finalize().as_bytes()),
            len,
        })
    }
}

#[cfg(test)]
mod tests;
