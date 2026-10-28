//! Local checkout overrides for a Git dependency shared by Cargo workspaces.
//!
//! The caller chooses the repository, patch, opt-out marker and original-lock
//! backup paths. Linking resolves every workspace against the checkout and
//! preserves the original locks until unlinking. Returned errors restore the
//! files as they were before the operation, including an earlier active link.
//! A killed process can leave the operation unfinished; original backups remain
//! available for the caller's next unlink.
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, String>;

/// One local dependency override and the workspace locks it owns.
///
/// Paths are distinct and absolute. Each lock's parent is its Cargo workspace directory;
/// each paired backup retains the Git-backed lock across successful relinks.
pub struct LocalLink<'a> {
    pub repository: &'a str,
    pub patch: &'a Path,
    pub disabled: &'a Path,
    pub locks: &'a [(&'a Path, &'a Path)],
}

impl LocalLink<'_> {
    /// Resolves every workspace against `checkout`, returning linked crate names.
    pub fn link(&self, checkout: &Path) -> Result<Vec<String>> {
        let checkout = fs::canonicalize(checkout)
            .map_err(|error| format!("{}: {error}", checkout.display()))?;
        let available = crate_directories(&metadata(&checkout, true)?);
        self.change(|| {
            for (lock, _) in self.locks {
                read(lock)?;
            }
            remove(self.patch)?;
            let mut used = BTreeSet::new();
            for (lock, _) in self.locks {
                used.extend(shared_crates(&metadata(workspace(lock)?, true)?, self.repository));
            }
            if used.is_empty() {
                return Err(format!("no workspace dependency comes from {}", self.repository));
            }
            let patch = patch_config(self.repository, &used, &available)?;
            for (lock, backup) in self.locks {
                if !backup.exists() {
                    write(backup, &read(lock)?)?;
                }
            }
            write(self.patch, patch.as_bytes())?;
            remove(self.disabled)?;
            for (lock, _) in self.locks {
                let resolved = metadata(workspace(lock)?, false)?;
                if let Some(package) = resolved["packages"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|package| from_repository(&package["source"], self.repository))
                {
                    return Err(format!(
                        "{}: crate {} still resolves to Git; the checkout does not satisfy this workspace",
                        lock.display(),
                        package["name"].as_str().unwrap_or("unknown")
                    ));
                }
            }
            Ok(used.into_iter().collect())
        })
    }

    /// Restores original locks, removes the patch and disables automatic linking.
    pub fn unlink(&self) -> Result<Vec<PathBuf>> {
        self.change(|| {
            remove(self.patch)?;
            let mut restored = Vec::new();
            for (lock, backup) in self.locks {
                if backup.exists() {
                    fs::rename(backup, lock)
                        .map_err(|error| format!("restoring {}: {error}", lock.display()))?;
                    restored.push(lock.to_path_buf());
                }
            }
            write(
                self.disabled,
                b"Automatic local dependency linking is disabled.\n",
            )?;
            Ok(restored)
        })
    }

    fn change<T>(&self, operation: impl FnOnce() -> Result<T>) -> Result<T> {
        if self.locks.is_empty() {
            return Err("a local dependency link needs a workspace lock".into());
        }
        let paths: BTreeSet<&Path> = std::iter::once(self.patch)
            .chain(std::iter::once(self.disabled))
            .chain(
                self.locks
                    .iter()
                    .flat_map(|(lock, backup)| [*lock, *backup]),
            )
            .collect();
        let snapshots: Vec<Snapshot> = paths
            .into_iter()
            .map(Snapshot::read)
            .collect::<Result<_>>()?;
        match operation() {
            Ok(value) => Ok(value),
            Err(error) => {
                let failures: Vec<String> = snapshots
                    .iter()
                    .filter_map(|snapshot| snapshot.restore().err())
                    .collect();
                if failures.is_empty() {
                    Err(error)
                } else {
                    Err(format!(
                        "{error}; restoring previous files failed: {}",
                        failures.join("; ")
                    ))
                }
            }
        }
    }
}

struct Snapshot {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
}

impl Snapshot {
    fn read(path: &Path) -> Result<Self> {
        let bytes = match fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(format!("reading {}: {error}", path.display())),
        };
        Ok(Self {
            path: path.to_owned(),
            bytes,
        })
    }

    fn restore(&self) -> Result<()> {
        match &self.bytes {
            Some(bytes) => write(&self.path, bytes),
            None => remove(&self.path),
        }
    }
}

fn workspace(lock: &Path) -> Result<&Path> {
    lock.parent()
        .ok_or_else(|| format!("{} has no parent", lock.display()))
}

fn metadata(workspace: &Path, no_dependencies: bool) -> Result<Value> {
    let mut command = crate::cargo_command();
    command.args(["metadata", "--format-version", "1"]);
    if no_dependencies {
        command.arg("--no-deps");
    }
    let output = command
        .current_dir(workspace)
        .output()
        .map_err(|error| format!("cargo metadata: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata in {}: {}",
            workspace.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())
}

fn from_repository(source: &Value, repository: &str) -> bool {
    source
        .as_str()
        .and_then(|source| source.strip_prefix("git+"))
        .and_then(|source| source.split(['?', '#']).next())
        == Some(repository)
}

fn shared_crates(metadata: &Value, repository: &str) -> BTreeSet<String> {
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|package| package["dependencies"].as_array().into_iter().flatten())
        .filter(|dependency| from_repository(&dependency["source"], repository))
        .filter_map(|dependency| dependency["name"].as_str().map(str::to_owned))
        .collect()
}

fn crate_directories(metadata: &Value) -> BTreeMap<String, PathBuf> {
    metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|package| {
            let name = package["name"].as_str()?;
            let manifest = Path::new(package["manifest_path"].as_str()?);
            Some((name.to_owned(), manifest.parent()?.to_owned()))
        })
        .collect()
}

fn patch_config(
    repository: &str,
    used: &BTreeSet<String>,
    available: &BTreeMap<String, PathBuf>,
) -> Result<String> {
    let mut config = format!(
        "# Local dependency checkout; removed by unlinking.\n[patch.{}]\n",
        toml_string(repository)
    );
    for name in used {
        let directory = available
            .get(name)
            .ok_or_else(|| format!("the checkout has no crate {name}"))?;
        config += &format!(
            "{name} = {{ path = {} }}\n",
            toml_string(&directory.display().to_string())
        );
    }
    Ok(config)
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("a string is JSON serializable")
}

fn read(path: &Path) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| format!("reading {}: {error}", path.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    fs::write(path, bytes).map_err(|error| format!("writing {}: {error}", path.display()))
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("removing {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests;
