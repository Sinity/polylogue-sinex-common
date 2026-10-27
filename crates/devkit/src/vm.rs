//! Run exported NixOS scenarios and keep each scenario's original log.
use crate::Run;
use std::{fs, path::PathBuf, process::{Command, Stdio}};
#[derive(Debug, clap::Args)]
pub struct Vm {
    #[arg(long = "scenario")] pub scenarios: Vec<String>,
    #[arg(long, default_value = "x86_64-linux")] pub system: String,
    #[arg(long)] pub output: PathBuf,
}
pub fn select(requested: &[String], exported: &[String]) -> Result<Vec<String>, String> {
    if requested.is_empty() { return Err("no VM scenarios requested".into()); }
    for name in requested { if !exported.contains(name) { return Err(format!("requested VM scenario not exported: {name}")); } }
    let mut selected = requested.to_vec(); selected.sort(); selected.dedup(); Ok(selected)
}
pub fn execute(run: &mut Run, options: &Vm, defaults: &[&str]) -> Result<(), String> {
    execute_with(run, options, defaults, ||Command::new("nix"))
}

fn execute_with(run: &mut Run, options: &Vm, defaults: &[&str], nix: impl Fn() -> Command) -> Result<(),String> {
    let tree = crate::Tree::observe(&run.root);
    if let Some(error) = tree.observation_error { return Err(error); }
    if tree.dirty.is_some() { return Err("VM checks require a committed checkout, including new files, so Nix tests the recorded input".into()); }
    let flake = format!("{}#checks.{}",run.root.display(),options.system);
    let output = nix().args(["eval","--json",&flake,"--apply","builtins.attrNames"]).output().map_err(|e|e.to_string())?;
    if !output.status.success() { return Err(format!("VM discovery failed: {}",String::from_utf8_lossy(&output.stderr))); }
    let exported: Vec<String> = serde_json::from_slice(&output.stdout).map_err(|e|format!("VM discovery: {e}"))?;
    let requested = if options.scenarios.is_empty() { defaults.iter().map(|s|s.to_string()).collect() } else { options.scenarios.clone() };
    let selected = select(&requested,&exported)?;
    fs::create_dir_all(&options.output).map_err(|e|e.to_string())?;
    let mut failures = Vec::new();
    let mut results = Vec::new();
    for scenario in selected {
        let log = options.output.join(format!("{scenario}.log"));
        let file = fs::OpenOptions::new().write(true).create_new(true).open(&log).map_err(|e|format!("{}: {e}",log.display()))?;
        let mut command = nix();
        command.args(["build","--no-link","--print-build-logs",&format!("{flake}.{scenario}")])
            .stdout(Stdio::from(file.try_clone().map_err(|e|e.to_string())?)).stderr(Stdio::from(file));
        let result = run.command(&scenario,&mut command);
        println!("{scenario}: {} ({})",if result.is_ok(){"passed"}else{"failed"},log.display());
        if let Err(error) = &result { failures.push(format!("{scenario}: {error}; log {}",log.display())); }
        results.push(serde_json::json!({"scenario":scenario,"passed":result.is_ok(),"log":log}));
    }
    fs::write(options.output.join("results.json"),serde_json::to_vec_pretty(&results).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    if failures.is_empty() { Ok(()) } else { Err(failures.join("\n")) }
}
#[cfg(test)] mod tests;
