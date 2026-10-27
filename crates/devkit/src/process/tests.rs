use super::*;
#[test]
fn source_changes_include_new_removed_and_same_length_edits() {
    let dir = tempfile::tempdir().unwrap();
    let first = source_hash(dir.path()).unwrap();
    fs::write(dir.path().join("new.rs"), "one").unwrap();
    let second = source_hash(dir.path()).unwrap();
    assert_ne!(first, second);
    fs::write(dir.path().join("new.rs"), "two").unwrap();
    assert_ne!(second, source_hash(dir.path()).unwrap());
    fs::remove_file(dir.path().join("new.rs")).unwrap();
    assert_eq!(first, source_hash(dir.path()).unwrap());
    fs::create_dir(dir.path().join("target")).unwrap();
    fs::write(dir.path().join("target/ignored.rs"), "build").unwrap();
    assert_eq!(first, source_hash(dir.path()).unwrap());
}
#[test]
fn failed_spawn_and_failed_child_are_reported() {
    assert!(Process::start(&mut Command::new("/does-not-exist")).is_err());
    let mut child = Process::start(Command::new("sh").args(["-c", "exit 7"])).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(7));
}
#[test]
fn stopping_process_stops_its_descendants() {
    let dir = tempfile::tempdir().unwrap();
    let pid = dir.path().join("pid");
    let mut child = Process::start(
        Command::new("sh")
            .args(["-c", "sleep 300 & echo $! > \"$1\"; wait", "sh"])
            .arg(&pid),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pid.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let descendant = fs::read_to_string(pid).unwrap();
    child.stop().unwrap();
    child.stop().unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", descendant.trim()));
    assert!(stat.is_err() || stat.unwrap().split_whitespace().nth(2) == Some("Z"));
}

#[test]
fn selected_runtime_output_is_excluded_from_watch_changes() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("runtime");
    fs::create_dir(&output).unwrap();
    let before = source_hash_excluding(dir.path(), std::slice::from_ref(&output)).unwrap();
    fs::write(output.join("status.json"), "{}").unwrap();
    assert_eq!(
        before,
        source_hash_excluding(dir.path(), &[output]).unwrap()
    );
}
