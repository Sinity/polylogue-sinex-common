# polylogue-sinex-common

Rust crates and development tooling shared by
[Polylogue](https://github.com/Sinity/polylogue) and
[Sinex](https://github.com/Sinity/sinex).

| Crate | What it is |
|---|---|
| [`hashed-blobs`](crates/hashed-blobs) | Content-addressed file store: plain, delta and sealed blobs with checked range reads |
| [`credential-masking`](crates/credential-masking) | Streaming credential masking that never refuses input ([behaviour](docs/credential-masking.md)) |
| [`devkit`](crates/devkit) | The core of each project's `xtask`: test and check runs, their recorded history, changed-package selection |
| [`devkit-macros`](crates/devkit-macros) | The `#[devkit::test]` attribute |

Projects depend on a tag:

```toml
hashed-blobs = { git = "https://github.com/Sinity/polylogue-sinex-common", tag = "v0.1.0" }
```

## devkit

A project's `xtask` starts a `devkit::Run`, adds its steps (`cargo` commands
with captured diagnostics, a nextest run with each test's outcome, its own
checks) and finishes it. Every run is recorded in one machine-wide SQLite
store at `$XDG_STATE_HOME/devkit/history.db`:

- `xtask status`: failing tests and compiler errors from this checkout's last run;
- `xtask history runs | failures [RUN] | diagnostics [RUN] [--level error]`, each with `--json`;
- `xtask changed`: the nextest filterset for packages changed since the merge base.

Passing tests keep no output and each diagnostic text is stored once, so the
store stays small without deleting anything.

With the `pg` feature, devkit also provides PostgreSQL for projects with
compile-time-checked SQL. The server is the per-user service on the socket in
`$XDG_RUNTIME_DIR/devkit/pg` (Sinnix's `devkit-pg`), or a throwaway cluster on
tmpfs when it is not running. Each schema version, named by a hash of the
schema sources, is built once into a template database and copied to the
read-only database `DATABASE_URL` points at; every test clones the template.
`xtask db ensure|url|psql|exec|drop-unused|ready` drives it. Half-built
schema names include their owner PID: cleanup preserves live bootstraps even
while they compile without a database connection, and removes them after the
owner exits. PID reuse can delay cleanup. Older half-built names have no owner
evidence and are preserved rather than guessed abandoned.

With the `nats` feature, devkit starts a throwaway `nats-server` with
JetStream on a free loopback port. A project's xtask starts one per test run
and passes its URL through `DEVKIT_NATS_URL`; `devkit::nats::test_nats()`
gives each test that server and a unique subject prefix, or a server of its
own when run outside xtask.

## Development runs

Use the shared repository's declared operations for queued work:

```sh
agentctl job start /path/to/checkout check
agentctl job start /path/to/checkout focused -- -p devkit --all-features
agentctl job start /path/to/checkout compile -- -p devkit --all-features
```

Nextest 0.9.143 or newer is required for JUnit skipped-case reporting. Devkit
requests all skipped cases, records ignore-selection exclusions separately from
filters, and retains the emitted reason. Under `--run-ignored only`, ordinary
tests are excluded by the ignore selection too. Unrecognized reasons remain
`skipped`; missing, truncated or inconsistent reports fail a reporting step.
Counts cover reported test cases across each invocation. A filter can exclude
an entire binary before nextest lists its tests; an empty report does not prove
that binary contained zero ignored tests. Historical reports are not backfilled.

The check summary separates compiler time from test execution. Compare those
stages before accepting a slow suite as normal; a test process waiting for a
server or a saturated disk needs diagnosis, not a larger timeout. Focused checks
stay cheap, and full checks belong to coherent batches. Tests selecting no cases
fail. Keep extended runs in the queue while continuing independent work.

`Nextest { tmpfs: true, .. }` creates an exclusive child of `AGENTCTL_SCRATCH`
when the native job supplies it. Declare `scratch = "tmpfs"` on test operations:
the existing AgentCTL owner then removes retained test files after normal exit,
cancellation or a killed process. Devkit never adopts or removes the parent.
For long Linux paths, a held directory descriptor exposes a short `/proc` path
for Unix sockets; the files still live inside the job-owned directory.

Outside a managed job, the fallback is an exclusive child under `/dev/shm`
when available, otherwise the inherited temporary directory. Normal Drop removes
only that child; standalone abrupt death has no external cleanup guarantee.
Tests of real filesystem durability still use `TEST_DURABLE_TMPDIR`.

## Benchmark reports

`LoadMonitor` samples host load once per second and at the beginning and end,
keeping a constant-size summary. Reports show missing readings, observed busy
samples, peaks and the largest sampling gap. Comparisons decline runs with
unknown/busy readings or gaps above two seconds, including older start-only
records. Passing those checks permits descriptive differences on the same
sample and storage; it does not establish reliable timing or its cause.

`linear_estimate` labels the input-byte assumption and `calibrated: false`.
Missing or invalid times/scales leave the estimate unknown. One sample cannot
establish how fixed costs, caching, decompression or nonlinear stages scale.
Recorded input bytes, reported material bytes, physical I/O and final storage
size keep their own units; none is a substitute for another. Decoded or hashed
byte totals remain unmeasured where a runner provides no such counter.

Nested Cargo commands use `devkit::cargo_command()` to drop inherited
`CARGO_MANIFEST_DIR`, `CARGO_MANIFEST_PATH` and `CARGO_PKG_*` package metadata.
Compiler, toolchain, target and job configuration remain inherited. This avoids
parent-checkout changes invalidating build-script environment fingerprints; it
does not eliminate other reasons Cargo rebuilds a dependency.
