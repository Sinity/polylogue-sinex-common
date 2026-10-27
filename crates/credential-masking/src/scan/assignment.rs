//! Generic assignments use entropy because their labels alone are ambiguous.
use super::{Finding, Sha256};
use sha2::Digest;

pub(super) struct Assignment {
    label: String,
    previous_word: bool,
    stage: u8,
    start: usize,
    end: usize,
    counts: [u64; 67],
    length: u64,
    hash: Sha256,
    pub done: Vec<Finding>,
}

const LABELS: &[&str] = &[
    "api_key",
    "api-key",
    "apikey",
    "secret",
    "token",
    "password",
    "passwd",
    "access_key",
    "access-key",
    "accesskey",
];
const ALPHABET: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/_=-";

impl Assignment {
    pub fn step(&mut self, c: char, pos: usize) {
        self.character(c, pos);
        self.previous_word = crate::word_ranges::is_word_char(c);
    }

    fn character(&mut self, c: char, pos: usize) {
        match self.stage {
            0 => {
                if self.label.is_empty() && self.previous_word {
                    return;
                }
                if crate::word_ranges::is_word_char(c) || c == '-' {
                    self.label.push(c.to_ascii_lowercase());
                    if !LABELS.iter().any(|label| label.starts_with(&self.label)) {
                        self.stage = if crate::word_ranges::is_word_char(c) {
                            4
                        } else {
                            0
                        };
                        self.label.clear();
                    }
                } else if LABELS.contains(&self.label.as_str())
                    && (c.is_whitespace() || ":=".contains(c))
                {
                    self.stage = if ":=".contains(c) { 2 } else { 1 };
                    self.label.clear();
                } else {
                    self.label.clear();
                }
            }
            1 => {
                if ":=".contains(c) {
                    self.stage = 2;
                } else if !c.is_whitespace() {
                    self.reset();
                    self.character(c, pos);
                }
            }
            2 => {
                if c.is_whitespace() {
                    return;
                }
                self.stage = 3;
                if "\"'".contains(c) {
                    return;
                }
                self.value(c, pos);
            }
            3 => self.value(c, pos),
            _ => {
                if !crate::word_ranges::is_word_char(c) {
                    self.reset();
                }
            }
        }
    }

    fn value(&mut self, c: char, pos: usize) {
        if let Some(index) = ALPHABET.find(c) {
            if self.length == 0 {
                self.start = pos;
            }
            self.end = pos + c.len_utf8();
            // All characters are ASCII. Counts hold only the byte distribution.
            self.counts[index] += 1;
            self.length += 1;
            self.hash.update([c as u8]);
        } else {
            self.boundary();
        }
    }

    pub fn boundary(&mut self) {
        self.previous_word = false;
        if self.length >= 20 {
            let entropy: f64 = self
                .counts
                .iter()
                .filter(|&&n| n > 0)
                .map(|&n| {
                    let p = n as f64 / self.length as f64;
                    -p * p.log2()
                })
                .sum();
            if entropy >= 3.0 {
                self.done.push(Finding {
                    pattern: "Generic credential assignment",
                    start: self.start,
                    end: self.end,
                    fingerprint: format!("{:x}", self.hash.clone().finalize()),
                });
            }
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.stage = 0;
        self.label.clear();
        self.length = 0;
        self.counts.fill(0);
        self.hash = Sha256::new();
    }
}

impl Default for Assignment {
    fn default() -> Self {
        Self {
            label: String::new(),
            previous_word: false,
            stage: 0,
            start: 0,
            end: 0,
            counts: [0; 67],
            length: 0,
            hash: Sha256::new(),
            done: Vec::new(),
        }
    }
}
