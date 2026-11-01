//! Percent-encoding with the character sets the reference grammar uses.
//!
//! Every printed form encodes only the characters that would end its part
//! (`@`, `#`, `%`, whitespace and control characters in a reference; anything
//! but unreserved characters in a selector value or a URL path segment).
//! Decoding is lenient about a `%` that does not start an escape: it stays a
//! literal `%`, and printing encodes it as `%25`, so every input has one
//! printed form and nothing typed is refused for it.

/// Characters a reference's identity or revision may hold unencoded.
pub(crate) fn plain_in_reference(c: char) -> bool {
    !matches!(c, '%' | '@' | '#') && !c.is_whitespace() && !c.is_control()
}

/// Characters a revision may hold unencoded (an `@` inside it is plain).
pub(crate) fn plain_in_revision(c: char) -> bool {
    !matches!(c, '%' | '#') && !c.is_whitespace() && !c.is_control()
}

/// Characters a locator value may hold unencoded. Locators take no revision,
/// so `@` stays plain (`node_modules/@types`, `medium.com/@name`).
pub(crate) fn plain_in_locator(c: char) -> bool {
    !matches!(c, '%' | '#') && !c.is_whitespace() && !c.is_control()
}

/// RFC 3986 unreserved characters: the only ones a selector value keeps.
pub(crate) fn unreserved(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~')
}

/// Characters a URL path segment keeps: unreserved, sub-delimiters and `:`.
/// `/`, `@`, `?`, `#` and `%` are encoded, so an identity is one segment and
/// a revision starts at the first literal `@`.
pub(crate) fn plain_in_path_segment(c: char) -> bool {
    unreserved(c)
        || matches!(
            c,
            '!' | '$' | '&' | '\'' | '(' | ')' | '*' | '+' | ',' | ';' | '=' | ':'
        )
}

pub(crate) fn encode(text: &str, plain: fn(char) -> bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if plain(c) {
            out.push(c);
        } else {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                out.push('%');
                out.push(hex_digit(byte >> 4));
                out.push(hex_digit(byte & 0xf));
            }
        }
    }
    out
}

fn hex_digit(value: u8) -> char {
    char::from_digit(u32::from(value), 16)
        .expect("a nibble is a hex digit")
        .to_ascii_uppercase()
}

fn hex_value(byte: u8) -> Option<u8> {
    (byte as char).to_digit(16).map(|digit| digit as u8)
}

/// Decodes `%XX` escapes. A `%` not followed by two hex digits is a literal
/// `%`. Fails only when the decoded bytes are not UTF-8.
pub(crate) fn decode(text: &str) -> Result<String, NotUtf8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| NotUtf8)
}

/// Decoded percent escapes that do not form UTF-8 text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NotUtf8;
