use super::*;

mod benchmark;
mod capabilities;
mod lifecycle;
mod slots;

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
    let user = Server::at(server.socket().to_owned());
    let databases = user.ensure_schema(&schema).unwrap();
    assert_eq!(databases.schema, "demo_schema_abc123");
    server
        .sql(
            "postgres",
            &format!(
                "ALTER DATABASE {} WITH IS_TEMPLATE false",
                databases.template
            ),
        )
        .unwrap();
    // A second ensure reuses the data and restores template status.
    let failing = |_: &Server, _: &str| -> Result { Err("must not run".into()) };
    let again = Schema {
        project: "demo",
        hash: "abc123".into(),
        bootstrap: &failing,
    };
    assert_eq!(user.ensure_schema(&again).unwrap(), databases);
    assert_eq!(
        server
            .sql(
                "postgres",
                &format!(
                    "SELECT datistemplate FROM pg_database WHERE datname = '{}'",
                    databases.template
                )
            )
            .unwrap()
            .trim(),
        "t"
    );
    // Compile-time checks read the schema; test clones remain writable.
    assert!(
        server
            .sql(&databases.schema, "INSERT INTO notes VALUES (1, 'x')")
            .is_err()
    );

    // A test takes a slot and clones the template into it; a passing test
    // leaves nothing behind.
    let one = TestDatabase::take(
        server.socket(),
        &databases.template,
        Family::Ordinary,
        &Slots::default(),
    )
    .unwrap();
    assert_eq!(one.name, "demo_test0");
    one.sql("INSERT INTO notes VALUES (1, 'one')").unwrap();
    drop(one);
    assert!(!server.database_exists("demo_test0").unwrap());

    // An old version stays while a server value that ensured it lives, and
    // goes, schema and template together, once none does.
    for name in [&databases.schema, &databases.template] {
        server
            .sql(
                "postgres",
                &format!("COMMENT ON DATABASE {name} IS 'devkit used 1'"),
            )
            .unwrap();
    }
    assert!(server.reap(&Reap::default()).unwrap().is_empty());
    drop(user);
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
    // Through a value of its own, dropped here, so the version is not in use
    // afterwards.
    Server::at(server.socket().to_owned())
        .ensure_schema(&Schema {
            project,
            hash: hash.into(),
            bootstrap: &bootstrap,
        })
        .unwrap()
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
                        now - 7200 + i
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
fn names_are_checked() {
    assert!(SchemaDatabases::new("bad-name", "x").is_err());
    assert!(SchemaDatabases::new("demo", "x;drop table t").is_err());
    assert!(SchemaDatabases::new(&"a".repeat(50), "0123456789abcdef").is_err());
    assert!(SchemaDatabases::new(&"a".repeat(37), "0123456789abcdef").is_ok());
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
    let linked = tempfile::tempdir().unwrap();
    let linked_file = linked.path().join("linked.sql");
    fs::write(&linked_file, "linked one").unwrap();
    std::os::unix::fs::symlink(&linked_file, dir.path().join("linked.sql")).unwrap();
    let with_link = schema_hash(&[dir.path().to_owned()], &["pg 18"]).unwrap();
    fs::write(&linked_file, "linked two").unwrap();
    assert_ne!(
        with_link,
        schema_hash(&[dir.path().to_owned()], &["pg 18"]).unwrap()
    );
    fs::remove_file(dir.path().join("linked.sql")).unwrap();
    fs::write(dir.path().join("a.sql"), "two").unwrap();
    assert_ne!(
        first,
        schema_hash(&[dir.path().to_owned()], &["pg 18"]).unwrap()
    );
    assert_ne!(
        schema_hash(&[], &["a\0b"]).unwrap(),
        schema_hash(&[], &["a", "b"]).unwrap()
    );
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

#[test]
fn schema_builds_and_reaping_in_parallel_keep_every_live_schema() {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let complete = AtomicBool::new(false);
    let (started, building) = mpsc::sync_channel(0);
    std::thread::scope(|scope| {
        let builder = scope.spawn(|| {
            let result = (|| -> Result<Vec<SchemaDatabases>> {
                let mut all = Vec::new();
                for i in 0..8 {
                    let bootstrap = |server: &Server, name: &str| -> Result {
                        if i == 0 {
                            started.send(()).unwrap();
                        }
                        server
                            .sql(name, "CREATE TABLE marker (id integer)")
                            .map(drop)
                    };
                    let databases = server.ensure_schema(&Schema {
                        project: "parallel",
                        hash: format!("h{i}"),
                        bootstrap: &bootstrap,
                    })?;
                    server.sql(&databases.schema, "SELECT count(*) FROM marker")?;
                    all.push(databases);
                }
                Ok(all)
            })();
            complete.store(true, Ordering::Release);
            result
        });
        building.recv().unwrap();
        let policy = Reap {
            schemas: 0,
            days: 0,
        };
        while !complete.load(Ordering::Acquire) {
            assert!(server.reap(&policy).unwrap().is_empty());
        }
        for databases in builder.join().unwrap().unwrap() {
            assert!(server.database_exists(&databases.template).unwrap());
            assert_eq!(
                server
                    .sql(&databases.schema, "SELECT count(*) FROM marker")
                    .unwrap()
                    .trim(),
                "0"
            );
        }
    });
}

#[test]
fn a_run_without_an_explicit_server_leaves_databases_alone() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    // A test database name of earlier devkit versions: cleanup drops it.
    let name = "t_1_1_1";
    server
        .sql("postgres", &format!("CREATE DATABASE {name}"))
        .unwrap();
    let runtime = tempfile::tempdir_in(server.socket()).unwrap();
    fs::create_dir(runtime.path().join("devkit")).unwrap();
    std::os::unix::fs::symlink(server.socket(), runtime.path().join("devkit/pg")).unwrap();
    let service = Server::at(runtime.path().join("devkit/pg"));
    assert!(service.ready());
    for ambient_socket in [false, true] {
        let marker = runtime.path().join(format!("probe-{ambient_socket}"));
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "pg::tests::cleanup_without_server_child"])
            .env("DEVKIT_CLEANUP_PROBE", &marker)
            .env_remove("DEVKIT_PG_SOCKET")
            .env("XDG_RUNTIME_DIR", runtime.path());
        if ambient_socket {
            child.env("DEVKIT_PG_SOCKET", server.socket());
        }
        assert!(child.status().unwrap().success());
        assert_eq!(fs::read_to_string(marker).unwrap(), "done");
        assert!(server.database_exists(name).unwrap());
    }
    Server::prepare_run(&[(
        "DEVKIT_PG_SOCKET".into(),
        server.socket().display().to_string(),
    )]);
    assert!(!server.database_exists(name).unwrap());
}

