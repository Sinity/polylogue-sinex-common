use super::*;

/// Every test database a project has on the server, in name order.
fn test_databases(server: &Server, project: &str) -> Vec<String> {
    server
        .sql(
            "postgres",
            &format!(
                "SELECT datname FROM pg_database
                 WHERE datname ~ '^{project}_(test|large|failed)[0-9]+$' ORDER BY datname"
            ),
        )
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn take(server: &Server, template: &str, family: Family, slots: &Slots) -> TestDatabase {
    TestDatabase::take(server.socket(), template, family, slots).unwrap()
}

#[test]
fn slots_are_taken_in_order_and_each_test_gets_a_fresh_clone() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "order", "one");
    let slots = Slots {
        ordinary: 2,
        large: 1,
        failed: 5,
    };
    let first = take(&server, &databases.template, Family::Ordinary, &slots);
    let second = take(&server, &databases.template, Family::Ordinary, &slots);
    let large = take(&server, &databases.template, Family::Large, &slots);
    assert_eq!(
        [&first.name, &second.name, &large.name],
        ["order_test0", "order_test1", "order_large0"]
    );
    drop(first);
    assert!(!server.database_exists("order_test0").unwrap());
    // A database that a killed holder of the slot left is replaced.
    server
        .clone_database(&databases.template, "order_test0")
        .unwrap();
    server
        .sql("order_test0", "INSERT INTO t VALUES (1)")
        .unwrap();
    let reused = take(&server, &databases.template, Family::Ordinary, &slots);
    assert_eq!(reused.name, "order_test0");
    assert_eq!(reused.sql("SELECT count(*) FROM t").unwrap().trim(), "0");
    drop((reused, second, large));
    assert!(test_databases(&server, "order").is_empty());
}

/// Holds `slots` ordinary slots of `template` in a child process until the
/// returned child is killed; returns once it holds them.
fn hold_in_child(server: &Server, template: &str, slots: usize) -> std::process::Child {
    let ready = tempfile::tempdir().unwrap();
    let child = Command::new(env::current_exe().unwrap())
        .args(["--exact", "pg::tests::slots::slot_holder_child"])
        .env("DEVKIT_SLOT_HOLDER_READY", ready.path())
        .envs(server.test_env(template))
        .env("DEVKIT_PG_SLOTS", slots.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while fs::read_dir(ready.path()).unwrap().count() == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "the child takes a slot"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child
}

#[test]
fn a_test_waits_for_a_slot_while_another_process_holds_all() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "wait", "one");
    let slots = Slots {
        ordinary: 1,
        large: 1,
        failed: 5,
    };
    let mut holder = hold_in_child(&server, &databases.template, 1);
    let (taken, waiting) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let database = take(&server, &databases.template, Family::Ordinary, &slots);
            taken.send(database.name.clone()).unwrap();
        });
        assert!(
            waiting
                .recv_timeout(std::time::Duration::from_millis(500))
                .is_err(),
            "a test holding no slot waits while the only slot is taken"
        );
        holder.kill().unwrap();
        holder.wait().unwrap();
        assert_eq!(
            waiting
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap(),
            "wait_test0"
        );
    });
}

/// Two tests that each hold a slot and wait for a second one would wait for
/// each other forever, so a test that holds one takes its second at once.
#[test]
fn a_test_holding_a_slot_takes_another_without_waiting() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "pair", "one");
    let slots = Slots {
        ordinary: 1,
        large: 1,
        failed: 5,
    };
    let mut other = hold_in_child(&server, &databases.template, 1);
    // The child holds pair_test0; this process holds none, so it would wait.
    // A slot of the other family does not count.
    let large = take(&server, &databases.template, Family::Large, &slots);
    let (taken, result) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            let first = take(&server, &databases.template, Family::Ordinary, &slots);
            let second = take(&server, &databases.template, Family::Ordinary, &slots);
            taken
                .send((first.name.clone(), second.name.clone()))
                .unwrap();
        });
        assert!(
            result
                .recv_timeout(std::time::Duration::from_millis(500))
                .is_err(),
            "the first ordinary slot is waited for"
        );
        other.kill().unwrap();
        other.wait().unwrap();
        assert_eq!(
            result
                .recv_timeout(std::time::Duration::from_secs(30))
                .unwrap(),
            ("pair_test0".to_owned(), "pair_test1".to_owned())
        );
    });
    drop(large);
    assert!(test_databases(&server, "pair").is_empty());
}

