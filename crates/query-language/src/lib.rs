//! Pure query mechanics shared by product-owned catalogs and compilers.
pub mod ast;
pub mod bind;
pub mod catalog;
pub mod complete;
pub mod cursor;
pub mod format;
pub mod identity;
pub mod laws;
pub mod matches;
pub mod number;
pub mod parse;
pub mod scope;
pub mod semantics;
pub mod time;

use serde::{Deserialize, Serialize};

/// Stable language refusal kinds. Transport mappings belong to command-surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidQuery,
    UnknownField,
    BadValue,
    NotAQuery,
    Ambiguous,
    StaleCursor,
}

impl ErrorKind {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidQuery => "invalid_query",
            Self::UnknownField => "unknown_field",
            Self::BadValue => "bad_value",
            Self::NotAQuery => "not_a_query",
            Self::Ambiguous => "ambiguous",
            Self::StaleCursor => "stale_cursor",
        }
    }
}

/// A byte-positioned refusal, with an executable correction where one exists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Error {
    pub kind: ErrorKind,
    pub message: String,
    pub position: usize,
    pub suggestion: Option<String>,
    pub hint: Option<String>,
    pub retryable: bool,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>, position: usize) -> Self {
        Self {
            kind,
            message: message.into(),
            position,
            suggestion: None,
            hint: None,
            retryable: false,
        }
    }
    pub fn suggestion(mut self, query: impl Into<String>) -> Self {
        self.suggestion = Some(query.into());
        self
    }
    pub fn hint(mut self, text: impl Into<String>) -> Self {
        self.hint = Some(text.into());
        self
    }
    /// Render the source line and a caret at a valid Unicode character boundary.
    pub fn diagnostic(&self, source: &str) -> String {
        let mut offset = self.position.min(source.len());
        while !source.is_char_boundary(offset) {
            offset -= 1;
        }
        let start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
        let end = source[offset..]
            .find('\n')
            .map_or(source.len(), |i| offset + i);
        let column = source[start..offset].chars().count();
        format!(
            "{}\n{}\n{}^",
            self.message,
            &source[start..end],
            " ".repeat(column)
        )
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}
