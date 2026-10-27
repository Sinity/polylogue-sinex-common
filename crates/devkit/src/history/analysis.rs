//! Comparisons over recorded stages and tests. Native queue and resource costs stay with AgentCTL.
use super::Store;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

impl Store {
    pub fn explain(&self, project: &str, id: i64) -> Result<Value, String> {
        let mut header = self.conn.query_row(
            "SELECT command,args,host,toolchain,status,duration_ms,started_ms,agentctl_job,dirty FROM runs WHERE project=?1 AND id=?2",
            params![project,id],
            |row| Ok(json!({"command":row.get::<_,String>(0)?,"args":row.get::<_,String>(1)?,"host":row.get::<_,Option<String>>(2)?,"toolchain":row.get::<_,Option<String>>(3)?,"status":row.get::<_,String>(4)?,"duration_ms":row.get::<_,Option<i64>>(5)?,"started_ms":row.get::<_,i64>(6)?,"agentctl_job":row.get::<_,Option<String>>(7)?,"dirty_hash":row.get::<_,Option<String>>(8)?})),
        ).optional().map_err(|error|error.to_string())?.ok_or("run not found in this project")?;
        let observed = self
            .run(id)
            .map_err(|error| error.to_string())?
            .ok_or("run not found")?;
        header["status"] = json!(observed.status);
        header["checkout"] = json!(observed.checkout);
        header["head"] = json!(observed.head);
        header["git_tree"] = json!(observed.git_tree);
        header["dirty"] = json!(observed.dirty);
        header["observation_error"] = json!(observed.observation_error);
        header["failure"] = json!(observed.failure);
        let mut statement = self.conn.prepare(
            "SELECT name,sum(duration_ms),min(ok) FROM stages WHERE run=?1 GROUP BY name ORDER BY min(position)",
        ).map_err(|error| error.to_string())?;
        let stages = statement.query_map([id], |row| Ok(json!({"name":row.get::<_,String>(0)?,"duration_ms":row.get::<_,i64>(1)?,"ok":row.get::<_,bool>(2)?})))
            .map_err(|error| error.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())?;
        let mut statement = self.conn.prepare(
            "SELECT binary,name,status,duration_ms FROM tests WHERE run=?1 ORDER BY duration_ms DESC,binary,name",
        ).map_err(|error| error.to_string())?;
        let tests = statement.query_map([id], |row| Ok(json!({"binary":row.get::<_,String>(0)?,"name":row.get::<_,String>(1)?,"status":row.get::<_,String>(2)?,"duration_ms":row.get::<_,i64>(3)?})))
            .map_err(|error| error.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())?;
        let counted = tests
            .iter()
            .filter(|test| matches!(test["status"].as_str(), Some("passed" | "failed" | "flaky")))
            .count();
        let passed = tests
            .iter()
            .filter(|test| test["status"] == "passed")
            .count();
        let pass_rate = (counted > 0).then(|| 100.0 * passed as f64 / counted as f64);
        let mut statement = self.conn.prepare(
            "SELECT other.id,other.project,other.command,other.agentctl_job,
                    min(target.started_ms+target.duration_ms,other.started_ms+other.duration_ms)-max(target.started_ms,other.started_ms)
             FROM runs target JOIN runs other ON other.id!=target.id AND other.host=target.host
             WHERE target.id=?1 AND other.started_ms < target.started_ms+target.duration_ms
               AND target.started_ms < other.started_ms+other.duration_ms ORDER BY other.id",
        ).map_err(|error| error.to_string())?;
        let overlaps = statement.query_map([id], |row| Ok(json!({"run":row.get::<_,i64>(0)?,"project":row.get::<_,String>(1)?,"command":row.get::<_,String>(2)?,"agentctl_job":row.get::<_,Option<String>>(3)?,"overlap_ms":row.get::<_,i64>(4)?})))
            .map_err(|error| error.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())?;
        Ok(
            json!({"run":id,"header":header,"stages":stages,"tests_slowest_first":tests,"pass_rate_percent":pass_rate,"overlaps":overlaps,"queue_ms":null,"resources":null,"resource_note":"native job timing and resources belong to AgentCTL; overlap does not establish contention"}),
        )
    }

    pub fn compare(&self, project: &str, before: i64, after: i64) -> Result<Value, String> {
        let before = self.explain(project, before)?;
        let after = self.explain(project, after)?;
        for field in ["command", "args", "host", "toolchain"] {
            if before["header"][field].is_null()
                || before["header"][field] != after["header"][field]
            {
                return Err(format!("runs differ in {field} or it was not recorded"));
            }
        }
        for report in [&before, &after] {
            if report["header"]["status"] != "passed" {
                return Err("duration comparison requires passed runs".into());
            }
            if !report["header"]["observation_error"].is_null() {
                return Err("run source could not be observed consistently".into());
            }
        }
        let names: BTreeSet<&str> = [&before, &after]
            .into_iter()
            .flat_map(|report| report["stages"].as_array().into_iter().flatten())
            .filter_map(|stage| stage["name"].as_str())
            .collect();
        let mut stages = Vec::new();
        for name in names {
            let duration = |report: &Value| {
                report["stages"]
                    .as_array()
                    .and_then(|stages| stages.iter().find(|stage| stage["name"] == name))
                    .map(|stage| stage["duration_ms"].clone())
                    .unwrap_or(Value::Null)
            };
            stages.push(
                json!({"name":name,"change":duration_delta(&duration(&before), &duration(&after))}),
            );
        }
        Ok(
            json!({"duration_change":duration_delta(&before["header"]["duration_ms"], &after["header"]["duration_ms"]),"stage_changes":stages,"before":before,"after":after}),
        )
    }

