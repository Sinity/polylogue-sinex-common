//! Source-file hashing shared by schema builds and development watches.
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn hash(
    paths: &[PathBuf],
    extra: &[&str],
    follow_links: bool,
    include: impl Fn(&Path, bool) -> bool,
) -> Result<blake3::Hash, String> {
    fn collect(
        path: &Path,
        follow_links: bool,
        include: &impl Fn(&Path, bool) -> bool,
        files: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !include(path, meta.is_dir()) {
            return Ok(());
        }
        if meta.is_file() {
            files.push(path.to_owned());
        } else if meta.is_dir() {
            for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
                let entry = entry.map_err(|e| e.to_string())?;
                if !follow_links && entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
                    continue;
                }
                collect(&entry.path(), follow_links, include, files)?;
            }
        }
        Ok(())
    }
    let mut hash = blake3::Hasher::new();
    hash.update(&(paths.len() as u64).to_le_bytes());
    for path in paths {
        let mut files = Vec::new();
        collect(path, follow_links, &include, &mut files)?;
        files.sort();
        hash.update(&(files.len() as u64).to_le_bytes());
        for file in files {
            let relative = file.strip_prefix(path).unwrap_or(&file);
            let name = relative.as_os_str().as_encoded_bytes();
            let contents = fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            hash.update(&(name.len() as u64).to_le_bytes());
            hash.update(name);
            hash.update(&(contents.len() as u64).to_le_bytes());
            hash.update(&contents);
        }
    }
    hash.update(&(extra.len() as u64).to_le_bytes());
    for value in extra {
        hash.update(&(value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    }
    Ok(hash.finalize())
}
