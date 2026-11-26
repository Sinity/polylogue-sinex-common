//! Benchmarks that compare two builds on the same inputs.
//!
//! A project lists its corpus as [`Candidate`] files, each with a stratum
//! (an origin, a format era…) and the group it is drawn with (a session with
//! the files that belong to it). [`select`] draws a sample of a target size
//! that keeps each stratum's share of bytes and spread of sizes, taking each
//! group whole. The sample is written down as a [`Manifest`] (paths, sizes,
//! BLAKE3) and reused until it is drawn again.
//!
//! [`measure`] runs one measured process under the caller's memory limit in
//! a transient systemd unit and reads the unit's own accounting.
//! [`interleave`] runs the builds in turn, A B A B …, and [`report`] compares
//! them per figure: the ratio of medians with its range, or no difference
//! when the two builds' ranges overlap. Host load is recorded with every
//! repetition and gates nothing. [`record`] keeps the result in the history
//! store.
use crate::history::Store;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

/// One file of a project's corpus.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Where the file is now.
    pub path: PathBuf,
    /// Where it goes under the bench directory (the project's own layout).
    pub rel: PathBuf,
    /// Origin, source family or era; the sample keeps each stratum's share.
    pub stratum: String,
    pub size: u64,
    /// Files with the same group are drawn together (a session with its
    /// subagents and tool results); `None` draws the file alone. Members of
    /// a group share its stratum.
    pub group: Option<String>,
}

/// Size distribution of a set of files.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spread {
    pub files: u64,
    pub bytes: u64,
    pub p50: u64,
    pub p90: u64,
    pub p99: u64,
    pub max: u64,
}

impl Spread {
    pub fn of(sizes: &mut [u64]) -> Spread {
        sizes.sort_unstable();
        let q = |f: f64| -> u64 {
            if sizes.is_empty() {
                0
            } else {
                sizes[((sizes.len() - 1) as f64 * f).round() as usize]
            }
        };
        Spread {
            files: sizes.len() as u64,
            bytes: sizes.iter().sum(),
            p50: q(0.5),
            p90: q(0.9),
            p99: q(0.99),
            max: sizes.last().copied().unwrap_or(0),
        }
    }

    fn to_json(&self) -> Value {
        json!({"files": self.files, "bytes": self.bytes, "p50": self.p50,
               "p90": self.p90, "p99": self.p99, "max": self.max})
    }

    fn from_json(v: &Value) -> Spread {
        let g = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        Spread {
            files: g("files"),
            bytes: g("bytes"),
            p50: g("p50"),
            p90: g("p90"),
            p99: g("p99"),
            max: g("max"),
        }
    }
}

/// Per-stratum spread of a set of candidates.
pub fn shape<'a>(items: impl IntoIterator<Item = &'a Candidate>) -> BTreeMap<String, Spread> {
    let mut sizes: BTreeMap<String, Vec<u64>> = BTreeMap::new();
    for c in items {
        sizes.entry(c.stratum.clone()).or_default().push(c.size);
    }
    sizes
        .into_iter()
        .map(|(k, mut v)| (k, Spread::of(&mut v)))
        .collect()
}

/// Files drawn together, with their bytes summed.
struct Unit<'a> {
    key: String,
    stratum: &'a str,
    size: u64,
    files: Vec<&'a Candidate>,
}

fn units(all: &[Candidate]) -> Vec<Unit<'_>> {
    let mut by_key: BTreeMap<String, Vec<&Candidate>> = BTreeMap::new();
    for c in all {
        let key = match &c.group {
            Some(group) => format!("group\0{group}"),
            None => format!("file\0{}", c.rel.display()),
        };
        by_key.entry(key).or_default().push(c);
    }
    by_key
        .into_iter()
        .map(|(key, mut files)| {
            files.sort_by(|a, b| a.rel.cmp(&b.rel));
            Unit {
                key,
                stratum: &files[0].stratum,
                size: files.iter().map(|c| c.size).sum(),
                files,
            }
        })
        .collect()
}

/// Deterministic order within a size band, independent of listing order.
fn rank(seed: u64, unit: &Unit<'_>) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&seed.to_le_bytes());
    h.update(unit.key.as_bytes());
    *h.finalize().as_bytes()
}

