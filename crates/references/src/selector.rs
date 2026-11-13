//! Selectors: the part of a reference after `#` that names a place inside the
//! thing referenced, including its recorded member ordinal.
//!
//! The parts are W3C Media Fragments (`t=` time, `xywh=` rectangle), line ranges
//! (`L40-52`), text ranges (`b=`, `c=`, `u16=`, the W3C text position), the
//! W3C Web Annotation text quote (`exact=`, `prefix=`, `suffix=`) and a named
//! place (`id=`, the Media Fragments name dimension, which also keeps a web
//! page's own `#section`). Parts are joined with `&` and print in one
//! order, so every selector has one printed form.

use crate::escape;
use std::fmt;
use std::ops::Range;
use std::str::FromStr;

/// A place inside a referenced thing. At least one part is present in a
/// parsed selector; a reference never holds an empty one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct Selector {
    /// A recorded member ordinal, counted from zero; never a database row ID.
    pub ordinal: Option<u64>,
    pub lines: Option<LineRange>,
    pub time: Option<TimeRange>,
    pub rectangle: Option<Rectangle>,
    pub text_range: Option<TextRange>,
    pub quote: Option<TextQuote>,
    /// A place the document itself names, kept as written: a web page's
    /// element id from its URL fragment (`#section-2`), a media chapter.
    /// `#section-2`).
    pub id: Option<String>,
}

/// Lines `first` to `last`, both included, counted from 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LineRange {
    first: u64,
    last: u64,
}

/// A media time range in seconds. Without an end it runs to the end of the
/// media.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TimeRange {
    start: Seconds,
    end: Option<Seconds>,
}

/// A non-negative number of seconds with any number of fraction digits,
/// kept exactly as a decimal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Seconds {
    whole: u64,
    /// Fraction digits without trailing zeros, so equal values are equal
    /// strings and the derived order is numeric order.
    fraction: String,
}

/// A rectangle in pixels or in percent of the media's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rectangle {
    unit: RectangleUnit,
    x: u64,
    y: u64,
    width: u64,
    height: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RectangleUnit {
    Pixel,
    Percent,
}

/// A half-open text range `[start, end)` counted in one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextRange {
    unit: TextUnit,
    start: u64,
    end: u64,
}

/// What a text range counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextUnit {
    /// UTF-8 bytes (`b=`).
    Bytes,
    /// Unicode code points (`c=`), as the W3C text position selector counts them.
    CodePoints,
    /// UTF-16 code units (`u16=`), as browser selections report them.
    Utf16,
}

/// The exact text a span held, with the text just before and after it, so the
/// span can be found again after its container changes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextQuote {
    exact: String,
    prefix: String,
    suffix: String,
}

/// What is known about the thing a selector points into. An unknown bound
/// is not checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bounds {
    pub duration: Option<Seconds>,
    pub width: Option<u64>,
    pub height: Option<u64>,
    pub lines: Option<u64>,
    pub text: Option<TextLength>,
}

/// A text's length in each unit a text range can count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLength {
    pub bytes: u64,
    pub code_points: u64,
    pub utf16: u64,
}

