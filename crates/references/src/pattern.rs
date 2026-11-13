//! URL patterns: declared URL shapes that name one identifier, with the
//! reference text each shape becomes.
//!
//! A pattern is data: a host template, a path template, query parameters and
//! an output. `{name}` captures one host label, one path segment or one query
//! value, with optional literal text before and after it in the same piece
//! (`intl-{locale}`, `{id}.pdf`); `{name*}` as the last path piece captures
//! the rest of the path, slashes included. Captures are inserted into the
//! output as they appear in the URL (still percent-encoded); the output is
//! then parsed as ordinary reference or locator text, which decodes them and
//! checks the identifier. A pattern whose output does not parse does not
//! match.
//!
//! The shared catalog patterns live in [`crate::locator`]; each product
//! declares its own provider-URL patterns in its [`crate::ProductCatalog`]
//! with the same type.

use crate::escape;
use std::collections::BTreeMap;
use url::Url;

/// One declared URL shape and the reference text it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlPattern {
    host: Vec<Piece>,
    path: Vec<Piece>,
    rest: Option<Piece>,
    query: Vec<(String, Piece)>,
    output: String,
    literal_captures: Vec<String>,
}

/// Literal text with at most one capture inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Piece {
    before: String,
    capture: Option<String>,
    after: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PatternError {
    #[error("`{0}` has an unclosed or nested capture")]
    Unbalanced(String),
    #[error("`{0}` has more than one capture in one piece")]
    TwoCaptures(String),
    #[error("the path template `{0}` does not start with `/`")]
    PathNotAbsolute(String),
    #[error("`{{{0}*}}` captures the rest of the path and must be the last path piece")]
    RestNotLast(String),
    #[error("the output `{output}` uses `{{{name}}}`, which the pattern does not capture")]
    UncapturedOutput { output: String, name: String },
    #[error("`{0}` is not a capture declared by this URL pattern")]
    UnknownCapture(String),
    #[error("the pattern captures `{0}` twice")]
    CapturedTwice(String),
}

impl UrlPattern {
    /// Declares a pattern: `host` such as `{lang}.wikipedia.org`, `path` such
    /// as `/wiki/{title*}`, required query parameters such as
    /// `[("v", "{id}")]`, and the output such as `wikipedia:{lang}/{title}`.
    pub fn new(
        host: &str,
        path: &str,
        query: &[(&str, &str)],
        output: &str,
    ) -> Result<Self, PatternError> {
        let host = host
            .split('.')
            .map(|label| Piece::parse(label, false).map(|(piece, _)| piece))
            .collect::<Result<Vec<_>, _>>()?;
        let segments = path
            .strip_prefix('/')
            .ok_or_else(|| PatternError::PathNotAbsolute(path.to_owned()))?;
        let mut pieces = Vec::new();
        let mut rest = None;
        for segment in segments.split('/') {
            if let Some(name) = &rest {
                return Err(PatternError::RestNotLast(String::clone(name)));
            }
            let (piece, is_rest) = Piece::parse(segment, true)?;
            if is_rest {
                rest = piece.capture.clone();
                pieces.push(piece);
            } else {
                pieces.push(piece);
            }
        }
        let rest = rest.map(|_| pieces.pop().expect("the rest piece was pushed"));
        let query = query
            .iter()
            .map(|(key, value)| Ok((key.to_string(), Piece::parse(value, false)?.0)))
            .collect::<Result<Vec<_>, PatternError>>()?;
        let pattern = Self {
            host,
            path: pieces,
            rest,
            query,
            output: output.to_owned(),
            literal_captures: Vec::new(),
        };
        pattern.check_output()?;
        Ok(pattern)
    }

    /// Keep this capture's URL spelling as its identity, including percent
    /// escapes. Other captures retain the ordinary decoded reference meaning.
    pub fn with_literal_capture(mut self, name: &str) -> Result<Self, PatternError> {
        if !self.captures().any(|capture| capture == name) {
            return Err(PatternError::UnknownCapture(name.to_owned()));
        }
        if !self.literal_captures.iter().any(|capture| capture == name) {
            self.literal_captures.push(name.to_owned());
        }
        Ok(self)
    }

    pub(crate) fn has_literal_captures(&self) -> bool {
        !self.literal_captures.is_empty()
    }

    fn captures(&self) -> impl Iterator<Item = &String> {
        self.host
            .iter()
            .chain(&self.path)
            .chain(&self.rest)
            .chain(self.query.iter().map(|(_, piece)| piece))
            .filter_map(|piece| piece.capture.as_ref())
    }