    /// UTC daily groups keep exact command scope, host and toolchain separate.
    pub fn compare_days(
        &self,
        project: &str,
        checkout: &str,
        before: &str,
        after: &str,
    ) -> Result<Value, String> {
        for day in [before, after] {
            let valid: bool = self
                .conn
                .query_row("SELECT coalesce(date(?1)=?1,0)", [day], |row| row.get(0))
                .map_err(|error| error.to_string())?;
            if !valid || day.len() != 10 {
                return Err("days must be YYYY-MM-DD in UTC".into());
            }
        }
        let mut statement = self.conn.prepare(
            "SELECT command,args,host,toolchain,date(started_ms/1000,'unixepoch'),count(*),sum(status='passed'),
                    avg(CASE WHEN status='passed' AND duration_ms>=0 AND observation_error IS NULL THEN duration_ms END),
                    count(CASE WHEN status='passed' AND duration_ms>=0 AND observation_error IS NULL THEN 1 END)
             FROM runs WHERE project=?1 AND checkout=?2 AND date(started_ms/1000,'unixepoch') IN (?3,?4)
             GROUP BY command,args,host,toolchain,date(started_ms/1000,'unixepoch') ORDER BY command,args,host,toolchain,5",
        ).map_err(|error| error.to_string())?;
        let rows = statement.query_map(params![project,checkout,before,after], |row| Ok(json!({"command":row.get::<_,String>(0)?,"args":row.get::<_,String>(1)?,"host":row.get::<_,Option<String>>(2)?,"toolchain":row.get::<_,Option<String>>(3)?,"day":row.get::<_,String>(4)?,"runs":row.get::<_,i64>(5)?,"passed":row.get::<_,i64>(6)?,"mean_passed_ms":row.get::<_,Option<f64>>(7)?,"duration_samples":row.get::<_,i64>(8)?})))
            .map_err(|error| error.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())?;
        let mut statement = self.conn.prepare(
            "SELECT r.command,r.args,r.host,r.toolchain,date(r.started_ms/1000,'unixepoch'),s.name,count(*),avg(s.duration_ms)
             FROM runs r JOIN stages s ON s.run=r.id WHERE r.project=?1 AND r.checkout=?2
               AND date(r.started_ms/1000,'unixepoch') IN (?3,?4) AND r.status='passed'
               AND r.observation_error IS NULL AND s.ok=1 AND s.duration_ms>=0
             GROUP BY r.command,r.args,r.host,r.toolchain,date(r.started_ms/1000,'unixepoch'),s.name
             ORDER BY r.command,r.args,r.host,r.toolchain,5,s.name",
        ).map_err(|error| error.to_string())?;
        let stages = statement.query_map(params![project,checkout,before,after], |row| Ok(json!({"command":row.get::<_,String>(0)?,"args":row.get::<_,String>(1)?,"host":row.get::<_,Option<String>>(2)?,"toolchain":row.get::<_,Option<String>>(3)?,"day":row.get::<_,String>(4)?,"name":row.get::<_,String>(5)?,"samples":row.get::<_,i64>(6)?,"mean_duration_ms":row.get::<_,Option<f64>>(7)?})))
            .map_err(|error| error.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|error| error.to_string())?;
        let fields = ["command", "args", "host", "toolchain"];
        Ok(
            json!({"before":before,"after":after,"changes":daily_changes(&rows,before,after,&fields,"mean_passed_ms"),"stage_changes":daily_changes(&stages,before,after,&["command","args","host","toolchain","name"],"mean_duration_ms"),"groups":rows,"stage_groups":stages}),
        )
    }
}

fn daily_changes(
    rows: &[Value],
    before: &str,
    after: &str,
    fields: &[&str],
    metric: &str,
) -> Vec<Value> {
    let mut groups: BTreeMap<Vec<String>, (Option<&Value>, Option<&Value>)> = BTreeMap::new();
    for row in rows {
        let Some(key) = fields
            .iter()
            .map(|field| row[*field].as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let group = groups.entry(key).or_default();
        if row["day"] == before {
            group.0 = Some(row);
        }
        if row["day"] == after {
            group.1 = Some(row);
        }
    }
    groups
        .into_iter()
        .map(|(key, (before, after))| {
            let mut output = serde_json::Map::new();
            for (field, value) in fields.iter().zip(key) {
                output.insert((*field).to_owned(), json!(value));
            }
            let before = before.map(|row| &row[metric]).unwrap_or(&Value::Null);
            let after = after.map(|row| &row[metric]).unwrap_or(&Value::Null);
            output.insert("change".into(), duration_delta(before, after));
            Value::Object(output)
        })
        .collect()
}

fn duration_delta(before: &Value, after: &Value) -> Value {
    let before = before.as_f64().filter(|value| *value >= 0.0);
    let after = after.as_f64().filter(|value| *value >= 0.0);
    let delta = before.zip(after).map(|(before, after)| after - before);
    let percent = before
        .zip(delta)
        .filter(|(before, _)| *before > 0.0)
        .map(|(before, delta)| 100.0 * delta / before);
    let trend = delta.map(|delta| {
        if delta > 0.0 {
            "slower"
        } else if delta < 0.0 {
            "faster"
        } else {
            "stable"
        }
    });
    json!({"before_ms":before,"after_ms":after,"delta_ms":delta,"percent":percent,"trend":trend})
}
#[cfg(test)]
mod tests;
