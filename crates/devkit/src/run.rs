use crate::{Result, junit};
use serde_json::Value;
use std::{
    env, fmt, fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{self, Command, ExitCode, Stdio},
    time::{Duration, Instant, SystemTime},
};

/// A step that did not succeed. The message ends up in the summary and in
/// the recorded run.
#[derive(Debug, Clone)]
pub struct Failed(pub String);

impl fmt::Display for Failed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Failed> for String {
    fn from(failed: Failed) -> Self {
        failed.0
    }
}

impl From<String> for Failed {
    fn from(message: String) -> Self {
        Self(message)
    }
}

impl From<&str> for Failed {
    fn from(message: &str) -> Self {
        Self(message.to_owned())
    }
}

/// One timed step of a run.
#[derive(Debug, Clone)]
pub struct Stage {
    pub name: String,
    pub duration: Duration,
    pub ok: bool,
}

/// A compiler or clippy message at warning level or above.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub package: String,
    pub level: String,
    pub code: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    /// The rendered message without terminal colours.
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TestStatus {
    Passed,
    Failed,
    /// Failed, then passed on a retry.
    Flaky,
    /// Excluded by nextest's run-ignored selection (also non-ignored tests
    /// when `--run-ignored only` is selected).
    Ignored,
    /// Excluded by a filter, default filter, or partition.
    Filtered,
    /// Another or unrecognized skip reason; the reason is kept.
    Skipped,
}

impl TestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Flaky => "flaky",
            Self::Ignored => "ignored",
            Self::Filtered => "filtered",
            Self::Skipped => "skipped",
        }
    }
}

/// One test's result. Output is kept only for tests that did not pass.
#[derive(Debug, Clone)]
pub struct TestOutcome {
    /// The nextest binary ID, such as `polylogue-store` or `sinexd::postgres`.
    pub binary: String,
    pub name: String,
    pub status: TestStatus,
    pub duration: Duration,
    pub output: Option<String>,
}

/// What a run tested: the commit, and a hash of every uncommitted change
/// (tracked diffs and untracked files) if there are any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree {
    pub head: Option<String>,
    pub dirty: Option<String>,
    /// The committed Git tree object, separate from uncommitted content.
    pub git_tree: Option<String>,
    /// Why the checkout could not be read; a missing dirty hash then says nothing.
    pub observation_error: Option<String>,
}

impl Tree {
    pub fn observe(root: &Path) -> Self {
        use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
        let git = |args: &[&str]| -> std::result::Result<Vec<u8>, String> {
            let output = Command::new("git")
                .args(args)
                .current_dir(root)
                .stderr(Stdio::null())
                .output()
                .map_err(|_| format!("git {} unavailable", args[0]))?;
            if !output.status.success() {
                return Err(format!("git {} failed", args[0]));
            }
            Ok(output.stdout)
        };
        let mut observed = Self {
            head: None,
            dirty: None,
            git_tree: None,
            observation_error: None,
        };
        let result = (|| -> std::result::Result<(), String> {
            observed.head = Some(
                String::from_utf8_lossy(&git(&["rev-parse", "HEAD"])?)
                    .trim()
                    .to_owned(),
            );
            let tree = String::from_utf8_lossy(&git(&["rev-parse", "HEAD^{tree}"])?)
                .trim()
                .to_owned();
            let diff = git(&["diff", "--no-ext-diff", "--no-textconv", "HEAD", "--binary"])?;
            let untracked = git(&["ls-files", "--others", "--exclude-standard", "-z"])?;
            if !diff.is_empty() || !untracked.is_empty() {
                let mut hasher = blake3::Hasher::new();
                hasher.update(&(diff.len() as u64).to_le_bytes());
                hasher.update(&diff);
                for name in untracked.split(|b| *b == 0).filter(|n| !n.is_empty()) {
                    hasher.update(&(name.len() as u64).to_le_bytes());
                    hasher.update(name);
                    let path = root.join(std::ffi::OsStr::from_bytes(name));
                    let metadata =
                        fs::symlink_metadata(&path).map_err(|_| "untracked metadata unreadable")?;
                    if metadata.file_type().is_symlink() {
                        hasher.update(b"symlink\0");
                        let target =
                            fs::read_link(&path).map_err(|_| "untracked symlink unreadable")?;
                        let bytes = target.as_os_str().as_bytes();
                        hasher.update(&(bytes.len() as u64).to_le_bytes());
                        hasher.update(bytes);
                    } else if metadata.is_file() {
                        hasher.update(b"file\0");
                        hasher.update(&(metadata.permissions().mode() & 0o111).to_le_bytes());
                        let bytes = fs::read(&path).map_err(|_| "untracked file unreadable")?;
                        hasher.update(&(bytes.len() as u64).to_le_bytes());
                        hasher.update(&bytes);
                    } else {
                        return Err("untracked entry is not a file or symlink".into());
                    }
                    hasher.update(&[0]);
                }
                observed.dirty = Some(hasher.finalize().to_hex().to_string());
            }
            // A commit made while reading the worktree makes the reading unusable.
            let head = String::from_utf8_lossy(&git(&["rev-parse", "HEAD"])?)
                .trim()
                .to_owned();
            if observed.head.as_deref() != Some(&head) {
                return Err("HEAD changed during observation".into());
            }
            observed.git_tree = Some(tree);
            Ok(())
        })();
        if let Err(error) = result {
            observed.git_tree = None;
            observed.observation_error = Some(error);
        }
        observed
    }
}

