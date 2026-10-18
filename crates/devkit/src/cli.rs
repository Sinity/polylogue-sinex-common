//! Commands every project's xtask includes: `status`, `history …` and
//! `changed`. A project flattens [`Command`] into its own clap enum and
//! passes it to [`execute`].
use crate::{
    history::{RunRow, Store},
    select,
};
use serde_json::json;
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command as Process, ExitCode},
};

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Failing tests and compiler errors from this checkout's last run.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Read the recorded runs.
    #[command(subcommand)]
    History(History),
    /// The nextest filterset for packages changed since the merge base.
    Changed {
        #[arg(long, default_value = "origin/master")]
        base: String,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum History {
    /// Recent runs of this checkout, newest first.
    Runs {
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Failed and flaky tests of a run, with their output (default: this
    /// checkout's last run).
    Failures {
        run: Option<i64>,
        #[arg(long)]
        json: bool,
    },
    /// Compiler diagnostics of a run (default: this checkout's last run).
    Diagnostics {
        run: Option<i64>,
        /// `error`, or `warning` for warnings and errors.
        #[arg(long, default_value = "warning")]
        level: String,
        #[arg(long)]
        json: bool,
    },
}

pub fn execute(project: &str, command: Command) -> ExitCode {
    let root = checkout_root();
    let result = match command {
        Command::Changed { base } => {
            select::changed(&root, &base).map(|selection| match selection.filterset() {
                Some(filter) => println!("{filter}"),
                None => println!("all()"),
            })
        }
        Command::Status { json } => with_store(|store| status(store, project, &root, json)),
        Command::History(History::Runs { limit, json }) => with_store(|store| {
            let runs = store
                .runs(project, &root.display().to_string(), limit)
                .map_err(|e| e.to_string())?;
            if json {
                println!("{}", json!(runs.iter().map(run_json).collect::<Vec<_>>()));
            } else {
                for run in &runs {
                    println!("{}", run_line(run));
                }
            }
            Ok(())
        }),
        Command::History(History::Failures { run, json }) => with_store(|store| {
            let Some(run) = resolve(store, project, &root, run)? else {
                return Ok(());
            };
            let failures = store.failures(run.id).map_err(|e| e.to_string())?;
            if json {
                let rows: Vec<_> = failures
                    .iter()
                    .map(|t| json!({"binary": t.binary, "name": t.name, "status": t.status, "duration_ms": t.duration_ms, "output": t.output}))
                    .collect();
                println!("{}", json!({"run": run_json(&run), "tests": rows}));
                return Ok(());
            }
            println!("{}", run_line(&run));
            if failures.is_empty() {
                println!("no failed or flaky tests recorded");
            }
            for test in &failures {
                println!(
                    "\n== {} {} {} ({} ms)",
                    test.status, test.binary, test.name, test.duration_ms
                );
                if let Some(output) = &test.output {
                    println!("{}", output.trim_end());
                }
            }
            Ok(())
        }),
        Command::History(History::Diagnostics { run, level, json }) => with_store(|store| {
            let Some(run) = resolve(store, project, &root, run)? else {
                return Ok(());
            };
            let rows = store
                .diagnostics(run.id, &level)
                .map_err(|e| e.to_string())?;
            if json {
                let rows: Vec<_> = rows
                    .iter()
                    .map(|d| json!({"package": d.package, "level": d.level, "code": d.code, "file": d.file, "line": d.line, "text": d.text}))
                    .collect();
                println!("{}", json!({"run": run_json(&run), "diagnostics": rows}));
                return Ok(());
            }
            println!("{}", run_line(&run));
            if rows.is_empty() {
                println!("no diagnostics at {level} level");
            }
            for d in &rows {
                println!("\n{}", d.text.trim_end());
            }
            Ok(())
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn status(store: &Store, project: &str, root: &Path, json: bool) -> Result<(), String> {
    let checkout = root.display().to_string();
    let Some(run) = store
        .last_run(project, &checkout)
        .map_err(|e| e.to_string())?
    else {
        println!("no recorded runs for {checkout}");
        return Ok(());
    };
    let counts = store.test_counts(run.id).map_err(|e| e.to_string())?;
    let failures = store.failures(run.id).map_err(|e| e.to_string())?;
    let errors = store
        .diagnostics(run.id, "error")
        .map_err(|e| e.to_string())?;
    if json {
        println!(
            "{}",
            json!({
                "run": run_json(&run),
                "tests": counts.iter().map(|(s, n)| (s.clone(), json!(n))).collect::<serde_json::Map<_, _>>(),
                "failed_tests": failures.iter().map(|t| format!("{} {}", t.binary, t.name)).collect::<Vec<_>>(),
                "errors": errors.iter().map(|d| json!({"package": d.package, "file": d.file, "line": d.line, "code": d.code})).collect::<Vec<_>>(),
            })
        );
        return Ok(());
    }
    println!("{}", run_line(&run));
    if !counts.is_empty() {
        let parts: Vec<String> = counts.iter().map(|(s, n)| format!("{n} {s}")).collect();
        println!("  tests: {}", parts.join(", "));
    }
    for test in &failures {
        println!("  {} {} {}", test.status, test.binary, test.name);
    }
    for d in &errors {
        let at = match (&d.file, d.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => String::new(),
        };
        let first = d.text.lines().next().unwrap_or_default();
        println!("  error {} {at}: {first}", d.package);
    }
    if !failures.is_empty() {
        println!("  output: xtask history failures {}", run.id);
    }
    if !errors.is_empty() {
        println!(
            "  messages: xtask history diagnostics {} --level error",
            run.id
        );
    }
    Ok(())
}

fn resolve(
    store: &Store,
    project: &str,
    root: &Path,
    run: Option<i64>,
) -> Result<Option<RunRow>, String> {
    let row = match run {
        Some(id) => store.run(id),
        None => store.last_run(project, &root.display().to_string()),
    }
    .map_err(|e| e.to_string())?;
    if row.is_none() {
        println!("no such run");
    }
    Ok(row)
}

fn run_line(run: &RunRow) -> String {
    let duration = run.duration_ms.map_or(String::new(), |ms| {
        format!(" in {:.1}s", ms as f64 / 1000.0)
    });
    let head = run.head.as_deref().map_or("no commit".into(), |h| {
        h.chars().take(10).collect::<String>()
    });
    let dirty = if run.dirty { "+changes" } else { "" };
    let mut line = format!(
        "run {}: {} {} at {head}{dirty}{duration}",
        run.id, run.command, run.status
    );
    if let Some(failure) = &run.failure {
        line += &format!(" ({failure})");
    }
    line
}

fn run_json(run: &RunRow) -> serde_json::Value {
    json!({
        "id": run.id, "project": run.project, "checkout": run.checkout, "command": run.command,
        "args": run.args, "head": run.head, "dirty": run.dirty, "started_ms": run.started_ms,
        "duration_ms": run.duration_ms, "status": run.status, "failure": run.failure,
    })
}

fn with_store(f: impl FnOnce(&Store) -> Result<(), String>) -> Result<(), String> {
    let store = Store::open().map_err(|e| format!("{}: {e}", Store::path().display()))?;
    f(&store)
}

fn checkout_root() -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    Process::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
        .unwrap_or(cwd)
}
