//! Tests affected by a diff against a base: which packages' tests run, with or
//! without their reverse dependencies, which integration-test binaries and
//! which named tests, and which packages must be built to run them.
//!
//! Each changed file selects by the narrowest rule that holds for it:
//! workspace settings select everything; a declared file read by tests
//! selects those tests; other Markdown outside packages selects nothing; any
//! other file outside packages selects everything. Inside a package, a
//! Cargo-declared integration-test file selects its binary; a change confined
//! to test functions selects those tests; a change to test-only source (a
//! module compiled only under `cfg(test)`, or a file under the package's
//! `tests/` directory) selects the package's own tests; anything else selects
//! the package and its reverse dependencies.
mod functions;

use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
    process::Command,
};

/// The tests a diff affects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Every test in the workspace: a change no narrower rule covers.
    Workspace,
    /// These tests; none when empty.
    Tests(Tests),
}

/// Tests selected within the workspace. A package named in one field is
/// removed from the narrower fields after it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tests {
    /// Packages whose tests run together with every package that depends on
    /// them (nextest's `rdeps`).
    pub with_dependents: BTreeSet<String>,
    /// Packages whose own tests run: only their test-only source changed.
    pub packages: BTreeSet<String>,
    /// Cargo-declared integration-test binaries, as (package, binary).
    pub binaries: BTreeSet<(String, String)>,
    /// Tests selected by name within a package, matched as nextest's
    /// `test(name)`.
    pub functions: BTreeMap<String, BTreeSet<String>>,
}

impl Tests {
    pub fn is_empty(&self) -> bool {
        self.with_dependents.is_empty()
            && self.packages.is_empty()
            && self.binaries.is_empty()
            && self.functions.is_empty()
    }

    fn normalize(&mut self) {
        let Self {
            with_dependents,
            packages,
            binaries,
            functions,
        } = self;
        packages.retain(|package| !with_dependents.contains(package));
        let covered =
            |package: &String| with_dependents.contains(package) || packages.contains(package);
        binaries.retain(|(package, _)| !covered(package));
        functions.retain(|package, _| !covered(package));
    }
}

impl Selection {
    /// Whether no test is selected, so a run needs no build.
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Tests(tests) if tests.is_empty())
    }

    /// The nextest filterset, or `None` for the whole workspace.
    pub fn filterset(&self) -> Option<String> {
        let tests = match self {
            Self::Workspace => return None,
            Self::Tests(tests) if tests.is_empty() => return Some("none()".to_owned()),
            Self::Tests(tests) => tests,
        };
        let terms = tests
            .with_dependents
            .iter()
            .map(|package| format!("rdeps(={package})"))
            .chain(
                tests
                    .packages
                    .iter()
                    .map(|package| format!("package(={package})")),
            )
            .chain(
                tests
                    .binaries
                    .iter()
                    .map(|(package, binary)| format!("(package(={package}) & binary(={binary}))")),
            )
            .chain(tests.functions.iter().map(|(package, names)| {
                format!(
                    "(package(={package}) & ({}))",
                    names
                        .iter()
                        .map(|name| format!("test({name})"))
                        .collect::<Vec<_>>()
                        .join(" | ")
                )
            }));
        Some(terms.collect::<Vec<_>>().join(" | "))
    }
}

/// A workspace member, as `cargo metadata` describes it.
#[derive(Debug, Clone)]
struct Package {
    name: String,
    directory: PathBuf,
    /// Cargo-declared integration-test targets: name and source file.
    tests: Vec<(String, PathBuf)>,
    /// Every target's root source file (library, binaries, tests, ...).
    roots: Vec<PathBuf>,
    /// Workspace members this package depends on, of any dependency kind.
    dependencies: BTreeSet<String>,
    /// Files outside Rust sources that named tests of this package read,
    /// declared under `[package.metadata.devkit.read-by-tests]`.
    read_by_tests: Vec<(PathBuf, BTreeSet<String>)>,
}

/// The workspace's members and their dependencies on each other.
#[derive(Debug, Clone)]
pub struct Workspace {
    root: PathBuf,
    packages: Vec<Package>,
}

