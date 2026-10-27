# polylogue-sinex-common

Rust crates and development tooling shared by
[Polylogue](https://github.com/Sinity/polylogue) and
[Sinex](https://github.com/Sinity/sinex).

| Crate | What it is |
|---|---|
| [`hashed-blobs`](crates/hashed-blobs) | Content-addressed file store: plain, delta and sealed blobs with checked range reads |
| [`credential-masking`](crates/credential-masking) | Streaming credential masking that never refuses input ([behaviour](docs/credential-masking.md)) |
| [`devkit`](crates/devkit) | The core of each project's `xtask`: test and check runs, their recorded history, changed-package selection, test databases and NATS |
| [`devkit-macros`](crates/devkit-macros) | The `#[devkit::test]` attribute |

Projects depend on a tag:

```toml
hashed-blobs = { git = "https://github.com/Sinity/polylogue-sinex-common", tag = "v0.7.3" }
```

## devkit

A project's `xtask` starts a `devkit::Run`, adds its steps (`cargo` commands
with captured diagnostics, a nextest run with each test's outcome, its own
checks) and finishes it. Every run is recorded in one machine-wide SQLite
store at `$XDG_STATE_HOME/devkit/history.db`:

- `xtask status`: failing tests and compiler errors from this checkout's last run;
- `xtask history runs | failures [RUN] | diagnostics [RUN] [--level error]`, each with `--json`;
- `xtask history intermittent | first-failing | durations | diagnostic-groups`: queries across runs and worktrees ([details](docs/history.md));
- `xtask changed`: the nextest filterset for packages changed since the merge base.

`Run::nextest` accepts `--changed` in its arguments (for example,
`cargo xtask test --changed`). It compares against the merge base with
`origin/master`; `--changed-base=<ref>` chooses another base. The diff includes
committed, staged, unstaged and untracked files. Production changes select the
owning packages and their reverse dependencies through nextest's `rdeps`.
Changes confined to test functions select those tests within their package.
Every changed line must be inside a test in both versions; a partial overlap
keeps package scope. A change to a Cargo-declared integration test file selects
that package's test binary. Shared helpers and other package files select the package and reverse
dependencies; files outside packages select the workspace, except Markdown.
Explicit `-E` filtersets intersect this selection. An unchanged tree succeeds
without building tests. This repository's `focused` operation uses the same path.

Passing tests keep no output and each diagnostic text is stored once, so the
store stays small without deleting anything.

### Tests

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
compile-time-checked SQL. The server is the per-user service on the socket in
`$XDG_RUNTIME_DIR/devkit/pg` (Sinnix's `devkit-pg`), or a throwaway cluster on
tmpfs when it is not running. Each schema version, named by a hash of the
schema sources, is built once into a template database and copied to the
read-only database `DATABASE_URL` points at; every test clones the template
(`TestDatabase`), using PostgreSQL's `WAL_LOG` strategy. At the start of each
`Run::nextest` invocation, devkit reaps abandoned databases on the supplied
server (or the running per-user service). This also runs when no schema is
built. Failed-test databases follow the existing newest-five policy.
`xtask db ensure|url|psql|exec|drop-unused|ready` drives it.
A half-built template's name carries its builder's PID, and cleanup drops it
only after that process has exited.

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

### Benchmarks

The `bench` feature draws a stratified sample of a corpus, records each bench
run with its metrics and the host load, sampled every second by
`LoadMonitor`, and compares a run with the previous quiet run on the same
sample and storage. A run is quiet when every load reading was below the
thresholds and no reading was more than two seconds late.
`linear_estimate` scales a sample's time to the whole corpus by input bytes
and labels itself uncalibrated.

## Development runs

Run checks through the repository's declared AgentCTL operations:

```sh
agentctl job start --project /path/to/checkout check
agentctl job start --project /path/to/checkout focused -- -p devkit --all-features
agentctl job start --project /path/to/checkout compile -- -p devkit --all-features
```

An explicit `CARGO_TARGET_DIR` in the caller's environment is passed to the job.

The PostgreSQL clone benchmark is an ignored devkit test. It creates a synthetic
template in its own throwaway cluster, checks clone contents and isolation, and
times batches of 50 clones for `FILE_COPY`, `WAL_LOG`, and a four-entry pool
replenished in the background using `WAL_LOG`. Each batch is dropped after its
clone timing ends. Three alternating rounds run both with an idle half-gigabyte
working set and with a writer updating it. Pool timing includes creating the
initial databases and background replenishment. Drop times are reported
separately. The default
12,662,463-byte target matches the largest measured Sinex schema template;
`DEVKIT_BENCH_TEMPLATE_BYTES` can set another size. `DEVKIT_BENCH_DIRECTORY`
chooses the storage directory; without it, the cluster uses tmpfs. Use a short
path on the same filesystem as the development service for representative
results. Run it through AgentCTL:

```sh
agentctl job start --project /path/to/checkout exec -- cargo test -p devkit --features pg benchmark_database_clones -- --ignored --nocapture
```
