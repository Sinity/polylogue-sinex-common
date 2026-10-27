//! Generic assignments use entropy because their labels alone are ambiguous.
use super::{Finding, Fingerprint};
use hmac::Mac;

pub(super) struct Assignment {
    label: String,
    previous_word: bool,
    stage: Stage,
    quote: Option<char>,
    key: Fingerprint,
    start: usize,
    end: usize,
    counts: [u64; 67],
    length: u64,
    hash: Fingerprint,
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

#[derive(Clone, Copy)]
enum Stage {
    Label,
    Separator,
    ValueStart,
    Value,
    SkipLabel,
}

impl Assignment {
    pub fn step(&mut self, c: char, pos: usize) {
        self.character(c, pos);
        self.previous_word = crate::word_ranges::is_word_char(c)
            && !(c == '_' && matches!(self.stage, Stage::Label) && self.label.is_empty());
    }

    fn character(&mut self, c: char, pos: usize) {
        match self.stage {
            Stage::Label => {
                if self.label.is_empty() && self.previous_word {
                    return;
                }
                if self.label.is_empty() && matches!(c, '\'' | '"') {
                    self.quote = Some(c);
                } else if crate::word_ranges::is_word_char(c) || c == '-' {
                    self.label.push(c.to_ascii_lowercase());
                    if !LABELS.iter().any(|label| label.starts_with(&self.label)) {
                        self.label.clear();
                        self.stage = if matches!(c, '_' | '-') {
                            Stage::Label
                        } else {
                            Stage::SkipLabel
                        };
                    }
                } else if LABELS.contains(&self.label.as_str()) {
                    if c == '=' || c == ':' {
                        self.stage = Stage::ValueStart;
                    } else if c.is_whitespace() || Some(c) == self.quote {
                        self.stage = Stage::Separator;
                    } else {
                        self.reset();
                    }
                    self.label.clear();
                } else {
                    self.reset();
                }
            }
            Stage::Separator => {
                if matches!(c, ':' | '=') {
                    self.stage = Stage::ValueStart;
                } else if !c.is_whitespace() {
                    self.reset();
                    self.character(c, pos);
                }
            }
            Stage::ValueStart => {
                if c.is_whitespace() {
                    return;
                }
                self.stage = Stage::Value;
                if matches!(c, '\'' | '"') {
                    return;
                }
                self.value(c, pos);
            }
            Stage::Value => self.value(c, pos),
            Stage::SkipLabel => {
                if !crate::word_ranges::is_word_char(c) || matches!(c, '_' | '-') {
                    let quote = self.quote;
                    self.reset();
                    if matches!(c, '_' | '-') {
                        self.quote = quote;
                    }
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
            self.hash.update(&[c as u8]);
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
                    fingerprint: format!("{:x}", self.hash.clone().finalize().into_bytes()),
                });
            }
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.stage = Stage::Label;
        self.quote = None;
        self.label.clear();
        self.length = 0;
        self.counts.fill(0);
        self.hash = self.key.clone();
    }
}

impl Assignment {
    pub fn new(key: Fingerprint) -> Self {
        Self {
            label: String::new(),
            previous_word: false,
            stage: Stage::Label,
            quote: None,
            key: key.clone(),
            start: 0,
            end: 0,
            counts: [0; 67],
            length: 0,
            hash: key,
            done: Vec::new(),
        }
    }
}
