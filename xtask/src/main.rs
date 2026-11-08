//! `cargo xtask check` for this repository, plus devkit's shared commands.
use clap::Parser;
use devkit::{Nextest, Run};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "xtask")]
enum Cli {
    /// Formatting, clippy and every test.
    Check,
    /// Run selected tests, optionally limited to the current diff.
    Test {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    #[command(flatten)]
    Shared(devkit::cli::Command),
}

fn main() -> ExitCode {
    match Cli::parse() {
        Cli::Check => {
            let mut run = Run::start("polylogue-sinex-common", "check", &[]);
            let _ = check(&mut run);
            run.finish()
        }
        Cli::Test { args } => {
            let args = test_features(args);
            let mut run = Run::start("polylogue-sinex-common", "test", &args);
            let _ = run.nextest(
                "test",
                &Nextest {
                    args,
                    ..Nextest::default()
                },
            );
            run.finish()
        }
        Cli::Shared(command) => devkit::cli::execute("polylogue-sinex-common", command),
    }
}

fn check(run: &mut Run) -> devkit::Result {
    run.command(
        "fmt",
        devkit::cargo_command().args(["fmt", "--all", "--", "--check"]),
    )?;
    // Compile runtime and fixture surfaces without the workspace's all-feature
    // unification, then retain the complete clippy and conformance run below.
    for package in ["query-language", "web-shell", "devkit"] {
        run.cargo(
            &format!("minimal-{package}"),
            &[
                "check",
                "--locked",
                "-p",
                package,
                "--all-targets",
                "--no-default-features",
            ],
        )?;
    }
    run.cargo(
        "devkit-service-fixtures",
        &[
            "check",
            "--locked",
            "-p",
            "devkit",
            "--all-targets",
            "--no-default-features",
            "--features",
            "pg,nats",
        ],
    )?;
    run.cargo(
        "devkit-runner",
        &[
            "check",
            "--locked",
            "-p",
            "devkit",
            "--all-targets",
            "--no-default-features",
            "--features",
            "runner",
        ],
    )?;
    run.command(
        "web-shell",
        std::process::Command::new("node").args(["crates/web-shell/build.mjs", "--test"]),
    )?;
    run.cargo(
        "clippy",
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run.nextest(
        "test",
        &Nextest {
            args: vec!["--workspace".into(), "--all-features".into()],
            ..Nextest::default()
        },
    )
}

/// Focused test runs include the selected packages' conformance cases and generators unless
/// the caller explicitly chooses a feature configuration.
fn test_features(mut args: Vec<String>) -> Vec<String> {
    let explicit = args
        .iter()
        .take_while(|arg| arg.as_str() != "--")
        .any(|arg| {
            matches!(
                arg.as_str(),
                "--all-features" | "--no-default-features" | "--features"
            ) || arg.starts_with("--features=")
                || arg.starts_with("-F")
        });
    if !explicit {
        args.insert(0, "--all-features".into());
    }
    args
}

#[cfg(test)]
mod tests;
