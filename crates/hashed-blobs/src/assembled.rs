//! Seekable reads over extents, keeping a few verified plain blobs open.
use super::{CachedFrame, Error, Extent, Hash, PlainRead, Reader, Result, Store};
use std::collections::VecDeque;
use std::io::{self, Read, Seek, SeekFrom};

// A frequently used base and a few others stay open, without one descriptor per
// distinct blob in a wide assembly. A closed blob is verified again when it is
// used again, so there is no limit on extents or blobs.
const OPEN_PLAIN: usize = 4;

pub struct Assembled<'a> {
    store: &'a Store,
    reader: Reader,
    // Least recently used first. Each entry owns one FD and its complete seek
    // table; its decoded-frame slot is empty while it waits here.
    opened: VecDeque<PlainRead>,
    frame: Option<(Hash, CachedFrame)>,
    extents: Vec<Extent>,
    starts: Vec<u64>,
    total: u64,
    pos: u64,
}

impl<'a> Assembled<'a> {
    pub(super) fn new(store: &'a Store, extents: &[Extent]) -> Result<Self> {
        let mut starts = Vec::with_capacity(extents.len());
        let mut total = 0u64;
        for e in extents {
            if !store.contains(&e.blob) {
                return Err(Error::Missing(e.blob));
            }
            starts.push(total);
            total = total
                .checked_add(e.len)
                .ok_or_else(|| Error::Refused("assembled length overflow".into()))?;
        }
        Ok(Self {
            store,
            reader: Reader::new(&store.write_root, store.roots[1..].to_vec()),
            opened: VecDeque::new(),
            frame: None,
            extents: extents.to_vec(),
            starts,
            total,
            pos: 0,
        })
    }

    fn range(&mut self, hash: &Hash, offset: u64, len: u64) -> Result<Vec<u8>> {
        let mut plain =
            if let Some(index) = self.opened.iter().position(|plain| plain.hash == *hash) {
                let mut plain = self.opened.remove(index).expect("located verified blob");
                plain.current = self
                    .frame
                    .take()
                    .and_then(|(cached, frame)| (cached == *hash).then_some(frame));
                plain
            } else {
                // Verifying a whole blob holds one frame. Drop the cached frame
                // first, so switching blobs never holds two.
                self.frame = None;
                if self.opened.len() == OPEN_PLAIN {
                    self.opened.pop_front();
                }
                if self.store.kind(hash)? != super::Kind::Plain {
                    return self.store.range_any(&self.reader, hash, offset, len);
                }
                self.store.open_plain::<Error>(hash, None, |_| Ok(()))?.0
            };
        let result = plain.range::<Error>(offset, len, &mut || Ok(()));
        if result.is_ok() {
            self.frame = plain.current.take().map(|frame| (*hash, frame));
            self.opened.push_back(plain);
        } else {
            // As in Reader, a failed read keeps nothing for a retry.
            self.frame = None;
        }
        result
    }
}

impl Read for Assembled<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.total || buf.is_empty() {
            return Ok(0);
        }
        let i = match self.starts.binary_search(&self.pos) {
            Ok(i) => {
                // several zero-length extents may share a start; take the last one that has bytes
                let mut j = i;
                while j + 1 < self.starts.len() && self.starts[j + 1] == self.pos {
                    j += 1;
                }
                j
            }
            Err(i) => i - 1,
        };
        let e = self.extents[i];
        let within = self.pos - self.starts[i];
        let take = (buf.len() as u64).min(e.len - within).min(1 << 20);
        let offset = e
            .offset
            .checked_add(within)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "extent offset overflow"))?;
        let data = self
            .range(&e.blob, offset, take)
            .map_err(io::Error::other)?;
        buf[..data.len()].copy_from_slice(&data);
        self.pos += data.len() as u64;
        Ok(data.len())
    }
}

impl Seek for Assembled<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let p = match to {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::End(d) => self.total as i128 + d as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
        };
        if p < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before the start",
            ));
        }
        self.pos = u64::try_from(p)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "seek position overflow"))?;
        Ok(self.pos)
    }
}

#[cfg(test)]
pub(super) mod tests;
