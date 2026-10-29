use super::*;
use std::{
    process::Child,
    time::{Duration, Instant},
};

// A failed assertion must not leave synthetic servers holding nextest's pipes.
struct FixtureChild(Child);
impl Drop for FixtureChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn fixture(command: &mut Command) -> FixtureChild {
    let child = FixtureChild(command.spawn().unwrap());
    eprintln!(
        "fixture child identity: {}",
        fs::read_to_string(format!("/proc/{}/stat", child.0.id())).unwrap()
    );
    child
}

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
    let mut owner = fixture(Command::new("sleep").arg("60"));
    let mut server = fixture(Command::new("sleep").arg("300").process_group(0));
    watch(owner.0.id(), &owned, &group_cleanup(server.0.id(), &owned));
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    wait_until("the server to be stopped", || {
        server.0.try_wait().unwrap().is_some()
    });
    wait_until("the directory to be removed", || !owned.exists());
}

#[test]
fn the_watchdog_leaves_an_orderly_shutdown_alone() {
    let dir = tempfile::tempdir().unwrap();
    let owned = dir.path().join("devkit-x-1-2");
    fs::create_dir(&owned).unwrap();
    let marker = dir.path().join("cleanup-ran");
    let mut owner = fixture(Command::new("sleep").arg("60"));
    watch(
        owner.0.id(),
        &owned,
        &format!("touch {}", shell_quote(&marker.display().to_string())),
    );
    // The orderly path removes the directory itself, then the owner exits.
    fs::remove_dir(&owned).unwrap();
    std::thread::sleep(Duration::from_secs(3));
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
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