/// Where a quoted span is in a container's current text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpanState {
    /// The stored text range still holds the quoted text.
    Current(Range<usize>),
    /// The quote, with its prefix and suffix, occurs exactly once, at this
    /// byte range; the span is shown there, labelled re-anchored.
    Moved(Range<usize>),
    /// No unique exact match: the text the span named has changed.
    ChangedSince,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SelectorError {
    #[error("the selector is empty")]
    Empty,
    #[error(
        "`{0}` is not a selector part; use ordinal=, L<line>[-<line>], t=, xywh=, b=, c=, u16=, exact=, prefix=, suffix= or id="
    )]
    UnknownPart(String),
    #[error("the selector names `{0}` twice")]
    Repeated(&'static str),
    #[error("`{value}` is not a valid {part} selector; expected {expected}")]
    Malformed {
        part: &'static str,
        value: String,
        expected: &'static str,
    },
    #[error("prefix= and suffix= need exact= beside them")]
    ContextWithoutExact,
    #[error("exact= is empty; a quote names at least one character")]
    EmptyQuote,
    #[error("the {0} selector's escapes do not decode to UTF-8 text")]
    NotUtf8(&'static str),
    #[error("{part} {detail}")]
    OutOfBounds { part: &'static str, detail: String },
}

impl LineRange {
    pub fn new(first: u64, last: u64) -> Result<Self, SelectorError> {
        if first == 0 || last < first {
            return Err(malformed(
                "line",
                format!("L{first}-{last}"),
                LINES_EXPECTED,
            ));
        }
        Ok(Self { first, last })
    }
    pub fn first(&self) -> u64 {
        self.first
    }
    pub fn last(&self) -> u64 {
        self.last
    }
}

impl TimeRange {
    pub fn new(start: Seconds, end: Option<Seconds>) -> Result<Self, SelectorError> {
        if let Some(end) = &end
            && *end <= start
        {
            return Err(malformed("time", format!("t={start},{end}"), TIME_EXPECTED));
        }
        Ok(Self { start, end })
    }
    pub fn start(&self) -> &Seconds {
        &self.start
    }
    pub fn end(&self) -> Option<&Seconds> {
        self.end.as_ref()
    }
}

impl Seconds {
    /// `whole` seconds plus the decimal `fraction` digits (`"5"` is half a
    /// second). Trailing zeros are dropped.
    pub fn new(whole: u64, fraction: &str) -> Result<Self, SelectorError> {
        if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(malformed(
                "time",
                format!("{whole}.{fraction}"),
                TIME_EXPECTED,
            ));
        }
        Ok(Self {
            whole,
            fraction: fraction.trim_end_matches('0').to_owned(),
        })
    }
    pub fn whole(&self) -> u64 {
        self.whole
    }
    /// The fraction digits, without trailing zeros.
    pub fn fraction(&self) -> &str {
        &self.fraction
    }
}

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.whole)?;
        if !self.fraction.is_empty() {
            write!(f, ".{}", self.fraction)?;
        }
        Ok(())
    }
}

impl FromStr for Seconds {
    type Err = SelectorError;
    /// Normal play time: seconds (`90`, `90.5`) or a clock (`1:30`,
    /// `0:01:30.5`), with minutes and seconds below 60.
    fn from_str(text: &str) -> Result<Self, SelectorError> {
        let bad = || malformed("time", text.to_owned(), TIME_EXPECTED);
        let (clock, fraction) = match text.split_once('.') {
            Some((clock, fraction)) => (clock, fraction),
            None => (text, ""),
        };
        let fields: Vec<&str> = clock.split(':').collect();
        let number = |field: &str| digits(field).ok_or_else(bad);
        let sixty = |field: &str| match field.len() == 2 {
            true => number(field).and_then(|value| if value < 60 { Ok(value) } else { Err(bad()) }),
            false => Err(bad()),
        };
        let whole = match fields.as_slice() {
            [seconds] => number(seconds)?,
            [minutes, seconds] => sixty(minutes)?
                .checked_mul(60)
                .and_then(|minutes| minutes.checked_add(sixty(seconds).ok()?))
                .ok_or_else(bad)?,
            [hours, minutes, seconds] => number(hours)?
                .checked_mul(3600)
                .and_then(|total| total.checked_add(sixty(minutes).ok()? * 60))
                .and_then(|total| total.checked_add(sixty(seconds).ok()?))
                .ok_or_else(bad)?,
            _ => return Err(bad()),
        };
        Seconds::new(whole, fraction).map_err(|_| bad())
    }
}

impl Rectangle {
    pub fn new(
        unit: RectangleUnit,
        x: u64,
        y: u64,
        width: u64,
        height: u64,
    ) -> Result<Self, SelectorError> {
        let printed = || format!("{x},{y},{width},{height}");
        if width == 0 || height == 0 {
            return Err(malformed("rectangle", printed(), RECTANGLE_EXPECTED));
        }
        if unit == RectangleUnit::Percent && (exceeds(x, width, 100) || exceeds(y, height, 100)) {
            return Err(SelectorError::OutOfBounds {
                part: "rectangle",
                detail: format!("percent:{} reaches past 100%", printed()),
            });
        }
        Ok(Self {
            unit,
            x,
            y,
            width,
            height,
        })
    }
    pub fn unit(&self) -> RectangleUnit {
        self.unit
    }
    pub fn x(&self) -> u64 {
        self.x
    }
    pub fn y(&self) -> u64 {
        self.y
    }
    pub fn width(&self) -> u64 {
        self.width
    }
    pub fn height(&self) -> u64 {
        self.height
    }
}