impl Workspace {
    /// Reads the members of the workspace at `root` through `cargo metadata`.
    pub fn read(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("{}: {error}", root.display()))?;
        let packages = packages(&root)?;
        Ok(Self { root, packages })
    }

    /// The tests affected by changes since the merge base with `base`:
    /// committed, staged, unstaged and untracked files.
    pub fn changed(&self, base: &str) -> Result<Selection, String> {
        let root = self.root.as_path();
        let merge_base = git(root, &["merge-base", "HEAD", base])?;
        let merge_base = merge_base.trim();
        let mut files = paths(&git(
            root,
            &["diff", "--no-renames", "--name-only", "-z", merge_base],
        )?);
        files.extend(paths(&git(
            root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?));
        Ok(select(root, &files, &self.packages, |file, package| {
            functions::changed_tests(root, merge_base, file, package)
        }))
    }

    /// The packages whose test targets must be built to run `selection`,
    /// or `None` for every package. A package selected with its dependents
    /// brings every member that depends on it, directly or through others.
    pub fn packages_to_build(&self, selection: &Selection) -> Option<BTreeSet<String>> {
        let Selection::Tests(tests) = selection else {
            return None;
        };
        let mut build: BTreeSet<String> = tests
            .packages
            .iter()
            .cloned()
            .chain(tests.binaries.iter().map(|(package, _)| package.clone()))
            .chain(tests.functions.keys().cloned())
            .collect();
        let mut pending: Vec<&String> = tests.with_dependents.iter().collect();
        let mut reached = BTreeSet::new();
        while let Some(package) = pending.pop() {
            if !reached.insert(package.clone()) {
                continue;
            }
            pending.extend(
                self.packages
                    .iter()
                    .filter(|member| member.dependencies.contains(package))
                    .map(|member| &member.name),
            );
        }
        build.extend(reached);
        Some(build)
    }
}

/// The tests affected by changes since the merge base with `base`.
pub fn changed(root: &Path, base: &str) -> Result<Selection, String> {
    Workspace::read(root)?.changed(base)
}

