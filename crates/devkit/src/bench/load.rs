//! Host-load observations during a benchmark, summarized in constant memory.
use serde_json::{Value, json};
use std::{
    fs,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const PERIOD: Duration = Duration::from_secs(1);
const MAX_GAP_SECONDS: f64 = 2.0;

/// One host-load reading. It describes its observation, not an entire run.
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

    /// Whether this reading is below the comparison thresholds. Missing
    /// readings are unknown, never evidence of an idle host.
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

    pub fn to_json(self) -> Value {
        json!({"load1": self.load1, "cpus": self.cpus, "io_full": self.io_full,
               "memory_full": self.memory_full, "quiet": self.quiet()})
    }
}

/// Sampled load over a run. Even frequent samples leave unobserved gaps.
#[derive(Debug, Clone)]
pub struct LoadInterval {
    start: HostLoad,
    end: HostLoad,
    peak: HostLoad,
    samples: u64,
    unknown: u64,
    busy: u64,
    elapsed: Duration,
    max_gap: Duration,
}

impl LoadInterval {
    pub(super) fn new(reading: HostLoad) -> Self {
        let mut interval = Self {
            start: reading,
            end: reading,
            peak: reading,
            samples: 0,
            unknown: 0,
            busy: 0,
            elapsed: Duration::ZERO,
            max_gap: Duration::ZERO,
        };
        interval.observe(Duration::ZERO, reading);
        interval
    }

    pub(super) fn observe(&mut self, elapsed: Duration, reading: HostLoad) {
        self.max_gap = self.max_gap.max(elapsed.saturating_sub(self.elapsed));
        self.elapsed = elapsed;
        self.end = reading;
        self.samples += 1;
        let known = reading.cpus.is_some_and(|n| n > 0)
            && [reading.load1, reading.io_full, reading.memory_full]
                .iter()
                .all(|value| value.is_some_and(|v| v.is_finite() && v >= 0.0));
        if !known {
            self.unknown += 1;
        } else if !reading.quiet() {
            self.busy += 1;
        }
        fn maximum(a: Option<f64>, b: Option<f64>) -> Option<f64> {
            match (a, b) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            }
        }
        self.peak.load1 = maximum(self.peak.load1, reading.load1);
        self.peak.io_full = maximum(self.peak.io_full, reading.io_full);
        self.peak.memory_full = maximum(self.peak.memory_full, reading.memory_full);
    }

    /// All captured readings passed the thresholds and the sampler kept its
    /// cadence within one extra period. This does not prove a controlled run.
    pub fn checks_passed(&self) -> bool {
        self.samples >= 2
            && self.unknown == 0
            && self.busy == 0
            && self.max_gap.as_secs_f64() <= MAX_GAP_SECONDS
    }

    pub fn to_json(&self) -> Value {
        json!({
            "sampling": "periodic", "period_seconds": PERIOD.as_secs_f64(),
            "sample_count": self.samples, "elapsed_seconds": self.elapsed.as_secs_f64(),
            "max_gap_seconds": self.max_gap.as_secs_f64(),
            "maximum_gap_limit_seconds": MAX_GAP_SECONDS,
            "unknown_samples": self.unknown, "busy_samples": self.busy,
            "checks_passed": self.checks_passed(),
            "start": self.start.to_json(), "end": self.end.to_json(),
            "peak": {"load1": self.peak.load1, "io_full": self.peak.io_full,
                     "memory_full": self.peak.memory_full},
            "limitation": "Load between samples is unobserved; passing these checks does not establish timing reliability."
        })
    }
}

/// Old start-only rows remain readable, but cannot pass interval checks.
pub(super) fn checks_passed_json(value: &Value) -> bool {
    value["sampling"] == "periodic"
        && value["checks_passed"] == true
        && value["sample_count"].as_u64().is_some_and(|n| n >= 2)
        && value["unknown_samples"].as_u64() == Some(0)
        && value["busy_samples"].as_u64() == Some(0)
        && value["max_gap_seconds"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && (0.0..=MAX_GAP_SECONDS).contains(&v))
        && value["elapsed_seconds"]
            .as_f64()
            .is_some_and(|v| v.is_finite() && v >= 0.0)
}

/// One short-lived sampler owned by a benchmark. Finish it after the measured
/// operation; dropping it on an error also stops and joins its thread.
pub struct LoadMonitor {
    stop: Option<mpsc::SyncSender<()>>,
    worker: Option<thread::JoinHandle<LoadInterval>>,
}

impl LoadMonitor {
    pub fn start() -> Result<Self, String> {
        let started = Instant::now();
        let first = HostLoad::now();
        let (stop, receiver) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("bench-load".into())
            .spawn(move || {
                let mut interval = LoadInterval::new(first);
                loop {
                    let finished = !matches!(
                        receiver.recv_timeout(PERIOD),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    );
                    interval.observe(started.elapsed(), HostLoad::now());
                    if finished {
                        return interval;
                    }
                }
            })
            .map_err(|e| format!("start benchmark load sampler: {e}"))?;
        Ok(Self {
            stop: Some(stop),
            worker: Some(worker),
        })
    }

    pub fn finish(mut self) -> Result<LoadInterval, String> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.try_send(());
        }
        self.worker
            .take()
            .expect("load sampler exists")
            .join()
            .map_err(|_| "benchmark load sampler stopped unexpectedly".to_owned())
    }
}

impl Drop for LoadMonitor {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.try_send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;
