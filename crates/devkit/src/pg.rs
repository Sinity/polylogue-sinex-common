//! PostgreSQL for development: one server, one database per schema version,
//! and a fixed set of test database slots per project.
//!
//! - **Server.** The per-user service, when it runs and has the required
//!   capabilities, listens on the socket in `$XDG_RUNTIME_DIR/devkit/pg`.
//!   Without it (CI, Nix builds, a host that does not run the service)
//!   [`Server::start`] runs a throwaway cluster on
//!   tmpfs, owned by the calling process and removed when dropped. Neither
//!   listens on TCP, so nothing collides across checkouts. PostgreSQL refuses
//!   to run as root, so a cluster started by root (cloud VMs, containers)
//!   runs as `nobody`, with a superuser named `root`: root's clients connect
//!   exactly as they would to a cluster of their own.
//! - **Schema databases.** A project's schema is identified by a hash of its
//!   sources ([`schema_hash`]). [`Server::ensure_schema`] builds
//!   `<project>_template_<hash>` once, by running the project's bootstrap,
//!   and copies it to `<project>_schema_<hash>`, the database compile-time
//!   query checks connect to. Worktrees with the same schema share both; a
//!   schema change gives a new name, so `DATABASE_URL` changes and exactly
//!   the crates that check queries against it rebuild.
//! - **Test databases.** A test takes one of a fixed number of slots
//!   ([`test_database`], [`Slots`]) by locking the slot's file in the
//!   server's socket directory, and clones the template into the slot's
//!   database, replacing whatever a previous holder left there. The kernel
//!   releases the lock however the test process ends, so a project never has
//!   more test databases than it has slots, one more per test process that
//!   holds several, plus the few failed ones kept for inspection. Test databases are separate from the compile database
//!   because a database cannot be cloned while anything is connected to it,
//!   and compilers and editors hold connections.
//!
//! PostgreSQL's tools (`postgres`, `initdb`, `pg_ctl`, `psql`) must be on PATH.
//! Everything here is disposable: losing it costs a rebuild of the schema
//! databases.
use std::{
    env, fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub type Result<T = ()> = std::result::Result<T, String>;

const DAY_SECONDS: u64 = 86_400;
// The same default startup allowance as pg_ctl -w.
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// How every database here is copied from its template. On a tmpfs server
/// without fsync, `FILE_COPY` copied a 12.7 MB template about twice as fast
/// as `WAL_LOG`, one at a time and from four slots at once, idle and beside a
/// writer (`benchmark_database_clones`), despite the checkpoint it forces.
const CLONE_STRATEGY: &str = "FILE_COPY";
/// How often a process waiting for a test database slot looks again.
const SLOT_WAIT: Duration = Duration::from_millis(50);

/// Required server capabilities and settings for an owned cluster.
#[derive(Debug, Clone, Default)]
pub struct ClusterOptions {
    /// Extensions that the server must offer for schema bootstrap.
    pub required_extensions: Vec<String>,
    /// Required `shared_preload_libraries`, such as `timescaledb`.
    pub preload: Vec<String>,
    /// Further `-c name=value` settings for a newly started server.
    pub settings: Vec<(String, String)>,
}

/// A running PostgreSQL server reachable through a Unix socket directory.
pub struct Server {
    socket: PathBuf,
    owned: Option<Owned>,
    /// Shared locks on the schema versions this value has ensured, held
    /// until it is dropped: cleanup never drops a version in use.
    in_use: Mutex<Vec<fs::File>>,
}

struct Owned {
    data: PathBuf,
    child: Option<Child>,
}

impl Server {
    /// A server already listening at `socket`, which this value does not own.
    fn at(socket: PathBuf) -> Self {
        Self {
            socket,
            owned: None,
            in_use: Mutex::default(),
        }
    }

    /// The socket directory of the per-user service.
    pub fn service_socket() -> Option<PathBuf> {
        env::var_os("XDG_RUNTIME_DIR").map(|dir| Path::new(&dir).join("devkit").join("pg"))
    }

    /// The per-user service if it is running.
    pub fn service() -> Option<Self> {
        let server = Self::at(Self::service_socket()?);
        server.ready().then_some(server)
    }

    /// Reuses the per-user service when its PostgreSQL major matches the
    /// current tools and it offers the required extensions and preloads.
    /// Otherwise starts an owned cluster, leaving the service untouched.
    pub fn service_or_start(options: &ClusterOptions) -> Result<Self> {
        if let Some(server) = Self::service() {
            match server.verify(options) {
                Ok(()) => return Ok(server),
                Err(error) => eprintln!(
                    "PostgreSQL service {}: {error}; starting an isolated cluster",
                    server.socket.display()
                ),
            }
        }
        Self::start(options)
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
    /// must exist, its path must fit PostgreSQL's Unix socket limit, and when
    /// the caller is root, `nobody` must be able to reach it. The cluster is
    /// stopped and removed when the server is dropped.
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
        let mut server = Self::at(root.clone());
        server.owned = Some(Owned {
            data: data.clone(),
            child: None,
        });
        let mut initdb = Command::new("initdb");
        initdb.args([
            "--no-sync",
            "--no-locale",
            "--auth=trust",
            "--data-checksums",
        ]);
        if crate::owner::running_as_root() {
            give_to_nobody(&root)?;
            // libpq's default user is the OS user's name.
            initdb.arg("--username=root");
        }
        run(crate::owner::as_owner_of(&mut initdb, &root)
            .arg("-D")
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
        let log = fs::File::create(root.join("postgres.log")).map_err(|e| e.to_string())?;
        let mut command = Command::new("postgres");
        crate::owner::as_owner_of(&mut command, &root)
            .arg("-D")
            .arg(&data)
            .arg("-k")
            .arg(&root)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            .process_group(0);
        for (name, value) in &settings {
            command.arg("-c").arg(format!("{name}={value}"));
        }
        server.owned.as_mut().expect("owned server").child =
            Some(command.spawn().map_err(|e| format!("postgres: {e}"))?);
        // The direct child is waited on normal shutdown. If its owner is
        // killed, the watchdog still stops the server and removes its files.
        let quoted_data = crate::owner::shell_quote(&data.display().to_string());
        let quoted_root = crate::owner::shell_quote(&root.display().to_string());
        crate::owner::watch(
            std::process::id(),
            &root,
            &format!("pg_ctl -D {quoted_data} -m immediate -w stop; rm -rf {quoted_root}"),
        );
        server.wait_ready()?;
        server.verify(options)?;
        Ok(server)
    }

    fn wait_ready(&mut self) -> Result {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            let status = self
                .owned
                .as_mut()
                .and_then(|owned| owned.child.as_mut())
                .expect("only an owned server is started")
                .try_wait()
                .map_err(|e| format!("postgres status: {e}"))?;
            if status.is_some() || Instant::now() >= deadline {
                let log = fs::read_to_string(self.socket.join("postgres.log")).unwrap_or_default();
                return Err(format!(
                    "postgres did not become ready (status {status:?}):\n{}",
                    log.trim()
                ));
            }
            if self.ready() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
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
    /// building them with its bootstrap if they do not. The version stays in
    /// use, and cleanup leaves it alone, until this server value is dropped.
    pub fn ensure_schema(&self, schema: &Schema<'_>) -> Result<SchemaDatabases> {
        let databases = SchemaDatabases::new(schema.project, &schema.hash)?;
        // Taken before the build lock, so cleanup holding the use lock
        // exclusively also knows that no build of this version is running.
        let in_use = self.version_lock(&databases.schema, VersionLock::Use)?;
        in_use
            .lock_shared()
            .map_err(|e| format!("schema use lock: {e}"))?;
        self.in_use.lock().expect("schema use locks").push(in_use);
        let build = self.version_lock(&databases.schema, VersionLock::Build)?;
        build.lock().map_err(|e| format!("schema lock: {e}"))?;
        if !self.database_exists(&databases.template)? {
            // Only the holder of the build lock creates this name, so one
            // already there was left by a builder that was killed.
            let building = building_name(&databases.template);
            self.drop_database(&building)?;
            self.sql("postgres", &format!("CREATE DATABASE {building}"))?;
            if let Err(error) = (schema.bootstrap)(self, &building) {
                let _ = self.drop_database(&building);
                return Err(format!("schema bootstrap: {error}"));
            }
            self.stamp_used(&building)?;
            // Renaming installs the template only once it is complete. If a
            // builder that does not take this lock finished first, its copy
            // is used.
            if self
                .sql(
                    "postgres",
                    &format!("ALTER DATABASE {building} RENAME TO {}", databases.template),
                )
                .is_err()
            {
                let _ = self.drop_database(&building);
                if !self.database_exists(&databases.template)? {
                    return Err(format!("could not install {}", databases.template));
                }
            }
        }
        // A reused template must be marked before cloning or inspecting it.
        self.stamp_used(&databases.template)?;
        // A clone fails while any session is connected to its template, and
        // autovacuum or a TimescaleDB scheduler connects to every database
        // that allows it. A built template accepts no connections, so none
        // can start; sessions from before this point are ended.
        self.sql(
            "postgres",
            &format!(
                "ALTER DATABASE {name} WITH IS_TEMPLATE true ALLOW_CONNECTIONS false;
                 SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity
                 WHERE datname = '{name}'",
                name = databases.template
            ),
        )?;
        if !self.database_exists(&databases.schema)? {
            let created = self.sql(
                "postgres",
                &format!(
                    "CREATE DATABASE {} TEMPLATE {} STRATEGY {CLONE_STRATEGY}",
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
        // The last use orders versions for `reap`.
        for name in [&databases.template, &databases.schema] {
            self.stamp_used(name)?;
        }
        Ok(databases)
    }

    fn stamp_used(&self, name: &str) -> Result {
        self.sql(
            "postgres",
            &format!("COMMENT ON DATABASE {name} IS 'devkit used {}'", now_secs()),
        )
        .map(drop)
    }

    /// A fresh database copied from `template`.
    pub fn clone_database(&self, template: &str, name: &str) -> Result {
        check_name(template)?;
        check_name(name)?;
        self.sql(
            "postgres",
            &format!("CREATE DATABASE {name} TEMPLATE {template} STRATEGY {CLONE_STRATEGY}"),
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

    /// Drops schema versions nothing will use again, never one in use by a
    /// live server value or with a connection:
    /// - versions beyond the newest [`Reap::schemas`] per project, or unused
    ///   for [`Reap::days`], schema and template database together;
    /// - half-built templates whose builder was killed;
    /// - test databases named by earlier devkit versions
    ///   (`t_<pid>_<seconds>_<number>`), which slots replace.
    ///
    /// Test databases in slots need no cleanup: their number is fixed.
    /// Returns the dropped names.
    pub fn reap(&self, policy: &Reap) -> Result<Vec<String>> {
        let now = now_secs();
        let rows = self.sql(
            "postgres",
            "SELECT datname, coalesce(shobj_description(oid, 'pg_database'), '')
             FROM pg_database
             WHERE datname ~ '_(schema|template|building)_' OR datname LIKE 't\\_%'",
        )?;
        let mut drop = Vec::new();
        let mut versions: std::collections::BTreeMap<(String, String), u64> = Default::default();
        for line in rows.lines() {
            let Some((name, comment)) = line.split_once('|') else {
                continue;
            };
            match schema_parts(name) {
                // Earlier devkit versions named builds `devkit_building_<pid>_<number>`
                // and drop their own; those are left alone.
                Some((_, "_building_", hash)) if !hash.contains('_') => drop.push(name.to_owned()),
                Some((_, "_building_", _)) => {}
                Some((project, _, hash)) => {
                    // A missing or unreadable stamp ranks as used now.
                    let used = comment
                        .strip_prefix("devkit used ")
                        .and_then(|s| s.trim().parse::<u64>().ok())
                        .unwrap_or(now);
                    let last = versions
                        .entry((project.to_owned(), hash.to_owned()))
                        .or_default();
                    *last = (*last).max(used);
                }
                None if earlier_test_database(name) => drop.push(name.to_owned()),
                None => {}
            }
        }
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
                if rank >= policy.schemas
                    || now.saturating_sub(used) > policy.days.saturating_mul(DAY_SECONDS)
                {
                    drop.push(format!("{project}_schema_{hash}"));
                    drop.push(format!("{project}_template_{hash}"));
                }
            }
        }
        self.drop_unused(drop)
    }

    /// The lock file of one kind for the schema version that `name` belongs
    /// to. The file stays in place so every process locks the same inode.
    fn version_lock(&self, name: &str, kind: VersionLock) -> Result<fs::File> {
        let (project, _, hash) =
            schema_parts(name).ok_or_else(|| format!("not a schema database: {name:?}"))?;
        let extension = match kind {
            VersionLock::Build => "lock",
            VersionLock::Use => "use",
        };
        lock_file(
            &self
                .socket
                .join(format!("{project}_schema_{hash}.{extension}")),
        )
    }

    /// Whether `name` exists and neither it nor the other database of its
    /// schema version has a connection.
    fn unused(&self, name: &str) -> Result<bool> {
        let other = match schema_parts(name) {
            Some((project, "_schema_", hash)) => format!("{project}_template_{hash}"),
            Some((project, "_template_", hash)) => format!("{project}_schema_{hash}"),
            _ => name.to_owned(),
        };
        let rows = self.sql(
            "postgres",
            &format!(
                "SELECT d.datname,
                    (SELECT count(*) FROM pg_stat_activity a WHERE a.datname = d.datname)
                 FROM pg_database d WHERE d.datname IN ('{name}', '{other}')"
            ),
        )?;
        if !rows.lines().any(|row| row.split('|').next() == Some(name)) {
            return Ok(false);
        }
        Ok(rows
            .lines()
            .all(|row| row.split_once('|').is_some_and(|(_, n)| n.trim() == "0")))
    }

    fn drop_unused(&self, names: Vec<String>) -> Result<Vec<String>> {
        let mut dropped = Vec::new();
        for name in names {
            check_name(&name)?;
            // Held through the drop: a version that a server value uses, or
            // that a builder is building, is skipped.
            let mut _version_locks = Vec::new();
            if schema_parts(&name).is_some() {
                let mut free = true;
                for kind in [VersionLock::Use, VersionLock::Build] {
                    let file = self.version_lock(&name, kind)?;
                    match file.try_lock() {
                        Ok(()) => _version_locks.push(file),
                        Err(fs::TryLockError::WouldBlock) => free = false,
                        Err(fs::TryLockError::Error(error)) => {
                            return Err(format!("schema lock: {error}"));
                        }
                    }
                }
                if !free {
                    continue;
                }
            }
            if !self.unused(&name)? {
                continue;
            }
            let template = schema_parts(&name).is_some_and(|(_, kind, _)| kind == "_template_");
            if template {
                // PostgreSQL refuses DROP while IS_TEMPLATE is true.
                self.sql(
                    "postgres",
                    &format!("ALTER DATABASE {name} WITH IS_TEMPLATE false"),
                )?;
                let rechecked = self.unused(&name);
                if !matches!(rechecked, Ok(true)) {
                    let restored = self.sql(
                        "postgres",
                        &format!("ALTER DATABASE {name} WITH IS_TEMPLATE true"),
                    );
                    rechecked?;
                    restored?;
                    continue;
                }
            }
            // DROP without FORCE also refuses connections opened after the
            // last check. Restore template status if that happens.
            if self
                .sql("postgres", &format!("DROP DATABASE {name}"))
                .is_ok()
            {
                dropped.push(name);
            } else if template {
                let restored = self.sql(
                    "postgres",
                    &format!("ALTER DATABASE {name} WITH IS_TEMPLATE true"),
                );
                if restored.is_err() && self.database_exists(&name)? {
                    restored?;
                }
            }
        }
        Ok(dropped)
    }

    /// Drops unused schema versions before a run, including runs that reuse
    /// an existing schema. Cleanup failure is reported without failing tests.
    pub fn prepare_test_run(&self) {
        if let Err(error) = self.reap(&Reap::default()) {
            eprintln!("devkit: could not drop unused databases: {error}");
        }
    }

    /// Reaps only on a server explicitly supplied by this run.
    pub(crate) fn prepare_run(env: &[(String, String)]) {
        let socket = env
            .iter()
            .rev()
            .find(|(name, _)| name == "DEVKIT_PG_SOCKET")
            .map(|(_, value)| PathBuf::from(value));
        if let Some(socket) = socket {
            Self::at(socket).prepare_test_run();
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
        if let Some(owned) = &mut self.owned {
            if let Some(child) = &mut owned.child {
                if matches!(child.try_wait(), Ok(None)) {
                    let _ = crate::owner::as_owner_of(&mut Command::new("pg_ctl"), &self.socket)
                        .arg("-D")
                        .arg(&owned.data)
                        .args(["-m", "immediate", "-w", "stop"])
                        .output();
                    // The exact child remains reserved until wait, including
                    // when pg_ctl fails or the postmaster already exited.
                    let _ = child.kill();
                }
                let _ = child.wait();
            }
            let _ = fs::remove_dir_all(&self.socket);
        }
    }
}

#[derive(Clone, Copy)]
enum VersionLock {
    /// Held exclusively while a version's template is built.
    Build,
    /// Held shared by every server value that ensured the version; cleanup
    /// needs it exclusively.
    Use,
}

/// What [`Server::reap`] keeps.
#[derive(Debug, Clone)]
pub struct Reap {
    /// Schema versions kept per project, newest by last use.
    pub schemas: usize,
    /// Schema versions unused for this many days are dropped regardless.
    pub days: u64,
}

impl Default for Reap {
    fn default() -> Self {
        Self {
            schemas: 8,
            days: 14,
        }
    }
}

/// How many test databases one project may have on one server. Every run
/// on the server shares these, so waiting for a slot also bounds the load
/// that runs together put on it.
///
/// [`Slots::from_env`] reads them from `DEVKIT_PG_SLOTS`,
/// `DEVKIT_PG_LARGE_SLOTS` and `DEVKIT_PG_FAILED_KEPT`, which a product's
/// `xtask` or the host's job runner sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slots {
    /// Databases ordinary tests use at once: `<project>_test<k>`. Default:
    /// the number of threads nextest runs tests on, the host's parallelism.
    pub ordinary: usize,
    /// Databases large tests, such as crash tests, use at once:
    /// `<project>_large<k>` ([`large_test_database`]). Default: 4.
    pub large: usize,
    /// Failed test databases kept for inspection, newest first:
    /// `<project>_failed<j>`. Default: 5.
    pub failed: usize,
}

impl Default for Slots {
    fn default() -> Self {
        Self {
            ordinary: std::thread::available_parallelism().map_or(4, |n| n.get()),
            large: 4,
            failed: 5,
        }
    }
}

impl Slots {
    /// The defaults, replaced by each variable that is set.
    pub fn from_env() -> Result<Self> {
        let mut slots = Self::default();
        for (variable, value, least) in [
            ("DEVKIT_PG_SLOTS", &mut slots.ordinary, 1),
            ("DEVKIT_PG_LARGE_SLOTS", &mut slots.large, 1),
            ("DEVKIT_PG_FAILED_KEPT", &mut slots.failed, 0),
        ] {
            if let Ok(text) = env::var(variable) {
                *value = text
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n >= least)
                    .ok_or_else(|| {
                        format!("{variable}={text:?} is not a whole number of at least {least}")
                    })?;
            }
        }
        Ok(slots)
    }
}

/// Which slots a test database is taken from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Ordinary,
    Large,
}

impl Family {
    fn word(self) -> &'static str {
        match self {
            Self::Ordinary => "test",
            Self::Large => "large",
        }
    }

    fn count(self, slots: &Slots) -> usize {
        match self {
            Self::Ordinary => slots.ordinary,
            Self::Large => slots.large,
        }
    }

    /// How many of this family's slots this process holds.
    fn held(self) -> &'static AtomicUsize {
        static ORDINARY: AtomicUsize = AtomicUsize::new(0);
        static LARGE: AtomicUsize = AtomicUsize::new(0);
        match self {
            Self::Ordinary => &ORDINARY,
            Self::Large => &LARGE,
        }
    }
}

/// The parts of a schema version's database name: project, kind
/// (`_schema_`, `_template_` or `_building_`) and hash.
fn schema_parts(name: &str) -> Option<(&str, &str, &str)> {
    ["_schema_", "_template_", "_building_"]
        .into_iter()
        .filter_map(|kind| Some((name.rfind(kind)?, kind)))
        .max_by_key(|(start, _)| *start)
        .map(|(start, kind)| (&name[..start], kind, &name[start + kind.len()..]))
}

/// Where a template is built before it is renamed into place.
fn building_name(template: &str) -> String {
    match schema_parts(template) {
        Some((project, _, hash)) => format!("{project}_building_{hash}"),
        None => unreachable!("{template} is a template name"),
    }
}

/// A test database name of earlier devkit versions, `t_<pid>_<seconds>_<number>`.
fn earlier_test_database(name: &str) -> bool {
    name.strip_prefix("t_").is_some_and(|rest| {
        let parts: Vec<_> = rest.split('_').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })
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
    crate::files::hash(paths, extra, true, |path, directory| {
        !directory || path.file_name().is_none_or(|n| n != "target")
    })
    .map(|hash| hash.to_hex().to_string())
}

/// A database for one test, in a slot it holds until the value is dropped.
/// It is dropped with the value, unless the test is panicking: then it is
/// renamed into the kept failed databases, replacing the oldest, and its
/// name printed. A killed test leaves its database in the slot, and the
/// next test to take the slot replaces it.
///
/// The name is the slot's, so it is reused by later tests; a test that
/// needs a unique path makes its own.
pub struct TestDatabase {
    socket: PathBuf,
    pub name: String,
    project: String,
    failed_kept: usize,
    family: Family,
    // Held while the test runs; the kernel releases it however the process
    // ends.
    _slot: fs::File,
}

/// An ordinary test database, cloned from the template named in the
/// environment that [`Server::test_env`] produced. Waits while every slot
/// is taken.
pub fn test_database() -> Result<TestDatabase> {
    TestDatabase::from_env(Family::Ordinary)
}

/// A test database from the large tests' own, smaller set of slots, for
/// tests whose databases grow far beyond the template, such as crash tests.
pub fn large_test_database() -> Result<TestDatabase> {
    TestDatabase::from_env(Family::Large)
}

/// `#[devkit::test] fn name(db: TestDatabase)` takes an ordinary slot.
impl crate::Fixture for TestDatabase {
    fn create() -> Result<Self> {
        test_database()
    }
}

impl TestDatabase {
    fn from_env(family: Family) -> Result<Self> {
        let socket = env::var_os("DEVKIT_PG_SOCKET")
            .map(PathBuf::from)
            .ok_or("DEVKIT_PG_SOCKET is not set; run the tests through xtask")?;
        let template =
            env::var("DEVKIT_PG_TEMPLATE").map_err(|_| "DEVKIT_PG_TEMPLATE is not set")?;
        Self::take(&socket, &template, family, &Slots::from_env()?)
    }

    /// Takes the first free slot of the family and clones `template` into
    /// it. A process that holds none of the family's slots waits for any to
    /// come free. A process that already holds one never waits: two tests
    /// each holding a slot and waiting for another would wait forever, so it
    /// takes the first free slot past the configured count instead, and
    /// the family has at most one extra database per such process.
    fn take(socket: &Path, template: &str, family: Family, slots: &Slots) -> Result<Self> {
        check_name(template)?;
        let Some((project, "_template_", _)) = schema_parts(template) else {
            return Err(format!("not a template database: {template:?}"));
        };
        let count = family.count(slots);
        if count == 0 {
            return Err(format!(
                "no {} database slots are configured",
                family.word()
            ));
        }
        let try_slot = |k: usize| -> Result<Option<(String, fs::File)>> {
            let name = format!("{project}_{}{k}", family.word());
            check_name(&name)?;
            let file = lock_file(&socket.join(format!("{name}.lock")))?;
            match file.try_lock() {
                Ok(()) => Ok(Some((name, file))),
                Err(fs::TryLockError::WouldBlock) => Ok(None),
                Err(fs::TryLockError::Error(error)) => Err(format!("{name}: {error}")),
            }
        };
        let first_free = |slots: std::ops::Range<usize>| -> Result<Option<(String, fs::File)>> {
            for k in slots {
                if let Some(taken) = try_slot(k)? {
                    return Ok(Some(taken));
                }
            }
            Ok(None)
        };
        let holding = family.held().load(Ordering::SeqCst) > 0;
        let (name, file) = if holding {
            match first_free(0..usize::MAX)? {
                Some(taken) => taken,
                None => unreachable!("some slot number is free"),
            }
        } else {
            let mut said = false;
            loop {
                if let Some(taken) = first_free(0..count)? {
                    break taken;
                }
                if !said {
                    eprintln!(
                        "devkit: all {count} {} database slots of {project} are taken; waiting for one",
                        family.word()
                    );
                    said = true;
                }
                std::thread::sleep(SLOT_WAIT);
            }
        };
        family.held().fetch_add(1, Ordering::SeqCst);
        sql_at(
            socket,
            "postgres",
            &format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"),
        )?;
        sql_at(
            socket,
            "postgres",
            &format!("CREATE DATABASE {name} TEMPLATE {template} STRATEGY {CLONE_STRATEGY}"),
        )?;
        Ok(Self {
            socket: socket.to_owned(),
            name,
            project: project.to_owned(),
            failed_kept: slots.failed,
            family,
            _slot: file,
        })
    }

    pub fn url(&self, user: Option<&str>) -> String {
        url(&self.socket, &self.name, user)
    }

    pub fn sql(&self, sql: &str) -> Result<String> {
        sql_at(&self.socket, &self.name, sql)
    }

    /// Renames this database to the kept failed database with the oldest
    /// failure, or a free one, and returns that name. With none kept it
    /// stays in the slot.
    fn keep_failed(&self) -> Result<Option<String>> {
        if self.failed_kept == 0 {
            return Ok(None);
        }
        let ring = lock_file(&self.socket.join(format!("{}_failed.lock", self.project)))?;
        ring.lock()
            .map_err(|e| format!("kept failed databases: {e}"))?;
        let names: Vec<String> = (0..self.failed_kept)
            .map(|j| format!("{}_failed{j}", self.project))
            .collect();
        for name in &names {
            check_name(name)?;
        }
        let quoted: Vec<String> = names.iter().map(|name| format!("'{name}'")).collect();
        let rows = sql_at(
            &self.socket,
            "postgres",
            &format!(
                "SELECT datname, coalesce(shobj_description(oid, 'pg_database'), '')
                 FROM pg_database WHERE datname IN ({})",
                quoted.join(", ")
            ),
        )?;
        let failed_at: std::collections::BTreeMap<&str, u64> = rows
            .lines()
            .filter_map(|row| row.split_once('|'))
            .map(|(name, comment)| {
                let at = comment
                    .strip_prefix("devkit failed ")
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                (name, at)
            })
            .collect();
        // A free name sorts first (None), then the oldest failure.
        let target = names
            .iter()
            .min_by_key(|name| failed_at.get(name.as_str()).copied())
            .expect("at least one kept failed database");
        sql_at(
            &self.socket,
            "postgres",
            &format!("DROP DATABASE IF EXISTS {target} WITH (FORCE)"),
        )?;
        // Nanoseconds, so failures in the same second keep their order.
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        sql_at(
            &self.socket,
            "postgres",
            &format!(
                "COMMENT ON DATABASE {} IS 'devkit failed {stamp}'",
                self.name
            ),
        )?;
        // The failing test's own connections may still be closing.
        let mut attempt = 0;
        loop {
            let renamed = sql_at(
                &self.socket,
                "postgres",
                &format!(
                    "SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity
                     WHERE datname = '{name}' AND pid <> pg_backend_pid();
                     ALTER DATABASE {name} RENAME TO {target}",
                    name = self.name
                ),
            );
            match renamed {
                Ok(_) => return Ok(Some(target.clone())),
                Err(error) if attempt >= 2 => return Err(error),
                Err(_) => attempt += 1,
            }
        }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        if std::thread::panicking() {
            match self.keep_failed() {
                Ok(Some(kept)) => eprintln!(
                    "devkit: kept the failed test's database as {kept}; open it with `psql -h {} {kept}`",
                    self.socket.display()
                ),
                Ok(None) => eprintln!(
                    "devkit: the failed test's database {} at {} stays until its slot is taken again",
                    self.name,
                    self.socket.display()
                ),
                Err(error) => eprintln!(
                    "devkit: could not keep the failed test's database {}: {error}; it stays until its slot is taken again",
                    self.name
                ),
            }
        } else {
            let _ = sql_at(
                &self.socket,
                "postgres",
                &format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", self.name),
            );
        }
        self.family.held().fetch_sub(1, Ordering::SeqCst);
    }
}

/// Opens a lock file, creating it if needed and never truncating it.
fn lock_file(path: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))
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

/// Hands a new cluster directory to `nobody`, which the cluster then runs as.
fn give_to_nobody(dir: &Path) -> Result {
    let mut account = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0 as libc::c_char; 4096];
    let mut found = std::ptr::null_mut();
    // SAFETY: every pointer refers to live memory of the stated size, and the
    // result is read only when getpwnam_r reports that it filled `account`.
    let error = unsafe {
        libc::getpwnam_r(
            c"nobody".as_ptr(),
            account.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if error != 0 || found.is_null() {
        return Err(
            "PostgreSQL refuses root, and there is no `nobody` account to run it as".into(),
        );
    }
    // SAFETY: getpwnam_r filled `account`.
    let account = unsafe { account.assume_init() };
    std::os::unix::fs::chown(dir, Some(account.pw_uid), Some(account.pw_gid))
        .map_err(|e| format!("{}: {e}", dir.display()))
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

mod capabilities;
