//! Builds measured in turn and compared per figure.
//!
//! Each repetition gives named figures. A figure is work (CPU-seconds,
//! bytes written per input byte, syncs, peak memory: what the build did,
//! which another host would see alike) or time (wall seconds, which depend
//! on the host and its load). Work is listed before time. Two builds are
//! compared by each figure's range over their repetitions: when the ranges
//! overlap the figure shows no difference; otherwise the comparison gives
//! the ratio of the second build's median to the first's, with the range
//! that ratio can take between the two ranges.
use super::{Limit, Measured};
use serde_json::{Value, json};

/// What a figure measures, which decides where it is listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// What the build did, independent of the host's speed and load.
    Work,
    /// Elapsed time on this host.
    Time,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Work => "work",
            Kind::Time => "time",
        }
    }
}

/// One named reading of a repetition; `None` when it could not be taken.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    pub name: String,
    pub kind: Kind,
    pub value: Option<f64>,
}

impl Figure {
    pub fn work(name: impl Into<String>, value: Option<f64>) -> Self {
        Self {
            name: name.into(),
            kind: Kind::Work,
            value: value.filter(|v| v.is_finite()),
        }
    }

    pub fn time(name: impl Into<String>, value: Option<f64>) -> Self {
        Self {
            name: name.into(),
            kind: Kind::Time,
            value: value.filter(|v| v.is_finite()),
        }
    }

    /// The figures a measured process's unit gives: CPU-seconds, peak
    /// memory, bytes read and written, and wall seconds.
    pub fn of_unit(measured: &Measured) -> Vec<Figure> {
        let a = &measured.accounting;
        vec![
            Figure::work("cpu_seconds", a.cpu_seconds),
            Figure::work("memory_peak_bytes", a.memory_peak_bytes.map(|n| n as f64)),
            Figure::work("read_bytes", a.read_bytes.map(|n| n as f64)),
            Figure::work("written_bytes", a.written_bytes.map(|n| n as f64)),
            Figure::time("wall_seconds", a.wall_seconds),
        ]
    }
}

/// One build being measured: its label and what identifies it (a commit, a
/// binary's hash).
#[derive(Debug, Clone)]
pub struct Arm {
    pub label: String,
    pub build: Value,
}

/// One measured run of one build.
#[derive(Debug, Clone)]
pub struct Repetition {
    /// The index of its build among the arms.
    pub arm: usize,
    pub round: u32,
    pub figures: Vec<Figure>,
    /// Everything else kept about the run: its unit accounting and host
    /// load, and the product's own result.
    pub record: Value,
}

/// Runs every build once per round, in order, for `rounds` rounds (A B A B
/// …), so drifting host conditions fall on all builds alike. `run` receives
/// the arm's index and the round.
pub fn interleave(
    arms: usize,
    rounds: u32,
    mut run: impl FnMut(usize, u32) -> Result<Repetition, String>,
) -> Result<Vec<Repetition>, String> {
    let mut repetitions = Vec::with_capacity(arms * rounds as usize);
    for round in 0..rounds {
        for arm in 0..arms {
            repetitions.push(run(arm, round)?);
        }
    }
    Ok(repetitions)
}

/// The figures' names and kinds, work first, in first-seen order.
fn names(repetitions: &[Repetition]) -> Vec<(String, Kind)> {
    let mut seen: Vec<(String, Kind)> = Vec::new();
    for figure in repetitions.iter().flat_map(|r| &r.figures) {
        if !seen.iter().any(|(name, _)| *name == figure.name) {
            seen.push((figure.name.clone(), figure.kind));
        }
    }
    // A stable sort keeps first-seen order within each kind.
    seen.sort_by_key(|(_, kind)| *kind);
    seen
}

fn values(repetitions: &[Repetition], arm: usize, name: &str) -> Vec<Option<f64>> {
    repetitions
        .iter()
        .filter(|r| r.arm == arm)
        .map(|r| {
            r.figures
                .iter()
                .find(|f| f.name == name)
                .and_then(|f| f.value)
        })
        .collect()
}

struct Range {
    median: f64,
    min: f64,
    max: f64,
}

/// The median and range of a figure's values; `None` when any is unknown,
/// since a range over the known ones alone would be narrower than the truth.
fn range(values: &[Option<f64>]) -> Option<Range> {
    let mut known: Vec<f64> = values.iter().copied().collect::<Option<Vec<_>>>()?;
    if known.is_empty() {
        return None;
    }
    known.sort_by(f64::total_cmp);
    let n = known.len();
    let median = if n % 2 == 1 {
        known[n / 2]
    } else {
        (known[n / 2 - 1] + known[n / 2]) / 2.0
    };
    Some(Range {
        median,
        min: known[0],
        max: known[n - 1],
    })
}

