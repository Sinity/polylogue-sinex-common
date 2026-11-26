//! The machine-wide record of xtask runs: one SQLite file for every project
//! and checkout, at `$XDG_STATE_HOME/devkit/runs.db`.
//!
//! Rows are never deleted. The store stays small because each run's passed,
//! ignored and filtered tests are one compressed set, only failed, flaky and
//! skipped tests are rows, passing tests keep no output, and each distinct
//! diagnostic text and test name is stored once. Other programs may read it;
//! the tables below are its interface.
mod analysis;
mod outcomes;
mod queries;

use crate::Run;
pub(crate) use outcomes::Outcome;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS runs (
    -- never reused, so a run ID printed once names that run for good
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    checkout TEXT NOT NULL,
    command TEXT NOT NULL,
    args TEXT NOT NULL,
    head TEXT,
    dirty TEXT,
    toolchain TEXT,
    host TEXT,
    pid INTEGER NOT NULL,
    -- the job runner's identity for the job that ran this, from DEVKIT_JOB_ID
    job_id TEXT,
    started_ms INTEGER NOT NULL,
    duration_ms INTEGER,
    -- running, passed or failed; a running row whose process is gone, or
    -- whose machine has booted since, was interrupted
    status TEXT NOT NULL,
    failure TEXT,
    -- the committed Git tree object; NULL when the checkout could not be read
    git_tree TEXT,
    -- why the checkout could not be read
    observation_error TEXT,
    -- the kernel's boot ID and the process's start time in clock ticks after
    -- boot: together with pid they identify the running process
    boot_id TEXT,
    process_start INTEGER
);
CREATE INDEX IF NOT EXISTS runs_by_checkout ON runs (project, checkout, id);
CREATE TABLE IF NOT EXISTS stages (
    run INTEGER NOT NULL REFERENCES runs (id),
    position INTEGER NOT NULL,
    name TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    ok INTEGER NOT NULL,
    PRIMARY KEY (run, position)
);
-- nextest binary IDs, such as `polylogue-store` or `sinexd::postgres`
CREATE TABLE IF NOT EXISTS test_binaries (
    id INTEGER PRIMARY KEY,
    binary TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS test_names (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
);
-- each run's passed, ignored and filtered tests, as one zstd frame of
-- entries ordered by binary and name ID: the binary ID's increase over the
-- previous entry, then the name ID (its increase when the binary is the
-- same), a status byte (0 passed, 1 ignored, 2 filtered) and the duration
-- in milliseconds, the numbers as LEB128 unsigned varints
CREATE TABLE IF NOT EXISTS test_sets (
    run INTEGER PRIMARY KEY REFERENCES runs (id),
    tests BLOB NOT NULL
);
-- each run's failed, flaky and skipped tests
CREATE TABLE IF NOT EXISTS test_results (
    run INTEGER NOT NULL REFERENCES runs (id),
    binary INTEGER NOT NULL REFERENCES test_binaries (id),
    name INTEGER NOT NULL REFERENCES test_names (id),
    -- failed, flaky or skipped
    status TEXT NOT NULL,
    duration_ms INTEGER NOT NULL,
    -- zstd-compressed failure output or skipped-test reason
    output BLOB
);
CREATE INDEX IF NOT EXISTS test_results_by_run ON test_results (run);
CREATE INDEX IF NOT EXISTS test_results_by_name ON test_results (name, binary);
CREATE TABLE IF NOT EXISTS diagnostic_texts (
    hash TEXT PRIMARY KEY,
    text TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS diagnostics (
    run INTEGER NOT NULL REFERENCES runs (id),
    package TEXT NOT NULL,
    level TEXT NOT NULL,
    code TEXT,
    file TEXT,
    line INTEGER,
    text_hash TEXT NOT NULL REFERENCES diagnostic_texts (hash)
);
CREATE INDEX IF NOT EXISTS diagnostics_by_run ON diagnostics (run);
";

/// A recorded run's header.
#[derive(Debug, Clone)]
pub struct RunRow {
    pub id: i64,
    pub project: String,
    pub checkout: String,
    pub command: String,
    pub args: String,
    pub head: Option<String>,
    pub dirty: bool,
    pub git_tree: Option<String>,
    pub observation_error: Option<String>,
    pub started_ms: i64,
    pub duration_ms: Option<i64>,
    /// passed, failed, running or interrupted
    pub status: String,
    pub failure: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FailedTest {
    pub binary: String,
    pub name: String,
    pub status: String,
    pub duration_ms: i64,
    pub output: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DiagnosticRow {
    pub package: String,
    pub level: String,
    pub code: Option<String>,
    pub file: Option<String>,
    pub line: Option<i64>,
    pub text: String,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    /// `$XDG_STATE_HOME/devkit/runs.db`, with `XDG_STATE_HOME` defaulting
    /// to `~/.local/state`.
    pub fn path() -> PathBuf {
        let state = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| Path::new(&home).join(".local/state")))
            .unwrap_or_else(|| PathBuf::from("."));
        state.join("devkit").join("runs.db")
    }

    /// The open database, for modules that keep their own tables here.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn open() -> rusqlite::Result<Self> {
        let path = Self::path();
        if let Some(dir) = path.parent()
            && !dir.exists()
        {
            std::fs::create_dir_all(dir)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
            // SQLite on copy-on-write btrfs fragments badly. The flag only
            // takes effect on an empty directory; elsewhere it fails
            // harmlessly.
            let _ = Command::new("chattr").arg("+C").arg(dir).output();
        }
        Self::open_at(&path)
    }

    pub fn open_at(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(30))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    pub(crate) fn begin(&self, run: &Run) -> rusqlite::Result<i64> {
        let toolchain = Command::new("rustc")
            .arg("--version")
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
        let host = std::fs::read_to_string("/etc/hostname")
            .ok()
            .map(|h| h.trim().to_owned());
        self.conn.execute(
            "INSERT INTO runs (project, checkout, command, args, head, dirty, toolchain, host,
                               pid, job_id, started_ms, status, git_tree, observation_error,
                               boot_id, process_start)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'running', ?12, ?13, ?14, ?15)",
            params![
                run.project,
                run.root.display().to_string(),
                run.command,
                serde_json::to_string(&run.args).unwrap_or_default(),
                run.tree.head,
                run.tree.dirty,
                toolchain,
                host,
                std::process::id(),
                env::var("DEVKIT_JOB_ID").ok(),
                millis(run.started),
                run.tree.git_tree,
                run.tree.observation_error,
                boot_id(),
                process_start(std::process::id().into()),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub(crate) fn finish(&self, id: i64, run: &Run, elapsed: Duration) -> rusqlite::Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE runs SET duration_ms = ?2, status = ?3, failure = ?4,
                 git_tree = ?5, observation_error = ?6 WHERE id = ?1",
            params![
                id,
                elapsed.as_millis() as i64,
                if run.failure.is_some() {
                    "failed"
                } else {
                    "passed"
                },
                run.failure,
                run.tree.git_tree,
                run.tree.observation_error,
            ],
        )?;
        for (position, stage) in run.stages.iter().enumerate() {
            tx.execute(
                "INSERT INTO stages (run, position, name, duration_ms, ok) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, position as i64, stage.name, stage.duration.as_millis() as i64, stage.ok],
            )?;
        }
        {
            outcomes::insert(&tx, id, &run.tests)?;
            let mut text =
                tx.prepare("INSERT OR IGNORE INTO diagnostic_texts (hash, text) VALUES (?1, ?2)")?;
            let mut insert = tx.prepare(
                "INSERT INTO diagnostics (run, package, level, code, file, line, text_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for d in &run.diagnostics {
                let hash = blake3::hash(d.text.as_bytes()).to_hex().to_string();
                text.execute(params![hash, d.text])?;
                insert.execute(params![
                    id, d.package, d.level, d.code, d.file, d.line, hash
                ])?;
            }
        }
        tx.commit()
    }

    /// The most recent finished or running run of a checkout.
    pub fn last_run(&self, project: &str, checkout: &str) -> rusqlite::Result<Option<RunRow>> {
        self.conn
            .query_row(
                &format!(
                    "{RUN_SELECT} WHERE project = ?1 AND checkout = ?2 ORDER BY id DESC LIMIT 1"
                ),
                params![project, checkout],
                run_row,
            )
            .optional()
    }

    pub fn run(&self, id: i64) -> rusqlite::Result<Option<RunRow>> {
        self.conn
            .query_row(&format!("{RUN_SELECT} WHERE id = ?1"), params![id], run_row)
            .optional()
    }

    /// Recent runs of a checkout, newest first.
    pub fn runs(
        &self,
        project: &str,
        checkout: &str,
        limit: usize,
    ) -> rusqlite::Result<Vec<RunRow>> {
        let mut statement = self.conn.prepare(&format!(
            "{RUN_SELECT} WHERE project = ?1 AND checkout = ?2 ORDER BY id DESC LIMIT ?3"
        ))?;
        statement
            .query_map(params![project, checkout, limit as i64], run_row)?
            .collect()
    }

    /// Tests of a run that failed or were flaky, with their output.
    pub fn failures(&self, run: i64) -> rusqlite::Result<Vec<FailedTest>> {
        let mut statement = self.conn.prepare(
            "SELECT b.binary, n.name, t.status, t.duration_ms, t.output FROM test_results t
             JOIN test_binaries b ON b.id = t.binary JOIN test_names n ON n.id = t.name
             WHERE t.run = ?1 AND t.status IN ('failed', 'flaky') ORDER BY b.binary, n.name",
        )?;
        statement
            .query_map(params![run], |row| {
                let output: Option<Vec<u8>> = row.get(4)?;
                Ok(FailedTest {
                    binary: row.get(0)?,
                    name: row.get(1)?,
                    status: row.get(2)?,
                    duration_ms: row.get(3)?,
                    output: output.map(|bytes| {
                        zstd::decode_all(bytes.as_slice())
                            .map(|b| String::from_utf8_lossy(&b).into_owned())
                            .unwrap_or_else(|e| format!("(output unreadable: {e})"))
                    }),
                })
            })?
            .collect()
    }

    /// Diagnostics of a run at `level` or above (`warning` includes errors).
    pub fn diagnostics(&self, run: i64, level: &str) -> rusqlite::Result<Vec<DiagnosticRow>> {
        let levels: &[&str] = if level == "error" {
            &["error"]
        } else {
            &["error", "warning"]
        };
        let mut statement = self.conn.prepare(
            "SELECT d.package, d.level, d.code, d.file, d.line, t.text
             FROM diagnostics d JOIN diagnostic_texts t ON t.hash = d.text_hash
             WHERE d.run = ?1 AND d.level IN (SELECT value FROM json_each(?2))
             ORDER BY d.level = 'warning', d.package, d.file, d.line",
        )?;
        statement
            .query_map(
                params![run, serde_json::to_string(levels).unwrap_or_default()],
                |row| {
                    Ok(DiagnosticRow {
                        package: row.get(0)?,
                        level: row.get(1)?,
                        code: row.get(2)?,
                        file: row.get(3)?,
                        line: row.get(4)?,
                        text: row.get(5)?,
                    })
                },
            )?
            .collect()
    }

    /// Counts of a run's tests by status.
    pub fn test_counts(&self, run: i64) -> rusqlite::Result<Vec<(String, i64)>> {
        let mut counts = std::collections::BTreeMap::<String, i64>::new();
        for outcome in self.outcomes(run)? {
            *counts.entry(outcome.status.to_owned()).or_default() += 1;
        }
        Ok(counts.into_iter().collect())
    }

    /// Every recorded test outcome of a run.
    pub(crate) fn outcomes(&self, run: i64) -> rusqlite::Result<Vec<Outcome>> {
        outcomes::outcomes(&self.conn, run)
    }
}