/// Draws about `target` bytes from `all`, each group whole. Each stratum
/// gets its share of the corpus's bytes; within a stratum, groups are split
/// into ten size bands of equal count and each band contributes its share of
/// the stratum's bytes, so small, typical and large groups are all present.
/// A stratum's largest group is taken first when it fits in half the
/// stratum's share (the largest inputs are where memory and throughput
/// problems show).
pub fn select(all: &[Candidate], target: u64, seed: u64) -> Vec<Candidate> {
    let total: u64 = all.iter().map(|c| c.size).sum();
    if total == 0 {
        return Vec::new();
    }
    let all_units = units(all);
    let mut by_stratum: BTreeMap<&str, Vec<&Unit<'_>>> = BTreeMap::new();
    for unit in &all_units {
        by_stratum.entry(unit.stratum).or_default().push(unit);
    }
    let mut chosen: Vec<&Unit<'_>> = Vec::new();
    for groups in by_stratum.values_mut() {
        let bytes: u64 = groups.iter().map(|u| u.size).sum();
        let share = (u128::from(target) * u128::from(bytes) / u128::from(total)) as u64;
        groups.sort_by(|a, b| a.size.cmp(&b.size).then_with(|| a.key.cmp(&b.key)));
        if share == 0 || groups[0].size > share {
            // A stratum too small for a share, or whose every group exceeds
            // it, is still represented by its smallest group.
            chosen.push(groups[0]);
            continue;
        }
        let mut used = 0u64;
        if let Some(largest) = groups.last()
            && largest.size <= share / 2
        {
            chosen.push(largest);
            used += largest.size;
            groups.pop();
        }
        if groups.is_empty() {
            continue;
        }
        let bands = 10.min(groups.len()).max(1);
        let per = groups.len().div_ceil(bands);
        let remaining = share.saturating_sub(used);
        let rest_bytes: u64 = groups.iter().map(|u| u.size).sum::<u64>().max(1);
        for band in groups.chunks(per) {
            let band_bytes: u64 = band.iter().map(|u| u.size).sum();
            let quota =
                (u128::from(remaining) * u128::from(band_bytes) / u128::from(rest_bytes)) as u64;
            let mut order: Vec<&&Unit<'_>> = band.iter().collect();
            order.sort_by_key(|u| rank(seed, u));
            let mut got = 0u64;
            for unit in order {
                if got >= quota {
                    break;
                }
                // A group bigger than the stratum's whole share would swamp
                // the sample; the representativeness report shows its absence.
                if unit.size > share {
                    continue;
                }
                // Keep the band's quota within one group of its target.
                if got > 0 && got + unit.size > quota + quota / 2 {
                    continue;
                }
                got += unit.size;
                chosen.push(unit);
            }
        }
    }
    let mut files: Vec<Candidate> = chosen
        .into_iter()
        .flat_map(|u| u.files.iter().map(|c| (*c).clone()))
        .collect();
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    files
}

/// A drawn sample, reused until it is drawn again.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub project: String,
    pub target_bytes: u64,
    pub seed: u64,
    pub entries: Vec<Entry>,
    /// The full corpus's shape when the sample was drawn.
    pub corpus: BTreeMap<String, Spread>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub source: PathBuf,
    pub rel: PathBuf,
    pub stratum: String,
    pub size: u64,
    pub blake3: String,
}

