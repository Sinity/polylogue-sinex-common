//! Test outcomes in the store: each run's passed, ignored and filtered tests
//! as one compressed set (`test_sets`), its failed, flaky and skipped tests as
//! rows (`test_results`), and every binary ID and test name once.
use crate::{TestOutcome, TestStatus};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

/// One recorded test outcome, with interned binary and name IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub binary: i64,
    pub name: i64,
    pub status: &'static str,
    pub duration_ms: i64,
}

const SET_STATUSES: [&str; 3] = ["passed", "ignored", "filtered"];

fn set_status(status: TestStatus) -> Option<u8> {
    match status {
        TestStatus::Passed => Some(0),
        TestStatus::Ignored => Some(1),
        TestStatus::Filtered => Some(2),
        TestStatus::Failed | TestStatus::Flaky | TestStatus::Skipped => None,
    }
}

fn row_status(status: &str) -> &'static str {
    match status {
        "failed" => "failed",
        "flaky" => "flaky",
        _ => "skipped",
    }
}

/// The IDs of `values` in `table` (`test_binaries` or `test_names`), each
/// added when new.
fn intern<'a>(
    conn: &Connection,
    table: &str,
    column: &str,
    values: impl Iterator<Item = &'a str>,
) -> rusqlite::Result<HashMap<&'a str, i64>> {
    let mut ids: HashMap<&str, i64> = values.map(|value| (value, 0)).collect();
    let wanted = serde_json::to_string(&ids.keys().collect::<Vec<_>>()).expect("strings serialize");
    let mut known = conn.prepare(&format!(
        "SELECT {column}, id FROM {table} WHERE {column} IN (SELECT value FROM json_each(?1))"
    ))?;
    let rows = known.query_map([wanted], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?;
    for row in rows {
        let (value, id) = row?;
        if let Some(slot) = ids.get_mut(value.as_str()) {
            *slot = id;
        }
    }
    let mut add = conn.prepare(&format!("INSERT INTO {table} ({column}) VALUES (?1)"))?;
    for (value, id) in ids.iter_mut().filter(|(_, id)| **id == 0) {
        add.execute([*value])?;
        *id = conn.last_insert_rowid();
    }
    Ok(ids)
}

