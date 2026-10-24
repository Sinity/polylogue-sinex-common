use super::*;

#[test]
fn managed_scratch_is_the_parent_and_drop_removes_only_our_child() {
    let owner = tempfile::tempdir().unwrap();
    fs::write(owner.path().join("keep"), b"owner").unwrap();
    let unrelated = tempfile::tempdir().unwrap();
    let one = RunTemp::with_roots(unrelated.path(), Some(owner.path())).unwrap();
    let two = RunTemp::with_roots(unrelated.path(), Some(owner.path())).unwrap();
    assert_eq!(one.directory.parent(), Some(owner.path()));
    assert_eq!(fs::canonicalize(one.path()).unwrap(), one.directory);
    assert_ne!(one.directory, two.directory);
    fs::write(one.path().join("synthetic"), b"bytes").unwrap();
    let child = one.directory.clone();
    drop(one);
    assert!(!child.exists());
    assert!(two.directory.exists());
    assert_eq!(fs::read(owner.path().join("keep")).unwrap(), b"owner");
    assert!(unrelated.path().exists());
}

#[test]
fn standalone_drop_preserves_the_inherited_directory() {
    let inherited = tempfile::tempdir().unwrap();
    fs::write(inherited.path().join("keep"), b"caller").unwrap();
    let temp = RunTemp::with_roots(inherited.path(), None).unwrap();
    let child = temp.directory.clone();
    fs::write(temp.path().join("synthetic"), b"test").unwrap();
    drop(temp);
    assert!(!child.exists());
    assert_eq!(fs::read(inherited.path().join("keep")).unwrap(), b"caller");
}

#[test]
fn invalid_managed_scratch_does_not_escape_to_standalone_storage() {
    let inherited = tempfile::tempdir().unwrap();
    let absent = inherited.path().join("not-created");
    assert!(RunTemp::with_roots(inherited.path(), Some(&absent)).is_err());
    assert!(!absent.exists());
    assert_eq!(fs::read_dir(inherited.path()).unwrap().count(), 0);
}

#[cfg(target_os = "linux")]
#[test]
fn long_managed_root_keeps_short_sockets_accessible_from_child_processes() {
    let owner = tempfile::tempdir().unwrap();
    let long = owner.path().join("long-job-root-".repeat(12));
    fs::create_dir(&long).unwrap();
    let temp = RunTemp::with_roots(owner.path(), Some(&long)).unwrap();
    assert_eq!(temp.directory.parent(), Some(long.as_path()));
    assert!(temp.path().as_os_str().len() < 40);
    let result = Command::new(env::current_exe().unwrap())
        .args(["--exact", "run::temp_tests::socket_child", "--nocapture"])
        .env("DEVKIT_TEST_SOCKET_ROOT", temp.path())
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("1 passed"));
    assert!(temp.directory.join("archive/run/daemon.sock").exists());
    let child = temp.directory.clone();
    drop(temp);
    assert!(!child.exists());
    assert!(long.exists());
}

#[cfg(target_os = "linux")]
#[test]
fn socket_child() {
    use std::os::unix::net::UnixListener;
    let Some(root) = env::var_os("DEVKIT_TEST_SOCKET_ROOT") else {
        return;
    };
    let socket = PathBuf::from(root).join("archive/run/daemon.sock");
    fs::create_dir_all(socket.parent().unwrap()).unwrap();
    let _listener = UnixListener::bind(socket).unwrap();
}

#[test]
#[ignore = "waits to be cancelled; run explicitly under AgentCTL"]
fn native_cancellation_fixture() {
    let owner = PathBuf::from(env::var_os("AGENTCTL_SCRATCH").expect("job-owned scratch required"));
    let temp = RunTemp::new().unwrap();
    assert_eq!(temp.directory.parent(), Some(owner.as_path()));
    fs::write(temp.path().join("synthetic-wal"), vec![7; 1 << 20]).unwrap();
    fs::write(
        owner.join("fixture-ready.json"),
        serde_json::json!({
            "pid": process::id(), "directory": temp.directory, "visible": temp.path(),
        })
        .to_string(),
    )
    .unwrap();
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the job was not cancelled within 30 seconds"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
