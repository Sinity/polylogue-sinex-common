//! Throwaway servers that cannot outlive the process that started them.
//!
//! A test killed by a timeout or a signal never runs its `Drop`, and
//! PostgreSQL is daemonized by `pg_ctl`, so neither server would stop with
//! it. Each owned server's directory is named `<prefix>-<owner pid>-<id>`.
//! A watchdog per server stops it once the owner is gone, and every start
//! first sweeps directories whose owner is gone, in case a watchdog was
//! killed too.
//!
//! Started as root, a server whose directory belongs to another account runs
//! as that account, and so do its cleanup commands: PostgreSQL refuses to
//! run as root.
use std::{
    fs,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::Path,
    process::{Command, Stdio},
};

/// Whether this process runs as root.
pub(crate) fn running_as_root() -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() == 0 }
}

/// When this process is root, runs `command` as the account that owns
/// `dir`, inside `dir`, so a root caller's server and its cleanup run as the
/// server's account. Otherwise leaves `command` as it is.
pub(crate) fn as_owner_of<'a>(command: &'a mut Command, dir: &Path) -> &'a mut Command {
    if running_as_root()
        && let Ok(metadata) = fs::metadata(dir)
        && metadata.uid() != 0
    {
        command
            .uid(metadata.uid())
            .gid(metadata.gid())
            .current_dir(dir);
    }
    command
}

/// Starts a detached watchdog that runs `cleanup` (a shell command) once
/// process `owner` has exited, unless `dir` was removed first by an orderly
/// shutdown. It has its own process group and no inherited output, so a
/// test runner neither kills it with the test nor waits for it. It runs as
/// `dir`'s owner ([`as_owner_of`]), which may not signal the owner, so it
/// checks `/proc` for the owner instead.
pub(crate) fn watch(owner: u32, dir: &Path, cleanup: &str) {
    let quoted = shell_quote(&dir.display().to_string());
    let script = format!(
        "while [ -d /proc/{owner} ] && [ -d {quoted} ]; do sleep 2; done; \
         if [ -d {quoted} ]; then {cleanup}; fi"
    );
    let _ = as_owner_of(&mut Command::new("sh"), dir)
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

/// Runs `cleanup` for every `<base>/<prefix>-<pid>-<id>` directory whose
/// owner pid no longer exists. `cleanup` gets the directory's path as `$1`
/// and runs as the directory's owner ([`as_owner_of`]).
pub(crate) fn sweep(base: &Path, prefix: &str, cleanup: &str) {
    let Ok(entries) = fs::read_dir(base) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix('-'))
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<u32>().ok())
        else {
            continue;
        };
        if !alive(pid) {
            let _ = as_owner_of(&mut Command::new("sh"), &entry.path())
                .arg("-c")
                .arg(cleanup)
                .arg("sh")
                .arg(entry.path())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Whether a process with this pid exists on this host.
pub(crate) fn alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests;
