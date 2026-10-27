//! Comparisons over recorded execution, without inferring native queue or resource costs.
use super::Store;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

impl Store {
    pub fn explain(&self, project: &str, id: i64) -> Result<Value, String> {
        let mut header = self.conn.query_row("SELECT command,args,host,toolchain,status,duration_ms,started_ms,agentctl_job FROM runs WHERE project=?1 AND id=?2", params![project,id], |r| Ok(json!({"command":r.get::<_,String>(0)?,"args":r.get::<_,String>(1)?,"host":r.get::<_,Option<String>>(2)?,"toolchain":r.get::<_,Option<String>>(3)?,"status":r.get::<_,String>(4)?,"duration_ms":r.get::<_,Option<i64>>(5)?,"started_ms":r.get::<_,i64>(6)?,"agentctl_job":r.get::<_,Option<String>>(7)?}))).optional().map_err(|e|e.to_string())?.ok_or("run not found in this project")?;
        header["status"] = json!(
            self.run(id)
                .map_err(|e| e.to_string())?
                .ok_or("run not found")?
                .status
        );
        let mut statement = self.conn.prepare("SELECT name,sum(duration_ms),min(ok) FROM stages WHERE run=?1 GROUP BY name ORDER BY min(position)").map_err(|e|e.to_string())?;
        let stages = statement.query_map([id], |r| Ok(json!({"name":r.get::<_,String>(0)?,"duration_ms":r.get::<_,i64>(1)?,"ok":r.get::<_,bool>(2)?}))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
        let mut statement = self.conn.prepare("SELECT binary,name,status,duration_ms FROM tests WHERE run=?1 ORDER BY duration_ms DESC,binary,name").map_err(|e|e.to_string())?;
        let tests = statement.query_map([id], |r| Ok(json!({"binary":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?,"duration_ms":r.get::<_,i64>(3)?}))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
        let counted = tests
            .iter()
            .filter(|t| matches!(t["status"].as_str(), Some("passed" | "failed" | "flaky")))
            .count();
        let passed = tests.iter().filter(|t| t["status"] == "passed").count();
        let pass_rate = (counted > 0).then(|| 100.0 * passed as f64 / counted as f64);
        let mut statement = self.conn.prepare("SELECT other.id,other.project,other.command,other.agentctl_job,min(target.started_ms+target.duration_ms,other.started_ms+other.duration_ms)-max(target.started_ms,other.started_ms) FROM runs target JOIN runs other ON other.id!=target.id AND other.host=target.host WHERE target.id=?1 AND other.started_ms < target.started_ms+target.duration_ms AND target.started_ms < other.started_ms+other.duration_ms ORDER BY other.id").map_err(|e|e.to_string())?;
        let overlaps = statement.query_map([id], |r| Ok(json!({"run":r.get::<_,i64>(0)?,"project":r.get::<_,String>(1)?,"command":r.get::<_,String>(2)?,"agentctl_job":r.get::<_,Option<String>>(3)?,"overlap_ms":r.get::<_,i64>(4)?}))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
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
        if before["header"]["status"] != "passed" || after["header"]["status"] != "passed" {
            return Err("duration comparison requires passed runs".into());
        }
        let delta = duration_delta(
            &before["header"]["duration_ms"],
            &after["header"]["duration_ms"],
        );
        let mut stages = Vec::new();
        for stage in before["stages"].as_array().ok_or("stages missing")? {
            let other = after["stages"]
                .as_array()
                .and_then(|s| s.iter().find(|s| s["name"] == stage["name"]));
            stages.push(json!({"name":stage["name"],"change":duration_delta(&stage["duration_ms"], &other.map(|s|s["duration_ms"].clone()).unwrap_or(Value::Null))}));
        }
        Ok(json!({"before":before,"after":after,"duration_change":delta,"stage_changes":stages}))
    }

    /// Daily groups keep exact scope, host and toolchain separate.
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
                .query_row("SELECT coalesce(date(?1)=?1,0)", [day], |r| r.get(0))
                .map_err(|e| e.to_string())?;
            if !valid || day.len() != 10 {
                return Err("days must be YYYY-MM-DD in UTC".into());
            }
        }
        let mut statement = self.conn.prepare("SELECT command,args,host,toolchain,date(started_ms/1000,'unixepoch'),count(*),sum(status='passed'),avg(CASE WHEN status='passed' THEN duration_ms END) FROM runs WHERE project=?1 AND checkout=?2 AND date(started_ms/1000,'unixepoch') IN (?3,?4) GROUP BY command,args,host,toolchain,date(started_ms/1000,'unixepoch') ORDER BY command,args,host,toolchain,5").map_err(|e|e.to_string())?;
        let rows = statement.query_map(params![project,checkout,before,after],|r|Ok(json!({"command":r.get::<_,String>(0)?,"args":r.get::<_,String>(1)?,"host":r.get::<_,Option<String>>(2)?,"toolchain":r.get::<_,Option<String>>(3)?,"day":r.get::<_,String>(4)?,"runs":r.get::<_,i64>(5)?,"passed":r.get::<_,i64>(6)?,"mean_passed_ms":r.get::<_,Option<f64>>(7)?}))).map_err(|e|e.to_string())?.collect::<rusqlite::Result<Vec<_>>>().map_err(|e|e.to_string())?;
        let mut changes = Vec::new();
        for row in rows.iter().filter(|r| r["day"] == before) {
            if row["host"].is_null() || row["toolchain"].is_null() {
                continue;
            }
            if let Some(other) = rows.iter().find(|r| {
                r["day"] == after
                    && ["command", "args", "host", "toolchain"]
                        .iter()
                        .all(|f| r[*f] == row[*f])
            }) {
                changes.push(json!({"command":row["command"],"args":row["args"],"host":row["host"],"toolchain":row["toolchain"],"change":duration_delta(&row["mean_passed_ms"], &other["mean_passed_ms"])}));
            }
        }
        Ok(json!({"before":before,"after":after,"groups":rows,"changes":changes}))
    }
}

fn duration_delta(before: &Value, after: &Value) -> Value {
    match (before.as_f64(), after.as_f64()) {
        (Some(before), Some(after)) if before >= 0.0 && after >= 0.0 => {
            json!({"before_ms":before,"after_ms":after,"delta_ms":after-before,"percent":if before>0.0 {Some(100.0*(after-before)/before)} else {None},"trend":if after>before {"slower"} else if after<before {"faster"} else {"stable"}})
        }
        _ => Value::Null,
    }
}
#[cfg(test)]
mod tests;
