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
        Self::start_in(options, &base)
    }

    /// Starts a throwaway cluster on the chosen filesystem. The directory
    /// must exist and its path must fit PostgreSQL's Unix socket limit.
    /// The cluster is stopped and removed when the server is dropped.
    pub fn start_in(options: &ClusterOptions, base: &Path) -> Result<Self> {
        // Clusters left by killed runs whose watchdog died too.
        crate::owner::sweep(
            base,
            "devkit-pg",
            r#"pg_ctl -D "$1/data" -m immediate -w stop; rm -rf "$1""#,
        );
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
        // pg_ctl daemonizes the server, so it would survive a killed test;
        // the watchdog stops it once this process is gone.
        let quoted_data = crate::owner::shell_quote(&data.display().to_string());
        let quoted_root = crate::owner::shell_quote(&root.display().to_string());
        crate::owner::watch(
            std::process::id(),
            &root,
            &format!("pg_ctl -D {quoted_data} -m immediate -w stop; rm -rf {quoted_root}"),
        );
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
            // Ownership is in the CREATE name itself: bootstrap may spend time
            // compiling without a database connection. A concurrent reaper
            // must distinguish that live build from an abandoned attempt.
            let building = format!("devkit_building_{}_{}", std::process::id(), unique());
            self.sql("postgres", &format!("CREATE DATABASE {building}"))?;
            if let Err(error) = (schema.bootstrap)(self, &building) {
                let _ = self.sql(
                    "postgres",
                    &format!("DROP DATABASE IF EXISTS {building} WITH (FORCE)"),
                );
                return Err(format!("schema bootstrap: {error}"));
            }
            // Renaming installs the template only once it is complete. If
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
                    return Err(format!("could not install {}", databases.template));
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
        // Mark both as used now, for `reap`.
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

    /// Drops databases nothing will use again, never one with a connection:
    /// - test databases whose test process has exited, except failed ones,
    ///   of which the newest [`Reap::failed`] are kept for inspection;
    /// - unfinished test databases from runs more than a day old;
    /// - half-built templates whose owning process exited;
    /// - schema versions beyond the newest [`Reap::schemas`] per project, or
    ///   unused for [`Reap::days`].
    ///
    /// Returns the dropped names.
    pub fn reap(&self, policy: &Reap) -> Result<Vec<String>> {
        let now = now_secs();
        let rows = self.sql(
            "postgres",
            "SELECT d.datname, coalesce(shobj_description(d.oid, 'pg_database'), ''),
                    (SELECT count(*) FROM pg_stat_activity a WHERE a.datname = d.datname)
             FROM pg_database d
             WHERE d.datname ~ '_(schema|template)_' OR d.datname LIKE 't\\_%'
                OR d.datname LIKE 'devkit\\_building\\_%'",
        )?;
        let mut drop = Vec::new();
        let mut failed = Vec::new();
        let mut versions: std::collections::BTreeMap<(String, String), u64> = Default::default();
        for line in rows.lines() {
            let mut fields = line.splitn(3, '|');
            let (Some(name), Some(comment), Some(connections)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            if connections.trim() != "0" {
                continue;
            }
            if let Some(test) = TestName::parse(name) {
                if let Some(at) = comment.strip_prefix("devkit failed ") {
                    failed.push((at.trim().parse::<u64>().unwrap_or(0), name.to_owned()));
                } else if !pid_alive(test.pid) || now > test.created + 86_400 {
                    drop.push(name.to_owned());
                }
            } else if let Some(owner) = building_owner(name) {
                // A bootstrap may compile for minutes without a connection;
                // only its owner's exit means it was abandoned.
                if !pid_alive(owner) {
                    drop.push(name.to_owned());
                }
            } else if let Some((project, hash)) = name
                .split_once("_schema_")
                .or_else(|| name.split_once("_template_"))
            {
                let used = comment
                    .strip_prefix("devkit used ")
                    .and_then(|s| s.trim().parse::<u64>().ok())
                    .unwrap_or(0);
                let last = versions
                    .entry((project.to_owned(), hash.to_owned()))
                    .or_default();
                *last = (*last).max(used);
            }
        }
        // Failed test databases: the newest are kept, the rest go.
        failed.sort_by(|a, b| b.cmp(a));
        drop.extend(failed.into_iter().skip(policy.failed).map(|(_, name)| name));
        // Schema versions, newest first per project; a version's schema and
        // template database go together.
        let mut by_project: std::collections::BTreeMap<String, Vec<(u64, String)>> =
            Default::default();
        for ((project, hash), used) in versions {
            by_project.entry(project).or_default().push((used, hash));
        }
        for (project, mut list) in by_project {
            list.sort_by(|a, b| b.cmp(a));
            for (rank, (used, hash)) in list.into_iter().enumerate() {
                if rank >= policy.schemas || now > used + policy.days * 86_400 {
                    drop.push(format!("{project}_schema_{hash}"));
                    drop.push(format!("{project}_template_{hash}"));
                }
            }
        }
        let mut dropped = Vec::new();
        for name in drop {
            if name.contains("_template_") {
                let _ = self.sql(
                    "postgres",
                    &format!("ALTER DATABASE {name} WITH IS_TEMPLATE false"),
                );
            }
            // Only databases without connections were chosen; one that gained
            // a connection since is refused by DROP without FORCE and kept.
            if self
                .sql("postgres", &format!("DROP DATABASE IF EXISTS {name}"))
                .is_ok()
            {
                dropped.push(name);
            }
        }
        Ok(dropped)
    }

    /// Clears abandoned test databases before a run, including runs that reuse
    /// an existing schema. Cleanup failure is reported without failing tests.
    pub fn prepare_test_run(&self) {
        if let Err(error) = self.reap(&Reap::default()) {
            eprintln!("devkit: could not drop unused databases: {error}");
        }
    }

    /// Reaps on the server supplied to a test run, or on the running service
    /// when the run does not need a database fixture. Never starts a server.
    pub(crate) fn prepare_run(env: &[(String, String)]) {
        let socket = env
            .iter()
            .rev()
            .find(|(name, _)| name == "DEVKIT_PG_SOCKET")
            .map(|(_, value)| PathBuf::from(value))
            .or_else(|| std::env::var_os("DEVKIT_PG_SOCKET").map(PathBuf::from));
        if let Some(socket) = socket {
            Self {
                socket,
                owned: None,
            }
            .prepare_test_run();
        } else if let Some(server) = Self::service() {
            server.prepare_test_run();
        }
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

/// What [`Server::reap`] keeps.
#[derive(Debug, Clone)]
pub struct Reap {
    /// Schema versions kept per project, newest by last use.
    pub schemas: usize,
    /// Schema versions unused for this many days are dropped regardless.
    pub days: u64,
    /// Failed test databases kept for inspection, newest first.
    pub failed: usize,
}

impl Default for Reap {
    fn default() -> Self {
        Self {
            schemas: 8,
            days: 14,
            failed: 5,
        }
    }
}

/// A test database name, `t_<pid>_<created>_<unique>`.
struct TestName {
    pid: u32,
    created: u64,
}

impl TestName {
    fn parse(name: &str) -> Option<Self> {
        let mut parts = name.strip_prefix("t_")?.split('_');
        Some(Self {
            pid: parts.next()?.parse().ok()?,
            created: parts.next()?.parse().ok()?,
        })
    }
}

/// The owner pid in a half-built template's name,
/// `devkit_building_<pid>_<unique>`: short enough for PostgreSQL's identifier
/// limit whatever the project's name.
fn building_owner(name: &str) -> Option<u32> {
    let mut parts = name.strip_prefix("devkit_building_")?.split('_');
    let pid = parts.next()?.parse().ok()?;
    parts.next()?.parse::<u64>().ok()?;
    parts.next().is_none().then_some(pid)
}

/// Whether a process with this pid exists on this host. A reused pid only
/// delays reaping until the one-day limit.
fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
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
        let databases = Self {
            schema: format!("{project}_schema_{short}"),
            template: format!("{project}_template_{short}"),
        };
        check_name(&databases.schema)?;
        check_name(&databases.template)?;
        Ok(databases)
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
/// value is, unless the test is panicking: then it is kept for inspection,
/// marked failed and its name printed. [`Server::reap`] keeps only the
/// newest few failed ones, and drops those of tests killed before they
/// could drop their own.
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

/// `#[devkit::test] fn name(db: TestDatabase)` clones a fresh database.
impl crate::Fixture for TestDatabase {
    fn create() -> Result<Self> {
        test_database()
    }
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
            let _ = sql_at(
                &self.socket,
                "postgres",
                &format!(
                    "COMMENT ON DATABASE {} IS 'devkit failed {}'",
                    self.name,
                    now_secs()
                ),
            );
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
mod tests;