impl Manifest {
    /// Hashes each chosen file.
    pub fn build(
        project: &str,
        all: &[Candidate],
        chosen: &[Candidate],
        target_bytes: u64,
        seed: u64,
    ) -> io::Result<Manifest> {
        let mut entries = Vec::with_capacity(chosen.len());
        for c in chosen {
            let blake3 = hash_file(&c.path)?;
            entries.push(Entry {
                source: c.path.clone(),
                rel: c.rel.clone(),
                stratum: c.stratum.clone(),
                size: c.size,
                blake3,
            });
        }
        Ok(Manifest {
            project: project.into(),
            target_bytes,
            seed,
            entries,
            corpus: shape(all),
        })
    }

    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| e.size).sum()
    }

    /// The sample's own shape, per stratum.
    pub fn sample_shape(&self) -> BTreeMap<String, Spread> {
        let mut sizes: BTreeMap<String, Vec<u64>> = BTreeMap::new();
        for e in &self.entries {
            sizes.entry(e.stratum.clone()).or_default().push(e.size);
        }
        sizes
            .into_iter()
            .map(|(k, mut v)| (k, Spread::of(&mut v)))
            .collect()
    }

    /// Identity of the sample's content: changes when any chosen file's
    /// bytes or place change.
    pub fn id(&self) -> String {
        let mut h = blake3::Hasher::new();
        for e in &self.entries {
            h.update(e.rel.as_os_str().as_encoded_bytes());
            h.update(e.blake3.as_bytes());
        }
        h.finalize().to_hex()[..16].to_owned()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "project": self.project,
            "id": self.id(),
            "target_bytes": self.target_bytes,
            "seed": self.seed,
            "bytes": self.bytes(),
            "corpus": self.corpus.iter().map(|(k, s)| (k.clone(), s.to_json())).collect::<serde_json::Map<_, _>>(),
            "entries": self.entries.iter().map(|e| json!({
                "source": e.source, "rel": e.rel, "stratum": e.stratum,
                "size": e.size, "blake3": e.blake3,
            })).collect::<Vec<_>>(),
        })
    }

    pub fn from_json(v: &Value) -> Option<Manifest> {
        let entries = v
            .get("entries")?
            .as_array()?
            .iter()
            .map(|e| {
                Some(Entry {
                    source: PathBuf::from(e.get("source")?.as_str()?),
                    rel: PathBuf::from(e.get("rel")?.as_str()?),
                    stratum: e.get("stratum")?.as_str()?.to_owned(),
                    size: e.get("size")?.as_u64()?,
                    blake3: e.get("blake3")?.as_str()?.to_owned(),
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Manifest {
            project: v.get("project")?.as_str()?.to_owned(),
            target_bytes: v.get("target_bytes")?.as_u64()?,
            seed: v.get("seed")?.as_u64()?,
            entries,
            corpus: v
                .get("corpus")?
                .as_object()?
                .iter()
                .map(|(k, s)| (k.clone(), Spread::from_json(s)))
                .collect(),
        })
    }

    pub fn write(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.to_json())?)?;
        fs::rename(tmp, path)
    }

    pub fn read(path: &Path) -> io::Result<Manifest> {
        let v: Value = serde_json::from_slice(&fs::read(path)?)?;
        Manifest::from_json(&v).ok_or_else(|| io::Error::other("not a bench manifest"))
    }

    /// How representative the sample is: per stratum, its share of bytes in
    /// the corpus and in the sample, and both size spreads.
    pub fn representativeness(&self) -> Value {
        let sample = self.sample_shape();
        let corpus_total: u64 = self.corpus.values().map(|s| s.bytes).sum::<u64>().max(1);
        let sample_total = self.bytes().max(1);
        let strata: Vec<Value> = self
            .corpus
            .iter()
            .map(|(k, c)| {
                let s = sample.get(k).cloned().unwrap_or_default();
                json!({
                    "stratum": k,
                    "corpus_share": c.bytes as f64 / corpus_total as f64,
                    "sample_share": s.bytes as f64 / sample_total as f64,
                    "corpus": c.to_json(),
                    "sample": s.to_json(),
                })
            })
            .collect();
        json!({"corpus_bytes": corpus_total, "sample_bytes": sample_total,
               "scale": corpus_total as f64 / sample_total as f64, "strata": strata})
    }

    /// Copies the sample into `dest` (reflinks where the filesystem allows),
    /// checking each copy against its hash. Does nothing when `dest` already
    /// holds this manifest.
    pub fn materialize(&self, dest: &Path) -> io::Result<bool> {
        let stamp = dest.join(".bench-manifest");
        if fs::read_to_string(&stamp).ok().as_deref() == Some(self.id().as_str()) {
            return Ok(false);
        }
        if dest.exists() {
            fs::remove_dir_all(dest)?;
        }
        for e in &self.entries {
            let to = dest.join(&e.rel);
            fs::create_dir_all(to.parent().unwrap_or(dest))?;
            let ok = Command::new("cp")
                .arg("--reflink=auto")
                .arg("--preserve=timestamps")
                .arg(&e.source)
                .arg(&to)
                .status()?
                .success();
            if !ok {
                return Err(io::Error::other(format!("copy {}", e.source.display())));
            }
            if hash_file(&to)? != e.blake3 {
                return Err(io::Error::other(format!(
                    "{} changed since the sample was drawn; draw it again",
                    e.source.display()
                )));
            }
        }
        fs::write(stamp, self.id())?;
        Ok(true)
    }
}

pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(fs::File::open(path)?)?;
    Ok(h.finalize().to_hex().to_string())
}

mod compare;
mod load;
mod measure;
pub use compare::{Arm, Figure, Kind, Repetition, compare, interleave, report, summarize};
pub use load::{HostLoad, LoadInterval, LoadMonitor};
pub use measure::{Accounting, Limit, Measured, Process, measure};

/// Where a bench run kept its output: on the production disk, or in memory
/// (which measures the pipeline alone).
pub fn storage_of(path: &Path) -> &'static str {
    let mounts = fs::read_to_string("/proc/self/mounts").unwrap_or_default();
    let mut best: Option<(&str, &str)> = None;
    for line in mounts.lines() {
        let mut f = line.split_whitespace();
        let (_, Some(point), Some(fstype)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if path.starts_with(point) && best.is_none_or(|(p, _)| point.len() > p.len()) {
            best = Some((point, fstype));
        }
    }
    match best {
        Some((_, "tmpfs" | "ramfs")) => "memory",
        _ => "disk",
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS bench_results (
    -- never reused, so a result ID printed once names that result for good
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    bench TEXT NOT NULL,
    -- the sample's manifest ID, or the project's name for its fixed inputs
    sample TEXT NOT NULL,
    -- memory or disk, and the directory the measured runs wrote to
    storage TEXT NOT NULL,
    output_dir TEXT NOT NULL,
    head TEXT,
    dirty TEXT,
    recorded_ms INTEGER NOT NULL,
    -- the result document: for a comparison, the limit, the builds, every
    -- repetition with its unit accounting and host load, and the comparison
    result TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS bench_results_by_project ON bench_results (project, id);
";

/// One recorded bench result.
#[derive(Debug, Clone)]
pub struct BenchResult {
    pub id: i64,
    pub project: String,
    pub bench: String,
    pub sample: String,
    pub storage: String,
    pub head: Option<String>,
    pub recorded_ms: i64,
    pub result: Value,
}

/// Records a result document (from [`report`], or a project's own) against
/// the checkout's commit.
pub fn record(
    store: &Store,
    checkout: &Path,
    project: &str,
    bench: &str,
    sample: &str,
    output_dir: &Path,
    result: &Value,
) -> rusqlite::Result<i64> {
    let conn = store.connection();
    conn.execute_batch(SCHEMA)?;
    let tree = crate::Tree::observe(checkout);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    conn.execute(
        "INSERT INTO bench_results (project, bench, sample, storage, output_dir, head, dirty,
            recorded_ms, result)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            project,
            bench,
            sample,
            storage_of(output_dir),
            output_dir.display().to_string(),
            if tree.observation_error.is_none() {
                tree.head
            } else {
                None
            },
            tree.dirty,
            now,
            result.to_string(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

const COLUMNS: &str = "SELECT id, project, bench, sample, storage, head, recorded_ms, result
                       FROM bench_results";

/// The project's latest results, newest first.
pub fn results(store: &Store, project: &str, limit: usize) -> rusqlite::Result<Vec<BenchResult>> {
    let conn = store.connection();
    conn.execute_batch(SCHEMA)?;
    let mut stmt = conn.prepare(&format!(
        "{COLUMNS} WHERE project = ?1 ORDER BY id DESC LIMIT ?2"
    ))?;
    let rows = stmt
        .query_map(params![project, limit as i64], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// One result of the project's, by ID.
pub fn result(store: &Store, project: &str, id: i64) -> rusqlite::Result<Option<BenchResult>> {
    let conn = store.connection();
    conn.execute_batch(SCHEMA)?;
    conn.query_row(
        &format!("{COLUMNS} WHERE project = ?1 AND id = ?2"),
        params![project, id],
        row,
    )
    .optional()
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BenchResult> {
    Ok(BenchResult {
        id: r.get(0)?,
        project: r.get(1)?,
        bench: r.get(2)?,
        sample: r.get(3)?,
        storage: r.get(4)?,
        head: r.get(5)?,
        recorded_ms: r.get(6)?,
        result: serde_json::from_str(&r.get::<_, String>(7)?).unwrap_or(Value::Null),
    })
}

#[cfg(test)]
mod tests;
