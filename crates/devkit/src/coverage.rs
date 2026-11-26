//! LLVM coverage with explicit scope and separate instrumentation output.
//! cargo llvm-cov computes the totals and applies the line threshold.
use crate::{Nextest, Run, cargo_command};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[derive(Debug, clap::Args)]
pub struct Coverage {
    #[arg(long)]
    pub output: PathBuf,
    #[arg(long, value_parser = ["html", "lcov", "json"], default_value = "json")]
    pub format: String,
    /// Fail when total line coverage is below this percentage; cargo
    /// llvm-cov's `--fail-under-lines` decides.
    #[arg(long)]
    pub threshold: Option<f64>,
    /// Package and test selection, including devkit's changed-file selection.
    #[arg(last = true)]
    pub args: Vec<String>,
}

pub fn execute(run: &mut Run, options: &Coverage, env: &[(String, String)]) -> Result<(), String> {
    execute_with(run, options, env, cargo_command)
}

fn execute_with(
    run: &mut Run,
    options: &Coverage,
    env: &[(String, String)],
    cargo: impl Fn() -> std::process::Command,
) -> Result<(), String> {
    if !["json", "html", "lcov"].contains(&options.format.as_str()) {
        return Err("unsupported coverage format".into());
    }
    if options
        .threshold
        .is_some_and(|t| !t.is_finite() || !(0.0..=100.0).contains(&t))
    {
        return Err("threshold must be between 0 and 100".into());
    }
    fs::create_dir_all(&options.output).map_err(|e| e.to_string())?;
    let output = fs::canonicalize(&options.output).map_err(|e| e.to_string())?;
    let summary = output.join("coverage.json");
    if summary.exists() {
        return Err("coverage output already used; choose a new directory".into());
    }
    // Reserving the actual instrumentation directory gives this invocation
    // exclusive output ownership before any compiler or report command runs.
    fs::create_dir(output.join("build")).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            "coverage output already used; choose a new directory".to_owned()
        } else {
            format!("coverage build directory: {error}")
        }
    })?;
    let mut env = env.to_vec();
    for name in [
        "CARGO_TARGET_DIR",
        "CARGO_LLVM_COV_TARGET_DIR",
        "CARGO_LLVM_COV_BUILD_DIR",
    ] {
        env.push((name.into(), output.join("build").display().to_string()));
    }
    let explicit_scope = options
        .args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| {
            arg == "-p"
                || arg == "--package"
                || arg.starts_with("--package=")
                || (arg.starts_with("-p") && arg.len() > 2)
                || arg == "--workspace"
        });
    let mut args = options.args.clone();
    if !explicit_scope {
        args.insert(0, "--workspace".into());
    }
    run.nextest_with(
        "coverage",
        &Nextest {
            args,
            env: env.clone(),
            ..Nextest::default()
        },
        &cargo,
        &["llvm-cov", "nextest", "--no-report"],
    )?;
    let mut command = cargo();
    command
        .args(["llvm-cov", "report", "--json", "--output-path"])
        .arg(&summary)
        .args(report_args(&options.args)?)
        .envs(env.iter().cloned());
    run.command("coverage JSON", &mut command)?;
    let report: Value =
        serde_json::from_slice(&fs::read(&summary).map_err(|e| format!("coverage report: {e}"))?)
            .map_err(|e| e.to_string())?;
    // llvm-cov's export carries its own totals; devkit only reports them.
    let percent = report["data"][0]["totals"]["lines"]["percent"]
        .as_f64()
        .ok_or("coverage report has no line total")?;
    if options.format != "json" {
        let mut command = cargo();
        command
            .args(["llvm-cov", "report"])
            .env("CARGO_TARGET_DIR", output.join("build"))
            .env("CARGO_LLVM_COV_TARGET_DIR", output.join("build"))
            .env("CARGO_LLVM_COV_BUILD_DIR", output.join("build"));
        let expected = if options.format == "html" {
            command.args(["--html", "--output-dir"]).arg(&output);
            output.join("html/index.html")
        } else {
            let file = output.join("coverage.lcov");
            command.args(["--lcov", "--output-path"]).arg(&file);
            file
        };
        command
            .args(report_args(&options.args)?)
            .envs(env.iter().cloned());
        run.command(
            &format!("coverage {}", options.format.to_ascii_uppercase()),
            &mut command,
        )?;
        if !expected.is_file() {
            return Err(format!("report missing: {}", expected.display()));
        }
    }
    let metadata = json!({"args": options.args, "line_percent": percent, "report": summary, "format": options.format, "regions": regions(&report)?});
    fs::write(
        output.join("summary.json"),
        serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    println!("{metadata}");
    if let Some(threshold) = options.threshold {
        let mut command = cargo();
        command
            .args(["llvm-cov", "report", "--summary-only", "--fail-under-lines"])
            .arg(threshold.to_string())
            .args(report_args(&options.args)?)
            .envs(env.iter().cloned());
        run.command("coverage threshold", &mut command)
            .map_err(|_| format!("line coverage {percent:.2}% below {threshold}%"))?;
    }
    Ok(())
}
#[cfg(test)]
mod tests;

fn report_args(args: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            break;
        }
        if matches!(
            arg.as_str(),
            "-p" | "--package" | "--exclude" | "--features" | "--target"
        ) {
            out.push(arg.clone());
            out.push(args.next().ok_or("selection value missing")?.clone());
        } else if matches!(
            arg.as_str(),
            "--workspace" | "--all-features" | "--no-default-features"
        ) || ["--package=", "--exclude=", "--features=", "--target="]
            .iter()
            .any(|p| arg.starts_with(p))
            || (arg.starts_with("-p") && arg.len() > 2)
        {
            out.push(arg.clone());
        }
    }
    Ok(out)
}

/// LLVM function regions use that function's file table, never the report's file order.
pub fn regions(report: &Value) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for data in report["data"].as_array().ok_or("coverage data missing")? {
        let functions = data["functions"]
            .as_array()
            .ok_or("LLVM function regions missing from coverage report")?;
        {
            for function in functions {
                for region in function["regions"]
                    .as_array()
                    .ok_or("function regions missing")?
                {
                    let id = region[5].as_u64().ok_or("region file id missing")? as usize;
                    let file = function["filenames"][id]
                        .as_str()
                        .ok_or("region file missing")?;
                    let number = |i: usize| {
                        region[i]
                            .as_u64()
                            .ok_or_else(|| "invalid region coordinates".to_string())
                    };
                    out.push(json!({"file":file,"start_line":number(0)?,"start_column":number(1)?,"end_line":number(2)?,"end_column":number(3)?,"count":number(4)?}));
                }
            }
        }
    }
    Ok(out)
}