impl TextRange {
    pub fn new(unit: TextUnit, start: u64, end: u64) -> Result<Self, SelectorError> {
        if end < start {
            return Err(malformed(
                unit.name(),
                format!("{start}-{end}"),
                TEXT_RANGE_EXPECTED,
            ));
        }
        Ok(Self { unit, start, end })
    }
    pub fn unit(&self) -> TextUnit {
        self.unit
    }
    pub fn start(&self) -> u64 {
        self.start
    }
    pub fn end(&self) -> u64 {
        self.end
    }

    /// The byte range this text range names in `text`, when both ends fall on
    /// character boundaries inside it.
    pub fn byte_range(&self, text: &str) -> Option<Range<usize>> {
        let start = byte_offset(text, self.unit, self.start)?;
        let end = byte_offset(text, self.unit, self.end)?;
        Some(start..end)
    }
}

impl TextUnit {
    pub fn name(self) -> &'static str {
        match self {
            Self::Bytes => "b",
            Self::CodePoints => "c",
            Self::Utf16 => "u16",
        }
    }
    fn from_name(name: &str) -> Option<Self> {
        match name {
            "b" => Some(Self::Bytes),
            "c" => Some(Self::CodePoints),
            "u16" => Some(Self::Utf16),
            _ => None,
        }
    }
}

fn byte_offset(text: &str, unit: TextUnit, offset: u64) -> Option<usize> {
    let offset = usize::try_from(offset).ok()?;
    match unit {
        TextUnit::Bytes => text.is_char_boundary(offset).then_some(offset),
        TextUnit::CodePoints => text
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(text.len()))
            .nth(offset),
        TextUnit::Utf16 => {
            let mut counted = 0;
            for (index, c) in text.char_indices() {
                if counted == offset {
                    return Some(index);
                }
                if counted > offset {
                    return None;
                }
                counted += c.len_utf16();
            }
            (counted == offset).then_some(text.len())
        }
    }
}

impl TextQuote {
    /// A quote of `exact` with the text before and after it. `exact` is not
    /// empty.
    pub fn new(exact: &str, prefix: &str, suffix: &str) -> Result<Self, SelectorError> {
        if exact.is_empty() {
            return Err(SelectorError::EmptyQuote);
        }
        Ok(Self {
            exact: exact.to_owned(),
            prefix: prefix.to_owned(),
            suffix: suffix.to_owned(),
        })
    }

    /// The quote of `text[range]` with up to `context` characters before
    /// and after it. `None` when the range is empty or not on character
    /// boundaries.
    pub fn around(text: &str, range: Range<usize>, context: usize) -> Option<Self> {
        let exact = text.get(range.clone()).filter(|exact| !exact.is_empty())?;
        let before = &text[..range.start];
        let after = &text[range.end..];
        let prefix_start = before
            .char_indices()
            .rev()
            .take(context)
            .last()
            .map_or(before.len(), |(index, _)| index);
        let suffix_end = after
            .char_indices()
            .nth(context)
            .map_or(after.len(), |(index, _)| index);
        Some(Self {
            exact: exact.to_owned(),
            prefix: before[prefix_start..].to_owned(),
            suffix: after[..suffix_end].to_owned(),
        })
    }

    pub fn exact(&self) -> &str {
        &self.exact
    }
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
    pub fn suffix(&self) -> &str {
        &self.suffix
    }

    /// The byte range of `exact` when prefix, exact and suffix together
    /// occur exactly once in `text`.
    pub fn find_unique(&self, text: &str) -> Option<Range<usize>> {
        let whole = format!("{}{}{}", self.prefix, self.exact, self.suffix);
        let mut found = None;
        let mut from = 0;
        while let Some(offset) = text[from..].find(&whole) {
            let at = from + offset;
            if found.is_some() {
                return None;
            }
            found = Some(at);
            // Overlapping occurrences count: continue one character later.
            from = at + text[at..].chars().next().map_or(1, char::len_utf8);
        }
        let at = found? + self.prefix.len();
        Some(at..at + self.exact.len())
    }
}

