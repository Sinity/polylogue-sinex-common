//! Owned service process groups and source changes for development commands.
use signal_hook::{
    consts::{SIGINT, SIGTERM},
    iterator::Signals,
};
use std::{
    collections::BTreeMap,
    fs, io,
    os::unix::process::{CommandExt, ExitStatusExt},
    path::Path,
    process::{Child, Command, ExitStatus},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};

type Groups = Arc<Mutex<BTreeMap<u32, Arc<AtomicI32>>>>;

fn groups() -> Result<&'static Groups, String> {
    static GROUPS: OnceLock<Result<Groups, String>> = OnceLock::new();
    GROUPS
        .get_or_init(|| {
            let mut signals = Signals::new([SIGINT, SIGTERM]).map_err(|e| e.to_string())?;
            let groups: Groups = Arc::default();
            let watched = Arc::clone(&groups);
            std::thread::Builder::new()
                .name("devkit service signals".into())
                .spawn(move || {
                    for signal in signals.forever() {
                        let groups = watched.lock().unwrap();
                        if groups.is_empty() {
                            let _ = signal_hook::low_level::emulate_default_handler(signal);
                        }
                        for (&pid, received) in groups.iter() {
                            received.store(signal, Ordering::Release);
                            let _ = signal_group(pid, signal);
                        }
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(groups)
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn signal_group(pid: u32, signal: i32) -> io::Result<bool> {
    // The group's leader stays unreaped until its final signal, so its id
    // cannot be assigned to another process group.
    if unsafe { libc::killpg(pid as libc::pid_t, signal) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(error)
    }
}

pub struct Process {
    child: Child,
    received: Arc<AtomicI32>,
    finished: Option<Result<ExitStatus, String>>,
}
impl Process {
    /// Starts a development-stack service in its own group. Ordinary run steps
    /// use the caller's group so terminal signals reach them directly.
    pub fn start(command: &mut Command) -> Result<Self, String> {
        let mut groups = groups()?.lock().map_err(|e| e.to_string())?;
        let child = command
            .process_group(0)
            .spawn()
            .map_err(|e| format!("{command:?}: {e}"))?;
        let received = Arc::new(AtomicI32::new(0));
        groups.insert(child.id(), Arc::clone(&received));
        Ok(Self {
            child,
            received,
            finished: None,
        })
    }
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    fn exit_status(&self) -> Result<Option<ExitStatus>, String> {
        // WNOWAIT observes exit without releasing the leader's pid.
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        let flags = libc::WEXITED | libc::WNOWAIT | libc::WNOHANG;
        loop {
            let result = unsafe { libc::waitid(libc::P_PID, self.id(), info.as_mut_ptr(), flags) };
            if result == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(format!("waiting for child: {error}"));
            }
        }
        let info = unsafe { info.assume_init() };
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        let code = unsafe { info.si_status() };
        let raw = if info.si_code == libc::CLD_EXITED {
            code << 8
        } else {
            code | if info.si_code == libc::CLD_DUMPED {
                0x80
            } else {
                0
            }
        };
        Ok(Some(ExitStatus::from_raw(raw)))
    }
    pub fn poll(&mut self) -> Result<Option<ExitStatus>, String> {
        if let Some(finished) = &self.finished {
            return finished.clone().map(Some);
        }
        if self.received.load(Ordering::Acquire) != 0 {
            self.stop()?;
            return self
                .finished
                .as_ref()
                .expect("finished process")
                .clone()
                .map(Some);
        }
        self.exit_status()
    }
    pub fn wait(&mut self) -> Result<ExitStatus, String> {
        if let Some(finished) = &self.finished {
            return finished.clone();
        }
        // poll observes forwarded signals as well as an ordinary child exit.
        loop {
            if let Some(status) = self.poll()? {
                return Ok(status);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    pub fn stop(&mut self) -> Result<(), String> {
        if let Some(finished) = &self.finished {
            return finished.clone().map(|_| ());
        }
        let received = self.received.load(Ordering::Acquire);
        signal_group(self.id(), if received == 0 { SIGTERM } else { received })
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_millis(500);
        // Give descendants their grace period even if the leader exits first.
        // The leader stays unreaped until the group's final signal.
        while Instant::now() < deadline && signal_group(self.id(), 0).map_err(|e| e.to_string())? {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut groups = groups()?.lock().map_err(|e| e.to_string())?;
        signal_group(self.id(), libc::SIGKILL).map_err(|e| e.to_string())?;
        groups.remove(&self.id());
        let finished = self.child.wait().map_err(|e| e.to_string());
        self.finished = Some(finished.clone());
        finished.map(|_| ())
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

/// Keep the file in place so every process locks the same inode.
pub fn development_lock(root: &Path) -> Result<fs::File, String> {
    let directory = root.join("target");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let path = directory.join("devkit-development.lock");
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| format!("development lock {}: {e}", path.display()))?;
    file.try_lock()
        .map_err(|e| format!("development stack already running or lock unavailable: {e}"))?;
    Ok(file)
}

/// Hash source contents, including additions and removals, without build outputs
/// or files under `excluded`.
pub fn source_hash_excluding(
    root: &Path,
    excluded: &[std::path::PathBuf],
) -> Result<blake3::Hash, String> {
    crate::files::hash(&[root.to_owned()], &[], false, |path, directory| {
        if excluded.iter().any(|excluded| path.starts_with(excluded)) {
            return false;
        }
        if directory {
            path == root
                || !matches!(
                    path.file_name().and_then(|s| s.to_str()),
                    Some("target" | ".git" | ".direnv" | "fuzz")
                )
        } else {
            path.extension()
                .is_some_and(|e| e == "rs" || e == "toml" || e == "json")
                || path.file_name().is_some_and(|name| name == "Cargo.lock")
        }
    })
}

#[cfg(test)]
mod tests;
