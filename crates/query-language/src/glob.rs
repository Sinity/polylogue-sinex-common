//! Case-sensitive text patterns with `*` as their only wildcard.

/// Match a whole string. Every character other than `*` is literal, including
/// SQL pattern characters and backslashes. Matching keeps Unicode text intact.
pub fn matches(text: &str, pattern: &str) -> bool {
    let mut parts = pattern.split('*').peekable();
    let Some(mut remaining) = text.strip_prefix(parts.next().unwrap()) else {
        return false;
    };
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            return remaining.ends_with(part);
        }
        let Some(at) = remaining.find(part) else {
            return false;
        };
        remaining = &remaining[at + part.len()..];
    }
    remaining.is_empty()
}
