//! Throwaway servers that cannot outlive the process that started them.
//!
//! A test killed by a timeout or a signal never runs its `Drop`, and
//! PostgreSQL is daemonized by `pg_ctl`, so neither server would stop with
//! it. Each owned server's directory is named `<prefix>-<owner pid>-<id>`.
//! A watchdog per server stops it once the owner is gone, and every start
//! first sweeps directories whose owner is gone, in case a watchdog was
//! killed too.
use std::{
    fs,
    os::unix::process::CommandExt,
    path::Path,
    process::{Command, Stdio},
};

/// Starts a detached watchdog that runs `cleanup` (a shell command) once
/// process `owner` has exited, unless `dir` was removed first by an orderly
/// shutdown. It has its own process group and no inherited output, so a
/// test runner neither kills it with the test nor waits for it.
pub(crate) fn watch(owner: u32, dir: &Path, cleanup: &str) {
    let dir = shell_quote(&dir.display().to_string());
    let script = format!(
        "while kill -0 {owner} 2>/dev/null && [ -d {dir} ]; do sleep 2; done; \
         if [ -d {dir} ]; then {cleanup}; fi"
    );
    let _ = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
}

/// Runs `cleanup` for every `<base>/<prefix>-<pid>-<id>` directory whose
/// owner pid no longer exists. `cleanup` gets the directory's path as `$1`.
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
            let _ = Command::new("sh")
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
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn the_watchdog_stops_a_server_whose_owner_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let owned = dir.path().join("devkit-x-1-1");
        fs::create_dir(&owned).unwrap();
        let mut owner = Command::new("sleep").arg("60").spawn().unwrap();
        let mut server = Command::new("sleep")
            .arg("300")
            .process_group(0)
            .spawn()
            .unwrap();
        watch(
            owner.id(),
            &owned,
            &format!(
                "kill -KILL -- -{}; rm -rf {}",
                server.id(),
                shell_quote(&owned.display().to_string())
            ),
        );
        owner.kill().unwrap();
        owner.wait().unwrap();
        wait_until("the server to be stopped", || {
            server.try_wait().unwrap().is_some()
        });
        wait_until("the directory to be removed", || !owned.exists());
    }

    #[test]
    fn the_watchdog_leaves_an_orderly_shutdown_alone() {
        let dir = tempfile::tempdir().unwrap();
        let owned = dir.path().join("devkit-x-1-2");
        fs::create_dir(&owned).unwrap();
        let marker = dir.path().join("cleanup-ran");
        let mut owner = Command::new("sleep").arg("60").spawn().unwrap();
        watch(
            owner.id(),
            &owned,
            &format!("touch {}", shell_quote(&marker.display().to_string())),
        );
        // The orderly path removes the directory itself, then the owner exits.
        fs::remove_dir(&owned).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        owner.kill().unwrap();
        owner.wait().unwrap();
        std::thread::sleep(Duration::from_secs(3));
        assert!(!marker.exists());
    }

    #[test]
    fn the_sweep_clears_only_directories_of_exited_owners() {
        let dir = tempfile::tempdir().unwrap();
        let dead = (4_000_000..4_194_304).find(|pid| !alive(*pid)).unwrap();
        let orphan = dir.path().join(format!("devkit-x-{dead}-7"));
        let live = dir
            .path()
            .join(format!("devkit-x-{}-8", std::process::id()));
        let other = dir.path().join(format!("devkit-y-{dead}-9"));
        for path in [&orphan, &live, &other] {
            fs::create_dir(path).unwrap();
        }
        sweep(dir.path(), "devkit-x", r#"rm -rf "$1""#);
        assert!(!orphan.exists());
        assert!(live.exists());
        assert!(other.exists());
    }
}
