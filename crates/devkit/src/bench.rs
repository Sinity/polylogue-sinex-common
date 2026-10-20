//! Representative benchmarks without running a project over its whole
//! corpus.
//!
//! A project lists its corpus as [`Candidate`] files, each with a stratum
//! (an origin or source family, a format era…). [`select`] draws a sample of
//! a target size that keeps each stratum's share of bytes and each stratum's
//! spread of file sizes, including its largest files when they fit. The
//! sample is frozen as a [`Manifest`] (paths, sizes, BLAKE3), materialized
//! once into a bench directory, and reused until the manifest changes.
//!
//! A bench run records its metrics with the host's load at the start in the
//! history store; [`compare`] shows a run against the previous run on the
//! same sample and storage, and refuses to compare runs made on a loaded
//! host. [`predict`] scales the sample's per-stage times to the full corpus.
use crate::history::Store;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
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
    /// For growing files: the sample is materialized cut back to this many
    /// bytes, and the rest is appended for the incremental pass.
    pub grows: bool,
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

/// Deterministic order within a size band, independent of listing order.
fn rank(seed: u64, c: &Candidate) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&seed.to_le_bytes());
    h.update(c.rel.as_os_str().as_encoded_bytes());
    *h.finalize().as_bytes()
}

/// Draws about `target` bytes from `all`. Each stratum gets its share of
/// the corpus's bytes; within a stratum, files are split into ten size
/// bands of equal file count and each band contributes its share of the
/// stratum's bytes, so small, typical and large files are all present. A
/// stratum's largest file is taken first when it fits in half the
/// stratum's share (the largest files are where memory and throughput
/// problems show).
pub fn select(all: &[Candidate], target: u64, seed: u64) -> Vec<Candidate> {
    let total: u64 = all.iter().map(|c| c.size).sum();
    if total == 0 {
        return Vec::new();
    }
    let mut by_stratum: BTreeMap<&str, Vec<&Candidate>> = BTreeMap::new();
    for c in all {
        by_stratum.entry(&c.stratum).or_default().push(c);
    }
    let mut chosen = Vec::new();
    for files in by_stratum.values_mut() {
        let bytes: u64 = files.iter().map(|c| c.size).sum();
        let share = (u128::from(target) * u128::from(bytes) / u128::from(total)) as u64;
        if share == 0 {
            // A stratum too small for a share still gets one file, so it
            // is exercised.
            if let Some(c) = files.iter().min_by_key(|c| c.size) {
                chosen.push((*c).clone());
            }
            continue;
        }
        files.sort_by_key(|c| c.size);
        if files.first().is_some_and(|c| c.size > share) {
            // Nothing in this stratum fits its share: its smallest file
            // still represents it.
            chosen.push((*files[0]).clone());
            continue;
        }
        let mut used = 0u64;
        if let Some(largest) = files.last()
            && largest.size <= share / 2
        {
            chosen.push((*largest).clone());
            used += largest.size;
            files.pop();
        }
        let bands = 10.min(files.len()).max(1);
        let per = files.len().div_ceil(bands);
        let remaining = share.saturating_sub(used);
        let rest_bytes: u64 = files.iter().map(|c| c.size).sum::<u64>().max(1);
        for band in files.chunks(per) {
            let band_bytes: u64 = band.iter().map(|c| c.size).sum();
            let quota =
                (u128::from(remaining) * u128::from(band_bytes) / u128::from(rest_bytes)) as u64;
            let mut order: Vec<&&Candidate> = band.iter().collect();
            order.sort_by_key(|c| rank(seed, c));
            let mut got = 0u64;
            for c in order {
                if got >= quota {
                    break;
                }
                // A file bigger than the stratum's whole share would swamp
                // the sample; such files are reported by the
                // representativeness check instead.
                if c.size > share {
                    continue;
                }
                // Keep the band's quota within one file of its target.
                if got > 0 && got + c.size > quota + quota / 2 {
                    continue;
                }
                got += c.size;
                chosen.push((**c).clone());
            }
        }
    }
    chosen.sort_by(|a, b| a.rel.cmp(&b.rel));
    chosen
}

