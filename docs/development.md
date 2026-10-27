# Shared development runners

`devkit::vm` validates the complete requested set of exported NixOS scenarios before running builds. It refuses uncommitted source input, writes each original scenario log, continues through failures and writes scenario results. A project supplies its scenario names. AgentCTL owns queueing and cancellation.

`devkit::coverage` runs cargo-llvm-cov nextest with the caller's package and test selection, separate instrumentation output, full LLVM JSON and an optional HTML or LCOV report. It checks weighted line totals, report existence and a caller-selected threshold. The summary records the exact selection and line regions mapped through each function's file table. Empty test selection fails. Output cannot be reused and is never broadly removed.

`devkit::fuzz` discovers binary targets through Cargo manifest metadata, runs a named target with explicit duration and memory budgets, and replays saved inputs once. Corpus and crash files stay in caller-selected directories after failure. No targets are generated or silently skipped. Nix library search directories are added to the inherited library path. `DEVKIT_FUZZ_RUSTFLAGS` adds experiment-only compiler flags to cargo-fuzz's instrumentation; a project can use [`-Zexternal-clangrt`](https://doc.rust-lang.org/unstable-book/compiler-flags/external-clangrt.html) with a matching Clang linker when its Rust compiler does not bundle sanitizer runtimes.

`devkit::process` owns child process groups and stops descendants when dropped. Source-content hashing detects additions, removals and edits while excluding build output. A project supplies service setup, executable arguments and a real readiness probe. AgentCTL's job unit also owns every process on cancellation, including descendants of interrupted build tools.

`devkit::process::development_lock` holds one development stack per checkout. Keep the returned file open until its services stop; the persistent file under the checkout’s ignored `target` directory must stay in place so concurrent processes lock the same inode. The kernel releases ownership when the process exits.
