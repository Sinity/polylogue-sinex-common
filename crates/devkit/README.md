# devkit

A project's `xtask` starts a `devkit::Run`, adds its steps (`cargo` commands
with captured diagnostics, a nextest run with each test's outcome, its own
checks) and finishes it. Every run is recorded in one machine-wide SQLite
store at `$XDG_STATE_HOME/devkit/history.db`:

- `xtask status`: failing tests and compiler errors from this checkout's last run;
- `xtask history runs | failures [RUN] | diagnostics [RUN] [--level error]`, each with `--json`;
- `xtask history intermittent | first-failing | durations | diagnostic-groups`: queries across runs and worktrees ([details](../../docs/history.md));
- `xtask changed`: the nextest filterset for packages changed since the merge base.

`Run::nextest` accepts `--changed` in its arguments (for example,
`cargo xtask test --changed`). It compares against the merge base with
`origin/master`; `--changed-base=<ref>` chooses another base. The diff includes
committed, staged, unstaged and untracked files. Production changes select the
owning packages and their reverse dependencies through nextest's `rdeps`.
Changes confined to test functions select those tests within their package.
Every changed line must be inside a test in both versions, and its name must
have no other uses in the package's Rust sources. Calls between tests and
partial overlaps keep package scope. A change to a Cargo-declared integration test file selects
that package's test binary. Shared helpers and other package files select the package and reverse
dependencies; files outside packages select the workspace, except Markdown.
Explicit `-E` filtersets intersect this selection. An unchanged tree succeeds
without building tests. This repository's `focused` operation uses the same path.

Passing tests keep no output and each diagnostic text is stored once, so the
store stays small without deleting anything.

## Tests

`#[devkit::test]` marks a test. A sync test without parameters is a plain
`#[test]`; an `async fn` test runs on a current-thread tokio runtime,
optionally with `timeout = "30s"`. `multi_thread` gives it a multi-thread
runtime, for a body that blocks a thread while spawned tasks must keep running;
the test crate then needs tokio's `rt-multi-thread` feature. Each parameter is a fixture whose type
implements `devkit::Fixture` and is created before the body runs:

```rust
#[devkit::test(timeout = "30s")]
async fn writes_events(db: devkit::pg::TestDatabase, bus: devkit::nats::TestNats) {
    let url = db.url(None);
    let subject = bus.subject("events");
    // ...
}
```

A project can implement `Fixture` for its own types, such as a context that
wraps a database and a connection.

With the `pg` feature, devkit provides PostgreSQL for projects with
compile-time-checked SQL. A project's xtask chooses the per-user service on
the socket in `$XDG_RUNTIME_DIR/devkit/pg` (Sinnix's `devkit-pg`) or an owned
throwaway cluster on tmpfs. Each schema version, named by a hash of the
schema sources, is built once into a template database and copied to the
read-only database `DATABASE_URL` points at; every test clones the template
(`TestDatabase`), using PostgreSQL's `FILE_COPY` strategy. The clone benchmark
compares it with `WAL_LOG` and a background pool. Failed-test databases
follow the newest-five policy.
`xtask db ensure|url|psql|exec|drop-unused|ready` drives it.
A half-built template's name carries its builder's PID, and cleanup drops it
only after that process has exited.

Database cleanup in `Run::nextest` requires `DEVKIT_PG_SOCKET` in that run's explicit environment. A project's xtask chooses the shared service or an owned server and supplies its socket when cleanup is wanted; the library never discovers the live service during a test run. Schema builds and cleanup use the same file lock in the server's socket directory; cleanup skips a busy version. Databases stamped used or failed within the last hour stay regardless of cleanup policy. Missing or unreadable schema stamps count as current use, and cleanup rechecks both timestamps and connections before each drop.

With the `nats` feature, devkit starts a throwaway `nats-server` with
JetStream on a free loopback port. A project's xtask starts one per test run
and passes its URL through `DEVKIT_NATS_URL`; `TestNats` gives each test that
server and a unique subject prefix, or a server of its own when run outside
xtask.

`Nextest { tmpfs: true, .. }` gives the tests a `TMPDIR` of their own: a child
of the job's `AGENTCTL_SCRATCH` when AgentCTL supplies one (declare
`scratch = "tmpfs"` on test operations, and AgentCTL removes it even after a
killed run), otherwise a child of `/dev/shm`. Tests of real filesystem
durability use `TEST_DURABLE_TMPDIR`. Nextest 0.9.143 or newer is required.

