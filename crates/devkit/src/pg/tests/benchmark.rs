use super::*;
use std::time::Instant;

/// Explicit benchmark of the two clone strategies, in a throwaway cluster:
/// 50 clones one after another, then four slots each replacing its database
/// twelve times at once, as concurrent tests do. The default template size
/// matches the largest observed Sinex schema template.
#[test]
#[ignore = "clone timing benchmark; run explicitly with --run-ignored only --no-capture"]
fn benchmark_database_clones() {
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
                let clone = |name: &str| {
                    server.sql("postgres", &format!("CREATE DATABASE {name} TEMPLATE clone_bench_template STRATEGY {strategy}")).unwrap();
                };
                let mut names = Vec::new();
                let started = Instant::now();
                for i in 0..50 {
                    let name = format!("clone_bench_{round}_{i}");
                    clone(&name);
                    names.push(name);
                }
                let clone_time = started.elapsed();
                assert_clone_contents(&server, &names[0], &expected);
                let started = Instant::now();
                for name in names {
                    server.drop_database(&name).unwrap();
                }
                println!(
                    "workload={workload} round={round} strategy={strategy} shape=sequential clones=50 clone_ms={:.3} drop_ms={:.3}",
                    clone_time.as_secs_f64() * 1000.0,
                    started.elapsed().as_secs_f64() * 1000.0
                );
                // Each slot drops its previous database and clones anew, as
                // a test taking the slot does.
                let started = Instant::now();
                std::thread::scope(|scope| {
                    for slot in 0..4 {
                        let clone = &clone;
                        let server = &server;
                        scope.spawn(move || {
                            let name = format!("clone_bench_slot{slot}");
                            for _ in 0..12 {
                                server.drop_database(&name).unwrap();
                                clone(&name);
                            }
                        });
                    }
                });
                let slot_time = started.elapsed();
                assert_clone_contents(&server, "clone_bench_slot0", &expected);
                for slot in 0..4 {
                    server
                        .drop_database(&format!("clone_bench_slot{slot}"))
                        .unwrap();
                }
                println!(
                    "workload={workload} round={round} strategy={strategy} shape=four_slots clones=48 total_ms={:.3}",
                    slot_time.as_secs_f64() * 1000.0
                );
            }
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
