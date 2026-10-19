//! PostgreSQL for development: one server, one database per schema version,
//! and a fresh clone of it for every test.
//!
//! - **Server.** The per-user service, when it runs, listens on the socket in
//!   `$XDG_RUNTIME_DIR/devkit/pg`. Without it (CI, Nix builds, a host that
//!   does not run the service) [`Server::start`] runs a throwaway cluster on
//!   tmpfs, owned by the calling process and removed when dropped. Neither
//!   listens on TCP, so nothing collides across checkouts.
//! - **Schema databases.** A project's schema is identified by a hash of its
//!   sources ([`schema_hash`]). [`Server::ensure_schema`] builds
//!   `<project>_template_<hash>` once, by running the project's bootstrap,
//!   and copies it to `<project>_schema_<hash>`, the database compile-time
//!   query checks connect to. Worktrees with the same schema share both; a
//!   schema change gives a new name, so `DATABASE_URL` changes and exactly
//!   the crates that check queries against it rebuild.
//! - **Test databases.** Tests clone the template ([`test_database`]): the
//!   same schema the queries were checked against, fresh for every test.
//!   They are separate from the compile database because a database cannot
//!   be cloned while anything is connected to it, and compilers and editors
//!   hold connections.
//!
//! The PostgreSQL client tools (`initdb`, `pg_ctl`, `psql`) must be on PATH.
//! Everything here is disposable: losing it costs a rebuild of the schema
//! databases.
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

pub type Result<T = ()> = std::result::Result<T, String>;

/// Settings for a throwaway cluster.
#[derive(Debug, Clone, Default)]
pub struct ClusterOptions {
    /// `shared_preload_libraries`, such as `timescaledb`.
    pub preload: Vec<String>,
    /// Further `-c name=value` server settings.
    pub settings: Vec<(String, String)>,
}

/// A running PostgreSQL server reachable through a Unix socket directory.
pub struct Server {
    socket: PathBuf,
    owned: Option<Owned>,
}

struct Owned {
    data: PathBuf,
}

impl Server {
    /// The socket directory of the per-user service.
    pub fn service_socket() -> Option<PathBuf> {
        env::var_os("XDG_RUNTIME_DIR").map(|dir| Path::new(&dir).join("devkit").join("pg"))
    }

    /// The per-user service if it is running.
    pub fn service() -> Option<Self> {
        let socket = Self::service_socket()?;
        let server = Self {
            socket,
            owned: None,
        };
        server.ready().then_some(server)
    }

    /// The per-user service if it is running, otherwise a throwaway cluster.
    pub fn service_or_start(options: &ClusterOptions) -> Result<Self> {
        match Self::service() {
            Some(server) => Ok(server),
            None => Self::start(options),
        }
    }

    /// Starts a throwaway cluster on tmpfs, without fsync, stopped and
    /// removed when the returned value is dropped.
    pub fn start(options: &ClusterOptions) -> Result<Self> {
        let base = if Path::new("/dev/shm").is_dir() {
            PathBuf::from("/dev/shm")
        } else {
            env::temp_dir()
        };
        // A short path: Unix socket paths are limited to about 100 bytes.
        let root = base.join(format!("devkit-pg-{}-{}", std::process::id(), unique()));
        let data = root.join("data");
        fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        let owned = Owned { data: data.clone() };
        let server = Self {
            socket: root.clone(),
            owned: Some(owned),
        };
        run(Command::new("initdb")
            .args([
                "--no-sync",
                "--no-locale",
                "--auth=trust",
                "--data-checksums",
                "-D",
            ])
            .arg(&data))?;
        let mut settings = vec![
            ("listen_addresses".to_owned(), String::new()),
            ("fsync".to_owned(), "off".to_owned()),
            ("synchronous_commit".to_owned(), "off".to_owned()),
            ("full_page_writes".to_owned(), "off".to_owned()),
            ("max_connections".to_owned(), "300".to_owned()),
        ];
        if !options.preload.is_empty() {
            settings.push((
                "shared_preload_libraries".to_owned(),
                options.preload.join(","),
            ));
        }
        settings.extend(options.settings.iter().cloned());
        let mut server_options = format!("-k {}", root.display());
        for (name, value) in &settings {
            server_options += &format!(" -c {name}={value}");
        }
        run(Command::new("pg_ctl")
            .arg("-D")
            .arg(&data)
            .arg("-l")
            .arg(root.join("postgres.log"))
            .args(["-o", &server_options, "-w", "start"]))?;
        Ok(server)
    }