impl Bounds {
    /// The line count and text lengths of `text`. Lines are separated by
    /// `\n`; a final newline ends the last line rather than starting another.
    pub fn of_text(text: &str) -> Self {
        let newlines = text.bytes().filter(|&b| b == b'\n').count() as u64;
        let lines = match text {
            "" => 0,
            _ if text.ends_with('\n') => newlines,
            _ => newlines + 1,
        };
        Self {
            lines: Some(lines),
            text: Some(TextLength {
                bytes: text.len() as u64,
                code_points: text.chars().count() as u64,
                utf16: text
                    .chars()
                    .map(|character| character.len_utf16() as u64)
                    .sum(),
            }),
            ..Self::default()
        }
    }
}

impl Selector {
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }

    /// Combine distinct selector parts without replacing either selection.
    pub fn merge(mut self, other: Self) -> Result<Self, SelectorError> {
        fn part<T>(
            to: &mut Option<T>,
            from: Option<T>,
            name: &'static str,
        ) -> Result<(), SelectorError> {
            if let Some(value) = from {
                set(to, name, value)?;
            }
            Ok(())
        }
        part(&mut self.ordinal, other.ordinal, "ordinal")?;
        part(&mut self.lines, other.lines, "L")?;
        part(&mut self.time, other.time, "t")?;
        part(&mut self.rectangle, other.rectangle, "xywh")?;
        part(&mut self.text_range, other.text_range, "text range")?;
        part(&mut self.quote, other.quote, "exact")?;
        part(&mut self.id, other.id, "id")?;
        Ok(self)
    }

    /// Checks every part against the known bounds. A part past a bound is
    /// invalid; nothing is clamped.
    pub fn check(&self, bounds: &Bounds) -> Result<(), SelectorError> {
        let out =
            |part: &'static str, detail: String| Err(SelectorError::OutOfBounds { part, detail });
        if let (Some(lines), Some(count)) = (&self.lines, bounds.lines)
            && lines.last > count
        {
            return out(
                "lines",
                format!(
                    "L{}-{} pass the last line, {count}",
                    lines.first, lines.last
                ),
            );
        }
        if let (Some(time), Some(duration)) = (&self.time, &bounds.duration) {
            let past = match &time.end {
                Some(end) => end > duration,
                None => &time.start >= duration,
            };
            if past {
                return out(
                    "time",
                    format!("{} passes the duration, {duration}s", print_time(time)),
                );
            }
        }
        if let Some(rectangle) = &self.rectangle
            && rectangle.unit == RectangleUnit::Pixel
        {
            if let Some(width) = bounds.width
                && exceeds(rectangle.x, rectangle.width, width)
            {
                return out(
                    "rectangle",
                    format!(
                        "x {} + width {} passes the width, {width}",
                        rectangle.x, rectangle.width
                    ),
                );
            }
            if let Some(height) = bounds.height
                && exceeds(rectangle.y, rectangle.height, height)
            {
                return out(
                    "rectangle",
                    format!(
                        "y {} + height {} passes the height, {height}",
                        rectangle.y, rectangle.height
                    ),
                );
            }
        }
        if let (Some(range), Some(length)) = (&self.text_range, &bounds.text) {
            let available = match range.unit {
                TextUnit::Bytes => length.bytes,
                TextUnit::CodePoints => length.code_points,
                TextUnit::Utf16 => length.utf16,
            };
            if range.end > available {
                return out(
                    "text range",
                    format!(
                        "{}={}-{} passes the text's length, {available}",
                        range.unit.name(),
                        range.start,
                        range.end
                    ),
                );
            }
        }
        Ok(())
    }

    /// Finds the selected span in a container's current text: at the stored
    /// text range when it still holds the quote, else by a unique quote match.
    /// `None` when the selector has neither a quote nor a text range.
    pub fn span_in(&self, text: &str) -> Option<SpanState> {
        let at_range = self
            .text_range
            .as_ref()
            .and_then(|range| range.byte_range(text));
        Some(match (&self.quote, at_range) {
            (None, None) if self.text_range.is_none() => return None,
            (None, Some(range)) => SpanState::Current(range),
            (None, None) => SpanState::ChangedSince,
            (Some(quote), at_range) => {
                if let Some(range) = at_range
                    && text[range.clone()] == quote.exact
                {
                    SpanState::Current(range)
                } else if let Some(range) = quote.find_unique(text) {
                    SpanState::Moved(range)
                } else {
                    SpanState::ChangedSince
                }
            }
        })
    }

    /// Parses selector text (without the `#`).
    pub fn parse(text: &str) -> Result<Self, SelectorError> {
        if text.is_empty() {
            return Err(SelectorError::Empty);
        }
        let mut selector = Self::default();
        let mut exact = None;
        let mut prefix = None;
        let mut suffix = None;
        for part in text.split('&') {
            if let Some(lines) = part.strip_prefix('L')
                && lines.starts_with(|c: char| c.is_ascii_digit())
            {
                set(&mut selector.lines, "L", parse_lines(lines)?)?;
                continue;
            }
            let Some((name, value)) = part.split_once('=') else {
                return Err(SelectorError::UnknownPart(part.to_owned()));
            };
            match name {
                "ordinal" => set(&mut selector.ordinal, "ordinal", parse_ordinal(value)?)?,
                "t" => set(&mut selector.time, "t", parse_time(value)?)?,
                "xywh" => set(&mut selector.rectangle, "xywh", parse_rectangle(value)?)?,
                "exact" => set(&mut exact, "exact", decode("exact", value)?)?,
                "prefix" => set(&mut prefix, "prefix", decode("prefix", value)?)?,
                "suffix" => set(&mut suffix, "suffix", decode("suffix", value)?)?,
                "id" => set(&mut selector.id, "id", decode("id", value)?)?,
                _ => match TextUnit::from_name(name) {
                    Some(unit) => {
                        if selector.text_range.is_some() {
                            return Err(SelectorError::Repeated("text range"));
                        }
                        selector.text_range = Some(parse_text_range(unit, value)?);
                    }
                    None => return Err(SelectorError::UnknownPart(part.to_owned())),
                },
            }
        }
        selector.quote = match exact {
            Some(exact) => Some(TextQuote::new(
                &exact,
                prefix.as_deref().unwrap_or(""),
                suffix.as_deref().unwrap_or(""),
            )?),
            None if prefix.is_some() || suffix.is_some() => {
                return Err(SelectorError::ContextWithoutExact);
            }
            None => None,
        };
        if selector.id.as_deref() == Some("") {
            return Err(malformed("id", String::new(), "a non-empty name"));
        }
        Ok(selector)
    }
}

