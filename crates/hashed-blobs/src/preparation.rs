//! Bounded frame preparation, independent of segment ownership and durability.
use crate::{
    codec::{self, Frame, Index},
    DataKey, Error, Hash, Kind, Result, DEFAULT_FRAME,
};
use chacha20poly1305::{
    aead::{rand_core::RngCore, Aead, OsRng, Payload},
    ChaCha20Poly1305, KeyInit, Nonce,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Preparation {
    scratch: PathBuf,
    frame: usize,
}
pub struct Prepared {
    pub(crate) file: File,
    pub(crate) hash: Hash,
    pub(crate) decoded_len: u64,
    pub(crate) stored_len: u64,
    pub(crate) kind: Kind,
    pub(crate) index_hash: Hash,
    pub(crate) encoded_hash: Hash,
}
impl Prepared {
    pub fn hash(&self) -> Hash {
        self.hash
    }
    pub fn decoded_len(&self) -> u64 {
        self.decoded_len
    }
    pub fn stored_len(&self) -> u64 {
        self.stored_len
    }
    pub fn kind(&self) -> &Kind {
        &self.kind
    }
}
impl Preparation {
    pub fn new(scratch: impl AsRef<Path>) -> Result<Self> {
        std::fs::create_dir_all(scratch.as_ref())?;
        Ok(Self {
            scratch: scratch.as_ref().to_owned(),
            frame: DEFAULT_FRAME,
        })
    }
    pub fn with_frame_size(mut self, frame: usize) -> Result<Self> {
        if frame == 0 || frame > u32::MAX as usize {
            return Err(Error::Refused(
                "frame size must fit a positive zstd frame length".into(),
            ));
        }
        self.frame = frame;
        Ok(self)
    }
    pub fn plain(&self, reader: impl Read, level: i32) -> Result<Prepared> {
        self.plain_checked(reader, level, || Ok(()))
    }
    pub fn plain_checked<E: From<Error>>(
        &self,
        mut reader: impl Read,
        level: i32,
        mut check: impl FnMut() -> std::result::Result<(), E>,
    ) -> std::result::Result<Prepared, E> {
        let mut file = tempfile::tempfile_in(&self.scratch).map_err(Error::Io)?;
        let mut compressor = compressor(level).map_err(Error::Io)?;
        let mut index = Index {
            hash: Hash([0; 32]),
            binding: Hash([0; 32]),
            kind: Kind::Plain,
            salt: [0; 32],
            frames: Vec::new(),
        };
        let mut hasher = blake3::Hasher::new();
        let mut total = 0;
        let mut buffer = vec![0; self.frame];
        loop {
            check()?;
            let n = read_frame(&mut reader, &mut buffer).map_err(Error::Io)?;
            check()?;
            if n == 0 && !index.frames.is_empty() {
                break;
            }
            hasher.update(&buffer[..n]);
            total += n as u64;
            let encoded = compressor.compress(&buffer[..n]).map_err(Error::Io)?;
            file.write_all(&encoded).map_err(Error::Io)?;
            index.frames.push(Frame {
                stored: encoded.len() as u64,
                decoded: n as u64,
                digest: Hash::of(&encoded),
            });
            if n < self.frame {
                break;
            }
        }
        check()?;
        finish(file, Hash(*hasher.finalize().as_bytes()), total, index).map_err(E::from)
    }
    pub fn sealed(&self, reader: impl Read + Seek, level: i32, key: &DataKey) -> Result<Prepared> {
        self.sealed_checked(reader, level, key, || Ok(()))
    }
    pub fn sealed_checked<E: From<Error>>(
        &self,
        mut reader: impl Read + Seek,
        level: i32,
        key: &DataKey,
        mut check: impl FnMut() -> std::result::Result<(), E>,
    ) -> std::result::Result<Prepared, E> {
        let start = reader.stream_position().map_err(Error::Io)?;
        let mut compressor = compressor(level).map_err(Error::Io)?;
        let mut buffer = vec![0; self.frame];
        let mut index = Index {
            hash: Hash([0; 32]),
            binding: Hash([0; 32]),
            kind: Kind::Sealed {
                key_ref: key.key_ref.clone(),
            },
            salt: [0; 32],
            frames: Vec::new(),
        };
        OsRng.fill_bytes(&mut index.salt);
        let naming_key = codec::name_key(key);
        let mut naming = blake3::Hasher::new_keyed(&naming_key);
        let mut total = 0;
        // Determine the complete associated-data table before any encryption.
        // Compressed plaintext is held only in this frame buffer, never staged.
        loop {
            check()?;
            let n = read_frame(&mut reader, &mut buffer).map_err(Error::Io)?;
            check()?;
            if n == 0 && !index.frames.is_empty() {
                break;
            }
            naming.update(&buffer[..n]);
            total += n as u64;
            let compressed = compressor.compress(&buffer[..n]).map_err(Error::Io)?;
            index.frames.push(Frame {
                stored: compressed.len() as u64 + 16,
                decoded: n as u64,
                digest: Hash([0; 32]),
            });
            if n < self.frame {
                break;
            }
        }
        let hash = Hash(*naming.finalize().as_bytes());
        index.bind(hash);
        reader.seek(SeekFrom::Start(start)).map_err(Error::Io)?;
        let cipher = ChaCha20Poly1305::new((&codec::frame_key(key, hash, &index.salt)).into());
        let mut file = tempfile::tempfile_in(&self.scratch).map_err(Error::Io)?;
        let mut second = blake3::Hasher::new_keyed(&naming_key);
        for number in 0..index.frames.len() {
            check()?;
            let n = read_frame(&mut reader, &mut buffer).map_err(Error::Io)?;
            second.update(&buffer[..n]);
            let compressed = compressor.compress(&buffer[..n]).map_err(Error::Io)?;
            if n as u64 != index.frames[number].decoded
                || compressed.len() as u64 + 16 != index.frames[number].stored
            {
                return Err(
                    Error::Refused("sealed input changed during preparation".into()).into(),
                );
            }
            let encrypted = cipher
                .encrypt(
                    Nonce::from_slice(&codec::nonce(number)),
                    Payload {
                        msg: &compressed,
                        aad: &index.aad(number),
                    },
                )
                .map_err(|_| Error::Refused("frame encryption failed".into()))?;
            index.frames[number].digest = Hash::of(&encrypted);
            file.write_all(&encrypted).map_err(Error::Io)?;
        }
        if Hash(*second.finalize().as_bytes()) != hash
            || reader.read(&mut [0]).map_err(Error::Io)? != 0
        {
            return Err(Error::Refused("sealed input changed during preparation".into()).into());
        }
        check()?;
        finish(file, hash, total, index).map_err(E::from)
    }
    pub fn sealed_name(key: &DataKey, bytes: &[u8]) -> Hash {
        Hash(*blake3::keyed_hash(&codec::name_key(key), bytes).as_bytes())
    }
}
fn compressor(level: i32) -> std::io::Result<zstd::bulk::Compressor<'static>> {
    let mut c = zstd::bulk::Compressor::new(level)?;
    c.set_parameter(zstd::zstd_safe::CParameter::ChecksumFlag(true))?;
    Ok(c)
}
fn read_frame(reader: &mut impl Read, buffer: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buffer.len() {
        match reader.read(&mut buffer[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}
fn finish(mut file: File, hash: Hash, decoded_len: u64, mut index: Index) -> Result<Prepared> {
    index.bind(hash);
    let encoded = index.encode();
    let index_hash = Hash::of(&encoded);
    let metadata_len = encoded
        .len()
        .checked_add(16)
        .ok_or_else(|| Error::Refused("frame index length overflows".into()))?;
    file.write_all(&codec::SKIP.to_le_bytes())?;
    file.write_all(
        &u32::try_from(metadata_len)
            .map_err(|_| Error::Refused("zstd index frame length overflows".into()))?
            .to_le_bytes(),
    )?;
    file.write_all(&encoded)?;
    file.write_all(&(encoded.len() as u64).to_le_bytes())?;
    file.write_all(codec::TAIL)?;
    if index.kind == Kind::Plain {
        // A standard zstd seek table includes the skippable index as a zero-byte
        // decoded frame. Stock zstd can decode the complete extent unchanged.
        let count = u32::try_from(index.frames.len() + 1)
            .map_err(|_| Error::Refused("zstd frame count overflows".into()))?;
        file.write_all(&0x184D_2A5Eu32.to_le_bytes())?;
        file.write_all(
            &(count
                .checked_mul(8)
                .and_then(|n| n.checked_add(9))
                .ok_or_else(|| Error::Refused("zstd seek table overflows".into()))?)
            .to_le_bytes(),
        )?;
        for f in &index.frames {
            file.write_all(
                &u32::try_from(f.stored)
                    .map_err(|_| Error::Refused("zstd frame length overflows".into()))?
                    .to_le_bytes(),
            )?;
            file.write_all(&(f.decoded as u32).to_le_bytes())?;
        }
        file.write_all(
            &u32::try_from(metadata_len + 8)
                .map_err(|_| Error::Refused("zstd metadata length overflows".into()))?
                .to_le_bytes(),
        )?;
        file.write_all(&0u32.to_le_bytes())?;
        file.write_all(&count.to_le_bytes())?;
        file.write_all(&[0])?;
        file.write_all(&0x8F92_EAB1u32.to_le_bytes())?;
    }
    let stored_len = file.stream_position()?;
    file.rewind()?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    file.rewind()?;
    Ok(Prepared {
        file,
        hash,
        decoded_len,
        stored_len,
        kind: index.kind,
        index_hash,
        encoded_hash: Hash(*hasher.finalize().as_bytes()),
    })
}