    /// Whether the server accepts connections.
    pub fn ready(&self) -> bool {
        self.socket.join(".s.PGSQL.5432").exists()
            && Command::new("pg_isready")
                .arg("-q")
                .arg("-h")
                .arg(&self.socket)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// A connection URL for `database`, as `user` (default: the OS user,
    /// which owns the cluster).
    pub fn url(&self, database: &str, user: Option<&str>) -> String {
        url(&self.socket, database, user)
    }

    /// Runs SQL in `database` as the OS user and returns the unaligned output.
    pub fn sql(&self, database: &str, sql: &str) -> Result<String> {
        sql_at(&self.socket, database, sql)
    }

    /// Creates each role that does not exist yet.
    pub fn ensure_roles(&self, roles: &[&str]) -> Result {
        for role in roles {
            check_name(role)?;
            self.sql(
                "postgres",
                &format!(
                    "DO $$ BEGIN CREATE ROLE {role} LOGIN; EXCEPTION WHEN duplicate_object THEN NULL; END $$"
                ),
            )?;
        }
        Ok(())
    }

    pub fn database_exists(&self, name: &str) -> Result<bool> {
        check_name(name)?;
        Ok(self
            .sql(
                "postgres",
                &format!("SELECT 1 FROM pg_database WHERE datname = '{name}'"),
            )?
            .trim()
            == "1")
    }

    /// Makes sure the schema and template databases for `schema` exist,
    /// building them with its bootstrap if they do not.
    pub fn ensure_schema(&self, schema: &Schema<'_>) -> Result<SchemaDatabases> {
        let databases = SchemaDatabases::new(schema.project, &schema.hash)?;
        if !self.database_exists(&databases.template)? {
            let building = format!("{}_building_{}", databases.template, unique());
            self.sql("postgres", &format!("CREATE DATABASE {building}"))?;
            if let Err(error) = (schema.bootstrap)(self, &building) {
                let _ = self.sql(
                    "postgres",
                    &format!("DROP DATABASE IF EXISTS {building} WITH (FORCE)"),
                );
                return Err(format!("schema bootstrap: {error}"));
            }
            // Renaming publishes the template only once it is complete. If
            // another process finished first, its copy is used.
            if self
                .sql(
                    "postgres",
                    &format!("ALTER DATABASE {building} RENAME TO {}", databases.template),
                )
                .is_err()
            {
                let _ = self.sql(
                    "postgres",
                    &format!("DROP DATABASE IF EXISTS {building} WITH (FORCE)"),
                );
                if !self.database_exists(&databases.template)? {
                    return Err(format!("could not publish {}", databases.template));
                }
            } else {
                self.sql(
                    "postgres",
                    &format!(
                        "ALTER DATABASE {} WITH IS_TEMPLATE true",
                        databases.template
                    ),
                )?;
            }
        }
        if !self.database_exists(&databases.schema)? {
            let created = self.sql(
                "postgres",
                &format!(
                    "CREATE DATABASE {} TEMPLATE {} STRATEGY FILE_COPY",
                    databases.schema, databases.template
                ),
            );
            if created.is_ok() {
                // Compile-time checks only prepare statements; nothing should
                // change the schema database.
                self.sql(
                    "postgres",
                    &format!(
                        "ALTER DATABASE {} SET default_transaction_read_only = on",
                        databases.schema
                    ),
                )?;
            } else if !self.database_exists(&databases.schema)? {
                created?;
            }
        }
        // Schema and template databases are only prepared against and copied,
        // so TimescaleDB need not load in them; otherwise every one starts a
        // background scheduler and they exhaust the server's worker limit.
        // Database settings are not copied to clones, so test databases load
        // it as usual.
        // The template is checked through its copy: a session on the template
        // would make concurrent clones of it fail.
        let installed = self.sql(
            &databases.schema,
            "SELECT 1 FROM pg_extension WHERE extname = 'timescaledb'",
        )?;
        if installed.trim() == "1" {
            for name in [&databases.template, &databases.schema] {
                self.sql(
                    "postgres",
                    &format!("ALTER DATABASE {name} SET timescaledb.disable_load = on"),
                )?;
            }
        }
        // Mark both as used now, for `drop_unused`.
        let now = now_secs();
        for name in [&databases.template, &databases.schema] {
            self.sql(
                "postgres",
                &format!("COMMENT ON DATABASE {name} IS 'devkit used {now}'"),
            )?;
        }
        Ok(databases)
    }

    /// A fresh database copied from `template`.
    pub fn clone_database(&self, template: &str, name: &str) -> Result {
        check_name(template)?;
        check_name(name)?;
        self.sql(
            "postgres",
            &format!("CREATE DATABASE {name} TEMPLATE {template} STRATEGY FILE_COPY"),
        )
        .map(drop)
    }

    pub fn drop_database(&self, name: &str) -> Result {
        check_name(name)?;
        self.sql(
            "postgres",
            &format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        )
        .map(drop)
    }

    /// Drops schema and template databases not used for `days`, and test
    /// databases left over from runs older than a day. Returns their names.
    pub fn drop_unused(&self, days: u64) -> Result<Vec<String>> {
        let now = now_secs();
        let rows = self.sql(
            "postgres",
            "SELECT d.datname, coalesce(shobj_description(d.oid, 'pg_database'), '')
             FROM pg_database d WHERE d.datname ~ '_(schema|template|building)_' OR d.datname LIKE 't\\_%'",
        )?;
        let mut dropped = Vec::new();
        for line in rows.lines() {
            let Some((name, comment)) = line.split_once('|') else {
                continue;
            };
            let used = comment
                .strip_prefix("devkit used ")
                .and_then(|s| s.trim().parse::<u64>().ok());
            let stale = if let Some(test) = name.strip_prefix("t_") {
                test.split('_')
                    .nth(1)
                    .and_then(|s| s.parse::<u64>().ok())
                    .is_some_and(|t| now > t + 86_400)
            } else if name.contains("_building_") {
                true
            } else {
                used.is_none_or(|t| now > t + days * 86_400)
            };
            if stale {
                if name.contains("_template_") {
                    let _ = self.sql(
                        "postgres",
                        &format!("ALTER DATABASE {name} WITH IS_TEMPLATE false"),
                    );
                }
                self.drop_database(name)?;
                dropped.push(name.to_owned());
            }
        }
        Ok(dropped)
    }

    /// `PGHOST` and `PGPORT` for the PostgreSQL command-line tools.
    pub fn pg_env(&self) -> Vec<(String, String)> {
        vec![
            ("PGHOST".into(), self.socket.display().to_string()),
            ("PGPORT".into(), "5432".into()),
        ]
    }

    /// Environment that lets tests in a child process clone `template` with
    /// [`test_database`].
    pub fn test_env(&self, template: &str) -> Vec<(String, String)> {
        vec![
            ("DEVKIT_PG_SOCKET".into(), self.socket.display().to_string()),
            ("DEVKIT_PG_TEMPLATE".into(), template.to_owned()),
        ]
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(owned) = &self.owned {
            let _ = Command::new("pg_ctl")
                .arg("-D")
                .arg(&owned.data)
                .args(["-m", "immediate", "-w", "stop"])
                .output();
            let _ = fs::remove_dir_all(&self.socket);
        }
    }
}

/// How a project builds its schema into an empty database.
pub struct Schema<'a> {
    /// Lowercase letters, digits and underscores.
    pub project: &'a str,
    /// From [`schema_hash`].
    pub hash: String,
    /// Applies the schema to the named database on the server.
    pub bootstrap: &'a dyn Fn(&Server, &str) -> Result,
}

