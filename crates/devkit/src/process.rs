//! Owned process groups and source changes for development commands.
use std::{fs, io, os::unix::process::CommandExt, path::Path, process::{Child, Command, ExitStatus}, time::{Duration, Instant}};

pub struct Process { child: Child, stopped: bool }
impl Process {
    pub fn start(command: &mut Command) -> Result<Self, String> {
        command.process_group(0).spawn().map(|child| Self { child, stopped: false }).map_err(|e| format!("{command:?}: {e}"))
    }
    pub fn id(&self) -> u32 { self.child.id() }
    pub fn poll(&mut self) -> Result<Option<ExitStatus>, String> {
        self.child.try_wait().map_err(|e| format!("waiting for child: {e}"))
    }
    pub fn wait(&mut self) -> Result<ExitStatus, String> {
        self.child.wait().map_err(|e| format!("waiting for child: {e}"))
    }
    pub fn stop(&mut self) -> Result<(), String> {
        if self.stopped { return Ok(()); }
        let group = format!("-{}", self.child.id());
        let signal = |name: &str| Command::new("kill").args([name, "--", &group]).output();
        signal("-TERM").map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            let _ = self.child.try_wait();
            if !Command::new("kill").args(["-0", "--", &group]).output().is_ok_and(|o|o.status.success()) { break; }
            std::thread::sleep(Duration::from_millis(10));
        }
        signal("-KILL").map_err(|e| e.to_string())?;
        self.child.wait().map_err(|e| e.to_string())?;
        self.stopped = true;
        Ok(())
    }
}
impl Drop for Process { fn drop(&mut self) { let _ = self.stop(); } }

/// Hash source contents, including additions and removals, without build outputs.
pub fn source_hash(root: &Path) -> Result<blake3::Hash, String> {
    source_hash_excluding(root,&[])
}

pub fn source_hash_excluding(root: &Path, excluded: &[std::path::PathBuf]) -> Result<blake3::Hash,String> {
    fn visit(path: &Path, excluded: &[std::path::PathBuf], hash: &mut blake3::Hasher) -> io::Result<()> {
        let mut entries = fs::read_dir(path)?.collect::<io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if excluded.iter().any(|path|entry.path().starts_with(path)) { continue; }
            let name = entry.file_name();
            if matches!(name.to_str(), Some("target" | ".git" | ".direnv" | "fuzz")) { continue; }
            let kind = entry.file_type()?;
            if kind.is_dir() { visit(&entry.path(), excluded, hash)?; }
            else if kind.is_file() && (entry.path().extension().is_some_and(|e| e == "rs" || e == "toml" || e == "json") || name == "Cargo.lock") {
                let path=entry.path();let name=path.as_os_str().as_encoded_bytes();let content=fs::read(&path)?;
                hash.update(&(name.len() as u64).to_le_bytes());hash.update(name);
                hash.update(&(content.len() as u64).to_le_bytes());hash.update(&content);
            }
        }
        Ok(())
    }
    let mut hash = blake3::Hasher::new();
    visit(root, excluded, &mut hash).map_err(|e| format!("watch {}: {e}", root.display()))?;
    Ok(hash.finalize())
}

#[cfg(test)] mod tests;