fn paths(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

/// `a/b/../c` as `a/c`, without touching the filesystem: a declared file may
/// have been deleted.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Selects the tests for `files`. `changed_tests` names the test functions a
/// change to a package's Rust file is confined to, when it is.
fn select(
    root: &Path,
    files: &[String],
    packages: &[Package],
    changed_tests: impl Fn(&str, &Package) -> Option<BTreeSet<String>>,
) -> Selection {
    let mut tests = Tests::default();
    // Workspace settings decide before any file is read.
    if files.iter().any(|file| {
        matches!(
            file.as_str(),
            "Cargo.toml" | "Cargo.lock" | "flake.nix" | "flake.lock"
        ) || [".config/", ".cargo/"]
            .iter()
            .any(|prefix| file.starts_with(prefix))
    }) {
        return Selection::Workspace;
    }
    for file in files {
        let path = lexical(&root.join(file));
        let mut read = false;
        for package in packages {
            for (declared, names) in &package.read_by_tests {
                if *declared == path {
                    read = true;
                    tests
                        .functions
                        .entry(package.name.clone())
                        .or_default()
                        .extend(names.iter().cloned());
                }
            }
        }
        let owner = packages
            .iter()
            .filter(|p| path.starts_with(&p.directory))
            .max_by_key(|p| p.directory.components().count());
        let Some(package) = owner else {
            if read || path.extension().is_some_and(|extension| extension == "md") {
                continue;
            }
            return Selection::Workspace;
        };
        if let Some((binary, _)) = package.tests.iter().find(|(_, source)| *source == path) {
            tests
                .binaries
                .insert((package.name.clone(), binary.clone()));
        } else if let Some(names) = changed_tests(file, package) {
            tests
                .functions
                .entry(package.name.clone())
                .or_default()
                .extend(names);
        } else if test_only(package, &path) {
            tests.packages.insert(package.name.clone());
        } else {
            tests.with_dependents.insert(package.name.clone());
        }
    }
    tests.normalize();
    Selection::Tests(tests)
}

/// Whether `path` is compiled only into its package's own tests, so no
/// dependent can see it: a file under the package's `tests/` directory, or a
/// Rust module whose declaration, or an enclosing module's, carries
/// `cfg(test)` (or the file says `#![cfg(test)]`). A file whose name a
/// `#[path]` value or an `include!` names anywhere in the package is not
/// recognised, since its includer may be production code.
fn test_only(package: &Package, path: &Path) -> bool {
    if path.extension().is_none_or(|extension| extension != "rs") {
        return false;
    }
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if named_in_strings(&package.directory, file_name).unwrap_or(true) {
        return false;
    }
    if path.starts_with(package.directory.join("tests")) {
        return true;
    }
    under_cfg_test(package, path, 0)
}

fn under_cfg_test(package: &Package, path: &Path, depth: usize) -> bool {
    // Module trees are shallow; the bound only stops a cycle of symlinks.
    if depth > 64 || package.roots.iter().any(|root| root == path) {
        return parse(path).is_some_and(|file| cfg_test(&file.attrs));
    }
    let Some(file) = parse(path) else {
        return false;
    };
    if cfg_test(&file.attrs) {
        return true;
    }
    let (Some(stem), Some(parent)) = (
        path.file_stem().and_then(|stem| stem.to_str()),
        path.parent(),
    ) else {
        return false;
    };
    // `dir/name.rs` and `dir/name/mod.rs` are both module `name` of `dir`.
    let (name, directory) = if stem == "mod" {
        match (
            parent.file_name().and_then(|name| name.to_str()),
            parent.parent(),
        ) {
            (Some(name), Some(directory)) => (name, directory),
            _ => return false,
        }
    } else {
        (stem, parent)
    };
    // A crate root or `mod.rs` declares modules beside it; any other file
    // `dir.rs` declares them in `dir/`.
    let mut parents: Vec<PathBuf> = package
        .roots
        .iter()
        .filter(|root| root.parent() == Some(directory))
        .cloned()
        .collect();
    parents.push(directory.join("mod.rs"));
    let sibling = directory.with_extension("rs");
    if !package.roots.contains(&sibling) {
        parents.push(sibling);
    }
    for parent in parents.iter().filter(|parent| parent.as_path() != path) {
        let Some(source) = parse(parent) else {
            continue;
        };
        for item in &source.items {
            let syn::Item::Mod(module) = item else {
                continue;
            };
            if module.ident != name
                || module.content.is_some()
                || module.attrs.iter().any(|attr| attr.path().is_ident("path"))
            {
                continue;
            }
            return cfg_test(&module.attrs) || under_cfg_test(package, parent, depth + 1);
        }
    }
    false
}

fn parse(path: &Path) -> Option<syn::File> {
    syn::parse_file(&std::fs::read_to_string(path).ok()?).ok()
}

/// `cfg(test)`, or `cfg(all(test, ...))`, among the attributes.
fn cfg_test(attrs: &[syn::Attribute]) -> bool {
    fn requires_test(meta: &syn::Meta) -> bool {
        match meta {
            syn::Meta::Path(path) => path.is_ident("test"),
            syn::Meta::List(list) if list.path.is_ident("all") => list
                .parse_args_with(
                    syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
                )
                .is_ok_and(|items| items.iter().any(requires_test)),
            _ => false,
        }
    }
    attrs.iter().any(|attr| {
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Meta>()
                .is_ok_and(|meta| requires_test(&meta))
    })
}

/// Whether any Rust source of the package names `file_name` at the end of a
/// `path = "..."` attribute value or in an `include!`, `include_str!` or
/// `include_bytes!` argument. `None` when a source cannot be read.
fn named_in_strings(directory: &Path, file_name: &str) -> Option<bool> {
    use proc_macro2::{TokenStream, TokenTree};
    fn names(literal: &TokenTree, file_name: &str) -> bool {
        let TokenTree::Literal(literal) = literal else {
            return false;
        };
        let text = literal.to_string();
        text.contains('"')
            && text
                .trim_end_matches('#')
                .trim_end_matches('"')
                .ends_with(file_name)
    }
    fn any_literal(tokens: TokenStream, file_name: &str) -> bool {
        tokens.into_iter().any(|token| match &token {
            TokenTree::Group(group) => any_literal(group.stream(), file_name),
            other => names(other, file_name),
        })
    }
    fn walk(tokens: TokenStream, file_name: &str) -> bool {
        let tokens: Vec<TokenTree> = tokens.into_iter().collect();
        tokens.iter().enumerate().any(|(index, token)| match token {
            TokenTree::Group(group) => walk(group.stream(), file_name),
            TokenTree::Ident(ident) if ident == "path" => {
                matches!(tokens.get(index + 1), Some(TokenTree::Punct(p)) if p.as_char() == '=')
                    && tokens
                        .get(index + 2)
                        .is_some_and(|value| names(value, file_name))
            }
            TokenTree::Ident(ident)
                if ident == "include" || ident == "include_str" || ident == "include_bytes" =>
            {
                matches!(tokens.get(index + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!')
                    && matches!(tokens.get(index + 2), Some(TokenTree::Group(group)) if any_literal(group.stream(), file_name))
            }
            _ => false,
        })
    }
    let mut found = false;
    functions::each_source(directory, &mut |tokens| {
        found |= walk(tokens, file_name);
    })?;
    Some(found)
}

fn packages(root: &Path) -> Result<Vec<Package>, String> {
    let output = crate::cargo_command()
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let members = metadata["workspace_members"]
        .as_array()
        .ok_or("cargo metadata has no workspace members")?;
    let packages: Vec<&Value> = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?
        .iter()
        .filter(|p| members.contains(&p["id"]))
        .collect();
    let names: BTreeSet<&str> = packages.iter().filter_map(|p| p["name"].as_str()).collect();
    packages
        .iter()
        .map(|p| {
            let name = p["name"].as_str().ok_or("package has no name")?;
            let directory = Path::new(
                p["manifest_path"]
                    .as_str()
                    .ok_or("package has no manifest path")?,
            )
            .parent()
            .ok_or("manifest has no parent")?
            .to_owned();
            let targets = p["targets"].as_array().ok_or("package has no targets")?;
            let mut tests = Vec::new();
            let mut roots = Vec::new();
            for target in targets {
                let source = PathBuf::from(
                    target["src_path"]
                        .as_str()
                        .ok_or("target has no source path")?,
                );
                if target["kind"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|kind| kind == "test"))
                {
                    let test = target["name"].as_str().ok_or("test target has no name")?;
                    tests.push((test.to_owned(), source.clone()));
                }
                roots.push(source);
            }
            // A dependency on another member is a path dependency; it may be
            // renamed, so the package name decides.
            let dependencies = p["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|dependency| !dependency["path"].is_null())
                .filter_map(|dependency| dependency["name"].as_str())
                .filter(|dependency| names.contains(dependency) && *dependency != name)
                .map(str::to_owned)
                .collect();
            let read_by_tests = match &p["metadata"]["devkit"]["read-by-tests"] {
                Value::Null => Vec::new(),
                Value::Object(files) => files
                    .iter()
                    .map(|(file, names)| {
                        let names = names
                            .as_array()
                            .and_then(|names| {
                                names
                                    .iter()
                                    .map(|name| name.as_str().map(str::to_owned))
                                    .collect::<Option<BTreeSet<_>>>()
                            })
                            .filter(|names| !names.is_empty())
                            .ok_or_else(|| {
                                format!("{name}: read-by-tests.\"{file}\" must list test names")
                            })?;
                        Ok((lexical(&directory.join(file)), names))
                    })
                    .collect::<Result<_, String>>()?,
                _ => {
                    return Err(format!(
                        "{name}: package.metadata.devkit.read-by-tests must be a table"
                    ));
                }
            };
            Ok(Package {
                name: name.to_owned(),
                directory,
                tests,
                roots,
                dependencies,
                read_by_tests,
            })
        })
        .collect()
}

/// Consumes devkit's `--changed` and `--base REF` (or `--base=REF`, which
/// implies `--changed`) before nextest sees them. Explicit filtersets
/// intersect the changed selection. Returns whether the diff selects no
/// tests, so the run needs no build.
pub(crate) fn nextest_args(root: &Path, args: &[String]) -> Result<(Vec<String>, bool), String> {
    let mut enabled = false;
    let mut base = "origin/master".to_owned();
    let mut remaining = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            remaining.push(arg.clone());
            remaining.extend(args.cloned());
            break;
        }
        if arg == "--changed" {
            enabled = true;
        } else if arg == "--base" {
            base = args.next().ok_or("--base needs a Git reference")?.clone();
            enabled = true;
        } else if let Some(value) = arg.strip_prefix("--base=") {
            base = value.to_owned();
            enabled = true;
        } else {
            remaining.push(arg.clone());
        }
    }
    if !enabled {
        return Ok((remaining, false));
    }
    let selection = changed(root, &base)?;
    if selection.is_empty() {
        return Ok((remaining, true));
    }
    if let Some(filter) = selection.filterset() {
        remaining = intersect_filter(&remaining, &filter)?;
    }
    Ok((remaining, false))
}

fn intersect_filter(args: &[String], changed: &str) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    let mut found = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            if !found {
                result.extend(["-E".into(), changed.into()]);
            }
            result.push(arg.clone());
            result.extend(args.cloned());
            return Ok(result);
        }
        if arg == "-E" || arg == "--filterset" || arg == "--filter-expr" {
            let value = args.next().ok_or("missing nextest filterset")?;
            result.extend([arg.clone(), format!("({value}) & ({changed})")]);
            found = true;
        } else if let Some(value) = arg
            .strip_prefix("--filterset=")
            .or_else(|| arg.strip_prefix("--filter-expr="))
            .or_else(|| {
                arg.strip_prefix("-E")
                    .filter(|v| !v.is_empty())
                    .map(|value| value.strip_prefix('=').unwrap_or(value))
            })
        {
            result.extend(["-E".into(), format!("({value}) & ({changed})")]);
            found = true;
        } else {
            result.push(arg.clone());
        }
    }
    if !found {
        result.extend(["-E".into(), changed.into()]);
    }
    Ok(result)
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("git path is not UTF-8: {e}"))
}

#[cfg(test)]
mod tests;
