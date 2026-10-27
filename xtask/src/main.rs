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