fn range_json(values: &[Option<f64>]) -> Value {
    match range(values) {
        Some(r) => json!({"median": r.median, "min": r.min, "max": r.max, "values": values}),
        None => json!({"median": null, "values": values,
                       "unknown": values.iter().filter(|v| v.is_none()).count()}),
    }
}

/// Each figure's median and range for one build.
pub fn summarize(repetitions: &[Repetition], arm: usize) -> Vec<Value> {
    names(repetitions)
        .into_iter()
        .map(|(name, kind)| {
            let mut entry = range_json(&values(repetitions, arm, &name));
            entry["figure"] = json!(name);
            entry["kind"] = json!(kind.name());
            entry
        })
        .collect()
}

/// Every figure of build `b` against build `a`.
pub fn compare(repetitions: &[Repetition], a: usize, b: usize) -> Vec<Value> {
    names(repetitions)
        .into_iter()
        .map(|(name, kind)| {
            let va = values(repetitions, a, &name);
            let vb = values(repetitions, b, &name);
            let (verdict, ratio, ratio_range) = match (range(&va), range(&vb)) {
                (Some(ra), Some(rb)) => {
                    let ratio = |x: f64, y: f64| (y != 0.0).then(|| x / y);
                    let overlap = rb.min <= ra.max && ra.min <= rb.max;
                    let median_ratio = ratio(rb.median, ra.median);
                    let low = ratio(rb.min, ra.max);
                    let high = ratio(rb.max, ra.min);
                    let verdict = if overlap {
                        "no difference".to_owned()
                    } else {
                        match (median_ratio, low, high) {
                            (Some(m), Some(l), Some(h)) => format!("{m:.3}x ({l:.3} to {h:.3})"),
                            _ => format!("differs: {} against {}", rb.median, ra.median),
                        }
                    };
                    (verdict, json!(median_ratio), json!([low, high]))
                }
                _ => ("unknown".to_owned(), Value::Null, Value::Null),
            };
            json!({"figure": name, "kind": kind.name(), "verdict": verdict,
                   "ratio": ratio, "ratio_range": ratio_range,
                   "a": range_json(&va), "b": range_json(&vb)})
        })
        .collect()
}

/// The result document of a measurement: the sample, the limit, the
/// builds, every repetition and, for two builds, their comparison (the
/// second against the first). `headline` holds one line per figure.
pub fn report(sample: Value, limit: &Limit, arms: &[Arm], repetitions: &[Repetition]) -> Value {
    let repetition_json: Vec<Value> = repetitions
        .iter()
        .map(|r| {
            let figures: serde_json::Map<String, Value> = r
                .figures
                .iter()
                .map(|f| (f.name.clone(), json!(f.value)))
                .collect();
            json!({"build": arms.get(r.arm).map(|a| &a.label), "round": r.round,
                   "figures": figures, "record": r.record})
        })
        .collect();
    let mut result = json!({
        "sample": sample,
        "limit": limit.to_json(),
        "builds": arms.iter().map(|a| json!({"label": a.label, "build": a.build})).collect::<Vec<_>>(),
        "repetitions": repetition_json,
    });
    if arms.len() == 2 {
        let comparison = compare(repetitions, 0, 1);
        result["headline"] = json!(
            comparison
                .iter()
                .map(|c| format!(
                    "{} ({}): {}",
                    c["figure"].as_str().unwrap_or_default(),
                    c["kind"].as_str().unwrap_or_default(),
                    c["verdict"].as_str().unwrap_or_default()
                ))
                .collect::<Vec<_>>()
        );
        result["comparison"] = json!(comparison);
    } else {
        let builds: Vec<Value> = (0..arms.len())
            .map(|arm| json!({"build": arms[arm].label, "figures": summarize(repetitions, arm)}))
            .collect();
        result["headline"] = json!(
            builds
                .iter()
                .flat_map(|b| b["figures"].as_array().cloned().unwrap_or_default())
                .map(|f| match f["median"].as_f64() {
                    Some(m) => format!(
                        "{}: {m} ({} to {})",
                        f["figure"].as_str().unwrap_or_default(),
                        f["min"],
                        f["max"]
                    ),
                    None => format!("{}: unknown", f["figure"].as_str().unwrap_or_default()),
                })
                .collect::<Vec<_>>()
        );
        result["summary"] = json!(builds);
    }
    result
}
