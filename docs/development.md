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

A host may queue these commands through its own job runner.

For a change spanning common and a product, use the explicit
[local linking command](../README.md#depending-on-this-repository).
