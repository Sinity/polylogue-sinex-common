//! Law cases that both products run against their own keying paths.
//!
//! [`shared_locator_cases`] is the pinned table behind the
//! `locator-shared-module` law: every input with the canonical locator the
//! canonicalizer at [`crate::locator::VERSION`] gives it. Common's test checks
//! the canonicalizer against it; each product checks that the path by which
//! it keys URLs, files and catalog identifiers yields the same text. Changing
//! any output means raising the version and the table together.

use std::sync::LazyLock;

/// The pinned canonicalizer cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatorCases {
    /// The canonicalizer version the table was written for.
    pub version: u32,
    pub cases: Vec<LocatorCase>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocatorCase {
    /// An address as a product meets it: a URL, or locator text.
    pub input: String,
    /// The canonical locator text with its selector, or `None` when the
    /// input is refused.
    pub canonical: Option<String>,
}

static CASES: LazyLock<LocatorCases> =
    LazyLock::new(|| parse(include_str!("laws/locator-cases.txt")));

pub fn shared_locator_cases() -> &'static LocatorCases {
    &CASES
}

/// The table's format: a `version N` line, then one case per line as
/// `<input> TAB <canonical or "refused">`. Blank lines and lines starting
/// with `#` are notes.
fn parse(table: &str) -> LocatorCases {
    let mut lines = table
        .lines()
        .filter(|line| !line.trim().is_empty() && !line.starts_with('#'));
    let version = lines
        .next()
        .and_then(|line| line.strip_prefix("version "))
        .and_then(|number| number.trim().parse().ok())
        .expect("the case table starts with `version N`");
    let cases = lines
        .map(|line| {
            let (input, canonical) = line
                .split_once('\t')
                .unwrap_or_else(|| panic!("case line without a tab: {line:?}"));
            LocatorCase {
                input: input.to_owned(),
                canonical: (canonical != "refused").then(|| canonical.to_owned()),
            }
        })
        .collect();
    LocatorCases { version, cases }
}
