//! Lock-free readers addressed by immutable extent descriptors.
use crate::{
    DataKey, EncodedBlob, Error, Extent, Hash, Kind, Result,
    codec::{self, Index},
};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};

/// Per-operation budgets. Values exceeding these remain valid input; the
/// caller chooses whether this operation has enough memory to read them.
#[derive(Clone, Copy, Debug)]
pub struct ReadLimits {
    pub frame_bytes: usize,
    pub index_bytes: usize,
}
impl Default for ReadLimits {
    fn default() -> Self {
        Self {
            frame_bytes: usize::MAX,
            index_bytes: usize::MAX,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerificationStats {
    pub compressed_bytes: u64,
    pub decoded_bytes: u64,
    pub metadata_bytes: u64,
    pub frames: u64,
}
#[derive(Debug)]
pub struct Mismatch {
    pub extent: Extent,
    pub error: String,
}
#[derive(Clone)]
pub struct Reader {
    root: PathBuf,
}
struct OpenExtent {
    file: File,
    extent: Extent,
    index: Index,
    starts: Vec<(u64, u64)>,
}
pub struct BlobReader {
    extents: Vec<OpenExtent>,
    starts: Vec<u64>,
    len: u64,
    position: u64,
    key: Option<DataKey>,
    check: Box<dyn FnMut() -> Result<()> + Send>,
    cache: Option<(usize, usize, Vec<u8>)>,
    stats: VerificationStats,
    failed: bool,
}
impl Reader {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_owned(),
        }
    }
    pub fn open(&self, extents: &[Extent], key: Option<&DataKey>) -> Result<BlobReader> {
        self.open_checked(extents, key, || Ok(()))
    }
    pub fn open_checked(
        &self,
        extents: &[Extent],
        key: Option<&DataKey>,
        check: impl FnMut() -> Result<()> + Send + 'static,
    ) -> Result<BlobReader> {
        self.open_checked_with_limits(extents, key, ReadLimits::default(), check)
    }
    pub fn open_checked_with_limits(
        &self,
        extents: &[Extent],
        key: Option<&DataKey>,
        limits: ReadLimits,
        mut check: impl FnMut() -> Result<()> + Send + 'static,
    ) -> Result<BlobReader> {
        let mut opened = Vec::with_capacity(extents.len());
        let mut starts = Vec::with_capacity(extents.len());
        let mut length = 0u64;
        let mut metadata = 0;
        for extent in extents {
            check()?;
            let (value, bytes) = OpenExtent::open(&self.root, extent, limits)?;
            if let Kind::Sealed { key_ref } = &extent.kind {
                let key = key.ok_or(Error::Locked(extent.hash))?;
                if &key.key_ref != key_ref {
                    return Err(Error::Unwrap("sealed key reference differs".into()));
                }
            }
            starts.push(length);
            length = length
                .checked_add(extent.decoded_len)
                .ok_or_else(|| Error::Refused("decoded stream length overflows".into()))?;
            metadata += bytes;
            opened.push(value);
        }
        Ok(BlobReader {
            extents: opened,
            starts,
            len: length,
            position: 0,
            key: key.cloned(),
            check: Box::new(check),
            cache: None,
            stats: VerificationStats {
                metadata_bytes: metadata,
                ..VerificationStats::default()
            },
            failed: false,
        })
    }
    /// A complete read verifies each blob's plain or keyed name before returning.
    pub fn read(&self, extents: &[Extent], key: Option<&DataKey>) -> Result<Vec<u8>> {
        let mut reader = self.open(extents, key)?;
        let mut out = Vec::new();
        for index in 0..reader.extents.len() {
            let mut hasher = match &reader.extents[index].extent.kind {
                Kind::Plain => blake3::Hasher::new(),
                Kind::Sealed { .. } => blake3::Hasher::new_keyed(&codec::name_key(
                    reader.key.as_ref().expect("key checked"),
                )),
            };
            for frame in 0..reader.extents[index].index.frames.len() {
                (reader.check)()?;
                let bytes =
                    reader.extents[index].decode(frame, reader.key.as_ref(), &mut reader.stats)?;
                hasher.update(&bytes);
                out.extend(bytes);
            }
            if Hash(*hasher.finalize().as_bytes()) != reader.extents[index].extent.hash {
                return Err(Error::corrupt(
                    &reader.extents[index].extent,
                    "decoded content differs from blob name",
                ));
            }
        }
        Ok(out)
    }
    /// Decode only frames intersecting this half-open decoded byte range.
    pub fn read_range(
        &self,
        extents: &[Extent],
        offset: u64,
        len: u64,
        key: Option<&DataKey>,
    ) -> Result<Vec<u8>> {
        let mut reader = self.open(extents, key)?;
        let end = offset
            .checked_add(len)
            .filter(|&end| end <= reader.len)
            .ok_or_else(|| Error::Refused("range lies outside decoded stream".into()))?;
        reader.seek(SeekFrom::Start(offset))?;
        let mut out = Vec::new();
        let mut remaining = end - offset;
        let mut buffer = [0; 65536];
        while remaining > 0 {
            let want = remaining.min(buffer.len() as u64) as usize;
            let n = reader.read(&mut buffer[..want])?;
            if n == 0 {
                return Err(Error::Refused("decoded stream ended inside range".into()));
            }
            out.extend_from_slice(&buffer[..n]);
            remaining -= n as u64;
        }
        Ok(out)
    }
    /// Inspect every supplied committed extent without opening a writer or
    /// modifying files. Sealed content is checked without its data key.
    pub fn scan(&self, extents: &[Extent]) -> Vec<Mismatch> {
        self.scan_checked(extents, || Ok(()))
            .unwrap_or_else(|error| {
                extents
                    .first()
                    .map(|extent| {
                        vec![Mismatch {
                            extent: extent.clone(),
                            error: error.to_string(),
                        }]
                    })
                    .unwrap_or_default()
            })
    }
    pub fn scan_checked(
        &self,
        extents: &[Extent],
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Vec<Mismatch>> {
        let mut mismatches = Vec::new();
        for extent in extents {
            check()?;
            let mut cancelled = None;
            let result = {
                let mut check_frame = || match check() {
                    Ok(()) => Ok(()),
                    Err(error) => {
                        cancelled = Some(error);
                        Err(Error::Refused("scan cancelled".into()))
                    }
                };
                (|| {
                    let (value, _) = OpenExtent::open(&self.root, extent, ReadLimits::default())?;
                    let mut digest = blake3::Hasher::new();
                    let mut at = 0;
                    while at < extent.len {
                        check_frame()?;
                        let n = (extent.len - at).min(65536);
                        digest.update(&value.at(at, n)?);
                        at += n;
                    }
                    if Hash(*digest.finalize().as_bytes()) != extent.encoded_hash {
                        return Err(Error::corrupt(
                            extent,
                            "encoded bytes differ from committed digest",
                        ));
                    }
                    if extent.kind == Kind::Plain {
                        let mut naming = blake3::Hasher::new();
                        let mut stats = VerificationStats::default();
                        for frame in 0..value.index.frames.len() {
                            check_frame()?;
                            naming.update(&value.decode(frame, None, &mut stats)?);
                        }
                        if Hash(*naming.finalize().as_bytes()) != extent.hash {
                            return Err(Error::corrupt(
                                extent,
                                "decoded content differs from blob name",
                            ));
                        }
                    } else {
                        for (number, frame) in value.index.frames.iter().enumerate() {
                            check_frame()?;
                            if Hash::of(&value.at(value.starts[number].0, frame.stored)?)
                                != frame.digest
                            {
                                return Err(Error::corrupt(extent, "sealed frame digest differs"));
                            }
                        }
                    }
                    Ok(())
                })()
            };
            if let Some(error) = cancelled {
                return Err(error);
            }
            if let Err(error) = result {
                mismatches.push(Mismatch {
                    extent: extent.clone(),
                    error: error.to_string(),
                });
            }
        }
        Ok(mismatches)
    }
}
pub(crate) fn open_encoded(
    file: File,
    descriptor: &EncodedBlob,
    key: Option<&DataKey>,
    limits: ReadLimits,
) -> Result<BlobReader> {
    if file.metadata()?.len() != descriptor.stored_len {
        return Err(Error::Refused(
            "encoded file length differs from its descriptor".into(),
        ));
    }
    let extent = descriptor.extent();
    if let Kind::Sealed { key_ref } = &descriptor.kind {
        let key = key.ok_or(Error::Locked(descriptor.hash))?;
        if &key.key_ref != key_ref {
            return Err(Error::Unwrap("sealed key reference differs".into()));
        }
    }
    let (value, metadata) = OpenExtent::from_file(file, &extent, limits)?;
    Ok(BlobReader {
        extents: vec![value],
        starts: vec![0],
        len: descriptor.decoded_len,
        position: 0,
        key: key.cloned(),
        check: Box::new(|| Ok(())),
        cache: None,
        stats: VerificationStats {
            metadata_bytes: metadata,
            ..Default::default()
        },
        failed: false,
    })
}
impl OpenExtent {
    fn open(root: &Path, extent: &Extent, limits: ReadLimits) -> Result<(Self, u64)> {
        codec::segment_name(&extent.segment)?;
        let file = File::open(root.join(&extent.segment))
            .map_err(|e| Error::corrupt(extent, format!("cannot open segment: {e}")))?;
        Self::from_file(file, extent, limits)
    }
    fn from_file(file: File, extent: &Extent, limits: ReadLimits) -> Result<(Self, u64)> {
        if extent
            .offset
            .checked_add(extent.len)
            .is_none_or(|end| end > file.metadata().map(|m| m.len()).unwrap_or(0))
        {
            return Err(Error::corrupt(extent, "extent lies outside segment"));
        }
        let mut value = Self {
            file,
            extent: extent.clone(),
            index: Index {
                hash: Hash([0; 32]),
                binding: Hash([0; 32]),
                kind: Kind::Plain,
                salt: [0; 32],
                frames: Vec::new(),
            },
            starts: Vec::new(),
        };
        let mut end = extent.len;
        let mut seek_table = None;
        if extent.kind == Kind::Plain {
            if end < 17 {
                return Err(Error::corrupt(extent, "truncated zstd seek table"));
            }
            let tail = value.at(end - 9, 9)?;
            if tail[4] != 0 || u32::from_le_bytes(tail[5..].try_into().unwrap()) != 0x8F92_EAB1 {
                return Err(Error::corrupt(extent, "invalid zstd seek footer"));
            }
            let count = u32::from_le_bytes(tail[..4].try_into().unwrap()) as u64;
            let size = count
                .checked_mul(8)
                .and_then(|n| n.checked_add(17))
                .filter(|&n| n <= end)
                .ok_or_else(|| Error::corrupt(extent, "invalid zstd seek table length"))?;
            if count == 0 {
                return Err(Error::corrupt(extent, "empty zstd seek table"));
            }
            if size > limits.index_bytes as u64 {
                return Err(Error::Refused(
                    "zstd seek table exceeds caller index budget".into(),
                ));
            }
            end -= size;
            seek_table = Some(value.at(end, size)?);
        }
        if end < 24 {
            return Err(Error::corrupt(extent, "truncated frame index"));
        }
        let tail = value.at(end - 16, 16)?;
        if &tail[8..] != codec::TAIL {
            return Err(Error::corrupt(extent, "invalid frame index trailer"));
        }
        let length = u64::from_le_bytes(tail[..8].try_into().unwrap());
        let index_start = end
            .checked_sub(length)
            .and_then(|n| n.checked_sub(24))
            .ok_or_else(|| Error::corrupt(extent, "frame index length exceeds extent"))?;
        let header = value.at(index_start, 8)?;
        if u32::from_le_bytes(header[..4].try_into().unwrap()) != codec::SKIP
            || u32::from_le_bytes(header[4..].try_into().unwrap()) as u64 != length + 16
        {
            return Err(Error::corrupt(extent, "invalid frame index envelope"));
        }
        if length > limits.index_bytes as u64 {
            return Err(Error::Refused(
                "frame index exceeds caller index budget".into(),
            ));
        }
        let bytes = value.at(index_start + 8, length)?;
        if Hash::of(&bytes) != extent.index_hash {
            return Err(Error::corrupt(
                extent,
                "frame index differs from committed digest",
            ));
        }
        let index = Index::decode(&bytes).map_err(|e| Error::corrupt(extent, e.to_string()))?;
        if index.kind != extent.kind || index.hash != extent.hash {
            return Err(Error::corrupt(
                extent,
                "encoding kind differs from committed descriptor",
            ));
        }
        let (mut stored, mut decoded) = (0u64, 0u64);
        let compressed_limit =
            zstd::zstd_safe::compress_bound(limits.frame_bytes.min(u32::MAX as usize))
                .saturating_add(16) as u64;
        for frame in &index.frames {
            if frame.decoded > limits.frame_bytes as u64 || frame.stored > compressed_limit {
                return Err(Error::Refused("frame exceeds caller frame budget".into()));
            }
            value.starts.push((stored, decoded));
            stored = stored
                .checked_add(frame.stored)
                .ok_or_else(|| Error::corrupt(extent, "stored frame lengths overflow"))?;
            decoded = decoded
                .checked_add(frame.decoded)
                .ok_or_else(|| Error::corrupt(extent, "decoded frame lengths overflow"))?;
        }
        if stored != index_start || decoded != extent.decoded_len || index.frames.is_empty() {
            return Err(Error::corrupt(extent, "frame lengths differ from extent"));
        }
        if let Some(table) = seek_table {
            let count = index.frames.len() + 1;
            let expected = 17usize
                .checked_add(
                    count
                        .checked_mul(8)
                        .ok_or_else(|| Error::corrupt(extent, "seek count overflows"))?,
                )
                .ok_or_else(|| Error::corrupt(extent, "seek length overflows"))?;
            if table.len() != expected
                || u32::from_le_bytes(table[..4].try_into().unwrap()) != 0x184D_2A5E
                || u32::from_le_bytes(table[4..8].try_into().unwrap()) as usize != expected - 8
            {
                return Err(Error::corrupt(extent, "zstd seek table differs from index"));
            }
            for (number, (compressed, plain)) in index
                .frames
                .iter()
                .map(|f| (f.stored, f.decoded))
                .chain(std::iter::once((length + 24, 0)))
                .enumerate()
            {
                let at = 8 + number * 8;
                if u32::from_le_bytes(table[at..at + 4].try_into().unwrap()) as u64 != compressed
                    || u32::from_le_bytes(table[at + 4..at + 8].try_into().unwrap()) as u64 != plain
                {
                    return Err(Error::corrupt(extent, "zstd seek entry differs from index"));
                }
            }
        }
        value.index = index;
        Ok((value, extent.len - index_start))
    }
    fn at(&self, offset: u64, len: u64) -> Result<Vec<u8>> {
        if offset
            .checked_add(len)
            .is_none_or(|end| end > self.extent.len)
        {
            return Err(Error::corrupt(&self.extent, "read lies outside extent"));
        }
        let size = usize::try_from(len)
            .map_err(|_| Error::corrupt(&self.extent, "frame length exceeds address space"))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| Error::corrupt(&self.extent, "cannot allocate encoded frame"))?;
        bytes.resize(size, 0);
        self.file
            .read_exact_at(&mut bytes, self.extent.offset + offset)
            .map_err(|e| Error::corrupt(&self.extent, format!("cannot read extent: {e}")))?;
        Ok(bytes)
    }
    fn decode(
        &self,
        number: usize,
        key: Option<&DataKey>,
        stats: &mut VerificationStats,
    ) -> Result<Vec<u8>> {
        let frame = &self.index.frames[number];
        let encoded = self.at(self.starts[number].0, frame.stored)?;
        if Hash::of(&encoded) != frame.digest {
            return Err(Error::corrupt(&self.extent, "frame digest differs"));
        }
        let compressed = match &self.index.kind {
            Kind::Plain => encoded,
            Kind::Sealed { key_ref } => {
                let key = key.ok_or(Error::Locked(self.extent.hash))?;
                if &key.key_ref != key_ref {
                    return Err(Error::Unwrap("sealed key reference differs".into()));
                }
                let cipher = ChaCha20Poly1305::new(
                    (&codec::frame_key(key, self.extent.hash, &self.index.salt)).into(),
                );
                cipher
                    .decrypt(
                        Nonce::from_slice(&codec::nonce(number)),
                        Payload {
                            msg: &encoded,
                            aad: &self.index.aad(number),
                        },
                    )
                    .map_err(|_| {
                        Error::corrupt(&self.extent, "sealed frame authentication failed")
                    })?
            }
        };
        let expected = usize::try_from(frame.decoded)
            .map_err(|_| Error::corrupt(&self.extent, "decoded frame exceeds address space"))?;
        let bytes = zstd::bulk::decompress(&compressed, expected)
            .map_err(|_| Error::corrupt(&self.extent, "zstd frame decode or checksum failed"))?;
        if bytes.len() != expected {
            return Err(Error::corrupt(&self.extent, "decoded frame length differs"));
        }
        stats.frames += 1;
        stats.compressed_bytes += frame.stored;
        stats.decoded_bytes += bytes.len() as u64;
        Ok(bytes)
    }
}
impl BlobReader {
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn stats(&self) -> VerificationStats {
        self.stats
    }
    fn next(&mut self, out: &mut [u8]) -> Result<usize> {
        (self.check)()?;
        if out.is_empty() || self.position >= self.len {
            return Ok(0);
        }
        let extent = self
            .starts
            .partition_point(|&start| start <= self.position)
            .saturating_sub(1);
        let relative = self.position - self.starts[extent];
        let open = &self.extents[extent];
        let frame = open
            .starts
            .partition_point(|&(_, start)| start <= relative)
            .saturating_sub(1);
        if self
            .cache
            .as_ref()
            .is_none_or(|(e, f, _)| *e != extent || *f != frame)
        {
            let bytes = open.decode(frame, self.key.as_ref(), &mut self.stats)?;
            self.cache = Some((extent, frame, bytes));
        }
        (self.check)()?;
        let bytes = &self.cache.as_ref().expect("frame loaded").2;
        let at = (relative - open.starts[frame].1) as usize;
        let n = out.len().min(bytes.len() - at);
        out[..n].copy_from_slice(&bytes[at..at + n]);
        self.position += n as u64;
        Ok(n)
    }
}
impl Read for BlobReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io::Error::other("extent reader stopped after failure"));
        }
        match self.next(out) {
            Ok(n) => Ok(n),
            Err(error) => {
                self.failed = true;
                self.cache = None;
                Err(io::Error::other(error))
            }
        }
    }
}
impl Seek for BlobReader {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        if self.failed {
            return Err(io::Error::other("extent reader stopped after failure"));
        }
        if let Err(error) = (self.check)() {
            self.failed = true;
            self.cache = None;
            return Err(io::Error::other(error));
        }
        let position = match from {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(n) => i128::from(self.position) + i128::from(n),
            SeekFrom::End(n) => i128::from(self.len) + i128::from(n),
        };
        if position < 0 || position > i128::from(u64::MAX) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek position overflows",
            ));
        }
        self.position = position as u64;
        Ok(self.position)
    }
}

#[cfg(test)]
mod tests;
