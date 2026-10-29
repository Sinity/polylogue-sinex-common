# Development

`rust-toolchain.toml` selects Rust and its components for rustup and the Nix
shell. The flake reads that file through its pinned rust-overlay input.
Changing the compiler version means changing this file in common and both
products together.

```sh
nix develop
cargo xtask check
cargo xtask test -p devkit --all-features
```

`direnv allow` loads the same shell through `.envrc`. Its ignored
`.envrc.local` is the place for personal build-output placement, job counts
and compiler wrappers. Cargo's default output directory is `target/`; a
caller may set `CARGO_TARGET_DIR` explicitly.

`nix flake check` builds sealed formatting, Clippy, nextest and browser-shell
checks. Clippy and nextest reuse one Crane dependency artifact build with the
same Rust toolchain and all workspace features. The source includes fixture
JSON and browser assets as well as Rust files.

The nextest check excludes only devkit's `pg::` and `nats::` test groups,
which start database services. `cargo xtask check` runs those groups too and
remains the full native check. Database service tools stay in the shell:
PostgreSQL 18 with TimescaleDB and pgvector, and NATS. DBus and the frontend
tools support the existing notification and browser checks.

The shell installs this checkout's Git hooks when `core.hooksPath` is unset.
An explicitly configured hook path is preserved. The hooks check staged
whitespace and staged Rust formatting before a commit. Before a push they check
Rust formatting when Rust build inputs changed. They use the loaded shell's
tools when available, or enter the declared shell. Clippy and tests remain in
the native full check. Timezone tests use the shell's declared tzdata,
and notification tests start their own configured private D-Bus service.

`nix fmt` uses nixfmt. The shell also provides cargo-llvm-cov, cargo-fuzz,
cargo-deny, mold, sqlite and jq. Coverage uses the LLVM tools from the declared
Rust toolchain.

A host may queue these commands through its own job runner.

For a change spanning common and a product, use the explicit
[local linking command](../README.md#depending-on-this-repository).
