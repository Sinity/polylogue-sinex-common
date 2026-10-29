use super::*;

#[test]
fn startup_failure_returns_the_postgres_log_and_removes_its_directory() {
    let base = tempfile::tempdir_in(if Path::new("/dev/shm").is_dir() {
        Path::new("/dev/shm")
    } else {
        Path::new("/tmp")
    })
    .unwrap();
    if crate::owner::running_as_root() {
        give_to_nobody(base.path()).unwrap();
    }
    let error = match Server::start_in(
        &ClusterOptions {
            settings: vec![("devkit_fixture_unknown_setting".into(), "on".into())],
            ..Default::default()
        },
        base.path(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("an unknown server setting must fail startup"),
    };
    assert!(error.contains("devkit_fixture_unknown_setting"), "{error}");
    assert_eq!(fs::read_dir(base.path()).unwrap().count(), 0);
}

#[test]
fn normal_shutdown_reaps_the_postmaster_in_a_nonreaping_fixture() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "pg::tests::lifecycle::nonreaping_child"])
        .env("DEVKIT_PG_REAP_PROBE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn nonreaping_child() {
    if std::env::var_os("DEVKIT_PG_REAP_PROBE").is_none() {
        return;
    }
    // Only this dedicated subprocess adopts its own orphaned descendants.
    // It deliberately does not reap them before the assertion below. This
    // models a container init without changing the host or product runtime.
    assert_eq!(unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1) }, 0);
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let root = server.socket().to_owned();
    let pid: libc::pid_t = fs::read_to_string(root.join("data/postmaster.pid"))
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    assert!(server.ready());
    drop(server);
    assert!(
        !root.exists(),
        "ordinary shutdown removes its owned directory"
    );
    let mut status = 0;
    // An owned Child must already have been waited. The old daemonized
    // postmaster returns its PID here; this probe reaps it before failing.
    let observed = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    let error = std::io::Error::last_os_error();
    if observed == 0 {
        // This is the exact fixture child, still reserved by waitpid. Avoid
        // leaving a live child if normal shutdown itself regresses.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
            libc::waitpid(pid, &mut status, 0);
        }
    }
    assert_eq!(
        observed, -1,
        "unreaped owned postmaster {pid}, status {status}"
    );
    assert_eq!(error.raw_os_error(), Some(libc::ECHILD));
}
