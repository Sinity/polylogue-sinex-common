//! Tests affected by a diff: changed packages and their reverse dependencies.
//!
//! Files outside packages select the workspace, except Markdown documentation.
//! Changes confined to a Cargo integration-test target select that binary.
use serde_json::Value;
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    Nothing,
    /// Production changes affect these packages and their reverse dependencies.
    Packages(Vec<String>),
    /// A mixture of package changes and integration-test file changes.
    Tests {
        packages: Vec<String>,
        binaries: Vec<(String, String)>,
    },
    Workspace,
}

impl Selection {
    /// The nextest filterset, or `None` for the whole workspace.
    pub fn filterset(&self) -> Option<String> {
        let (packages, binaries) = match self {
            Self::Workspace => return None,
            Self::Nothing => return Some("none()".to_owned()),
            Self::Packages(packages) => (packages.as_slice(), &[][..]),
            Self::Tests { packages, binaries } => (packages.as_slice(), binaries.as_slice()),
        };
        Some(
            packages
                .iter()
                .map(|p| format!("rdeps(={p})"))
                .chain(
                    binaries.iter().map(|(package, binary)| {
                        format!("(package(={package}) & binary(={binary}))")
                    }),
                )
                .collect::<Vec<_>>()
                .join(" | "),
        )
    }
}

struct Package {
    name: String,
    directory: PathBuf,
    tests: Vec<(String, PathBuf)>,
}

/// Changes since the merge base, including staged, unstaged and untracked files.
pub fn changed(root: &Path, base: &str) -> Result<Selection, String> {
    let merge_base = git(root, &["merge-base", "HEAD", base])?;
    let mut files = paths(&git(
        root,
        &[
            "diff",
            "--no-renames",
            "--name-only",
            "-z",
            merge_base.trim(),
        ],
    )?);
    files.extend(paths(&git(
        root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?));
    if files.is_empty() {
        return Ok(Selection::Nothing);
    }
    Ok(select(root, &files, &packages(root)?))
}

fn paths(output: &str) -> Vec<String> {
    output
        .split('\0')
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect()
}

fn select(root: &Path, files: &[String], packages: &[Package]) -> Selection {
    let mut changed = BTreeSet::new();
    let mut binaries = BTreeSet::new();
    for file in files.iter().filter(|f| !f.is_empty()) {
        let path = root.join(file);
        let owner = packages
            .iter()
            .filter(|p| path.starts_with(&p.directory))
            .max_by_key(|p| p.directory.components().count());
        match owner {
            Some(package) => {
                if let Some((binary, _)) = package.tests.iter().find(|(_, source)| *source == path)
                {
                    binaries.insert((package.name.clone(), binary.clone()));
                } else {
                    changed.insert(package.name.clone());
                }
            }
            None if file.ends_with(".md") => {}
            None => return Selection::Workspace,
        }
    }
    binaries.retain(|(package, _)| !changed.contains(package));
    let packages: Vec<_> = changed.into_iter().collect();
    if !binaries.is_empty() {
        Selection::Tests {
            packages,
            binaries: binaries.into_iter().collect(),
        }
    } else if packages.is_empty() {
        Selection::Nothing
    } else {
        Selection::Packages(packages)
    }
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
    metadata["packages"]
        .as_array()
        .ok_or("cargo metadata has no packages")?
        .iter()
        .filter(|p| members.contains(&p["id"]))
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
            let tests = p["targets"]
                .as_array()
                .ok_or("package has no targets")?
                .iter()
                .filter(|target| {
                    target["kind"]
                        .as_array()
                        .is_some_and(|k| k.iter().any(|kind| kind == "test"))
                })
                .map(|target| {
                    Ok((
                        target["name"]
                            .as_str()
                            .ok_or("test target has no name")?
                            .to_owned(),
                        PathBuf::from(
                            target["src_path"]
                                .as_str()
                                .ok_or("test target has no source path")?,
                        ),
                    ))
                })
                .collect::<Result<Vec<_>, String>>()?;
            Ok(Package {
                name: name.to_owned(),
                directory,
                tests,
            })
        })
        .collect()
}

/// Consumes devkit's `--changed` and `--changed-base=<ref>` arguments before
/// nextest sees them. Explicit filtersets intersect the changed selection.
/// Returns whether the diff selects no tests, so a clean tree needs no build.
pub(crate) fn nextest_args(root: &Path, args: &[String]) -> Result<(Vec<String>, bool), String> {
    let mut enabled = false;
    let mut base = "origin/master";
    let mut remaining = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if arg == "--" {
            remaining.extend_from_slice(&args[index..]);
            break;
        }
        if arg == "--changed" {
            enabled = true;
        } else if let Some(value) = arg.strip_prefix("--changed-base=") {
            base = value;
            enabled = true;
        } else {
            remaining.push(arg.clone());
        }
    }
    if !enabled {
        return Ok((remaining, false));
    }
    let selection = changed(root, base)?;
    if selection == Selection::Nothing {
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
        if arg == "-E" || arg == "--filterset" {
            let value = args.next().ok_or("missing nextest filterset")?;
            result.extend([arg.clone(), format!("({value}) & ({changed})")]);
            found = true;
        } else if let Some(value) = arg
            .strip_prefix("--filterset=")
            .or_else(|| arg.strip_prefix("-E").filter(|v| !v.is_empty()))
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
