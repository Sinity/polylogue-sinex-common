use super::*;

#[test]
fn schema_template_and_test_clones() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    assert!(server.ready());
    server
        .ensure_roles(&["demo_writer", "demo_writer"])
        .unwrap();
    let bootstrap = |server: &Server, database: &str| -> Result {
        server
            .sql(
                database,
                "CREATE TABLE notes (id integer PRIMARY KEY, body text)",
            )
            .map(drop)
    };
    let schema = Schema {
        project: "demo",
        hash: "abc123".into(),
        bootstrap: &bootstrap,
    };
    let databases = server.ensure_schema(&schema).unwrap();
    assert_eq!(databases.schema, "demo_schema_abc123");
    // A second ensure reuses what exists.
    let failing = |_: &Server, _: &str| -> Result { Err("must not run".into()) };
    let again = Schema {
        project: "demo",
        hash: "abc123".into(),
        bootstrap: &failing,
    };
    assert_eq!(server.ensure_schema(&again).unwrap(), databases);
    // The compile database is read-only; the template is not touched.
    assert!(
        server
            .sql(&databases.schema, "INSERT INTO notes VALUES (1, 'x')")
            .is_err()
    );

    // Tests clone the template.
    let env = server.test_env(&databases.template);
    let socket = env[0].1.clone();
    let one = TestDatabase {
        socket: PathBuf::from(&socket),
        name: "t_1_1_1".into(),
    };
    server
        .clone_database(&databases.template, &one.name)
        .unwrap();
    one.sql("INSERT INTO notes VALUES (1, 'one')").unwrap();
    let name = one.name.clone();
    drop(one);
    assert!(!server.database_exists(&name).unwrap());

    // Unused schema versions go, schema and template together; a fresh
    // one stays.
    assert!(server.reap(&Reap::default()).unwrap().is_empty());
    for name in [&databases.schema, &databases.template] {
        server
            .sql(
                "postgres",
                &format!("COMMENT ON DATABASE {name} IS 'devkit used 1'"),
            )
            .unwrap();
    }
    let mut dropped = server.reap(&Reap::default()).unwrap();
    dropped.sort();
    assert_eq!(
        dropped,
        vec![databases.schema.clone(), databases.template.clone()]
    );
}

fn demo_schema(server: &Server, project: &str, hash: &str) -> SchemaDatabases {
    let bootstrap = |server: &Server, database: &str| -> Result {
        server
            .sql(database, "CREATE TABLE t (id integer)")
            .map(drop)
    };
    server
        .ensure_schema(&Schema {
            project,
            hash: hash.into(),
            bootstrap: &bootstrap,
        })
        .unwrap()
}

/// A pid no process has: exited tests look like this.
fn dead_pid() -> u32 {
    (4_000_000..4_194_304)
        .find(|pid| !pid_alive(*pid))
        .expect("a free pid")
}

#[test]
fn reap_drops_test_databases_of_exited_tests() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "reap", "abc");
    let now = now_secs();
    let dead = dead_pid();
    let exited = format!("t_{dead}_{now}_1");
    let running = format!("t_{}_{now}_2", std::process::id());
    let ancient = format!("t_{}_1_3", std::process::id());
    for name in [&exited, &running, &ancient] {
        server.clone_database(&databases.template, name).unwrap();
    }
    // Seven failed ones from exited tests: the newest five stay.
    let failed: Vec<String> = (0..7)
        .map(|i| format!("t_{dead}_{now}_{}", 10 + i))
        .collect();
    for (i, name) in failed.iter().enumerate() {
        server.clone_database(&databases.template, name).unwrap();
        server
            .sql(
                "postgres",
                &format!("COMMENT ON DATABASE {name} IS 'devkit failed {i}'"),
            )
            .unwrap();
    }
    // An exited test's database that something is still connected to stays.
    let connected = format!("t_{dead}_{now}_4");
    server
        .clone_database(&databases.template, &connected)
        .unwrap();
    let mut holder = Command::new("psql")
        .args(["-X", "-q", "-h"])
        .arg(server.socket())
        .args(["-d", &connected, "-c", "SELECT pg_sleep(60)"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    while server
        .sql(
            "postgres",
            &format!("SELECT count(*) FROM pg_stat_activity WHERE datname = '{connected}'"),
        )
        .unwrap()
        .trim()
        == "0"
    {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let mut dropped = server.reap(&Reap::default()).unwrap();
    dropped.sort();
    let mut expected = vec![exited, ancient, failed[0].clone(), failed[1].clone()];
    expected.sort();
    assert_eq!(dropped, expected);
    assert!(server.database_exists(&running).unwrap());
    assert!(server.database_exists(&connected).unwrap());
    for name in &failed[2..] {
        assert!(server.database_exists(name).unwrap());
    }
    let _ = holder.kill();
    let _ = holder.wait();
}

#[test]
fn reap_keeps_the_newest_schema_versions() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let now = now_secs();
    for i in 0..5u64 {
        let databases = demo_schema(&server, "keep", &format!("h{i}"));
        for name in [&databases.schema, &databases.template] {
            server
                .sql(
                    "postgres",
                    &format!(
                        "COMMENT ON DATABASE {name} IS 'devkit used {}'",
                        now - 10 + i
                    ),
                )
                .unwrap();
        }
    }
    // Another project's versions are counted separately.
    demo_schema(&server, "other", "x");
    let policy = Reap {
        schemas: 3,
        ..Reap::default()
    };
    let mut dropped = server.reap(&policy).unwrap();
    dropped.sort();
    assert_eq!(
        dropped,
        vec![
            "keep_schema_h0",
            "keep_schema_h1",
            "keep_template_h0",
            "keep_template_h1"
        ]
    );
    assert!(server.database_exists("keep_schema_h2").unwrap());
    assert!(server.database_exists("other_schema_x").unwrap());
}

#[test]
fn a_failed_test_keeps_its_database_marked_failed() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "fail", "abc");
    let name = format!("t_{}_{}_9", std::process::id(), now_secs());
    server.clone_database(&databases.template, &name).unwrap();
    let socket = server.socket().to_owned();
    let kept = name.clone();
    let outcome = std::panic::catch_unwind(move || {
        let _db = TestDatabase { socket, name: kept };
        panic!("the test failed");
    });
    assert!(outcome.is_err());
    let comment = server
        .sql(
            "postgres",
            &format!(
                "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = '{name}'"
            ),
        )
        .unwrap();
    assert!(comment.starts_with("devkit failed "), "{comment}");
}

