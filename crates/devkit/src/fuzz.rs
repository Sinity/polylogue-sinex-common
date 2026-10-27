//! Discover and execute cargo-fuzz targets, keeping corpora and crash files.
use crate::{Run, cargo_command};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, clap::Subcommand)]
pub enum Fuzz {
    List,
    Run {
        target: String,
        #[arg(long)]
        seconds: u64,
        #[arg(long)]
        memory_mb: u64,
        #[arg(long)]
        output: PathBuf,
    },
    Replay {
        target: String,
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

pub fn targets(root: &Path) -> Result<Vec<String>, String> {
    let manifest = root.join("fuzz/Cargo.toml");
    let result = cargo_command()
        .args([
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(&manifest)
        .output()
        .map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(format!(
            "fuzz manifest {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    let metadata: serde_json::Value =
        serde_json::from_slice(&result.stdout).map_err(|e| e.to_string())?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("fuzz packages missing")?;
    let mut names = Vec::new();
    for package in packages {
        if package["metadata"]["cargo-fuzz"] != true {
            continue;
        }
        for target in package["targets"]
            .as_array()
            .ok_or("fuzz targets missing")?
        {
            if target["kind"]
                .as_array()
                .is_some_and(|kinds| kinds.iter().any(|kind| kind == "bin"))
            {
                names.push(
                    target["name"]
                        .as_str()
                        .ok_or("fuzz target name missing")?
                        .to_owned(),
                );
            }
        }
    }
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Err("no fuzz targets declared".into());
    }
    Ok(names)
}

pub fn execute(run: &mut Run, options: &Fuzz) -> Result<(), String> {
    let names = targets(&run.root)?;
    execute_with(run, options, names, cargo_command)
}

fn execute_with(
    run: &mut Run,
    options: &Fuzz,
    names: Vec<String>,
    cargo: impl Fn() -> std::process::Command,
) -> Result<(), String> {
    let (target, output, budget, input) = match options {
        Fuzz::List => {
            for name in names {
                println!("{name}");
            }
            return Ok(());
        }
        Fuzz::Run {
            target,
            seconds,
            memory_mb,
            output,
        } => {
            if *seconds == 0 || *memory_mb == 0 {
                return Err("fuzz duration and memory budget must be positive".into());
            }
            (target, output, Some((*seconds, *memory_mb)), None)
        }
        Fuzz::Replay {
            target,
            input,
            output,
        } => (
            target,
            output,
            None,
            Some(fs::canonicalize(input).map_err(|e| format!("replay input: {e}"))?),
        ),
    };
    if input.as_ref().is_some_and(|path| !path.is_file()) {
        return Err("replay input must be a file".into());
    }
    if !names.contains(target) {
        return Err(format!(
            "unknown fuzz target {target}; available: {}",
            names.join(", ")
        ));
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    let output = fs::canonicalize(output).map_err(|e| e.to_string())?;
    let corpus = output.join("corpus");
    let artifacts = output.join("artifacts");
    fs::create_dir_all(&corpus).map_err(|e| e.to_string())?;
    fs::create_dir_all(&artifacts).map_err(|e| e.to_string())?;
    if budget.is_some() {
        seed_corpus(&run.root.join("fuzz/seeds").join(target), &corpus)?;
    }
    let mut command = cargo();
    // Nix's pinned compiler exposes sanitizer flags through this experiment-only setting.
    command
        .env("RUSTC_BOOTSTRAP", "1")
        .args(["fuzz", "run", target]);
    if let Some(input) = input {
        command.arg(input);
    } else {
        command.arg(corpus);
    }
    command
        .arg("--")
        .arg(format!("-artifact_prefix={}/", artifacts.display()));
    if let Some((seconds, memory)) = budget {
        command
            .arg(format!("-max_total_time={seconds}"))
            .arg(format!("-rss_limit_mb={memory}"));
    } else {
        command.arg("-runs=1");
    }
    let cxx = std::process::Command::new("g++")
        .arg("-print-file-name=libstdc++.so.6")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| {
            let path = PathBuf::from(s.trim());
            if path.is_absolute() {
                path.parent().map(Path::to_path_buf)
            } else {
                None
            }
        });
    let libraries = library_paths(
        std::env::var_os("LD_LIBRARY_PATH").as_deref(),
        std::env::var("NIX_LDFLAGS").ok().as_deref(),
        cxx.as_deref(),
    );
    command.env(
        "LD_LIBRARY_PATH",
        std::env::join_paths(libraries).map_err(|e| e.to_string())?,
    );
    println!("fuzz artifacts: {}", output.display());
    run.command("fuzz", &mut command)?;
    Ok(())
}
#[cfg(test)]
mod tests;

fn library_paths(
    existing: Option<&std::ffi::OsStr>,
    flags: Option<&str>,
    cxx: Option<&Path>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(flags) = flags {
        let mut words = flags.split_whitespace();
        while let Some(word) = words.next() {
            let path = if word == "-L" {
                words.next()
            } else {
                word.strip_prefix("-L")
            };
            if let Some(path) = path.filter(|s| !s.is_empty()) {
                paths.push(PathBuf::from(path));
            }
        }
    }
    if let Some(cxx) = cxx {
        paths.push(cxx.to_path_buf());
    }
    if let Some(existing) = existing {
        paths.extend(std::env::split_paths(existing).filter(|p| !p.as_os_str().is_empty()));
    }
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    unique
}

fn seed_corpus(seeds: &Path, corpus: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(seeds) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("seed directory {}: {e}", seeds.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().map_err(|e| e.to_string())?.is_file() {
            return Err(format!(
                "fuzz seed must be a file: {}",
                entry.path().display()
            ));
        }
        let bytes = fs::read(entry.path()).map_err(|e| e.to_string())?;
        let path = corpus.join(format!("seed-{}", blake3::hash(&bytes)));
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut file) => {
                use std::io::Write;
                file.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if fs::read(&path).map_err(|e| e.to_string())? != bytes {
                    return Err(format!("fuzz seed differs at {}", path.display()));
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}
