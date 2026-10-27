//! Finding positions and fingerprints from the same stream used for masking.
use crate::{Cell, Masker};
use hmac::{Hmac, Mac};
use sha2::Sha256;
type Fingerprint = Hmac<Sha256>;
use std::io;
mod assignment;

/// Changes whenever the scan rules or finding encoding changes.
pub const SCANNER_VERSION: &str = "credential-output-2";

/// One masked byte range. It contains no source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub pattern: &'static str,
    pub start: usize,
    pub end: usize,
    pub fingerprint: String,
}

#[derive(Clone)]
pub(crate) struct Observed {
    key: Fingerprint,
    current: Option<(usize, usize, &'static str, Fingerprint)>,
    done: Vec<Finding>,
}

impl Observed {
    pub(crate) fn cell(&mut self, cell: &Cell) {
        let Some(region) = cell.region else {
            self.flush();
            return;
        };
        if self
            .current
            .as_ref()
            .is_some_and(|(_, end, _, _)| *end != cell.pos || region.start >= *end)
        {
            self.flush();
        }
        let (_, end, _, hash) = self
            .current
            .get_or_insert_with(|| (cell.pos, cell.pos, region.kind, self.key.clone()));
        hash.update(cell.c.encode_utf8(&mut [0; 4]).as_bytes());
        *end = cell.pos + cell.c.len_utf8();
    }

    fn flush(&mut self) {
        if let Some((start, end, pattern, hash)) = self.current.take() {
            self.done.push(Finding {
                pattern,
                start,
                end,
                fingerprint: format!("{:x}", hash.finalize().into_bytes()),
            });
        }
    }
}

/// Streaming finding reader. Input pieces never end a match. A block boundary
/// ends tokens and emits that block's findings, keeping an open private key.
/// Offsets count UTF-8 bytes from the start of the stream.
pub struct Scanner(Masker, assignment::Assignment);

impl Scanner {
    /// Use a random key belonging to the archive whose text is being scanned.
    pub fn new(key: &[u8]) -> Self {
        let key = Fingerprint::new_from_slice(key).expect("HMAC accepts any key length");
        let mut stream = Masker::new();
        stream.findings = Some(Observed {
            key: key.clone(),
            current: None,
            done: Vec::new(),
        });
        Self(stream, assignment::Assignment::new(key))
    }

    pub fn push(&mut self, text: &str) -> io::Result<()> {
        for (offset, c) in text.char_indices() {
            self.1.step(c, self.0.position + offset);
        }
        self.0.push(text, &mut io::sink())
    }

    pub fn block_boundary(&mut self) -> io::Result<Vec<Finding>> {
        self.0.block_boundary(&mut io::sink())?;
        Ok(self.take())
    }

    pub fn finish(mut self) -> io::Result<Vec<Finding>> {
        self.0.finish(&mut io::sink())?;
        Ok(self.take())
    }

    fn take(&mut self) -> Vec<Finding> {
        self.1.boundary();
        let observed = self.0.findings.as_mut().expect("scanner observation");
        observed.flush();
        let mut done = std::mem::take(&mut observed.done);
        for finding in self.1.done.drain(..) {
            if !done
                .iter()
                .any(|other| other.start < finding.end && finding.start < other.end)
            {
                done.push(finding);
            }
        }
        done.sort_by_key(|finding| finding.start);
        done
    }
}

#[cfg(test)]
mod tests;