#[test]
fn cleanup_without_server_child() {
    if let Some(marker) = std::env::var_os("DEVKIT_CLEANUP_PROBE") {
        Server::prepare_run(&[]);
        fs::write(PathBuf::from(marker), "done").unwrap();
    }
}

#[test]
fn schema_names_keep_the_whole_project_name_during_cleanup() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "example_schema_project", "abc");
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
    assert_eq!(dropped, vec![databases.schema, databases.template]);
    assert!(
        server
            .drop_unused(vec!["absent_schema_one".into()])
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reap_does_not_drop_an_old_schema_during_a_template_rebuild() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "rebuilding", "one");
    server
        .sql(
            "postgres",
            &format!(
                "ALTER DATABASE {} WITH IS_TEMPLATE false",
                databases.template
            ),
        )
        .unwrap();
    server.drop_database(&databases.template).unwrap();
    server
        .sql(
            "postgres",
            &format!(
                "COMMENT ON DATABASE {} IS 'devkit used 1'",
                databases.schema
            ),
        )
        .unwrap();
    let policy = Reap {
        schemas: 0,
        days: 0,
    };
    let bootstrap = |server: &Server, building: &str| -> Result {
        let dropped =
            std::thread::scope(|scope| scope.spawn(|| server.reap(&policy)).join().unwrap())?;
        assert!(dropped.is_empty(), "{dropped:?}");
        assert!(server.database_exists(&databases.schema)?);
        server
            .sql(building, "CREATE TABLE t (id integer)")
            .map(drop)
    };
    assert_eq!(
        server
            .ensure_schema(&Schema {
                project: "rebuilding",
                hash: "one".into(),
                bootstrap: &bootstrap
            })
            .unwrap(),
        databases
    );
    assert_eq!(
        server
            .sql(&databases.schema, "SELECT count(*) FROM t")
            .unwrap()
            .trim(),
        "0"
    );
    assert!(server.reap(&policy).unwrap().is_empty());
}