/// How [`Run::nextest`] runs the tests.
#[derive(Debug, Default, Clone)]
pub struct Nextest {
    /// Arguments after `cargo nextest run`: packages, filtersets, test names.
    pub args: Vec<String>,
    /// Put `TMPDIR` in job-owned scratch, or a standalone tmpfs child,
    /// removed afterwards. Give durability tests a real-filesystem directory in
    /// `TEST_DURABLE_TMPDIR`.
    pub tmpfs: bool,
    /// Extra environment for the build and the tests.
    pub env: Vec<(String, String)>,
    /// Succeed when the arguments select no tests. By default that fails,
    /// so a typo in a filter cannot pass silently.
    pub allow_no_tests: bool,
}

/// One xtask invocation: its steps, test outcomes and diagnostics.
pub struct Run {
    pub project: String,
    pub command: String,
    pub args: Vec<String>,
    pub root: PathBuf,
    pub started: SystemTime,
    pub tree: Tree,
    pub stages: Vec<Stage>,
    pub diagnostics: Vec<Diagnostic>,
    pub tests: Vec<TestOutcome>,
    pub failure: Option<String>,
    clock: Instant,
    target: Option<PathBuf>,
    test_reported: bool,
    #[cfg(feature = "history")]
    record: Option<(crate::history::Store, i64)>,
}

impl Run {
    /// Starts a run in the current directory's checkout and records it.
    pub fn start(project: &str, command: &str, args: &[String]) -> Self {
        #[allow(unused_mut)]
        let mut run = Self::unrecorded(project, command, args);
        #[cfg(feature = "history")]
        {
            run.record = match crate::history::Store::open() {
                Ok(store) => match store.begin(&run) {
                    Ok(id) => Some((store, id)),
                    Err(error) => {
                        eprintln!("devkit: not recording this run: {error}");
                        None
                    }
                },
                Err(error) => {
                    eprintln!("devkit: not recording this run: {error}");
                    None
                }
            };
        }
        run
    }

