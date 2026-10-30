use super::*;

mod capabilities;
mod lifecycle;

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
    assert_eq!(server.ensure_schema(&again).unwrap(), databases);
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
        .find(|pid| !crate::owner::alive(*pid))
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
    let dropped = server
        .reap(&Reap {
            failed: 0,
            ..Reap::default()
        })
        .unwrap();
    assert!(!dropped.contains(&name));
    assert!(server.database_exists(&name).unwrap());
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

#[test]
fn every_test_run_reaps_without_building_a_schema() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "runs", "same");
    for run in 0..2 {
        let abandoned = format!("t_{}_{}_{run}", dead_pid(), now_secs());
        server
            .clone_database(&databases.template, &abandoned)
            .unwrap();
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
        assert!(!server.database_exists(&abandoned).unwrap());
        assert!(server.database_exists(&databases.template).unwrap());
    }
}

/// Explicit benchmark: 50 clones per strategy, with configurable repetitions.
/// The default size matches the largest observed Sinex schema template.
#[test]
#[ignore = "clone timing benchmark; run explicitly with --ignored --nocapture"]
fn benchmark_database_clones() {
    use std::time::Instant;
    let directory = env::var_os("DEVKIT_BENCH_DIRECTORY").map(PathBuf::from);
    let server = if let Some(directory) = &directory {
        Server::start_in(&ClusterOptions::default(), directory).unwrap()
    } else {
        Server::start(&ClusterOptions::default()).unwrap()
    };
    let target: u64 = env::var("DEVKIT_BENCH_TEMPLATE_BYTES")
        .unwrap_or_else(|_| "12662463".into())
        .parse()
        .unwrap();
    server
        .sql("postgres", "CREATE DATABASE clone_bench_template")
        .unwrap();
    server.sql("clone_bench_template", "CREATE TABLE sample (id bigint PRIMARY KEY, body text); ALTER TABLE sample ALTER COLUMN body SET STORAGE EXTERNAL").unwrap();
    let mut next = 0;
    let bytes = loop {
        let bytes: u64 = server
            .sql(
                "postgres",
                "SELECT pg_database_size('clone_bench_template')",
            )
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        if bytes >= target {
            break bytes;
        }
        let rows = ((target - bytes) / 1200).max(1);
        server.sql("clone_bench_template", &format!("INSERT INTO sample SELECT i, repeat(md5(i::text), 32) FROM generate_series({next}, {}) i", next + rows - 1)).unwrap();
        next += rows;
    };
    let expected = server
        .sql(
            "clone_bench_template",
            "SELECT count(*), sum(id), sum(length(body)) FROM sample",
        )
        .unwrap();
    server.sql("postgres", "CHECKPOINT").unwrap();
    println!(
        "template_bytes={bytes} target_bytes={target} postgres={}",
        server
            .sql("postgres", "SHOW server_version")
            .unwrap()
            .trim()
    );
    let rounds: u64 = env::var("DEVKIT_BENCH_ROUNDS")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .unwrap();
    assert!(rounds > 0, "the benchmark needs at least one round");
    let writes = env::var_os("DEVKIT_BENCH_WRITES").is_some();
    // A cluster-wide checkpoint also writes buffers from unrelated databases.
    if writes {
        server
            .sql("postgres", "CREATE DATABASE clone_bench_writes")
            .unwrap();
        server.sql("clone_bench_writes", "CREATE TABLE sample (id integer PRIMARY KEY, body text); ALTER TABLE sample ALTER COLUMN body SET STORAGE EXTERNAL; INSERT INTO sample SELECT i, repeat(md5(i::text), 32) FROM generate_series(0, 500000) i").unwrap();
        server.sql("postgres", "CHECKPOINT").unwrap();
    }
    let workloads = if writes {
        vec!["idle", "writes"]
    } else {
        vec!["idle"]
    };
    for workload in workloads {
        let mut writer = if workload == "writes" {
            Some(Writer(Command::new("psql").args(["-X", "-q", "-v", "ON_ERROR_STOP=1", "-h"])
            .arg(server.socket()).args(["-d", "clone_bench_writes", "-c",
                "DO $$ DECLARE first_row integer := 0; BEGIN LOOP UPDATE sample SET body = reverse(body) WHERE id BETWEEN first_row AND first_row + 127; first_row := (first_row + 128) % 500000; COMMIT; PERFORM pg_sleep(0.001); END LOOP; END $$"])
            .stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap()))
        } else {
            None
        };
        for round in 0..rounds {
            let strategies = if round % 2 == 0 {
                ["FILE_COPY", "WAL_LOG"]
            } else {
                ["WAL_LOG", "FILE_COPY"]
            };
            for strategy in strategies {
                let mut names = Vec::new();
                let started = Instant::now();
                for i in 0..50 {
                    let name = format!("clone_bench_{round}_{i}");
                    server.sql("postgres", &format!("CREATE DATABASE {name} TEMPLATE clone_bench_template STRATEGY {strategy}")).unwrap();
                    names.push(name);
                }
                let clone_time = started.elapsed();
                assert_clone_contents(&server, &names[0], &expected);
                let started = Instant::now();
                for name in names {
                    server.drop_database(&name).unwrap();
                }
                println!(
                    "workload={workload} round={round} strategy={strategy} clones=50 clone_ms={:.3} drop_ms={:.3}",
                    clone_time.as_secs_f64() * 1000.0,
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
            let started = Instant::now();
            let names = std::thread::scope(|scope| {
                let (ready, databases) = std::sync::mpsc::sync_channel(4);
                let server = &server;
                scope.spawn(move || {
                    for i in 0..50 {
                        let name = format!("clone_bench_pool_{round}_{i}");
                        server.sql("postgres", &format!("CREATE DATABASE {name} TEMPLATE clone_bench_template STRATEGY WAL_LOG")).unwrap();
                        ready.send(name).unwrap();
                    }
                });
                let mut names = Vec::new();
                for i in 0..50 {
                    let ready = databases.recv().unwrap();
                    let name = format!("clone_bench_claim_{round}_{i}");
                    server
                        .sql(
                            "postgres",
                            &format!("ALTER DATABASE {ready} RENAME TO {name}"),
                        )
                        .unwrap();
                    names.push(name);
                }
                names
            });
            let clone_time = started.elapsed();
            assert_clone_contents(&server, &names[0], &expected);
            let started = Instant::now();
            for name in names {
                server.drop_database(&name).unwrap();
            }
            println!(
                "workload={workload} round={round} strategy=POOL_WAL_LOG clones=50 clone_ms={:.3} drop_ms={:.3}",
                clone_time.as_secs_f64() * 1000.0,
                started.elapsed().as_secs_f64() * 1000.0
            );
        }
        if let Some(writer) = &mut writer {
            assert!(
                writer.0.try_wait().unwrap().is_none(),
                "background writer exited before the benchmark finished"
            );
        }
        drop(writer);
    }
}

fn assert_clone_contents(server: &Server, database: &str, expected: &str) {
    assert_eq!(
        server
            .sql(
                database,
                "SELECT count(*), sum(id), sum(length(body)) FROM sample"
            )
            .unwrap(),
        expected
    );
    server
        .sql(database, "INSERT INTO sample VALUES (-1, 'clone only')")
        .unwrap();
    assert_eq!(
        server
            .sql(
                "clone_bench_template",
                "SELECT count(*), sum(id), sum(length(body)) FROM sample"
            )
            .unwrap(),
        expected
    );
}

struct Writer(std::process::Child);

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn reap_keeps_new_and_unreadable_schema_stamps_even_with_no_versions_kept() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "recent", "one");
    let policy = Reap {
        schemas: 0,
        days: 0,
        failed: 0,
    };
    for comment in [
        format!("'devkit used {}'", now_secs()),
        "NULL".into(),
        "'devkit used unreadable'".into(),
    ] {
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
}

#[test]
fn reap_rechecks_use_and_connections_after_choosing_names() {
    let server = Server::start(&ClusterOptions::default()).unwrap();
    let databases = demo_schema(&server, "recheck", "one");
    let names = vec![databases.schema.clone(), databases.template.clone()];
    for name in &names {
        server
            .sql(
                "postgres",
                &format!("COMMENT ON DATABASE {name} IS 'devkit used 1'"),
            )
            .unwrap();
    }
    // The schema becomes used after cleanup has chosen both names.
    server.stamp_used(&databases.schema).unwrap();
    assert!(server.drop_unused(names.clone()).unwrap().is_empty());
    server
        .sql(
            "postgres",
            &format!(
                "COMMENT ON DATABASE {} IS 'devkit used 1'",
                databases.schema
            ),
        )
        .unwrap();
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
            failed: 0,
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
    let name = format!("t_{}_1_1", dead_pid());
    server
        .sql("postgres", &format!("CREATE DATABASE {name}"))
        .unwrap();
    let runtime = tempfile::tempdir_in(server.socket()).unwrap();
    fs::create_dir(runtime.path().join("devkit")).unwrap();
    std::os::unix::fs::symlink(server.socket(), runtime.path().join("devkit/pg")).unwrap();
    let service = Server {
        socket: runtime.path().join("devkit/pg"),
        owned: None,
    };
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
        assert!(server.database_exists(&name).unwrap());
    }
    Server::prepare_run(&[(
        "DEVKIT_PG_SOCKET".into(),
        server.socket().display().to_string(),
    )]);
    assert!(!server.database_exists(&name).unwrap());
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
        failed: 0,
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
