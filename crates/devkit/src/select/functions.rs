//! Changes confined to test functions need only those tests. Every changed
//! line must belong to a test in both versions; helpers select the package.
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
                        path.is_ident("test") || (path.segments.len() == 2
                            && path.segments.first().is_some_and(|part| part.ident == "devkit" || part.ident == "tokio")
                            && path.segments.last().is_some_and(|part| part.ident == "test"))
                    }) =>
                {
                    let start = function.attrs.first().map_or(function.span().start(), |attr| attr.span().start());
                    let end = function.span().end();
                    let prefix = source.lines().nth(start.line - 1).and_then(|line| line.get(..start.column));
                    let suffix = source.lines().nth(end.line - 1).and_then(|line| line.get(end.column..));
                    // A diff describes whole lines. Other items sharing a line
                    // cannot be attributed to this test alone.
                    if !prefix.is_some_and(|text| text.trim().is_empty()) || !suffix.is_some_and(|text| text.trim().is_empty()) { continue; }
                    tests.push(TestFunction {
                        name: function.sig.ident.to_string().trim_start_matches("r#").to_owned(),
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

pub(super) fn narrow(
    root: &Path,
    base: &str,
    files: &[String],
    packages: &[Package],
) -> Result<Selection, String> {
    let mut filters = BTreeSet::new();
    let mut narrowed = false;
    for file in files {
        let path = root.join(file);
        let Some(package) = packages
            .iter()
            .filter(|p| path.starts_with(&p.directory))
            .max_by_key(|p| p.directory.components().count())
        else {
            continue;
        };
        if let Some((binary, _)) = package.tests.iter().find(|(_, source)| *source == path) {
            filters.insert(format!("(package(={}) & binary(={binary}))", package.name));
            continue;
        }
        let names = if path.extension().is_some_and(|extension| extension == "rs") {
            let before = git(root, &["show", &format!("{base}:{file}")]);
            let after = std::fs::read_to_string(&path);
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
            )?;
            match (before, after) {
                (Ok(before), Ok(after)) => names_for_diff(&before, &after, &diff),
                _ => None,
            }
        } else {
            None
        };
        if let Some(names) = names {
            narrowed = true;
            filters.insert(format!(
                "(package(={}) & ({}))",
                package.name,
                names
                    .iter()
                    .map(|name| format!("test({name})"))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ));
        } else {
            filters.insert(format!("rdeps(={})", package.name));
        }
    }
    if !narrowed {
        return Ok(select(root, files, packages));
    }
    if filters.is_empty() {
        Ok(Selection::Nothing)
    } else {
        Ok(Selection::Filter(
            filters.into_iter().collect::<Vec<_>>().join(" | "),
        ))
    }
}

#[cfg(test)]
mod tests;