    /// A run that is not written to the history store.
    pub fn unrecorded(project: &str, command: &str, args: &[String]) -> Self {
        let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let root = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&cwd)
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
            .unwrap_or(cwd);
        let tree = Tree::observe(&root);
        Self {
            project: project.to_owned(),
            command: command.to_owned(),
            args: args.to_vec(),
            root,
            started: SystemTime::now(),
            tree,
            stages: Vec::new(),
            diagnostics: Vec::new(),
            tests: Vec::new(),
            failure: None,
            clock: Instant::now(),
            target: None,
            test_reported: false,
            #[cfg(feature = "history")]
            record: None,
        }
    }

    /// The recorded run's ID, if the history store is in use.
    pub fn id(&self) -> Option<i64> {
        #[cfg(feature = "history")]
        return self.record.as_ref().map(|(_, id)| *id);
        #[cfg(not(feature = "history"))]
        None
    }

    /// Runs one timed step. The first failing step's message becomes the
    /// run's failure.
    pub fn step(&mut self, name: &str, step: impl FnOnce(&mut Self) -> Result) -> Result {
        let began = Instant::now();
        let result = step(self);
        self.stages.push(Stage {
            name: name.to_owned(),
            duration: began.elapsed(),
            ok: result.is_ok(),
        });
        if let Err(failed) = &result {
            self.failure
                .get_or_insert_with(|| format!("{name}: {failed}"));
        }
        result
    }

    /// Runs a command as one step.
    pub fn command(&mut self, name: &str, command: &mut Command) -> Result {
        self.step(name, |run| {
            command.current_dir(&run.root);
            status(command)
        })
    }

    /// Runs a cargo subcommand that accepts `--message-format` (check, clippy,
    /// build, test, doc) as one step, printing compiler messages as usual and
    /// keeping warnings and errors as diagnostics.
    pub fn cargo(&mut self, name: &str, args: &[&str]) -> Result {
        self.cargo_with(name, args, &[])
    }

    pub fn cargo_with(&mut self, name: &str, args: &[&str], env: &[(String, String)]) -> Result {
        let (subcommand, rest) = args.split_first().ok_or("cargo needs a subcommand")?;
        let mut command = crate::cargo_command();
        command
            .arg(subcommand)
            .arg("--message-format=json-diagnostic-rendered-ansi")
            .args(rest)
            .envs(env.iter().cloned());
        self.step(name, |run| {
            command.current_dir(&run.root);
            run.capture_cargo_json(&mut command)
        })
    }

    /// Builds the tests (stage `<name> build`), then runs them under nextest
    /// (stage `<name>`), keeping each test's outcome. Every selected test runs:
    /// there is no fail-fast, and selecting no tests fails unless allowed.
    pub fn nextest(&mut self, name: &str, options: &Nextest) -> Result {
        let temp = if options.tmpfs {
            Some(RunTemp::new()?)
        } else {
            None
        };
        let mut env = options.env.clone();
        if let Some(temp) = &temp {
            let durable = self.target_dir()?.join("tmp");
            fs::create_dir_all(&durable).map_err(|e| format!("{}: {e}", durable.display()))?;
            env.push(("TMPDIR".into(), temp.path().display().to_string()));
            env.push(("TEST_DURABLE_TMPDIR".into(), durable.display().to_string()));
        }
        let args: Vec<&str> = options.args.iter().map(String::as_str).collect();

        let mut build = crate::cargo_command();
        build
            .args([
                "nextest",
                "run",
                "--no-run",
                "--cargo-message-format",
                "json-diagnostic-rendered-ansi",
            ])
            .args(&args)
            .envs(env.iter().cloned());
        self.step(&format!("{name} build"), |run| {
            build.current_dir(&run.root);
            run.capture_cargo_json(&mut build)
        })?;

        let target = self.target_dir()?;
        let junit = target
            .join("nextest")
            .join("default")
            .join("devkit-junit.xml");
        let _ = fs::remove_file(&junit);
        let config = target.join("devkit-nextest.toml");
        // Nextest's default store is workspace-relative, independent of
        // CARGO_TARGET_DIR. Give it the exact absolute report path we read.
        let report_path = serde_json::to_string(&junit).map_err(|e| format!("JUnit path: {e}"))?;
        fs::write(
            &config,
            format!("nextest-version = \"0.9.143\"\n[profile.default.junit]\npath = {report_path}\nstore-success-output = false\nstore-failure-output = true\nreport-skipped = \"all\"\n"),
        )
        .map_err(|e| format!("{}: {e}", config.display()))?;
        let mut test = crate::cargo_command();
        test.args(["nextest", "run", "--no-fail-fast"])
            .arg(if options.allow_no_tests {
                "--no-tests=pass"
            } else {
                "--no-tests=fail"
            })
            .arg("--tool-config-file")
            .arg(format!("devkit:{}", config.display()))
            .args(&args)
            .envs(env.iter().cloned());
        let result = self.step(name, |run| {
            test.current_dir(&run.root);
            status(&mut test)
        });
        let report = self.step(&format!("{name} report"), |run| {
            let xml = fs::read_to_string(&junit)
                .map_err(|error| format!("no test report at {}: {error}", junit.display()))?;
            let outcomes = junit::outcomes(&xml)
                .map_err(|error| format!("invalid test report at {}: {error}", junit.display()))?;
            run.tests.extend(outcomes);
            run.test_reported = true;
            Ok(())
        });
        drop(temp);
        result.and(report)
    }

    /// The workspace's cargo target directory.
    pub fn target_dir(&mut self) -> Result<PathBuf> {
        if let Some(target) = &self.target {
            return Ok(target.clone());
        }
        let output = crate::cargo_command()
            .args(["metadata", "--format-version", "1", "--no-deps"])
            .current_dir(&self.root)
            .output()
            .map_err(|e| format!("cargo metadata: {e}"))?;
        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
        }
        let metadata: Value =
            serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
        let target = metadata["target_directory"]
            .as_str()
            .map(PathBuf::from)
            .ok_or("cargo metadata has no target_directory")?;
        self.target = Some(target.clone());
        Ok(target)
    }

    fn capture_cargo_json(&mut self, command: &mut Command) -> Result {
        let mut child = command
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("{command:?}: {e}"))?;
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = std::io::stderr();
        for line in BufReader::new(stdout).lines() {
            let line = line.map_err(|e| e.to_string())?;
            match serde_json::from_str::<Value>(&line) {
                Ok(message) if message["reason"] == "compiler-message" => {
                    // The same message arrives once per target that compiles
                    // the file (the library, then its tests); show and keep it once.
                    let found = diagnostic(&message);
                    if found.as_ref().is_some_and(|d| self.diagnostics.contains(d)) {
                        continue;
                    }
                    let rendered = message["message"]["rendered"].as_str().unwrap_or_default();
                    let _ = stderr.write_all(rendered.as_bytes());
                    self.diagnostics.extend(found);
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = writeln!(stderr, "{line}");
                }
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("`{}` exited with {status}", describe(command)).into())
        }
    }

    /// Ends the run: checks that the checkout did not change underneath it,
    /// records it, prints a summary and returns the exit code.
    pub fn finish(mut self) -> ExitCode {
        if Tree::observe(&self.root) != self.tree {
            let reason = "checkout changed or could not be observed consistently during the run";
            self.tree.git_tree = None;
            self.tree.observation_error = Some(reason.into());
            self.failure.get_or_insert_with(|| reason.into());
        }
        let elapsed = self.clock.elapsed();
        let id = self.id();
        #[cfg(feature = "history")]
        if let Some((store, id)) = self.record.take()
            && let Err(error) = store.finish(id, &self, elapsed)
        {
            eprintln!("devkit: could not record this run: {error}");
        }
        eprintln!("{}", self.summary(elapsed, id));
        if self.failure.is_some() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        }
    }

    fn summary(&self, elapsed: Duration, id: Option<i64>) -> String {
        let count = |status| self.tests.iter().filter(|t| t.status == status).count();
        let errors = self
            .diagnostics
            .iter()
            .filter(|d| d.level == "error")
            .count();
        let warnings = self
            .diagnostics
            .iter()
            .filter(|d| d.level == "warning")
            .count();
        let stages: Vec<String> = self
            .stages
            .iter()
            .map(|s| format!("{} {}", s.name, seconds(s.duration)))
            .collect();
        let mut out = format!(
            "{} {}: {} in {} ({})",
            self.project,
            self.command,
            if self.failure.is_some() {
                "FAILED"
            } else {
                "passed"
            },
            seconds(elapsed),
            stages.join(", ")
        );
        if !self.tests.is_empty() {
            out += &format!(
                "\n  reported tests: {} passed, {} failed, {} flaky, {} skipped ({} by ignore selection, {} filtered)",
                count(TestStatus::Passed),
                count(TestStatus::Failed),
                count(TestStatus::Flaky),
                count(TestStatus::Skipped)
                    + count(TestStatus::Ignored)
                    + count(TestStatus::Filtered),
                count(TestStatus::Ignored),
                count(TestStatus::Filtered)
            );
        }
        if self.test_reported && self.tests.is_empty() {
            out += "\n  tests: no outcomes reported; unlisted binaries are not counted";
        }
        if errors + warnings > 0 {
            out += &format!("\n  diagnostics: {errors} errors, {warnings} warnings");
        }
        for test in self.tests.iter().filter(|t| t.status == TestStatus::Failed) {
            out += &format!("\n  failed: {} {}", test.binary, test.name);
        }
        if let Some(failure) = &self.failure {
            out += &format!("\n  {failure}");
        }
        match (id, self.failure.is_some()) {
            (Some(id), true) => {
                out += &format!("\n  recorded as run {id}; details: xtask history failures {id}")
            }
            (Some(id), false) => out += &format!("\n  recorded as run {id}"),
            (None, _) => {}
        }
        out
    }
}

