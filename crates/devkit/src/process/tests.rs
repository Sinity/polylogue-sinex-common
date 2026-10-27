use super::*;

#[test]
fn one_development_stack_owns_each_checkout_until_exit() {
    let checkout = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let first = development_lock(checkout.path()).unwrap();
    assert!(development_lock(checkout.path()).is_err());
    let independent = development_lock(other.path()).unwrap();
    drop(first);
    assert!(development_lock(checkout.path()).is_ok());
    drop(independent);
}
#[test]
fn source_changes_include_new_removed_and_same_length_edits() {
    let dir = tempfile::tempdir().unwrap();
    let first = source_hash_excluding(dir.path(), &[]).unwrap();
    fs::write(dir.path().join("new.rs"), "one").unwrap();
    let second = source_hash_excluding(dir.path(), &[]).unwrap();
    assert_ne!(first, second);
    fs::write(dir.path().join("new.rs"), "two").unwrap();
    assert_ne!(second, source_hash_excluding(dir.path(), &[]).unwrap());
    fs::remove_file(dir.path().join("new.rs")).unwrap();
    assert_eq!(first, source_hash_excluding(dir.path(), &[]).unwrap());
    fs::create_dir(dir.path().join("target")).unwrap();
    fs::write(dir.path().join("target/ignored.rs"), "build").unwrap();
    assert_eq!(first, source_hash_excluding(dir.path(), &[]).unwrap());
    let linked = tempfile::tempdir().unwrap();
    fs::write(linked.path().join("outside.rs"), "outside").unwrap();
    std::os::unix::fs::symlink(linked.path(), dir.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(
        linked.path().join("outside.rs"),
        dir.path().join("linked.rs"),
    )
    .unwrap();
    assert_eq!(first, source_hash_excluding(dir.path(), &[]).unwrap());
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

#[test]
fn observing_an_exit_keeps_the_leader_until_the_group_is_stopped() {
    let mut child = Process::start(Command::new("sh").args(["-c", "exit 7"])).unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(7));
    assert!(Path::new(&format!("/proc/{}", child.id())).exists());
    assert_eq!(child.poll().unwrap().unwrap().code(), Some(7));
    child.stop().unwrap();
    assert!(!Path::new(&format!("/proc/{}", child.id())).exists());
    assert_eq!(child.wait().unwrap().code(), Some(7));
}

#[test]
fn signal_forwarding_child() {
    let Some(directory) = std::env::var_os("DEVKIT_SIGNAL_PROBE") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    let mut children = Vec::new();
    for i in 0..2 {
        let mut command = Command::new("sh");
        command.args(["-c", r#"trap 'echo INT >> "$1"; exit 0' INT; trap 'echo TERM >> "$1"; exit 0' TERM; touch "$2"; while :; do sleep 0.1; done"#, "sh"])
            .arg(directory.join(format!("signal-{i}"))).arg(directory.join(format!("ready-{i}")));
        children.push(Process::start(&mut command).unwrap());
    }
    fs::write(directory.join("parent-ready"), "ready").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut done = 0;
        for child in &mut children {
            if child.poll().unwrap().is_some() {
                done += 1;
            }
        }
        if done == children.len() {
            break;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn interrupt_and_termination_reach_all_service_groups() {
    for (signal, expected) in [(SIGINT, "INT"), (SIGTERM, "TERM")] {
        let dir = tempfile::tempdir().unwrap();
        let mut probe = Command::new(std::env::current_exe().unwrap());
        probe
            .args([
                "--exact",
                "process::tests::signal_forwarding_child",
                "--nocapture",
            ])
            .env("DEVKIT_SIGNAL_PROBE", dir.path());
        // The probe's terminal group is separate from this test's group.
        let mut probe = probe.process_group(0).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        for file in ["parent-ready", "ready-0", "ready-1"] {
            while !dir.path().join(file).exists() {
                assert!(Instant::now() < deadline);
                assert!(probe.try_wait().unwrap().is_none());
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        // Only the supervisor gets this signal. Its two service groups must
        // receive it through forwarding.
        assert_eq!(unsafe { libc::kill(probe.id() as i32, signal) }, 0);
        let status = loop {
            if let Some(status) = probe.try_wait().unwrap() {
                break status;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(status.success());
        for i in 0..2 {
            assert!(
                fs::read_to_string(dir.path().join(format!("signal-{i}")))
                    .unwrap()
                    .lines()
                    .any(|line| line == expected)
            );
        }
    }
}

#[test]
fn ordinary_run_steps_inherit_the_callers_group() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("group");
    let mut run = crate::Run::unrecorded("demo", "group", &[]);
    run.command(
        "ordinary step",
        Command::new("sh")
            .args(["-c", "read -r child_pid child_name child_state child_parent child_group child_rest < /proc/self/stat; printf '%s' \"$child_group\" > \"$1\"", "sh"])
            .arg(&output),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(output)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap(),
        unsafe { libc::getpgrp() }
    );
}

#[test]
fn descendants_get_time_to_stop_after_the_leader_exits() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("child.sh");
    let finished = dir.path().join("finished");
    let ready = dir.path().join("ready");
    fs::write(
        &script,
        r#"trap 'sleep 0.2; printf done > "$1"; exit 0' TERM
touch "$2"
while :; do sleep 1; done
"#,
    )
    .unwrap();
    let mut child = Process::start(
        Command::new("sh")
            .args([
                "-c",
                r#"trap 'exit 0' TERM; sh "$1" "$2" "$3" & wait"#,
                "sh",
            ])
            .arg(script)
            .arg(&finished)
            .arg(&ready),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    child.stop().unwrap();
    assert_eq!(fs::read_to_string(finished).unwrap(), "done");
}