impl FromStr for Selector {
    type Err = SelectorError;
    fn from_str(text: &str) -> Result<Self, SelectorError> {
        Self::parse(text)
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if let Some(ordinal) = self.ordinal {
            parts.push(format!("ordinal={ordinal}"));
        }
        if let Some(lines) = &self.lines {
            parts.push(match lines.first == lines.last {
                true => format!("L{}", lines.first),
                false => format!("L{}-{}", lines.first, lines.last),
            });
        }
        if let Some(time) = &self.time {
            parts.push(print_time(time));
        }
        if let Some(rectangle) = &self.rectangle {
            let unit = match rectangle.unit {
                RectangleUnit::Pixel => "",
                RectangleUnit::Percent => "percent:",
            };
            parts.push(format!(
                "xywh={unit}{},{},{},{}",
                rectangle.x, rectangle.y, rectangle.width, rectangle.height
            ));
        }
        if let Some(range) = &self.text_range {
            parts.push(format!(
                "{}={}-{}",
                range.unit.name(),
                range.start,
                range.end
            ));
        }
        if let Some(quote) = &self.quote {
            parts.push(format!(
                "exact={}",
                escape::encode(&quote.exact, escape::unreserved)
            ));
            if !quote.prefix.is_empty() {
                parts.push(format!(
                    "prefix={}",
                    escape::encode(&quote.prefix, escape::unreserved)
                ));
            }
            if !quote.suffix.is_empty() {
                parts.push(format!(
                    "suffix={}",
                    escape::encode(&quote.suffix, escape::unreserved)
                ));
            }
        }
        if let Some(id) = &self.id {
            parts.push(format!("id={}", escape::encode(id, escape::unreserved)));
        }
        f.write_str(&parts.join("&"))
    }
}