fn seconds(duration: Duration) -> String {
    format!("{:.1}s", duration.as_secs_f64())
}

/// A command's program and leading subcommands, such as `cargo nextest run`.
fn describe(command: &Command) -> String {
    let mut words = vec![command.get_program().to_string_lossy().into_owned()];
    words.extend(
        command
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .take_while(|a| !a.starts_with('-')),
    );
    words.join(" ")
}

fn status(command: &mut Command) -> Result {
    let status = crate::process::Process::start(command)?.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{}` exited with {status}", describe(command)).into())
    }
}

fn diagnostic(message: &Value) -> Option<Diagnostic> {
    let inner = &message["message"];
    let level = inner["level"].as_str()?;
    if !matches!(level, "error" | "warning") {
        return None;
    }
    let primary = inner["spans"]
        .as_array()
        .and_then(|spans| spans.iter().find(|s| s["is_primary"] == true));
    Some(Diagnostic {
        package: package_name(message["package_id"].as_str().unwrap_or_default()),
        level: level.to_owned(),
        code: inner["code"]["code"].as_str().map(str::to_owned),
        file: primary
            .and_then(|s| s["file_name"].as_str())
            .map(str::to_owned),
        line: primary
            .and_then(|s| s["line_start"].as_u64())
            .map(|l| l as u32),
        text: strip_ansi(inner["rendered"].as_str().unwrap_or_default()),
    })
}

/// The package name in a cargo package ID: `path+file:///x/foo#0.1.0` is
/// `foo`, `path+file:///x/crates/model#polylogue-model@0.1.0` is
/// `polylogue-model`.
fn package_name(id: &str) -> String {
    match id.rsplit_once('#') {
        Some((_, fragment)) if fragment.contains('@') => {
            fragment.split('@').next().unwrap_or(fragment).to_owned()
        }
        Some((path, _)) => path.rsplit('/').next().unwrap_or(path).to_owned(),
        None => id.to_owned(),
    }
}

pub(crate) fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Owns only its exclusive child, never the inherited job scratch parent.
/// AgentCTL can remove that child after native cancellation without Rust Drop.
struct RunTemp {
    directory: PathBuf,
    visible: PathBuf,
    // Linux socket paths can use this short directory view while the run lives.
    _socket_root: Option<fs::File>,
}

impl RunTemp {
    fn new() -> Result<Self> {
        Self::with_roots(
            &env::temp_dir(),
            env::var_os("AGENTCTL_SCRATCH").as_deref().map(Path::new),
        )
    }

    fn with_roots(inherited: &Path, job: Option<&Path>) -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = if let Some(job) = job {
            let metadata = fs::symlink_metadata(job)
                .map_err(|error| format!("job scratch {}: {error}", job.display()))?;
            if !job.is_absolute() || !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("AGENTCTL_SCRATCH must name an existing absolute directory".into());
            }
            job
        } else if Path::new("/dev/shm").is_dir() {
            Path::new("/dev/shm")
        } else {
            inherited
        };
        loop {
            let path = base.join(format!(
                "dk-{}-{}",
                process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => {
                    let mut temp = Self {
                        directory: path.clone(),
                        visible: path,
                        _socket_root: None,
                    };
                    #[cfg(target_os = "linux")]
                    if temp.visible.as_os_str().len() > 40 {
                        use std::os::fd::AsRawFd;
                        let directory = fs::File::open(&temp.directory).map_err(|error| {
                            format!("test scratch {}: {error}", temp.directory.display())
                        })?;
                        let visible = PathBuf::from(format!(
                            "/proc/{}/fd/{}",
                            process::id(),
                            directory.as_raw_fd()
                        ));
                        if !visible.is_dir() {
                            return Err(
                                "short test socket path is unavailable through /proc".into()
                            );
                        }
                        temp.visible = visible;
                        temp._socket_root = Some(directory);
                    }
                    return Ok(temp);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("{}: {error}", path.display()).into()),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.visible
    }
}

impl Drop for RunTemp {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.directory) {
            eprintln!("devkit: {}: {error}", self.directory.display());
        }
    }
}

#[cfg(test)]
mod temp_tests;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod nextest_tests;

#[cfg(test)]
mod tree_tests;
