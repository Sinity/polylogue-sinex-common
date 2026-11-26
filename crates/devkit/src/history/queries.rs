//! Queries across recorded runs of one project, limited to the checkouts the
//! caller names.
use super::{Store, outcomes};
use rusqlite::params;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

/// A test at one Git tree and uncommitted-changes hash: tree, dirty hash,
/// binary ID, name ID.
type TestAtTree = (String, Option<String>, i64, i64);

/// A run header as the cross-run queries report it; `status` is as recorded.
struct ScopeRun {
    id: i64,
    checkout: String,
    head: Option<String>,
    dirty: Option<String>,
    git_tree: Option<String>,
    observation_error: Option<String>,
    command: String,
    args: String,
    status: String,
    started_ms: i64,
    toolchain: Option<String>,
    host: Option<String>,
}

fn paths(checkouts: &[String]) -> String {
    serde_json::to_string(checkouts).expect("strings serialize")
}

impl Store {
    /// Tests that passed in one run and failed in another at the same Git
    /// tree and the same uncommitted changes. Runs whose checkout could not be
    /// read are left out and their results counted.
    pub fn intermittent(&self, project: &str, checkouts: &[String]) -> rusqlite::Result<Value> {
        let runs = self.scope_runs(project, checkouts)?;
        let mut unreadable = 0;
        // (git tree, dirty hash) of each run that has a tree.
        let mut keys: BTreeMap<i64, (String, Option<String>)> = BTreeMap::new();
        for run in &runs {
            match &run.git_tree {
                Some(tree) if run.observation_error.is_none() => {
                    keys.insert(run.id, (tree.clone(), run.dirty.clone()));
                }
                _ => unreadable += outcomes::outcomes(&self.conn, run.id)?.len(),
            }
        }
        // Only a test that failed somewhere can be intermittent, and only
        // runs at the tree it failed at need their sets read.
        let mut candidates: BTreeMap<TestAtTree, Vec<(i64, &'static str)>> = BTreeMap::new();
        {
            let mut statement = self.conn.prepare(
                "SELECT t.run, t.binary, t.name FROM test_results t JOIN runs r ON r.id = t.run
                 WHERE r.project = ?1 AND r.checkout IN (SELECT value FROM json_each(?2))
                   AND t.status = 'failed'",
            )?;
            let failed = statement.query_map(params![project, paths(checkouts)], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?;
            for row in failed {
                let (run, binary, name) = row?;
                if let Some((tree, dirty)) = keys.get(&run) {
                    candidates.insert((tree.clone(), dirty.clone(), binary, name), Vec::new());
                }
            }
        }
        let trees: BTreeSet<(String, Option<String>)> = candidates
            .keys()
            .map(|(tree, dirty, ..)| (tree.clone(), dirty.clone()))
            .collect();
        for run in &runs {
            let Some(key) = keys.get(&run.id).filter(|key| trees.contains(key)) else {
                continue;
            };
            for outcome in outcomes::outcomes(&self.conn, run.id)? {
                if let Some(results) = candidates.get_mut(&(
                    key.0.clone(),
                    key.1.clone(),
                    outcome.binary,
                    outcome.name,
                )) {
                    results.push((run.id, outcome.status));
                }
            }
        }
        let names = outcomes::Names::load(&self.conn)?;
        let by_id: BTreeMap<i64, &ScopeRun> = runs.iter().map(|run| (run.id, run)).collect();
        let mut tests = Vec::new();
        for ((tree, dirty, binary, name), mut results) in candidates {
            let runs_with = |statuses: &[&str]| {
                results
                    .iter()
                    .filter(|(_, status)| statuses.contains(status))
                    .map(|(run, _)| *run)
                    .collect::<BTreeSet<_>>()
            };
            if runs_with(&["passed"]).is_empty()
                || runs_with(&["failed"]).is_empty()
                || runs_with(&["passed", "failed"]).len() < 2
            {
                continue;
            }
            results.sort();
            let runs: Vec<Value> = results
                .iter()
                .map(|(id, status)| {
                    let run = by_id[id];
                    json!({"run": run.id, "checkout": run.checkout, "head": run.head,
                        "command": run.command, "args": run.args, "run_status": run.status,
                        "test_status": status, "started_ms": run.started_ms,
                        "toolchain": run.toolchain, "host": run.host})
                })
                .collect();
            tests.push(json!({"git_tree": tree, "dirty_hash": dirty,
                "binary": names.binary(binary), "name": names.name(name), "runs": runs}));
        }
        tests.sort_by(|a, b| {
            let key = |test: &Value| {
                (
                    test["git_tree"].as_str().map(str::to_owned),
                    test["dirty_hash"].as_str().map(str::to_owned),
                    test["binary"].as_str().map(str::to_owned),
                    test["name"].as_str().map(str::to_owned),
                )
            };
            key(a).cmp(&key(b))
        });
        Ok(json!({"project": project, "checkouts": checkouts,
            "results_without_tree": unreadable, "tests": tests}))
    }

    /// Every recorded result of one test in start order, and the first that
    /// failed (or was flaky). A failure with uncommitted changes is not
    /// attributed to its HEAD.
    pub fn first_failing(
        &self,
        project: &str,
        checkouts: &[String],
        binary: &str,
        name: &str,
    ) -> rusqlite::Result<Value> {
        let mut results = Vec::new();
        if let Some((binary_id, name_id)) = outcomes::test_id(&self.conn, binary, name)? {
            for run in self.scope_runs(project, checkouts)? {
                let mut statuses: Vec<&str> = outcomes::outcomes(&self.conn, run.id)?
                    .into_iter()
                    .filter(|outcome| outcome.binary == binary_id && outcome.name == name_id)
                    .map(|outcome| outcome.status)
                    .collect();
                statuses.sort_unstable();
                for status in statuses {
                    let revision = if run.head.is_none() || run.observation_error.is_some() {
                        "unknown"
                    } else if run.dirty.is_some() {
                        "uncommitted changes"
                    } else {
                        "clean HEAD"
                    };
                    results.push(json!({"run": run.id, "checkout": run.checkout,
                        "head": run.head, "dirty_hash": run.dirty, "revision": revision,
                        "observation_error": run.observation_error,
                        "git_tree": run.git_tree,
                        "started_ms": run.started_ms, "command": run.command,
                        "args": run.args, "run_status": run.status,
                        "test_status": status}));
                }
            }
        }
        let first = results
            .iter()
            .find(|row| matches!(row["test_status"].as_str(), Some("failed" | "flaky")));
        Ok(
            json!({"project": project, "checkouts": checkouts, "binary": binary,
            "name": name, "first_failure": first, "results": results}),
        )
    }

    /// The runs of the named checkouts of a project, in start order.
    fn scope_runs(&self, project: &str, checkouts: &[String]) -> rusqlite::Result<Vec<ScopeRun>> {
        let mut statement = self.conn.prepare(
            "SELECT id, checkout, head, dirty, git_tree, observation_error, command, args,
                    status, started_ms, toolchain, host
             FROM runs WHERE project = ?1 AND checkout IN (SELECT value FROM json_each(?2))
             ORDER BY started_ms, id",
        )?;
        statement
            .query_map(params![project, paths(checkouts)], |row| {
                Ok(ScopeRun {
                    id: row.get(0)?,
                    checkout: row.get(1)?,
                    head: row.get(2)?,
                    dirty: row.get(3)?,
                    git_tree: row.get(4)?,
                    observation_error: row.get(5)?,
                    command: row.get(6)?,
                    args: row.get(7)?,
                    status: row.get(8)?,
                    started_ms: row.get(9)?,
                    toolchain: row.get(10)?,
                    host: row.get(11)?,
                })
            })?
            .collect()
    }

    /// Durations of passed runs of an exact command and argument list, or of
    /// one stage within them. Runs left out of the summary stay listed with
    /// the reason.
    pub fn durations(
        &self,
        project: &str,
        checkouts: &[String],
        command: &str,
        args: &[String],
        stage: Option<&str>,
    ) -> rusqlite::Result<Value> {
        let args_json = serde_json::to_string(args).expect("strings serialize");
        let mut statement = self.conn.prepare(
            "SELECT r.id, r.checkout, r.status, r.duration_ms, r.head, r.dirty,
                    r.toolchain, r.host, s.position, s.duration_ms, s.ok, r.observation_error, r.started_ms
             FROM runs r LEFT JOIN stages s ON s.run = r.id AND s.name = ?5
             WHERE r.project = ?1 AND r.checkout IN (SELECT value FROM json_each(?2))
               AND r.command = ?3 AND r.args = ?4
             ORDER BY r.started_ms, r.id, s.position",
        )?;
        let rows = statement
            .query_map(
                params![project, paths(checkouts), command, args_json, stage],
                |row| {
                    let status: String = row.get(2)?;
                    let total: Option<i64> = row.get(3)?;
                    let position: Option<i64> = row.get(8)?;
                    let stage_duration: Option<i64> = row.get(9)?;
                    let stage_ok: Option<bool> = row.get(10)?;
                    let duration = if stage.is_some() {
                        stage_duration
                    } else {
                        total
                    };
                    let left_out = if status != "passed" {
                        Some("run not passed")
                    } else if stage.is_some() && position.is_none() {
                        Some("stage not recorded")
                    } else if stage_ok == Some(false) {
                        Some("stage failed")
                    } else if duration.is_none() {
                        Some("duration not recorded")
                    } else if duration.is_some_and(|ms| ms < 0) {
                        Some("invalid duration")
                    } else {
                        None
                    };
                    Ok(
                        json!({"run": row.get::<_, i64>(0)?, "checkout": row.get::<_, String>(1)?,
                        "run_status": status, "head": row.get::<_, Option<String>>(4)?,
                        "dirty_hash": row.get::<_, Option<String>>(5)?,
                        "toolchain": row.get::<_, Option<String>>(6)?,
                        "host": row.get::<_, Option<String>>(7)?,
                        "stage_position": position, "stage_ok": stage_ok, "duration_ms": duration,
                        "observation_error": row.get::<_, Option<String>>(11)?,
                        "started_ms": row.get::<_, i64>(12)?,
                        "left_out": left_out}),
                    )
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut durations: Vec<i64> = rows
            .iter()
            .filter(|row| row["left_out"].is_null())
            .filter_map(|row| row["duration_ms"].as_i64())
            .collect();
        durations.sort_unstable();
        let median = if durations.is_empty() {
            None
        } else {
            let middle = durations.len() / 2;
            Some(if durations.len().is_multiple_of(2) {
                durations[middle - 1] as f64 / 2.0 + durations[middle] as f64 / 2.0
            } else {
                durations[middle] as f64
            })
        };
        let run_count = rows
            .iter()
            .filter_map(|row| row["run"].as_i64())
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        Ok(
            json!({"project": project, "checkouts": checkouts, "command": command,
            "args": args, "stage": stage, "run_count": run_count,
            "sample_count": durations.len(), "min_ms": durations.first(), "median_ms": median,
            "max_ms": durations.last(), "runs": rows}),
        )
    }

    /// Equal compiler messages across the named checkouts, grouped by text.
    pub fn diagnostic_groups(
        &self,
        project: &str,
        checkouts: &[String],
        level: &str,
    ) -> rusqlite::Result<Value> {
        let mut statement = self.conn.prepare(
            "SELECT d.text_hash, texts.text,
                    json_group_array(json_object('run', r.id, 'checkout', r.checkout,
                        'head', r.head, 'dirty_hash', r.dirty, 'run_status', r.status,
                        'command', r.command, 'args', r.args, 'package', d.package,
                        'level', d.level, 'code', d.code, 'file', d.file, 'line', d.line))
             FROM diagnostics d JOIN runs r ON r.id = d.run
             JOIN diagnostic_texts texts ON texts.hash = d.text_hash
             WHERE r.project = ?1 AND r.checkout IN (SELECT value FROM json_each(?2))
               AND (d.level = 'error' OR (?3 = 'warning' AND d.level = 'warning'))
             GROUP BY d.text_hash
             ORDER BY d.text_hash",
        )?;
        let groups = statement
            .query_map(params![project, paths(checkouts), level], |row| {
                let occurrences: String = row.get(2)?;
                Ok(json!({"text_hash": row.get::<_, String>(0)?,
                    "text": row.get::<_, String>(1)?,
                    "occurrences": decode_json(&occurrences)?}))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"project": project, "checkouts": checkouts, "level": level,
            "diagnostics": groups}),
        )
    }
}

fn decode_json(text: &str) -> rusqlite::Result<Value> {
    serde_json::from_str(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            text.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

#[cfg(test)]
mod tests;