const RUN_SELECT: &str = "SELECT id, project, checkout, command, args, head, dirty, started_ms,
                                 duration_ms, status, failure, pid, git_tree, observation_error,
                                 boot_id, process_start FROM runs";

fn run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RunRow> {
    let mut status: String = row.get(9)?;
    if status == "running" && interrupted(row.get(14)?, row.get(11)?, row.get(15)?) {
        status = "interrupted".into();
    }
    Ok(RunRow {
        id: row.get(0)?,
        project: row.get(1)?,
        checkout: row.get(2)?,
        command: row.get(3)?,
        args: row.get(4)?,
        head: row.get(5)?,
        dirty: row.get::<_, Option<String>>(6)?.is_some(),
        git_tree: row.get(12)?,
        observation_error: row.get(13)?,
        started_ms: row.get(7)?,
        duration_ms: row.get(8)?,
        status,
        failure: row.get(10)?,
    })
}

/// Whether a run recorded as running has no process any more: the machine
/// has booted since, its process is gone, or its pid now names a process
/// that started later. Runs recorded without a boot ID or start time are
/// judged by their pid alone.
fn interrupted(boot: Option<String>, pid: i64, started: Option<i64>) -> bool {
    if let (Some(boot), Some(current)) = (boot, boot_id())
        && boot != current
    {
        return true;
    }
    match process_start(pid) {
        None => true,
        Some(start) => started.is_some_and(|started| started != start),
    }
}

fn boot_id() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|id| id.trim().to_owned())
}

/// A process's start time in clock ticks after boot, field 22 of its
/// `/proc/<pid>/stat`; the command name before it may contain spaces.
fn process_start(pid: i64) -> Option<i64> {
    if pid <= 0 {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields = &stat[stat.rfind(')')? + 1..];
    fields.split_whitespace().nth(19)?.parse().ok()
}

fn millis(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests;
