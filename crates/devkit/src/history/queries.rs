//! Descriptive queries over recorded rows, scoped to explicit project/checkouts.
use super::Store;
use rusqlite::params;
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn population(project: &str, checkouts: &[String]) -> Value {
    json!({"project": project, "checkouts": checkouts})
}

fn paths(checkouts: &[String]) -> String {
    serde_json::to_string(checkouts).expect("strings serialize")
}

impl Store {
    /// Tests observed passed and failed in distinct runs with equal recorded
    /// committed tree objects and dirty-content hashes. Legacy HEAD objects
    /// are resolved only against an explicitly selected repository.
    pub fn intermittent(
        &self,
        project: &str,
        checkouts: &[String],
        repository: Option<&Path>,
    ) -> rusqlite::Result<Value> {
        let selected = paths(checkouts);
        let mut legacy = serde_json::Map::new();
        if let Some(repository) = repository {
            let mut heads = self.conn.prepare(
                "SELECT DISTINCT head FROM runs WHERE project = ?1
                 AND checkout IN (SELECT value FROM json_each(?2))
                 AND git_tree IS NULL AND observation_error IS NULL AND head IS NOT NULL",
            )?;
            for head in
                heads.query_map(params![project, selected], |row| row.get::<_, String>(0))?
            {
                let head = head?;
                if !(head.len() == 40 || head.len() == 64)
                    || !head.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    continue;
                }
                if let Ok(output) = Command::new("git")
                    .arg("-C")
                    .arg(repository)
                    .args(["rev-parse", "--verify", &format!("{head}^{{tree}}")])
                    .output()
                    && output.status.success()
                {
                    legacy.insert(head, json!(String::from_utf8_lossy(&output.stdout).trim()));
                }
            }
        }
        let identities = serde_json::to_string(&legacy).expect("strings serialize");
        const SELECTED: &str = "WITH selected AS (
            SELECT r.*, coalesce(r.git_tree,
                (SELECT value FROM json_each(?3) WHERE key = r.head)) AS effective_tree
            FROM runs r WHERE r.project = ?1
              AND r.checkout IN (SELECT value FROM json_each(?2))) ";
        let mut statement = self.conn.prepare(&format!(
            "{SELECTED}
            SELECT r.effective_tree, r.dirty, t.binary, t.name,
                   json_group_array(json_object('run', r.id, 'checkout', r.checkout,
                       'head', r.head, 'tree_source', CASE WHEN r.git_tree IS NULL
                           THEN 'resolved legacy HEAD' ELSE 'recorded' END,
                       'command', r.command, 'args', r.args, 'run_status', r.status,
                       'test_status', t.status, 'started_ms', r.started_ms,
                       'toolchain', r.toolchain, 'host', r.host))
            FROM selected r JOIN tests t ON t.run = r.id
            WHERE r.effective_tree IS NOT NULL AND r.observation_error IS NULL
            GROUP BY r.effective_tree, r.dirty, t.binary, t.name
            HAVING count(DISTINCT CASE WHEN t.status = 'passed' THEN r.id END) > 0
               AND count(DISTINCT CASE WHEN t.status = 'failed' THEN r.id END) > 0
               AND count(DISTINCT CASE WHEN t.status IN ('passed','failed') THEN r.id END) > 1
            ORDER BY r.effective_tree, r.dirty, t.binary, t.name"
        ))?;
        let groups = statement
            .query_map(params![project, selected, identities], |row| {
                let observations: String = row.get(4)?;
                Ok(
                    json!({"recorded_tree": {"git_tree": row.get::<_, String>(0)?,
                                      "dirty_hash": row.get::<_, Option<String>>(1)?},
                "binary": row.get::<_, String>(2)?, "name": row.get::<_, String>(3)?,
                "observations": decode_json(&observations)?}),
                )
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let unknown: i64 = self.conn.query_row(
            &format!(
                "{SELECTED}
            SELECT count(*) FROM selected r JOIN tests t ON t.run = r.id
            WHERE r.effective_tree IS NULL OR r.observation_error IS NOT NULL"
            ),
            params![project, selected, identities],
            |row| row.get(0),
        )?;
        Ok(
            json!({"population": population(project, checkouts), "legacy_repository": repository,
            "criterion": "recorded passed and failed in different runs at equal Git tree objects and dirty hashes",
            "unknown_tree_observations": unknown, "tests": groups}),
        )
    }

    /// The earliest recorded failed or retry-flaky result, ordered by recorded
    /// start time then run ID. A dirty HEAD is not attributed to the clean revision.
    pub fn first_failing(
        &self,
        project: &str,
        checkouts: &[String],
        binary: &str,
        name: &str,
    ) -> rusqlite::Result<Value> {
        let mut statement = self.conn.prepare(
            "SELECT r.id, r.checkout, r.head, r.dirty, r.started_ms, r.command,
                    r.args, r.status, t.status, r.observation_error, r.git_tree
             FROM runs r JOIN tests t ON t.run = r.id
             WHERE r.project = ?1 AND r.checkout IN (SELECT value FROM json_each(?2))
               AND t.binary = ?3 AND t.name = ?4
             ORDER BY r.started_ms, r.id, t.status",
        )?;
        let observations = statement.query_map(params![project, paths(checkouts), binary, name], |row| {
            let head: Option<String> = row.get(2)?;
            let dirty: Option<String> = row.get(3)?;
            let observation_error: Option<String> = row.get(9)?;
            let attribution = if head.is_none() || observation_error.is_some() { "unknown" }
                else if dirty.is_some() { "uncommitted changes" } else { "recorded clean HEAD" };
            Ok(json!({"run": row.get::<_, i64>(0)?, "checkout": row.get::<_, String>(1)?,
                "head": head, "dirty_hash": dirty, "revision_attribution": attribution,
                "observation_error": observation_error, "git_tree": row.get::<_, Option<String>>(10)?,
                "started_ms": row.get::<_, i64>(4)?, "command": row.get::<_, String>(5)?,
                "args": row.get::<_, String>(6)?, "run_status": row.get::<_, String>(7)?,
                "test_status": row.get::<_, String>(8)?}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        let first = observations
            .iter()
            .find(|row| matches!(row["test_status"].as_str(), Some("failed" | "flaky")));
        Ok(
            json!({"population": population(project, checkouts), "binary": binary, "name": name,
            "ordering": "recorded started_ms, then run ID; not a causal or bisect result",
            "first_recorded_failure": first, "observations": observations}),
        )
    }

    /// Descriptive successful-run samples for an exact command and argument
    /// selection. Excluded rows stay visible; absent stages are not zero-duration samples.
    pub fn durations(
        &self,
        project: &str,
        checkouts: &[String],
        command: &str,
        args: &[String],
        stage: Option<&str>,
    ) -> rusqlite::Result<Value> {
        let selection = serde_json::to_string(args).expect("strings serialize");
        let mut statement = self.conn.prepare(
            "SELECT r.id, r.checkout, r.status, r.duration_ms, r.head, r.dirty,
                    r.toolchain, r.host, s.position, s.duration_ms, s.ok, r.observation_error, r.started_ms
             FROM runs r LEFT JOIN stages s ON s.run = r.id AND s.name = ?5
             WHERE r.project = ?1 AND r.checkout IN (SELECT value FROM json_each(?2))
               AND r.command = ?3 AND r.args = ?4
             ORDER BY r.started_ms, r.id, s.position",
        )?;
        let rows = statement.query_map(params![project, paths(checkouts), command, selection, stage], |row| {
            let status: String = row.get(2)?;
            let total: Option<i64> = row.get(3)?;
            let position: Option<i64> = row.get(8)?;
            let stage_duration: Option<i64> = row.get(9)?;
            let stage_ok: Option<bool> = row.get(10)?;
            let duration = if stage.is_some() { stage_duration } else { total };
            let excluded = if status != "passed" { Some("run not passed") }
                else if stage.is_some() && position.is_none() { Some("stage not recorded") }
                else if stage_ok == Some(false) { Some("stage failed") }
                else if duration.is_none() { Some("duration not recorded") }
                else if duration.is_some_and(|ms| ms < 0) { Some("invalid duration") }
                else { None };
            Ok(json!({"run": row.get::<_, i64>(0)?, "checkout": row.get::<_, String>(1)?,
                "run_status": status, "head": row.get::<_, Option<String>>(4)?,
                "dirty_hash": row.get::<_, Option<String>>(5)?,
                "toolchain": row.get::<_, Option<String>>(6)?, "host": row.get::<_, Option<String>>(7)?,
                "stage_position": position, "stage_ok": stage_ok, "duration_ms": duration,
                "observation_error": row.get::<_, Option<String>>(11)?,
                "started_ms": row.get::<_, i64>(12)?,
                "excluded": excluded}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?;
        let mut durations: Vec<i64> = rows
            .iter()
            .filter(|row| row["excluded"].is_null())
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
        Ok(
            json!({"population": population(project, checkouts), "command": command,
            "args": args, "stage": stage, "sample_policy": "passed runs; recorded successful stage when selected",
            "interpretation": "descriptive recorded samples, not a completion promise",
            "selected_run_count": rows.iter().filter_map(|row| row["run"].as_i64()).collect::<std::collections::BTreeSet<_>>().len(),
            "sample_count": durations.len(), "min_ms": durations.first(), "median_ms": median,
            "max_ms": durations.last(), "observations": rows}),
        )
    }

    /// Equal recorded compiler messages across the selected worktrees. A missing
    /// diagnostic row supplies no evidence that a compiler step succeeded.
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
                Ok(
                    json!({"text_hash": row.get::<_, String>(0)?, "text": row.get::<_, String>(1)?,
                "occurrences": decode_json(&occurrences)?}),
                )
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(
            json!({"population": population(project, checkouts), "level": level, "diagnostics": groups}),
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
