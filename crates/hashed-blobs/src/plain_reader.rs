//! Sequential reads of one verified plain blob.
use super::{Error, PlainRead, Result, VerificationStats, View};
use std::io::{self, BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A plain blob verified before its first byte is returned. Reads use the file opened for
/// verification and decode each frame once, whatever the caller's buffer size.
/// It does not seek. A failed read stops this reader; create another reader to retry.
pub struct PlainReader {
    plain: PlainRead,
    frame: usize,
    decoder: Option<zstd::stream::read::Decoder<'static, BufReader<FrameInput>>>,
    decoded: u64,
    state: Arc<ReadState>,
    failed: bool,
}

impl PlainReader {
    pub(super) fn new(mut plain: PlainRead, check: Box<dyn FnMut() -> Result<()> + Send>) -> Self {
        // The scoped range reader's last frame cache is unnecessary for forward decoding.
        plain.current = None;
        Self {
            plain,
            frame: 0,
            decoder: None,
            decoded: 0,
            state: Arc::new(ReadState {
                check: Mutex::new(check),
                stopped: Mutex::new(None),
                compressed: AtomicU64::new(0),
            }),
            failed: false,
        }
    }

    /// Work completed so far, including the initial whole-name verification pass.
    pub fn stats(&self) -> VerificationStats {
        VerificationStats {
            compressed_bytes: self.plain.stats.compressed_bytes
                + self.state.compressed.load(Ordering::Relaxed),
            ..self.plain.stats
        }
    }

    fn corrupt(&self, what: &str) -> Error {
        Error::Corrupt {
            blob: self.plain.hash,
            what: what.into(),
        }
    }

    fn next(&mut self, bytes: &mut [u8]) -> Result<usize> {
        self.state.check()?;
        self.plain.unchanged()?;
        loop {
            if self.decoder.is_none() {
                let Some(&(offset, compressed, _)) = self.plain.table.get(self.frame) else {
                    return Ok(0);
                };
                let input = FrameInput {
                    view: Arc::clone(&self.plain.view),
                    position: offset,
                    end: offset + u64::from(compressed),
                    state: Arc::clone(&self.state),
                };
                let mut decoder = zstd::stream::read::Decoder::new(input)
                    .map_err(|_| self.corrupt("invalid frame decoder"))?
                    .single_frame();
                decoder.window_log_max(31).map_err(Error::Io)?;
                self.decoder = Some(decoder);
                self.plain.stats.frames += 1;
                self.decoded = 0;
            }
            let read = self.decoder.as_mut().expect("frame opened").read(bytes);
            if let Some(error) = self
                .state
                .stopped
                .lock()
                .map_err(|_| Error::Refused("plain reader state poisoned".into()))?
                .take()
            {
                return Err(error);
            }
            let n = read.map_err(|error| {
                if error.raw_os_error().is_some() {
                    Error::Io(error)
                } else {
                    self.corrupt("plain frame decode/checksum")
                }
            })?;
            self.plain.unchanged()?;
            self.decoded += n as u64;
            self.plain.stats.decoded_bytes += n as u64;
            let (offset, compressed, expected) = self.plain.table[self.frame];
            if self.decoded > u64::from(expected) {
                return Err(self.corrupt("frame exceeds seek length"));
            }
            if n != 0 {
                return Ok(n);
            }
            let input = self.decoder.take().expect("frame opened").finish();
            if self.decoded != u64::from(expected)
                || input.get_ref().position - input.buffer().len() as u64
                    != offset + u64::from(compressed)
            {
                return Err(self.corrupt("frame extent/decoded length differs from seek table"));
            }
            self.frame += 1;
            self.state.check()?;
        }
    }
}

impl Read for PlainReader {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.failed {
            return Err(io::Error::other("plain reader stopped after a failed read"));
        }
        match self.next(bytes) {
            Ok(n) => Ok(n),
            Err(error) => {
                self.failed = true;
                self.decoder = None;
                Err(io::Error::other(error))
            }
        }
    }
}

struct FrameInput {
    view: Arc<View>,
    position: u64,
    end: u64,
    state: Arc<ReadState>,
}

impl Read for FrameInput {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        use std::os::unix::fs::FileExt;
        let want = bytes.len().min((self.end - self.position) as usize);
        if want == 0 {
            return Ok(0);
        }
        if let Err(error) = self.state.check() {
            *self
                .state
                .stopped
                .lock()
                .map_err(|_| io::Error::other("plain reader state poisoned"))? = Some(error);
            return Err(io::Error::other("plain reader interrupted"));
        }
        let n = self.view.file.read_at(&mut bytes[..want], self.position)?;
        self.position += n as u64;
        self.state.compressed.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }
}

struct ReadState {
    check: Mutex<Box<dyn FnMut() -> Result<()> + Send>>,
    stopped: Mutex<Option<Error>>,
    compressed: AtomicU64,
}

impl ReadState {
    fn check(&self) -> Result<()> {
        self.check
            .lock()
            .map_err(|_| Error::Refused("plain reader state poisoned".into()))?()
    }
}

#[cfg(test)]
mod tests;
