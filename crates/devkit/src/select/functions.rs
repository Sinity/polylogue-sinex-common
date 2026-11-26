//! Changes confined to test functions need only those tests. Every changed
//! line must belong to a test in both versions; helpers keep the file's
//! ordinary selection.
use super::*;
use syn::spanned::Spanned;

struct TestFunction {
    name: String,
    start: usize,
    end: usize,
}

fn test_functions(source: &str) -> Option<Vec<TestFunction>> {
    fn collect(source: &str, items: &[syn::Item], tests: &mut Vec<TestFunction>) {
        for item in items {
            match item {
                syn::Item::Fn(function)
                    if function.attrs.iter().any(|attr| {
                        let path = attr.path();
                        path.is_ident("test")
                            || (path.segments.len() == 2
                                && path.segments.first().is_some_and(|part| {
                                    part.ident == "devkit" || part.ident == "tokio"
                                })
                                && path
                                    .segments
                                    .last()
                                    .is_some_and(|part| part.ident == "test"))
                    }) =>
                {
                    let start = function
                        .attrs
                        .first()
                        .map_or(function.span().start(), |attr| attr.span().start());
                    let end = function.span().end();
                    let prefix = source
                        .lines()
                        .nth(start.line - 1)
                        .and_then(|line| line.get(..start.column));
                    let suffix = source
                        .lines()
                        .nth(end.line - 1)
                        .and_then(|line| line.get(end.column..));
                    // A diff describes whole lines. Other items sharing a line
                    // cannot be attributed to this test alone.
                    if !prefix.is_some_and(|text| text.trim().is_empty())
                        || !suffix.is_some_and(|text| text.trim().is_empty())
                    {
                        continue;
                    }
                    tests.push(TestFunction {
                        name: function
                            .sig
                            .ident
                            .to_string()
                            .trim_start_matches("r#")
                            .to_owned(),
                        start: start.line,
                        end: end.line,
                    });
                }
                syn::Item::Mod(module) => {
                    if let Some((_, items)) = &module.content {
                        collect(source, items, tests);
                    }
                }
                _ => {}
            }
        }
    }
    let parsed = syn::parse_file(source).ok()?;
    let mut tests = Vec::new();
    collect(source, &parsed.items, &mut tests);
    Some(tests)
}

fn range(text: &str) -> Option<(usize, usize)> {
    let (line, count) = text.split_once(',').unwrap_or((text, "1"));
    Some((line.parse().ok()?, count.parse().ok()?))
}

/// A partial overlap never covers a hunk. Insertions and deletions need
/// changed test lines in both versions to avoid narrowing away removed code.
fn covering(tests: &[TestFunction], start: usize, count: usize) -> Option<&TestFunction> {
    if count == 0 {
        return None;
    }
    let end = start.checked_add(count.checked_sub(1)?)?;
    tests
        .iter()
        .find(|test| test.start <= start && test.end >= end)
}

fn names_for_diff(before: &str, after: &str, diff: &str) -> Option<BTreeSet<String>> {
    let before = test_functions(before)?;
    let after = test_functions(after)?;
    let mut names = BTreeSet::new();
    for line in diff.lines().filter(|line| line.starts_with("@@ ")) {
        let mut fields = line.split_whitespace();
        fields.next()?;
        let old = range(fields.next()?.strip_prefix('-')?)?;
        let new = range(fields.next()?.strip_prefix('+')?)?;
        covering(&before, old.0, old.1)?;
        names.insert(covering(&after, new.0, new.1)?.name.clone());
    }
    (!names.is_empty()).then_some(names)
}

/// The tests a change to `file` is confined to, when every changed line is
/// inside a test in both versions and no test's name has another use in the
/// package. `None` keeps the file's ordinary selection.
pub(super) fn changed_tests(
    root: &Path,
    base: &str,
    file: &str,
    package: &Package,
) -> Option<BTreeSet<String>> {
    let path = root.join(file);
    if path.extension().is_none_or(|extension| extension != "rs") {
        return None;
    }
    let before = git(root, &["show", &format!("{base}:{file}")]).ok()?;
    let after = std::fs::read_to_string(&path).ok()?;
    let diff = git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--unified=0",
            base,
            "--",
            file,
        ],
    )
    .ok()?;
    names_for_diff(&before, &after, &diff)
        .filter(|names| used_only_in_declarations(&package.directory, names).unwrap_or(false))
}

/// Calls `visit` with every token, groups' contents included.
pub(super) fn each_token(
    tokens: proc_macro2::TokenStream,
    visit: &mut impl FnMut(&proc_macro2::TokenTree),
) {
    for token in tokens {
        visit(&token);
        if let proc_macro2::TokenTree::Group(group) = &token {
            each_token(group.stream(), visit);
        }
    }
}

/// Calls `visit` with the tokens of every Rust source under `directory`,
/// skipping `target` and `.git`. `None` when a source cannot be read or
/// tokenized, or a symlink is found.
pub(super) fn each_source(
    directory: &Path,
    visit: &mut impl FnMut(proc_macro2::TokenStream),
) -> Option<()> {
    for entry in std::fs::read_dir(directory).ok()? {
        let entry = entry.ok()?;
        if entry.file_name() == "target" || entry.file_name() == ".git" {
            continue;
        }
        let path = entry.path();
        let kind = entry.file_type().ok()?;
        if kind.is_symlink() {
            return None;
        }
        if kind.is_dir() {
            each_source(&path, visit)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            let source = std::fs::read_to_string(path).ok()?;
            visit(source.parse::<proc_macro2::TokenStream>().ok()?);
        }
    }
    Some(())
}

// A test can call another test. Any other use of its name keeps package
// scope, including references from another module or a macro argument.
fn used_only_in_declarations(directory: &Path, names: &BTreeSet<String>) -> Option<bool> {
    let mut counts: std::collections::BTreeMap<String, usize> =
        names.iter().map(|name| (name.clone(), 0)).collect();
    each_source(directory, &mut |tokens| {
        each_token(tokens, &mut |token| {
            if let proc_macro2::TokenTree::Ident(name) = token
                && let Some(count) = counts.get_mut(name.to_string().trim_start_matches("r#"))
            {
                *count += 1;
            }
        });
    })?;
    Some(counts.values().all(|count| *count == 1))
}

#[cfg(test)]
mod tests;
