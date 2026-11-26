//! One measured process, run under the caller's memory limit.
//!
//! Where a systemd user manager answers, the process runs as a transient
//! service of its own (`systemd-run --user`) with `MemoryMax` (and no swap)
//! and memory, CPU and I/O accounting; peak memory, CPU time, bytes read and
//! written and the run's wall time are read from the unit after the process
//! exits. The unit is placed in the slice of the unit that started it, bound
//! to it and given its CPU and I/O weights and the caller's niceness, so a
//! cancelled job stops its measured process, the process's memory counts in
//! the same budget, and it yields as the job does. A job runner's own unit
//! cannot stand in for this one: it also holds the build, the sampling and
//! every other repetition.
//!
//! Without a user manager (a hosted runner, a cloud session) the process
//! runs directly, its wall time is measured from here, and the accounting
//! says that no limit was applied and why.
use super::{LoadInterval, LoadMonitor};
use serde_json::{Value, json};
use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// The command a measurement runs. It sees the caller's environment with
/// `env` laid over it, in `dir` (default: the caller's directory).
#[derive(Debug, Clone, Default)]
pub struct Process {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(String, OsString)>,
    pub dir: Option<PathBuf>,
}

impl Process {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(mut self, args: I) -> Self {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub fn env(mut self, key: &str, value: impl AsRef<OsStr>) -> Self {
        self.env.push((key.to_owned(), value.as_ref().to_owned()));
        self
    }
}

/// The experimental conditions a measurement runs under.
#[derive(Debug, Clone, Default)]
pub struct Limit {
    /// systemd's `MemoryMax` syntax (`4G`, `512M`); `None` runs without one.
    pub memory_max: Option<String>,
}

impl Limit {
    pub fn memory(max: &str) -> Self {
        Self {
            memory_max: Some(max.to_owned()),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"memory_max": self.memory_max})
    }
}

/// What the unit, or the direct run, says the process cost.
#[derive(Debug, Clone, PartialEq)]
pub struct Accounting {
    /// The transient unit that ran the process; `None` for a direct run.
    pub unit: Option<String>,
    /// The limit in force for this run; `None` when none was applied.
    pub memory_max: Option<String>,
    /// Why the run had no unit.
    pub not_applied: Option<String>,
    pub wall_seconds: Option<f64>,
    pub memory_peak_bytes: Option<u64>,
    pub cpu_seconds: Option<f64>,
    /// Block-device bytes the kernel attributed to the unit.
    pub read_bytes: Option<u64>,
    pub written_bytes: Option<u64>,
}

impl Accounting {
    pub fn to_json(&self) -> Value {
        json!({
            "unit": self.unit, "memory_max": self.memory_max,
            "not_applied": self.not_applied, "wall_seconds": self.wall_seconds,
            "memory_peak_bytes": self.memory_peak_bytes, "cpu_seconds": self.cpu_seconds,
            "read_bytes": self.read_bytes, "written_bytes": self.written_bytes,
        })
    }
}

/// One finished measured process.
#[derive(Debug, Clone)]
pub struct Measured {
    /// `success`, or what ended it: systemd's result (`exit-code`,
    /// `signal`, `oom-kill`…) or, for a direct run, `exit-code` or `signal`.
    pub outcome: String,
    pub exit_status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub accounting: Accounting,
    pub load: LoadInterval,
}

impl Measured {
    pub fn succeeded(&self) -> bool {
        self.outcome == "success"
    }

    /// The outcome, accounting and load, without the process's output.
    pub fn to_json(&self) -> Value {
        json!({"outcome": self.outcome, "exit_status": self.exit_status,
               "accounting": self.accounting.to_json(), "load": self.load.to_json()})
    }

    /// The last line of the process's standard error, for a failure message.
    pub fn last_error_line(&self) -> String {
        String::from_utf8_lossy(&self.stderr)
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or_default()
            .to_owned()
    }
}

/// Runs `process` under `limit` and waits for it. An error means the
/// measurement could not be made; a process that failed or was killed at
/// its limit is a [`Measured`] with that outcome.
pub fn measure(process: &Process, limit: &Limit) -> Result<Measured, String> {
    match user_manager() {
        Ok(()) => in_unit(process, limit),
        Err(reason) => direct(process, reason),
    }
}