    fn check_output(&self) -> Result<(), PatternError> {
        let mut seen = Vec::new();
        for name in self.captures() {
            if seen.contains(&name) {
                return Err(PatternError::CapturedTwice(name.clone()));
            }
            seen.push(name);
        }
        let mut text = self.output.as_str();
        while let Some(open) = text.find('{') {
            let close = text[open..]
                .find('}')
                .ok_or_else(|| PatternError::Unbalanced(self.output.clone()))?;
            let name = &text[open + 1..open + close];
            if !seen.iter().any(|captured| *captured == name) {
                return Err(PatternError::UncapturedOutput {
                    output: self.output.clone(),
                    name: name.to_owned(),
                });
            }
            text = &text[open + close + 1..];
        }
        Ok(())
    }

    /// The output text with this URL's captures filled in, when the URL has
    /// this pattern's shape. The URL's scheme is not part of the shape.
    pub fn apply(&self, url: &Url) -> Option<String> {
        let mut captured = BTreeMap::new();
        let host = url.host_str()?;
        let labels: Vec<&str> = host.split('.').collect();
        if labels.len() != self.host.len() {
            return None;
        }
        for (piece, label) in self.host.iter().zip(labels) {
            piece.capture_into(label, &mut captured)?;
        }
        let segments: Vec<&str> = url.path().strip_prefix('/')?.split('/').collect();
        match &self.rest {
            None if segments.len() != self.path.len() => return None,
            Some(_) if segments.len() <= self.path.len() => return None,
            _ => {}
        }
        for (piece, segment) in self.path.iter().zip(&segments) {
            piece.capture_into(segment, &mut captured)?;
        }
        let rest_of_path = segments[self.path.len().min(segments.len())..].join("/");
        if let Some(rest) = &self.rest {
            rest.capture_into(&rest_of_path, &mut captured)?;
        }
        let pairs: Vec<(&str, &str)> = url
            .query()
            .unwrap_or("")
            .split('&')
            .map(|pair| pair.split_once('=').unwrap_or((pair, "")))
            .collect();
        for (key, piece) in &self.query {
            let (_, value) = pairs.iter().find(|(name, _)| name == key)?;
            piece.capture_into(value, &mut captured)?;
        }
        // One pass over the template, so a captured value is never itself
        // read as a placeholder.
        let mut output = String::new();
        let mut template = self.output.as_str();
        while let Some(open) = template.find('{') {
            let close = open + template[open..].find('}')?;
            output.push_str(&template[..open]);
            let name = &template[open + 1..close];
            let value = captured.get(name)?;
            if self.literal_captures.iter().any(|capture| capture == name) {
                output.push_str(&escape::encode(value, escape::unreserved));
            } else {
                output.push_str(value);
            }
            template = &template[close + 1..];
        }
        output.push_str(template);
        Some(output)
    }
}

impl Piece {
    /// Parses one piece; the flag says whether it was `{name*}`.
    fn parse(text: &str, allow_rest: bool) -> Result<(Self, bool), PatternError> {
        let Some(open) = text.find('{') else {
            if text.contains('}') {
                return Err(PatternError::Unbalanced(text.to_owned()));
            }
            return Ok((
                Self {
                    before: text.to_owned(),
                    capture: None,
                    after: String::new(),
                },
                false,
            ));
        };
        let close = text[open..]
            .find('}')
            .map(|offset| open + offset)
            .ok_or_else(|| PatternError::Unbalanced(text.to_owned()))?;
        let name = &text[open + 1..close];
        if name.is_empty() || name.contains('{') {
            return Err(PatternError::Unbalanced(text.to_owned()));
        }
        let after = &text[close + 1..];
        if after.contains('{') {
            return Err(PatternError::TwoCaptures(text.to_owned()));
        }
        if after.contains('}') {
            return Err(PatternError::Unbalanced(text.to_owned()));
        }
        let (name, is_rest) = match name.strip_suffix('*') {
            Some(name) if allow_rest => (name, true),
            Some(_) => return Err(PatternError::Unbalanced(text.to_owned())),
            None => (name, false),
        };
        Ok((
            Self {
                before: text[..open].to_owned(),
                capture: Some(name.to_owned()),
                after: after.to_owned(),
            },
            is_rest,
        ))
    }

    fn capture_into<'a>(
        &'a self,
        text: &'a str,
        captured: &mut BTreeMap<&'a str, &'a str>,
    ) -> Option<()> {
        match &self.capture {
            None => (text == self.before).then_some(()),
            Some(name) => {
                let value = text
                    .strip_prefix(self.before.as_str())?
                    .strip_suffix(self.after.as_str())?;
                if value.is_empty() {
                    return None;
                }
                captured.insert(name, value);
                Some(())
            }
        }
    }
}