/// The two databases of one schema version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDatabases {
    /// What compile-time query checks connect to (`DATABASE_URL`).
    pub schema: String,
    /// What tests clone.
    pub template: String,
}

impl SchemaDatabases {
    pub fn new(project: &str, hash: &str) -> Result<Self> {
        check_name(project)?;
        let short: String = hash.chars().take(16).collect();
        Ok(Self {
            schema: format!("{project}_schema_{short}"),
            template: format!("{project}_template_{short}"),
        })
    }
}

/// A hash of the files under `paths` (recursively, in name order, `target`
/// directories skipped) and of `extra` strings such as server and extension
/// versions. File names enter the hash relative to the path they were found
/// under, so every checkout of the same sources gets the same hash.
pub fn schema_hash(paths: &[PathBuf], extra: &[&str]) -> Result<String> {
    let mut hasher = blake3::Hasher::new();
    for path in paths {
        let mut files = Vec::new();
        collect(path, &mut files)?;
        files.sort();
        for file in &files {
            let relative = file.strip_prefix(path).unwrap_or(file);
            hasher.update(relative.to_string_lossy().as_bytes());
            hasher.update(&[0]);
            hasher.update(&fs::read(file).map_err(|e| format!("{}: {e}", file.display()))?);
        }
        hasher.update(&[1]);
    }
    for value in extra {
        hasher.update(value.as_bytes());
        hasher.update(&[0]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collect(path: &Path, files: &mut Vec<PathBuf>) -> Result {
    let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_file() {
        files.push(path.to_owned());
    } else if meta.is_dir() && path.file_name().is_none_or(|n| n != "target") {
        for entry in fs::read_dir(path).map_err(|e| format!("{}: {e}", path.display()))? {
            collect(&entry.map_err(|e| e.to_string())?.path(), files)?;
        }
    }
    Ok(())
}

/// A database for one test, cloned from the template named in the
/// environment that [`Server::test_env`] produced. It is dropped when the
/// value is, unless the test is panicking: then it is kept for inspection
/// and its name printed.
pub struct TestDatabase {
    socket: PathBuf,
    pub name: String,
}

pub fn test_database() -> Result<TestDatabase> {
    let socket = env::var_os("DEVKIT_PG_SOCKET")
        .map(PathBuf::from)
        .ok_or("DEVKIT_PG_SOCKET is not set; run the tests through xtask")?;
    let template = env::var("DEVKIT_PG_TEMPLATE").map_err(|_| "DEVKIT_PG_TEMPLATE is not set")?;
    check_name(&template)?;
    let name = format!("t_{}_{}_{}", std::process::id(), now_secs(), unique());
    sql_at(
        &socket,
        "postgres",
        &format!("CREATE DATABASE {name} TEMPLATE {template} STRATEGY FILE_COPY"),
    )?;
    Ok(TestDatabase { socket, name })
}

impl TestDatabase {
    pub fn url(&self, user: Option<&str>) -> String {
        url(&self.socket, &self.name, user)
    }

    pub fn sql(&self, sql: &str) -> Result<String> {
        sql_at(&self.socket, &self.name, sql)
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!(
                "devkit: kept test database {} at {}",
                self.name,
                self.socket.display()
            );
        } else {
            let _ = sql_at(
                &self.socket,
                "postgres",
                &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name),
            );
        }
    }
}

fn url(socket: &Path, database: &str, user: Option<&str>) -> String {
    let user = user.map_or(String::new(), |u| format!("{u}@localhost"));
    format!("postgresql://{user}/{database}?host={}", socket.display())
}

fn sql_at(socket: &Path, database: &str, sql: &str) -> Result<String> {
    let output = Command::new("psql")
        .args(["-X", "-q", "-A", "-t", "-v", "ON_ERROR_STOP=1", "-h"])
        .arg(socket)
        .args(["-d", database, "-c", sql])
        .env_remove("PGDATABASE")
        .env_remove("PGUSER")
        .output()
        .map_err(|e| format!("psql: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "psql: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn run(command: &mut Command) -> Result {
    let output = command.output().map_err(|e| format!("{command:?}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{:?} failed: {}\n{}",
            command.get_program(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

/// Database and role names are used unquoted in SQL, so they are limited to
/// lowercase letters, digits and underscores.
fn check_name(name: &str) -> Result {
    let valid = !name.is_empty()
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if valid {
        Ok(())
    } else {
        Err(format!("not a usable database or role name: {name:?}"))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn unique() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as u64);
    nanos ^ (COUNTER.fetch_add(1, Ordering::Relaxed) << 32)
}

#[cfg(test)]
mod tests {
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

        // Unused schema versions go; a fresh one stays.
        assert!(server.drop_unused(30).unwrap().is_empty());
        server
            .sql(
                "postgres",
                &format!(
                    "COMMENT ON DATABASE {} IS 'devkit used 1'",
                    databases.schema
                ),
            )
            .unwrap();
        assert_eq!(
            server.drop_unused(30).unwrap(),
            vec![databases.schema.clone()]
        );
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
}