fn systemctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    } else {
        Err(format!(
            "systemctl --user {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

fn user_manager() -> Result<(), String> {
    systemctl(&["show-environment"])
        .map(drop)
        .map_err(|e| format!("no systemd user manager: {e}"))
}

/// The unit this process runs in, when the user manager knows it: its
/// name and the properties the measured unit takes from it (its slice and
/// its CPU and I/O weights, so a measurement yields as its job does).
fn enclosing() -> Option<(String, Vec<String>)> {
    let unit = systemctl(&["whoami"]).ok()?;
    let shown = systemctl(&["show", &unit, "-p", "Slice,CPUWeight,IOWeight"]).ok()?;
    let properties = parse_properties(&shown);
    let slice = properties.get("Slice").filter(|s| !s.is_empty())?;
    let mut taken = vec![format!("Slice={slice}"), format!("BindsTo={unit}")];
    for weight in ["CPUWeight", "IOWeight"] {
        if let Some(value) = properties.get(weight).and_then(|v| unit_number(v)) {
            taken.push(format!("{weight}={value}"));
        }
    }
    Some((unit, taken))
}

/// systemd accepts environment names of letters, digits and underscores,
/// and values of valid UTF-8 without control characters other than tab
/// and newline; anything else in the caller's environment stays out.
fn passable(key: &str, value: &OsStr) -> Option<String> {
    let name_ok = !key.is_empty()
        && !key.starts_with(|c: char| c.is_ascii_digit())
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let value = value.to_str()?;
    (name_ok
        && value
            .chars()
            .all(|c| !c.is_control() || c == '\t' || c == '\n'))
    .then(|| format!("{key}={value}"))
}

static UNITS: AtomicU64 = AtomicU64::new(0);

/// The scratch directory a unit's output is written to, removed on drop.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn in_unit(process: &Process, limit: &Limit) -> Result<Measured, String> {
    let serial = UNITS.fetch_add(1, Ordering::Relaxed);
    let name = format!("devkit-measure-{}-{serial}.service", std::process::id());
    let scratch =
        Scratch(env::temp_dir().join(format!("devkit-measure-{}-{serial}", std::process::id())));
    let _ = fs::remove_dir_all(&scratch.0);
    fs::create_dir_all(&scratch.0).map_err(|e| format!("{}: {e}", scratch.0.display()))?;
    let stdout = scratch.0.join("stdout");
    let stderr = scratch.0.join("stderr");
    let dir = match &process.dir {
        Some(dir) => dir.clone(),
        None => env::current_dir().map_err(|e| format!("the current directory: {e}"))?,
    };
    let mut command = Command::new("systemd-run");
    command
        .args(["--user", "--quiet", "--service-type=exec"])
        .arg(format!("--unit={name}"))
        .arg("--working-directory")
        .arg(&dir)
        // The unit stays loaded after its process exits, so its accounting
        // can be read; it is stopped below.
        .args(["-p", "RemainAfterExit=yes"])
        .args(["-p", "MemoryAccounting=yes"])
        .args(["-p", "CPUAccounting=yes"])
        .args(["-p", "IOAccounting=yes"])
        .arg("-p")
        .arg(format!("StandardOutput=truncate:{}", stdout.display()))
        .arg("-p")
        .arg(format!("StandardError=truncate:{}", stderr.display()));
    if let Some(max) = &limit.memory_max {
        command
            .arg("-p")
            .arg(format!("MemoryMax={max}"))
            .args(["-p", "MemorySwapMax=0"]);
    }
    if let Some((_, properties)) = enclosing() {
        for property in properties {
            command.arg("-p").arg(property);
        }
    }
    // The caller's niceness, which a unit started by the manager would not
    // inherit.
    // SAFETY: getpriority reads the calling process's own priority.
    let nice = unsafe { libc::getpriority(libc::PRIO_PROCESS, 0) };
    if nice != 0 {
        command.arg("-p").arg(format!("Nice={nice}"));
    }
    let mut environment: Vec<String> = env::vars_os()
        .filter(|(k, _)| !process.env.iter().any(|(o, _)| OsStr::new(o) == k))
        .filter_map(|(k, v)| passable(k.to_str()?, &v))
        .collect();
    environment.extend(process.env.iter().filter_map(|(k, v)| passable(k, v)));
    for assignment in environment {
        command.arg(format!("--setenv={assignment}"));
    }
    command.arg("--").arg(program(process)).args(&process.args);
    let load = LoadMonitor::start()?;
    let started = command.output().map_err(|e| format!("systemd-run: {e}"))?;
    if !started.status.success() {
        let _ = systemctl(&["reset-failed", &name]);
        return Err(format!(
            "systemd-run could not start {}: {}",
            process.program.display(),
            String::from_utf8_lossy(&started.stderr).trim()
        ));
    }
    let properties = loop {
        let shown = systemctl(&[
            "show",
            &name,
            "-p",
            "ActiveState,SubState,Result,ExecMainCode,ExecMainStatus,MemoryPeak,\
             CPUUsageNSec,IOReadBytes,IOWriteBytes,ExecMainStartTimestampMonotonic,\
             ExecMainExitTimestampMonotonic",
        ]);
        let properties = match shown {
            Ok(text) => parse_properties(&text),
            Err(e) => {
                let _ = systemctl(&["stop", &name]);
                return Err(e);
            }
        };
        let active = properties.get("ActiveState").map(String::as_str);
        let sub = properties.get("SubState").map(String::as_str);
        if matches!(active, Some("failed" | "inactive")) || sub == Some("exited") {
            break properties;
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let load = load.finish()?;
    let _ = systemctl(&["stop", &name]);
    let _ = systemctl(&["reset-failed", &name]);
    let number = |key: &str| properties.get(key).and_then(|v| unit_number(v));
    let wall_seconds = number("ExecMainStartTimestampMonotonic")
        .zip(number("ExecMainExitTimestampMonotonic"))
        .and_then(|(start, end)| end.checked_sub(start))
        .map(|us| us as f64 / 1e6);
    // ExecMainCode 1 is a normal exit, whose status is the exit code.
    let exit_status = (number("ExecMainCode") == Some(1))
        .then(|| number("ExecMainStatus").map(|s| s as i32))
        .flatten();
    Ok(Measured {
        outcome: properties
            .get("Result")
            .cloned()
            .unwrap_or_else(|| "unknown".into()),
        exit_status,
        stdout: fs::read(&stdout).unwrap_or_default(),
        stderr: fs::read(&stderr).unwrap_or_default(),
        accounting: Accounting {
            unit: Some(name),
            memory_max: limit.memory_max.clone(),
            not_applied: None,
            wall_seconds,
            memory_peak_bytes: number("MemoryPeak"),
            cpu_seconds: number("CPUUsageNSec").map(|ns| ns as f64 / 1e9),
            read_bytes: number("IOReadBytes"),
            written_bytes: number("IOWriteBytes"),
        },
        load,
    })
}

/// The program's path: a bare name is looked up in the caller's `PATH`,
/// as a direct run would.
fn program(process: &Process) -> PathBuf {
    let bare = process.program.components().count() == 1 && !process.program.is_absolute();
    let search = process
        .env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.clone())
        .or_else(|| env::var_os("PATH"));
    match search {
        Some(paths) if bare => env::split_paths(&paths)
            .map(|dir| dir.join(&process.program))
            .find(|candidate| candidate.is_file())
            .unwrap_or_else(|| process.program.clone()),
        _ => process.program.clone(),
    }
}

fn parse_properties(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .collect()
}

/// A numeric unit property; systemd shows an unknown one as `[not set]` or
/// as the largest 64-bit value.
fn unit_number(value: &str) -> Option<u64> {
    value.parse::<u64>().ok().filter(|n| *n != u64::MAX)
}

fn direct(process: &Process, reason: String) -> Result<Measured, String> {
    let mut command = Command::new(&process.program);
    command
        .args(&process.args)
        .envs(process.env.iter().map(|(k, v)| (k, v)));
    if let Some(dir) = &process.dir {
        command.current_dir(dir);
    }
    let load = LoadMonitor::start()?;
    let began = Instant::now();
    let output = command
        .output()
        .map_err(|e| format!("{}: {e}", process.program.display()))?;
    let wall = began.elapsed().as_secs_f64();
    let load = load.finish()?;
    let outcome = if output.status.success() {
        "success"
    } else if output.status.code().is_some() {
        "exit-code"
    } else {
        "signal"
    };
    Ok(Measured {
        outcome: outcome.into(),
        exit_status: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        accounting: Accounting {
            unit: None,
            memory_max: None,
            not_applied: Some(reason),
            wall_seconds: Some(wall),
            memory_peak_bytes: None,
            cpu_seconds: None,
            read_bytes: None,
            written_bytes: None,
        },
        load,
    })
}