/// Records a run's test outcomes inside the caller's transaction.
pub(super) fn insert(conn: &Connection, run: i64, tests: &[TestOutcome]) -> rusqlite::Result<()> {
    let binaries = intern(
        conn,
        "test_binaries",
        "binary",
        tests.iter().map(|test| test.binary.as_str()),
    )?;
    let names = intern(
        conn,
        "test_names",
        "name",
        tests.iter().map(|test| test.name.as_str()),
    )?;
    let mut set = Vec::new();
    for test in tests {
        let binary = binaries[test.binary.as_str()];
        let name = names[test.name.as_str()];
        let duration_ms = test.duration.as_millis() as i64;
        match set_status(test.status) {
            Some(status) => set.push((binary, name, status, duration_ms)),
            None => {
                let output = test
                    .output
                    .as_deref()
                    .map(|text| zstd::encode_all(text.as_bytes(), 3).unwrap_or_default());
                conn.prepare_cached(
                    "INSERT INTO test_results (run, binary, name, status, duration_ms, output)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                )?
                .execute(params![
                    run,
                    binary,
                    name,
                    test.status.as_str(),
                    duration_ms,
                    output
                ])?;
            }
        }
    }
    if !set.is_empty() {
        conn.execute(
            "INSERT INTO test_sets (run, tests) VALUES (?1, ?2)",
            params![run, encode(&mut set)],
        )?;
    }
    Ok(())
}

fn push_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn read_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

/// The `test_sets` encoding of (binary, name, status, duration) entries.
pub(super) fn encode(entries: &mut [(i64, i64, u8, i64)]) -> Vec<u8> {
    entries.sort_unstable();
    let mut raw = Vec::with_capacity(entries.len() * 5);
    let (mut binary, mut name) = (0i64, 0i64);
    for &(entry_binary, entry_name, status, duration) in entries.iter() {
        push_varint(&mut raw, (entry_binary - binary) as u64);
        if entry_binary != binary {
            name = 0;
        }
        push_varint(&mut raw, (entry_name - name) as u64);
        raw.push(status);
        push_varint(&mut raw, duration.max(0) as u64);
        (binary, name) = (entry_binary, entry_name);
    }
    zstd::encode_all(raw.as_slice(), 9).expect("encoding into memory succeeds")
}

pub(super) fn decode(blob: &[u8]) -> rusqlite::Result<Vec<Outcome>> {
    let invalid = |reason: String| {
        rusqlite::Error::FromSqlConversionFailure(
            blob.len(),
            rusqlite::types::Type::Blob,
            reason.into(),
        )
    };
    let raw = zstd::decode_all(blob).map_err(|error| invalid(error.to_string()))?;
    let mut out = Vec::new();
    let mut at = 0;
    let (mut binary, mut name) = (0i64, 0i64);
    while at < raw.len() {
        let entry = (|| {
            let binary_step = read_varint(&raw, &mut at)? as i64;
            let name_step = read_varint(&raw, &mut at)? as i64;
            let status = *SET_STATUSES.get(usize::from(*raw.get(at)?))?;
            at += 1;
            let duration = read_varint(&raw, &mut at)? as i64;
            Some((binary_step, name_step, status, duration))
        })()
        .ok_or_else(|| invalid("truncated test set".into()))?;
        if entry.0 != 0 {
            name = 0;
        }
        binary += entry.0;
        name += entry.1;
        out.push(Outcome {
            binary,
            name,
            status: entry.2,
            duration_ms: entry.3,
        });
    }
    Ok(out)
}

/// Every recorded outcome of one run: its set and its rows.
pub(crate) fn outcomes(conn: &Connection, run: i64) -> rusqlite::Result<Vec<Outcome>> {
    let mut out = match conn
        .prepare_cached("SELECT tests FROM test_sets WHERE run = ?1")?
        .query_row([run], |row| row.get::<_, Vec<u8>>(0))
        .optional()?
    {
        Some(blob) => decode(&blob)?,
        None => Vec::new(),
    };
    let mut statement = conn.prepare_cached(
        "SELECT binary, name, status, duration_ms FROM test_results WHERE run = ?1",
    )?;
    let rows = statement.query_map([run], |row| {
        Ok(Outcome {
            binary: row.get(0)?,
            name: row.get(1)?,
            status: row_status(&row.get::<_, String>(2)?),
            duration_ms: row.get(3)?,
        })
    })?;
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Binary IDs and test names by their interned IDs.
pub(crate) struct Names {
    binaries: HashMap<i64, String>,
    names: HashMap<i64, String>,
}

impl Names {
    pub fn load(conn: &Connection) -> rusqlite::Result<Self> {
        let table = |sql: &str| -> rusqlite::Result<HashMap<i64, String>> {
            conn.prepare(sql)?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect()
        };
        Ok(Self {
            binaries: table("SELECT id, binary FROM test_binaries")?,
            names: table("SELECT id, name FROM test_names")?,
        })
    }

    pub fn binary(&self, id: i64) -> &str {
        self.binaries.get(&id).map_or("", String::as_str)
    }

    pub fn name(&self, id: i64) -> &str {
        self.names.get(&id).map_or("", String::as_str)
    }
}

/// The interned IDs of one test, or `None` when it was never recorded.
pub(crate) fn test_id(
    conn: &Connection,
    binary: &str,
    name: &str,
) -> rusqlite::Result<Option<(i64, i64)>> {
    conn.query_row(
        "SELECT b.id, n.id FROM test_binaries b, test_names n WHERE b.binary = ?1 AND n.name = ?2",
        [binary, name],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}