Nested Cargo commands use `devkit::cargo_command()`, which drops the parent's
`CARGO_MANIFEST_DIR`, `CARGO_MANIFEST_PATH` and `CARGO_PKG_*` so they do not
invalidate build-script fingerprints.

## Benchmarks

The `bench` feature draws a stratified sample of a corpus, records each bench
run with its metrics and the host load, sampled every second by
`LoadMonitor`, and compares a run with the previous quiet run on the same
sample and storage. A run is quiet when every load reading was below the
thresholds and no reading was more than two seconds late.
`linear_estimate` scales a sample's time to the whole corpus by input bytes
and labels itself uncalibrated.

## Shared development runners

`devkit::vm` validates the complete requested set of exported NixOS scenarios before running builds. It refuses uncommitted source input, retrieves each original scenario log with `nix log` even for cached builds, writes build output separately, continues through failures and writes scenario results. A project supplies its scenario names. AgentCTL owns queueing and cancellation.

`devkit::coverage` runs cargo-llvm-cov through `Run::nextest` with the caller's package and test selection, separate instrumentation output, full LLVM JSON and an optional HTML or LCOV report. It checks weighted line totals, report existence and a caller-selected threshold. The summary records the exact selection and line regions mapped through each function's file table. Build diagnostics, stages and individual test outcomes enter the same history as ordinary test runs. Empty test selection fails. Output cannot be reused and is never broadly removed.

`devkit::fuzz` discovers binary targets through Cargo manifest metadata, runs a named target with explicit duration and memory budgets, and replays saved inputs once. Corpus and crash files stay in caller-selected directories after failure. No targets are generated or silently skipped. Nix library search directories are added to the inherited library path. The pinned Nix Rust compiler is stable, while cargo-fuzz requires unstable sanitizer flags. The fuzz command alone sets `RUSTC_BOOTSTRAP=1` to enable those flags. `DEVKIT_FUZZ_RUSTFLAGS` adds experiment-only compiler flags to cargo-fuzz's instrumentation; a project can use [`-Zexternal-clangrt`](https://doc.rust-lang.org/unstable-book/compiler-flags/external-clangrt.html) with a matching Clang linker when its Rust compiler does not bundle sanitizer runtimes.

`devkit::process` owns development-stack service groups, forwards SIGINT and SIGTERM to them and stops descendants when dropped. It observes child exits without reaping the leader until the final group signal, so an exited leader's id cannot be reused during cleanup. Ordinary `Run` commands stay in the caller's group and receive terminal signals directly. Source-content hashing detects additions, removals and edits while excluding build output. A project supplies service setup, executable arguments and a real readiness probe. AgentCTL's job unit also owns every process on cancellation, including descendants of interrupted build tools.

`devkit::process::development_lock` holds one development stack per checkout. Keep the returned file open until its services stop; the persistent file under the checkout’s ignored `target` directory must stay in place so concurrent processes lock the same inode. The kernel releases ownership when the process exits. `source_hash_excluding` and PostgreSQL schema hashes share the file hashing code; source watches skip symbolic links and select Rust, TOML, JSON and Cargo.lock while schemas follow links in the supplied paths.

## PostgreSQL clone benchmark

The PostgreSQL clone benchmark is an ignored devkit test. It creates a synthetic
template in its own throwaway cluster, checks clone contents and isolation, and
times batches of 50 clones for `FILE_COPY`, `WAL_LOG`, and a four-entry pool
replenished in the background using `WAL_LOG`. Each batch is dropped after its
clone timing ends. Pool timing includes creating the initial databases and
background replenishment. Drop times are reported separately. By default one
round runs on a cluster containing only the template. `DEVKIT_BENCH_ROUNDS`
sets the repetition count; setting `DEVKIT_BENCH_WRITES` adds a half-gigabyte
working set and runs both idle and background-writer cases. The default
12,662,463-byte target matches the largest measured Sinex schema template;
`DEVKIT_BENCH_TEMPLATE_BYTES` can set another size. `DEVKIT_BENCH_DIRECTORY`
chooses the storage directory; without it, the cluster uses tmpfs. Use a short
path on the same filesystem as the development service for representative
results. Run it through AgentCTL:

```sh
agentctl job start --project /path/to/checkout exec -- cargo test -p devkit --features pg benchmark_database_clones -- --ignored --nocapture
```