#[test]
fn failed_databases_are_kept_newest_first_in_a_fixed_set() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "ring", "one");
    let slots = Slots {
        ordinary: 2,
        large: 1,
        failed: 2,
    };
    for failure in 0..4 {
        let database = take(&server, &databases.template, Family::Ordinary, &slots);
        database
            .sql(&format!("INSERT INTO t VALUES ({failure})"))
            .unwrap();
        // The test's own connection is still open when it fails.
        let mut session = Command::new("psql")
            .args(["-X", "-q", "-h"])
            .arg(server.socket())
            .args(["-d", &database.name, "-c", "SELECT pg_sleep(60)"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _database = database;
            panic!("the test failed");
        }));
        assert!(outcome.is_err());
        let _ = session.kill();
        let _ = session.wait();
    }
    assert_eq!(
        test_databases(&server, "ring"),
        ["ring_failed0", "ring_failed1"]
    );
    let mut kept: Vec<String> = ["ring_failed0", "ring_failed1"]
        .iter()
        .map(|name| {
            server
                .sql(name, "SELECT id FROM t")
                .unwrap()
                .trim()
                .to_owned()
        })
        .collect();
    kept.sort();
    assert_eq!(kept, ["2", "3"]);
    // With none kept, a failed database stays in its slot until it is taken.
    let none = Slots { failed: 0, ..slots };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _database = take(&server, &databases.template, Family::Ordinary, &none);
        panic!("the test failed");
    }));
    assert!(outcome.is_err());
    assert_eq!(
        test_databases(&server, "ring"),
        ["ring_failed0", "ring_failed1", "ring_test0"]
    );
}

/// Run as a child process by the test below: takes a slot, writes into it,
/// says so, and waits to be killed.
#[test]
fn slot_holder_child() {
    let Some(ready) = env::var_os("DEVKIT_SLOT_HOLDER_READY") else {
        return;
    };
    let database = test_database().unwrap();
    database.sql("INSERT INTO t VALUES (1)").unwrap();
    fs::write(
        PathBuf::from(ready).join(std::process::id().to_string()),
        &database.name,
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_secs(120));
}

#[test]
fn test_databases_bounded_by_slots_when_tests_are_killed() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "killed", "one");
    let ready = tempfile::tempdir().unwrap();
    let slots = Slots {
        ordinary: 3,
        large: 1,
        failed: 2,
    };
    let mut children: Vec<_> = (0..12)
        .map(|_| {
            Command::new(env::current_exe().unwrap())
                .args(["--exact", "pg::tests::slots::slot_holder_child"])
                .env("DEVKIT_SLOT_HOLDER_READY", ready.path())
                .envs(server.test_env(&databases.template))
                .env("DEVKIT_PG_SLOTS", "3")
                .env("DEVKIT_PG_FAILED_KEPT", "2")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while fs::read_dir(ready.path()).unwrap().count() < 3 {
        assert!(
            std::time::Instant::now() < deadline,
            "three tests take the three slots"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // The other nine wait for a slot; every one is killed mid-run.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(fs::read_dir(ready.path()).unwrap().count(), 3);
    for child in &mut children {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    // A waiting test may take a slot freed by an earlier kill and be killed
    // between dropping the slot's database and cloning it again, so fewer
    // may remain, never more, and only slot names.
    let left = test_databases(&server, "killed");
    assert!(left.len() <= 3, "{left:?}");
    assert!(
        left.iter()
            .all(|name| ["killed_test0", "killed_test1", "killed_test2"].contains(&name.as_str())),
        "{left:?}"
    );
    // The next tests take the killed tests' slots at once and get fresh
    // clones; the count stays at the number of slots.
    let next: Vec<_> = (0..3)
        .map(|_| take(&server, &databases.template, Family::Ordinary, &slots))
        .collect();
    for database in &next {
        assert_eq!(database.sql("SELECT count(*) FROM t").unwrap().trim(), "0");
    }
    assert_eq!(test_databases(&server, "killed").len(), 3);
    drop(next);
    assert!(test_databases(&server, "killed").is_empty());
}

#[test]
fn slot_configuration_comes_from_the_environment() {
    let mut child = Command::new(env::current_exe().unwrap());
    child
        .args(["--exact", "pg::tests::slots::slot_configuration_child"])
        .env("DEVKIT_SLOT_CONFIGURATION_PROBE", "1")
        .env("DEVKIT_PG_SLOTS", "7")
        .env("DEVKIT_PG_LARGE_SLOTS", "2")
        .env("DEVKIT_PG_FAILED_KEPT", "0");
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[test]
fn slot_configuration_child() {
    if env::var_os("DEVKIT_SLOT_CONFIGURATION_PROBE").is_none() {
        return;
    }
    assert_eq!(
        Slots::from_env().unwrap(),
        Slots {
            ordinary: 7,
            large: 2,
            failed: 0
        }
    );
    // SAFETY: this child process runs only this test, on one thread.
    unsafe { env::set_var("DEVKIT_PG_SLOTS", "0") };
    assert!(Slots::from_env().unwrap_err().contains("DEVKIT_PG_SLOTS"));
    // SAFETY: as above.
    unsafe { env::set_var("DEVKIT_PG_SLOTS", "many") };
    assert!(Slots::from_env().is_err());
}