/// PostgreSQL refuses root, so a cluster started by root runs as another
/// account; either way the caller connects as a superuser without naming one.
#[test]
fn a_throwaway_cluster_runs_unprivileged_and_admits_its_caller() {
    use std::os::unix::fs::MetadataExt;
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let cluster_owner = fs::metadata(server.socket().join("data")).unwrap().uid();
    // SAFETY: geteuid has no preconditions and cannot fail.
    let caller = unsafe { libc::geteuid() };
    if caller == 0 {
        assert_ne!(cluster_owner, 0);
    } else {
        assert_eq!(cluster_owner, caller);
    }
    assert_eq!(
        server
            .sql(
                "postgres",
                "SELECT rolsuper FROM pg_roles WHERE rolname = current_user"
            )
            .unwrap()
            .trim(),
        "t"
    );
    let socket = server.socket().to_owned();
    drop(server);
    assert!(!socket.exists());
}

#[test]
fn schema_version_in_use_never_dropped() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "inuse", "one");
    let user = Server::at(server.socket().to_owned());
    assert_eq!(
        user.ensure_schema(&Schema {
            project: "inuse",
            hash: "one".into(),
            bootstrap: &|_: &Server, _: &str| -> Result { Err("must not run".into()) },
        })
        .unwrap(),
        databases
    );
    let policy = Reap {
        schemas: 0,
        days: 0,
    };
    // Old, missing and unreadable stamps alike: a version in use stays.
    for comment in ["'devkit used 1'", "NULL", "'devkit used unreadable'"] {
        for name in [&databases.schema, &databases.template] {
            server
                .sql(
                    "postgres",
                    &format!("COMMENT ON DATABASE {name} IS {comment}"),
                )
                .unwrap();
        }
        assert!(server.reap(&policy).unwrap().is_empty());
        assert!(server.database_exists(&databases.schema).unwrap());
        assert!(server.database_exists(&databases.template).unwrap());
    }
    drop(user);
    let mut dropped = server.reap(&policy).unwrap();
    dropped.sort();
    assert_eq!(dropped, vec![databases.schema, databases.template]);
}

#[test]
fn reap_rechecks_use_and_connections_after_choosing_names() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "recheck", "one");
    let names = vec![databases.schema.clone(), databases.template.clone()];
    // The version comes into use after cleanup has chosen both names.
    let user = Server::at(server.socket().to_owned());
    user.ensure_schema(&Schema {
        project: "recheck",
        hash: "one".into(),
        bootstrap: &|_: &Server, _: &str| -> Result { Err("must not run".into()) },
    })
    .unwrap();
    assert!(server.drop_unused(names.clone()).unwrap().is_empty());
    drop(user);
    // A connection opened after the names were chosen keeps them too.
    let mut holder = Command::new("psql")
        .args(["-X", "-q", "-h"])
        .arg(server.socket())
        .args(["-d", &databases.schema, "-c", "SELECT pg_sleep(60)"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server
        .sql(
            "postgres",
            &format!(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = '{}'",
                databases.schema
            ),
        )
        .unwrap()
        .trim()
        == "0"
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let dropped = server.drop_unused(names.clone()).unwrap();
    let _ = holder.kill();
    let _ = holder.wait();
    assert!(dropped.is_empty());
    for name in &names {
        assert!(server.database_exists(name).unwrap());
    }
    let template = server
        .sql(
            "postgres",
            &format!(
                "SELECT datistemplate FROM pg_database WHERE datname = '{}'",
                databases.template
            ),
        )
        .unwrap();
    assert_eq!(template.trim(), "t");
}

#[test]
fn half_built_template_outlives_its_builder_only() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    // Left by builders that were killed: one of another version, which
    // cleanup drops, and one of the version built below, which its next
    // builder replaces.
    for name in ["bootstrap_building_killed", "bootstrap_building_owned"] {
        server
            .sql("postgres", &format!("CREATE DATABASE {name}"))
            .unwrap();
        server.sql(name, "CREATE TABLE stale (id integer)").unwrap();
    }
    let bootstrap = |server: &Server, database: &str| -> Result {
        assert_eq!(database, "bootstrap_building_owned");
        assert!(
            server.sql(database, "SELECT count(*) FROM stale").is_err(),
            "the killed builder's database was replaced"
        );
        // Another run cleans up while this bootstrap compiles, before it
        // opens its first connection.
        let dropped = std::thread::scope(|scope| {
            scope
                .spawn(|| Server::at(server.socket().to_owned()).reap(&Reap::default()))
                .join()
                .unwrap()
        })?;
        assert_eq!(dropped, vec!["bootstrap_building_killed".to_owned()]);
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
    assert!(!server.database_exists("bootstrap_building_owned").unwrap());
    assert!(!server.database_exists("bootstrap_building_killed").unwrap());
}

#[cfg(feature = "runner")]
#[test]
fn every_test_run_reaps_without_building_a_schema() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "runs", "same");
    for run in 0..2 {
        let stale = demo_schema(&server, "runs", &format!("old{run}"));
        for name in [&stale.schema, &stale.template] {
            server
                .sql(
                    "postgres",
                    &format!("COMMENT ON DATABASE {name} IS 'devkit used 1'"),
                )
                .unwrap();
        }
        let directory = tempfile::tempdir().unwrap();
        let mut run = crate::Run::unrecorded("reaping", "test", &[]);
        run.root = directory.path().to_owned();
        // Selection fails before Cargo starts, after run cleanup has happened.
        assert!(
            run.nextest(
                "test",
                &crate::Nextest {
                    args: vec!["--changed-base=missing-ref".into()],
                    env: server.test_env(&databases.template),
                    ..crate::Nextest::default()
                }
            )
            .is_err()
        );
        assert!(!server.database_exists(&stale.template).unwrap());
        assert!(!server.database_exists(&stale.schema).unwrap());
        assert!(server.database_exists(&databases.template).unwrap());
    }
}

