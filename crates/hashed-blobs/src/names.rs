//! The blob files in a store's write root.

use std::ffi::OsStr;
use std::fs::{self, ReadDir};
use std::io;
use std::path::Path;

use crate::{Error, Hash, Result};

/// The names of the blob files in a store's write root, read one shard directory at a time.
///
/// Created by [`crate::Store::names`]. Extra roots and `blobs/tmp/` are not listed, and the
/// order is the directory order. A file installed or deleted while the listing runs may or may
/// not appear. An entry that is not a blob file yields an error of kind
/// `InvalidData` naming its path; the listing goes on after any error.
pub struct Names {
    shards: ReadDir,
    shard: Option<(u8, ReadDir)>,
}

impl Names {
    pub(crate) fn open(blobs: &Path) -> Result<Names> {
        Ok(Names {
            shards: fs::read_dir(blobs)?,
            shard: None,
        })
    }

    fn next_shard(&mut self) -> Option<Result<()>> {
        loop {
            let entry = match self.shards.next()? {
                Ok(entry) => entry,
                Err(error) => return Some(Err(Error::Io(error))),
            };
            let file_name = entry.file_name();
            if file_name == "tmp" {
                continue;
            }
            let is_directory = match entry.file_type() {
                Ok(file_type) => file_type.is_dir(),
                Err(error) => return Some(Err(Error::Io(error))),
            };
            let Some(prefix) = shard_prefix(&file_name).filter(|_| is_directory) else {
                return Some(Err(not_a_blob(&entry.path())));
            };
            return Some(match fs::read_dir(entry.path()) {
                Ok(entries) => {
                    self.shard = Some((prefix, entries));
                    Ok(())
                }
                Err(error) => Err(Error::Io(error)),
            });
        }
    }
}

impl Iterator for Names {
    type Item = Result<Hash>;

    fn next(&mut self) -> Option<Result<Hash>> {
        loop {
            if let Some((prefix, entries)) = &mut self.shard {
                match entries.next() {
                    Some(Ok(entry)) => {
                        return Some(
                            blob_name(&entry.file_name())
                                .filter(|hash| hash.0[0] == *prefix)
                                .ok_or_else(|| not_a_blob(&entry.path())),
                        );
                    }
                    Some(Err(error)) => return Some(Err(Error::Io(error))),
                    None => self.shard = None,
                }
            }
            if let Err(error) = self.next_shard()? {
                return Some(Err(error));
            }
        }
    }
}

fn not_a_blob(path: &Path) -> Error {
    Error::Io(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("{} is not a blob file", path.display()),
    ))
}

/// The first byte a shard directory's two lowercase hex digits name.
fn shard_prefix(file_name: &OsStr) -> Option<u8> {
    let text = file_name.to_str()?;
    let [byte] = parse_hex::<1>(text)?;
    Some(byte)
}

/// The hash in a blob file name, `<64 lowercase hex digits>.zst`.
fn blob_name(file_name: &OsStr) -> Option<Hash> {
    let hex = file_name.to_str()?.strip_suffix(".zst")?;
    parse_hex::<32>(hex).map(Hash)
}

/// Exactly `N` bytes written as lowercase hex, the way `Hash::hex` writes them.
fn parse_hex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let digits = text.as_bytes();
    if digits.len() != 2 * N {
        return None;
    }
    let digit = |d: u8| match d {
        b'0'..=b'9' => Some(d - b'0'),
        b'a'..=b'f' => Some(d - b'a' + 10),
        _ => None,
    };
    let mut bytes = [0; N];
    let (pairs, _) = digits.as_chunks::<2>();
    for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
        *byte = digit(*high)? << 4 | digit(*low)?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests;