#[test]
fn names_are_checked() {
    assert!(SchemaDatabases::new("bad-name", "x").is_err());
    assert!(check_name("demo_schema_0123").is_ok());
    assert!(check_name("drop table").is_err());
}

#[test]
fn hash_covers_contents_and_extras() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.sql"), "one").unwrap();
    let first = schema_hash(&[dir.path().to_owned()], &["pg 18"]).unwrap();
    assert_eq!(
        first,
        schema_hash(&[dir.path().to_owned()], &["pg 18"]).unwrap()
    );
    assert_ne!(
        first,
        schema_hash(&[dir.path().to_owned()], &["pg 17"]).unwrap()
    );
    let copy = tempfile::tempdir().unwrap();
    fs::write(copy.path().join("a.sql"), "one").unwrap();
    assert_eq!(
        first,
        schema_hash(&[copy.path().to_owned()], &["pg 18"]).unwrap()
    );
    fs::write(dir.path().join("a.sql"), "two").unwrap();
    assert_ne!(
        first,
        schema_hash(&[dir.path().to_owned()], &["pg 18"]).unwrap()
    );
}

#[test]
fn reap_keeps_a_live_disconnected_bootstrap_and_drops_a_dead_owners() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let dead = format!("devkit_building_{}_1", dead_pid());
    server
        .sql("postgres", &format!("CREATE DATABASE {dead}"))
        .unwrap();
    let bootstrap = |server: &Server, database: &str| -> Result {
        assert_eq!(building_owner(database), Some(std::process::id()));
        assert!(database.len() <= 63);
        assert_eq!(
            server
                .sql(
                    "postgres",
                    &format!("SELECT count(*) FROM pg_stat_activity WHERE datname = '{database}'")
                )?
                .trim(),
            "0"
        );
        // This is the failing interleaving: another run reaps while cargo
        // compiles, before schema application opens its first connection.
        let dropped = server.reap(&Reap::default())?;
        assert!(dropped.contains(&dead));
        assert!(!dropped.contains(&database.to_owned()));
        server
            .sql(database, "CREATE TABLE marker (id integer)")
            .map(drop)
    };
    let databases = server
        .ensure_schema(&Schema {
            project: "bootstrap",
            hash: "owned".into(),
            bootstrap: &bootstrap,
        })
        .unwrap();
    assert_eq!(
        server
            .sql(&databases.schema, "SELECT count(*) FROM marker")
            .unwrap()
            .trim(),
        "0"
    );
    assert!(!server.database_exists(&dead).unwrap());
}

#[test]
fn failed_bootstrap_removes_its_owned_database() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let building = std::cell::RefCell::new(String::new());
    let bootstrap = |_: &Server, database: &str| -> Result {
        building.replace(database.to_owned());
        Err("deliberate bootstrap failure".into())
    };
    let error = server
        .ensure_schema(&Schema {
            project: "failure",
            hash: "owned".into(),
            bootstrap: &bootstrap,
        })
        .unwrap_err();
    assert!(error.contains("deliberate bootstrap failure"));
    assert!(!server.database_exists(&building.borrow()).unwrap());
}
