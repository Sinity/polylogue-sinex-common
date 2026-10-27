//! LLVM coverage with explicit scope and separate instrumentation output.
use crate::{Run, cargo_command};
use serde_json::{Value, json};
use std::{fs, path::PathBuf};

#[derive(Debug, clap::Args)]
pub struct Coverage {
    #[arg(long)] pub output: PathBuf,
    #[arg(long, value_parser = ["html", "lcov", "json"], default_value = "json")] pub format: String,
    #[arg(long)] pub threshold: Option<f64>,
    /// Arguments passed unchanged to llvm-cov nextest, including package and test selection.
    #[arg(last = true)] pub args: Vec<String>,
}

pub fn percentage(report: &Value) -> Result<f64, String> {
    let data = report["data"].as_array().ok_or("coverage data missing")?;
    let mut count = 0u64;
    let mut covered = 0u64;
    for entry in data {
        let lines = &entry["totals"]["lines"];
        if lines["covered"].as_u64().zip(lines["count"].as_u64()).is_some_and(|(covered,count)|covered>count) { return Err("covered lines exceed line count".into()); }
        count = count.checked_add(lines["count"].as_u64().ok_or("line count missing")?).ok_or("line count overflow")?;
        covered = covered.checked_add(lines["covered"].as_u64().ok_or("covered lines missing")?).ok_or("covered lines overflow")?;
    }
    if count == 0 || covered > count { return Err("coverage has no lines or invalid totals".into()); }
    Ok(100.0 * covered as f64 / count as f64)
}

pub fn execute(run: &mut Run, options: &Coverage, env: &[(String, String)]) -> Result<(), String> {
    execute_with(run, options, env, cargo_command)
}

fn execute_with(run: &mut Run, options: &Coverage, env: &[(String,String)], cargo: impl Fn() -> std::process::Command) -> Result<(),String> {
    if !["json","html","lcov"].contains(&options.format.as_str()) { return Err("unsupported coverage format".into()); }
    if options.threshold.is_some_and(|t| !t.is_finite() || !(0.0..=100.0).contains(&t)) { return Err("threshold must be between 0 and 100".into()); }
    fs::create_dir_all(&options.output).map_err(|e| e.to_string())?;
    let output = fs::canonicalize(&options.output).map_err(|e| e.to_string())?;
    // Refuse reuse rather than report stale output from an earlier measurement.
    let summary = output.join("coverage.json");
    if summary.exists() || output.join("build").exists() { return Err("coverage output already used; choose a new directory".into()); }
    let mut command = cargo();
    command.args(["llvm-cov", "nextest", "--json", "--output-path"]).arg(&summary)
        .envs(env.iter().cloned()).env("CARGO_TARGET_DIR", output.join("build")).env("CARGO_LLVM_COV_TARGET_DIR", output.join("build")).env("CARGO_LLVM_COV_BUILD_DIR", output.join("build"));
    let explicit_scope=options.args.iter().take_while(|arg|arg.as_str()!="--").any(|arg|arg=="-p"||arg=="--package"||arg.starts_with("--package=")||(arg.starts_with("-p")&&arg.len()>2)||arg=="--workspace");
    if !explicit_scope { command.arg("--workspace"); }
    command.args(&options.args).arg("--no-tests=fail");
    run.command("coverage", &mut command)?;
    let report: Value = serde_json::from_slice(&fs::read(&summary).map_err(|e| format!("coverage report: {e}"))?).map_err(|e| e.to_string())?;
    let percent = percentage(&report)?;
    if options.format != "json" {
        let mut command = cargo();
        command.args(["llvm-cov", "report"]).env("CARGO_TARGET_DIR", output.join("build")).env("CARGO_LLVM_COV_TARGET_DIR", output.join("build")).env("CARGO_LLVM_COV_BUILD_DIR", output.join("build"));
        let expected = if options.format == "html" {
            command.args(["--html", "--output-dir"]).arg(&output);
            output.join("html/index.html")
        } else {
            let file = output.join("coverage.lcov");
            command.args(["--lcov", "--output-path"]).arg(&file);
            file
        };
        command.args(report_args(&options.args)?).envs(env.iter().cloned());
        run.command("coverage report", &mut command)?;
        if !expected.is_file() { return Err(format!("report missing: {}", expected.display())); }
    }
    let metadata = json!({"args": options.args, "line_percent": percent, "report": summary, "format": options.format, "regions": regions(&report)?});
    fs::write(output.join("summary.json"), serde_json::to_vec_pretty(&metadata).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    println!("{metadata}");
    if options.threshold.is_some_and(|t| percent < t) { return Err(format!("line coverage {percent:.2}% below requested threshold")); }
    Ok(())
}
#[cfg(test)] mod tests;

fn report_args(args: &[String]) -> Result<Vec<String>,String> {
    let mut out=Vec::new(); let mut args=args.iter();
    while let Some(arg)=args.next() {
        if matches!(arg.as_str(),"-p"|"--package"|"--exclude"|"--features"|"--target") {
            out.push(arg.clone());out.push(args.next().ok_or("selection value missing")?.clone());
        } else if matches!(arg.as_str(),"--workspace"|"--all-features"|"--no-default-features") || ["--package=","--exclude=","--features=","--target="].iter().any(|p|arg.starts_with(p)) || (arg.starts_with("-p") && arg.len()>2) { out.push(arg.clone()); }
    }
    Ok(out)
}

/// LLVM function regions use that function's file table, never the report's file order.
pub fn regions(report: &Value) -> Result<Vec<Value>, String> {
    let mut out=Vec::new();
    for data in report["data"].as_array().ok_or("coverage data missing")? {
        let functions=data["functions"].as_array().ok_or("LLVM function regions missing from coverage report")?;
        {
            for function in functions {
                for region in function["regions"].as_array().ok_or("function regions missing")? {
                    let id=region[5].as_u64().ok_or("region file id missing")? as usize;
                    let file=function["filenames"][id].as_str().ok_or("region file missing")?;
                    let number=|i:usize|region[i].as_u64().ok_or_else(||"invalid region coordinates".to_string());
                    out.push(json!({"file":file,"start_line":number(0)?,"start_column":number(1)?,"end_line":number(2)?,"end_column":number(3)?,"count":number(4)?}));
                }
            }
        }
    }
    Ok(out)
}