#[test]
fn earlier_test_databases_go_once_nothing_is_connected() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    // An earlier version's build may be compiling without a connection.
    for name in ["t_1_2_3", "t_4_5_6", "t_other", "devkit_building_1_2"] {
        server
            .sql("postgres", &format!("CREATE DATABASE {name}"))
            .unwrap();
    }
    let mut holder = Command::new("psql")
        .args(["-X", "-q", "-h"])
        .arg(server.socket())
        .args(["-d", "t_4_5_6", "-c", "SELECT pg_sleep(60)"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while server
        .sql(
            "postgres",
            "SELECT count(*) FROM pg_stat_activity WHERE datname = 't_4_5_6'",
        )
        .unwrap()
        .trim()
        == "0"
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let dropped = server.reap(&Reap::default()).unwrap();
    let _ = holder.kill();
    let _ = holder.wait();
    assert_eq!(dropped, vec!["t_1_2_3".to_owned()]);
    assert!(server.database_exists("t_4_5_6").unwrap());
    assert!(server.database_exists("t_other").unwrap());
    assert!(server.database_exists("devkit_building_1_2").unwrap());
}

/// A clone fails while any session is connected to its template, so a built
/// template accepts no connections, and ensuring it ends sessions that a
/// template from before this rule still has.
#[test]
fn a_built_template_accepts_no_connections() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "sealed", "one");
    let allows = |server: &Server| {
        server
            .sql(
                "postgres",
                &format!(
                    "SELECT datallowconn FROM pg_database WHERE datname = '{}'",
                    databases.template
                ),
            )
            .unwrap()
            .trim()
            .to_owned()
    };
    assert_eq!(allows(&server), "f");
    assert!(server.sql(&databases.template, "SELECT 1").is_err());
    // A template that still allows connections, with a session open on it.
    server
        .sql(
            "postgres",
            &format!(
                "ALTER DATABASE {} WITH ALLOW_CONNECTIONS true",
                databases.template
            ),
        )
        .unwrap();
    let mut session = Command::new("psql")
        .args(["-X", "-q", "-h"])
        .arg(server.socket())
        .args(["-d", &databases.template, "-c", "SELECT pg_sleep(60)"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while server
        .sql(
            "postgres",
            &format!(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = '{}'",
                databases.template
            ),
        )
        .unwrap()
        .trim()
        == "0"
    {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        server
            .clone_database(&databases.template, "sealed_copy")
            .unwrap_err()
            .contains("being accessed by other users")
    );
    let again = demo_schema(&server, "sealed", "one");
    assert_eq!(again, databases);
    assert_eq!(allows(&server), "f");
    server
        .clone_database(&databases.template, "sealed_copy")
        .unwrap();
    let _ = session.kill();
    let _ = session.wait();
}
