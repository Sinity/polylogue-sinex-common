//! Which tests a change can affect: the workspace packages whose files
//! changed since the merge base, and everything that depends on them.
//!
//! Selection errs towards running too much. A changed file outside every
//! package (a workspace manifest, a Nix file, a fixture read with
//! `include_str!` from elsewhere) selects the whole workspace; only Markdown
//! files outside packages are ignored.
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// Nothing changed.
    Nothing,
    /// These packages changed; their reverse dependencies are affected too.
    Packages(Vec<String>),
    Workspace,
}

impl Selection {
    /// The nextest filterset for this selection, or `None` for the whole
    /// workspace.
    pub fn filterset(&self) -> Option<String> {
        match self {
            Self::Workspace => None,
            Self::Nothing => Some("none()".to_owned()),
            Self::Packages(packages) => Some(
                packages
                    .iter()
                    .map(|p| format!("rdeps(={p})"))
                    .collect::<Vec<_>>()
                    .join(" | "),
            ),
        }
    }
}

/// Packages changed in the working tree since the merge base of `HEAD` and
/// `base`, untracked files included.
pub fn changed(root: &Path, base: &str) -> Result<Selection, String> {
    let merge_base = git(root, &["merge-base", "HEAD", base])?;
    let merge_base = merge_base.trim();
    let mut files: Vec<String> = git(root, &["diff", "--name-only", merge_base])?
        .lines()
        .map(str::to_owned)
        .collect();
    files.extend(
        git(root, &["ls-files", "--others", "--exclude-standard"])?
            .lines()
            .map(str::to_owned),
    );
    let packages = packages(root)?;
    Ok(select(root, &files, &packages))
}

fn select(root: &Path, files: &[String], packages: &[(String, PathBuf)]) -> Selection {
    let mut changed: Vec<String> = Vec::new();
    for file in files.iter().filter(|f| !f.is_empty()) {
        let path = root.join(file);
        let owner = packages
            .iter()
            .filter(|(_, dir)| path.starts_with(dir))
            .max_by_key(|(_, dir)| dir.components().count());
        match owner {
            Some((name, _)) => {
                if !changed.contains(name) {
                    changed.push(name.clone());
                }
            }
            None if file.ends_with(".md") => {}
            None => return Selection::Workspace,
        }
    }
    if changed.is_empty() {
        return Selection::Nothing;
    }
    changed.sort();
    Selection::Packages(changed)
}

fn packages(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let name = p["name"].as_str()?;
            let dir = Path::new(p["manifest_path"].as_str()?).parent()?;
            Some((name.to_owned(), dir.to_owned()))
        })
        .collect())
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
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packages() -> Vec<(String, PathBuf)> {
        vec![
            ("model".into(), PathBuf::from("/w/crates/model")),
            ("store".into(), PathBuf::from("/w/crates/store")),
            ("xtask".into(), PathBuf::from("/w/xtask")),
        ]
    }

    fn files(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn changed_files_map_to_their_packages() {
        let root = Path::new("/w");
        let selection = select(
            root,
            &files(&["crates/store/src/lib.rs", "crates/model/README.md"]),
            &packages(),
        );
        assert_eq!(
            selection,
            Selection::Packages(vec!["model".into(), "store".into()])
        );
        assert_eq!(
            selection.filterset().unwrap(),
            "rdeps(=model) | rdeps(=store)"
        );
    }

    #[test]
    fn a_file_outside_every_package_selects_the_workspace() {
        let root = Path::new("/w");
        assert_eq!(
            select(
                root,
                &files(&["crates/store/src/lib.rs", "flake.nix"]),
                &packages()
            ),
            Selection::Workspace
        );
        assert_eq!(
            select(root, &files(&["docs/development.md"]), &packages()),
            Selection::Nothing
        );
    }
}