/// A frozen sample.
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
    /// Where a growing file is cut for the first build; the rest is
    /// appended for the incremental pass.
    pub cut: Option<u64>,
}

impl Manifest {
    /// Hashes each chosen file and decides the cut of growing files at a
    /// line boundary near `cut_fraction` of their size.
    pub fn build(
        project: &str,
        all: &[Candidate],
        chosen: &[Candidate],
        target_bytes: u64,
        seed: u64,
        cut_fraction: f64,
    ) -> io::Result<Manifest> {
        let mut entries = Vec::with_capacity(chosen.len());
        for c in chosen {
            let blake3 = hash_file(&c.path)?;
            let cut = if c.grows && c.size > 0 {
                line_boundary_before(&c.path, (c.size as f64 * cut_fraction) as u64)?
            } else {
                None
            };
            entries.push(Entry {
                source: c.path.clone(),
                rel: c.rel.clone(),
                stratum: c.stratum.clone(),
                size: c.size,
                blake3,
                cut,
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
    /// bytes, place or cut change.
    pub fn id(&self) -> String {
        let mut h = blake3::Hasher::new();
        for e in &self.entries {
            h.update(e.rel.as_os_str().as_encoded_bytes());
            h.update(e.blake3.as_bytes());
            h.update(&e.cut.unwrap_or(u64::MAX).to_le_bytes());
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
                "size": e.size, "blake3": e.blake3, "cut": e.cut,
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
                    cut: e.get("cut").and_then(Value::as_u64),
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
    /// growing files cut back to their cut, and writes the rest of each
    /// growing file under `dest/../grow/` for the incremental pass. Does
    /// nothing when `dest` already holds this manifest.
    pub fn materialize(&self, dest: &Path) -> io::Result<bool> {
        let stamp = dest.join(".bench-manifest");
        if fs::read_to_string(&stamp).ok().as_deref() == Some(self.id().as_str()) {
            return Ok(false);
        }
        if dest.exists() {
            fs::remove_dir_all(dest)?;
        }
        let grow = grow_dir(dest);
        if grow.exists() {
            fs::remove_dir_all(&grow)?;
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
            if let Some(cut) = e.cut {
                let rest = grow.join(&e.rel);
                fs::create_dir_all(rest.parent().unwrap_or(&grow))?;
                let mut f = fs::File::open(&to)?;
                let mut tail = Vec::new();
                io::Seek::seek(&mut f, io::SeekFrom::Start(cut))?;
                f.read_to_end(&mut tail)?;
                fs::write(&rest, tail)?;
                fs::OpenOptions::new().write(true).open(&to)?.set_len(cut)?;
            }
        }
        fs::write(stamp, self.id())?;
        Ok(true)
    }

    /// Appends the rest of every growing file back onto `dest`, for the
    /// incremental pass. Returns the bytes appended.
    pub fn grow(&self, dest: &Path) -> io::Result<u64> {
        let grow = grow_dir(dest);
        let mut appended = 0;
        for e in self.entries.iter().filter(|e| e.cut.is_some()) {
            let rest = fs::read(grow.join(&e.rel))?;
            let mut f = fs::OpenOptions::new()
                .append(true)
                .open(dest.join(&e.rel))?;
            io::Write::write_all(&mut f, &rest)?;
            appended += rest.len() as u64;
        }
        // The sample no longer matches its stamp: the next run materializes
        // it again.
        let _ = fs::remove_file(dest.join(".bench-manifest"));
        Ok(appended)
    }
}

fn grow_dir(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|n| format!("{}.grow", n.to_string_lossy()))
        .unwrap_or_else(|| "grow".into());
    dest.with_file_name(name)
}

pub fn hash_file(path: &Path) -> io::Result<String> {
    let mut h = blake3::Hasher::new();
    h.update_reader(fs::File::open(path)?)?;
    Ok(h.finalize().to_hex().to_string())
}

/// The offset just after the last newline at or before `at`.
fn line_boundary_before(path: &Path, at: u64) -> io::Result<Option<u64>> {
    let mut f = fs::File::open(path)?;
    let window = at.min(1 << 20);
    io::Seek::seek(&mut f, io::SeekFrom::Start(at - window))?;
    let mut buf = vec![0; window as usize];
    f.read_exact(&mut buf)?;
    Ok(buf
        .iter()
        .rposition(|b| *b == b'\n')
        .map(|i| at - window + i as u64 + 1)
        .filter(|cut| *cut > 0))
}

/// Host load when a run starts: a run on a loaded host measures the host,
/// not the code.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HostLoad {
    pub load1: Option<f64>,
    pub cpus: Option<usize>,
    /// `full avg60` of /proc/pressure/io and /proc/pressure/memory.
    pub io_full: Option<f64>,
    pub memory_full: Option<f64>,
}

impl HostLoad {
    pub fn now() -> HostLoad {
        Self::from_readings(
            fs::read_to_string("/proc/loadavg").ok().as_deref(),
            std::thread::available_parallelism().ok().map(|n| n.get()),
            fs::read_to_string("/proc/pressure/io").ok().as_deref(),
            fs::read_to_string("/proc/pressure/memory").ok().as_deref(),
        )
    }

    fn from_readings(
        load: Option<&str>,
        cpus: Option<usize>,
        io: Option<&str>,
        memory: Option<&str>,
    ) -> Self {
        let number = |s: &str| s.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0);
        let full = |s: &str| {
            let line = s.lines().find(|l| l.starts_with("full "))?;
            number(
                line.split_whitespace()
                    .find_map(|w| w.strip_prefix("avg60="))?,
            )
        };
        Self {
            load1: load.and_then(|s| number(s.split_whitespace().next()?)),
            cpus: cpus.filter(|n| *n > 0),
            io_full: io.and_then(full),
            memory_full: memory.and_then(full),
        }
    }

    /// Quiet enough that timings mean something. Missing readings are
    /// unknown, never evidence of an idle host.
    pub fn quiet(&self) -> bool {
        let (Some(load), Some(cpus), Some(io), Some(memory)) =
            (self.load1, self.cpus, self.io_full, self.memory_full)
        else {
            return false;
        };
        cpus > 0
            && [load, io, memory]
                .iter()
                .all(|v| v.is_finite() && *v >= 0.0)
            && io < 10.0
            && memory < 5.0
            && load < cpus as f64 / 2.0
    }

    fn to_json(self) -> Value {
        json!({"load1": self.load1, "cpus": self.cpus, "io_full": self.io_full,
               "memory_full": self.memory_full, "quiet": self.quiet()})
    }
}

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
CREATE TABLE IF NOT EXISTS bench_runs (
    id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    bench TEXT NOT NULL,
    sample TEXT NOT NULL,
    -- memory or disk, and the directory the run wrote to
    storage TEXT NOT NULL,
    output_dir TEXT NOT NULL,
    head TEXT,
    dirty TEXT,
    started_ms INTEGER NOT NULL,
    load TEXT NOT NULL,
    quiet INTEGER NOT NULL,
    metrics TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS bench_by_sample ON bench_runs (project, bench, sample, storage, id);
";

/// One recorded bench run.
#[derive(Debug, Clone)]
pub struct BenchRun {
    pub id: i64,
    pub project: String,
    pub bench: String,
    pub sample: String,
    pub storage: String,
    pub head: Option<String>,
    pub started_ms: i64,
    pub load: Value,
    pub quiet: bool,
    pub metrics: Value,
}

/// Records a run. `metrics` is the project's flat JSON object of numbers
/// (seconds, MB/s, ratios…); nested `stages` objects are kept as given.
#[allow(clippy::too_many_arguments)]
pub fn record(
    store: &Store,
    checkout: &Path,
    project: &str,
    bench: &str,
    sample: &str,
    output_dir: &Path,
    load: HostLoad,
    metrics: &Value,
) -> rusqlite::Result<i64> {
    let conn = store.connection();
    conn.execute_batch(SCHEMA)?;
    let tree = crate::Tree::observe(checkout);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    conn.execute(
        "INSERT INTO bench_runs (project, bench, sample, storage, output_dir, head, dirty,
            started_ms, load, quiet, metrics)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            project,
            bench,
            sample,
            storage_of(output_dir),
            output_dir.display().to_string(),
            tree.head,
            tree.dirty,
            now,
            load.to_json().to_string(),
            load.quiet(),
            metrics.to_string(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn runs(store: &Store, project: &str, limit: usize) -> rusqlite::Result<Vec<BenchRun>> {
    let conn = store.connection();
    conn.execute_batch(SCHEMA)?;
    let mut stmt = conn.prepare(
        "SELECT id, project, bench, sample, storage, head, started_ms, load, quiet, metrics
         FROM bench_runs WHERE project = ?1 ORDER BY id DESC LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(params![project, limit as i64], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<BenchRun> {
    let parse = |s: String| serde_json::from_str(&s).unwrap_or(Value::Null);
    Ok(BenchRun {
        id: r.get(0)?,
        project: r.get(1)?,
        bench: r.get(2)?,
        sample: r.get(3)?,
        storage: r.get(4)?,
        head: r.get(5)?,
        started_ms: r.get(6)?,
        load: parse(r.get(7)?),
        quiet: r.get(8)?,
        metrics: parse(r.get(9)?),
    })
}

/// A run against the previous quiet run of the same bench, sample and
/// storage. A run made on a loaded host is reported as unreliable instead
/// of being compared.
pub fn compare(store: &Store, run: i64) -> rusqlite::Result<Value> {
    let conn = store.connection();
    conn.execute_batch(SCHEMA)?;
    let select =
        "SELECT id, project, bench, sample, storage, head, started_ms, load, quiet, metrics
                  FROM bench_runs";
    let Some(this) = conn
        .query_row(&format!("{select} WHERE id = ?1"), params![run], row)
        .optional()?
    else {
        return Ok(json!({"error": format!("no bench run {run}")}));
    };
    if !this.quiet {
        return Ok(json!({"run": run, "reliable": false, "load": this.load,
            "note": "the host was loaded or its load was unknown when this run started; its timings are not compared"}));
    }
    let before = conn
        .query_row(
            &format!(
                "{select} WHERE project = ?1 AND bench = ?2 AND sample = ?3 AND storage = ?4
                 AND quiet = 1 AND id < ?5 ORDER BY id DESC LIMIT 1"
            ),
            params![this.project, this.bench, this.sample, this.storage, run],
            row,
        )
        .optional()?;
    let Some(before) = before else {
        return Ok(json!({"run": run, "reliable": true,
            "note": "no earlier quiet run on this sample and storage"}));
    };
    let mut changes = serde_json::Map::new();
    for (k, v) in flatten(&this.metrics) {
        if let Some(old) = flatten(&before.metrics).get(&k)
            && *old != 0.0
        {
            changes.insert(
                k,
                json!({"before": old, "after": v, "change": (v - old) / old}),
            );
        }
    }
    Ok(json!({"run": run, "against": before.id, "reliable": true,
              "head": this.head, "against_head": before.head, "changes": changes}))
}

fn flatten(v: &Value) -> BTreeMap<String, f64> {
    let mut out = BTreeMap::new();
    fn walk(prefix: &str, v: &Value, out: &mut BTreeMap<String, f64>) {
        match v {
            Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    out.insert(prefix.to_owned(), f);
                }
            }
            Value::Object(m) => {
                for (k, v) in m {
                    let key = if prefix.is_empty() {
                        k.clone()
                    } else {
                        format!("{prefix}.{k}")
                    };
                    walk(&key, v, out);
                }
            }
            _ => {}
        }
    }
    walk("", v, &mut out);
    out
}

/// Scales a sample run's time to the full corpus. The model is linear in
/// input bytes, per stage: every stage of these pipelines does work in
/// proportion to the bytes it is given, and the sample keeps each
/// stratum's share and size spread, so bytes are the right unit. `seconds`
/// is the sample run's wall time; `stages` its per-stage seconds.
pub fn predict(seconds: f64, stages: &BTreeMap<String, f64>, scale: f64) -> Value {
    json!({
        "scale": scale,
        "seconds": seconds * scale,
        "stages": stages.iter().map(|(k, v)| (k.clone(), json!(v * scale))).collect::<serde_json::Map<_, _>>(),
    })
}

#[cfg(test)]
mod tests;