const LINES_EXPECTED: &str = "L<first>[-<last>] with 1 <= first <= last";
const TIME_EXPECTED: &str = "t=[npt:]<start>[,<end>] in seconds or h:mm:ss, with start before end";
const RECTANGLE_EXPECTED: &str =
    "xywh=[pixel:|percent:]<x>,<y>,<width>,<height> with width and height above 0";
const TEXT_RANGE_EXPECTED: &str = "<start>-<end> with start <= end";

pub(crate) fn parse_ordinal(text: &str) -> Result<u64, SelectorError> {
    if !text.is_empty()
        && (text.len() == 1 || !text.starts_with('0'))
        && let Some(value) = digits(text)
    {
        return Ok(value);
    }
    Err(malformed(
        "ordinal",
        text.to_owned(),
        "a nonnegative integer without leading zeros",
    ))
}

fn malformed(part: &'static str, value: String, expected: &'static str) -> SelectorError {
    SelectorError::Malformed {
        part,
        value,
        expected,
    }
}

fn set<T>(slot: &mut Option<T>, name: &'static str, value: T) -> Result<(), SelectorError> {
    if slot.is_some() {
        return Err(SelectorError::Repeated(name));
    }
    *slot = Some(value);
    Ok(())
}

fn exceeds(offset: u64, size: u64, limit: u64) -> bool {
    offset.checked_add(size).is_none_or(|end| end > limit)
}

/// An unsigned decimal number of ASCII digits only.
fn digits(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn decode(part: &'static str, value: &str) -> Result<String, SelectorError> {
    escape::decode(value).map_err(|_| SelectorError::NotUtf8(part))
}

fn parse_lines(text: &str) -> Result<LineRange, SelectorError> {
    let bad = || malformed("line", format!("L{text}"), LINES_EXPECTED);
    let (first, last) = match text.split_once('-') {
        Some((first, last)) => (first, last.strip_prefix('L').unwrap_or(last)),
        None => (text, text),
    };
    LineRange::new(
        digits(first).ok_or_else(bad)?,
        digits(last).ok_or_else(bad)?,
    )
    .map_err(|_| bad())
}

fn parse_time(value: &str) -> Result<TimeRange, SelectorError> {
    let bad = || malformed("time", format!("t={value}"), TIME_EXPECTED);
    let range = value.strip_prefix("npt:").unwrap_or(value);
    let (start, end) = match range.split_once(',') {
        Some((start, end)) => (start, Some(end)),
        None => (range, None),
    };
    let start = match start {
        "" if end.is_some() => Seconds::new(0, "")?,
        _ => start.parse().map_err(|_| bad())?,
    };
    let end = end
        .map(|end| end.parse::<Seconds>().map_err(|_| bad()))
        .transpose()?;
    TimeRange::new(start, end).map_err(|_| bad())
}

fn print_time(time: &TimeRange) -> String {
    match &time.end {
        Some(end) => format!("t={},{end}", time.start),
        None => format!("t={}", time.start),
    }
}

fn parse_rectangle(value: &str) -> Result<Rectangle, SelectorError> {
    let bad = || malformed("rectangle", format!("xywh={value}"), RECTANGLE_EXPECTED);
    let (unit, numbers) = if let Some(rest) = value.strip_prefix("percent:") {
        (RectangleUnit::Percent, rest)
    } else {
        (
            RectangleUnit::Pixel,
            value.strip_prefix("pixel:").unwrap_or(value),
        )
    };
    let numbers: Vec<u64> = numbers
        .split(',')
        .map(|number| digits(number).ok_or_else(bad))
        .collect::<Result<_, _>>()?;
    let [x, y, width, height] = numbers[..] else {
        return Err(bad());
    };
    Rectangle::new(unit, x, y, width, height)
}

fn parse_text_range(unit: TextUnit, value: &str) -> Result<TextRange, SelectorError> {
    let bad = || malformed(unit.name(), value.to_owned(), TEXT_RANGE_EXPECTED);
    let (start, end) = value.split_once('-').ok_or_else(bad)?;
    TextRange::new(
        unit,
        digits(start).ok_or_else(bad)?,
        digits(end).ok_or_else(bad)?,
    )
}
