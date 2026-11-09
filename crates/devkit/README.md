# devkit

The core of each product's `xtask`: test and check runs and their recorded
history, changed-package selection, test databases and NATS servers, the
`#[devkit::test]` fixtures, and the bench, coverage, fuzz and VM runners. A
product's `xtask` is a thin binary that starts a `devkit::Run`, adds its own
steps and checks, and passes the shared commands (`status`, `history`,
`changed`, `db`) to `devkit::cli`.

Devkit targets data and guarantees, not one machine. It starts what a run
needs and stops it on drop, takes paths and services from configuration or
the environment, and names its interfaces neutrally (`DEVKIT_JOB_ID`,
`DEVKIT_SCRATCH`, `DEVKIT_PG_SOCKET`, `DEVKIT_NATS_URL`) so that any job
runner can set them. Nothing here knows a particular runner or host.

## Build features

Fixture consumers use `default-features = false` and select `pg` and/or
`nats` when needed. This keeps `Fixture`, `#[devkit::test]`, service ownership
and process helpers available without the run recorder or source parser.
The `runner` feature adds `Run`, result/diagnostic types, changed-test
selection and local dependency linking. `history` includes `runner`; the
default `cli` feature includes history, and `bench` includes history too.
Products opt into CLI or runner features in their xtask dependency.
`cargo xtask check` compiles fixture-only, service-fixture and runner builds
separately before checking and testing every feature together.

## Runs

A run is one `xtask` invocation. It adds steps (`cargo` commands with
captured diagnostics, a nextest run with each test's outcome, the product's
own checks), prints a short summary and records itself in one machine-wide
SQLite store at `$XDG_STATE_HOME/devkit/history.db`. History is data, never a
gate: a store that cannot be opened is reported and the run continues.

Passing tests keep no output and each diagnostic text is stored once, so the
store stays small without deleting anything.

A run records its process ID, the process's start time and the kernel's
boot ID. A run still marked running reads as `interrupted` once the machine
has booted since, its process is gone, or its process ID names a later
process; nothing rewrites the record.

Each `Run::nextest` invocation keeps its generated configuration and JUnit
report in its existing exclusive scratch child, alongside its tests' state.
Concurrent invocations may share Cargo's target directory; neither rewrites
or removes another invocation's configuration or report. Outcomes enter the
run before its scratch child is removed. Compiler-cache locking remains Cargo's.

The tests of `Run::nextest` get an `XDG_STATE_HOME` of their own, removed
after the run, so a test that starts a run, devkit's own included, records
it in a throwaway store and never in the user's. Caller-supplied test
environment entries cannot replace this owned state directory.

Nested Cargo commands use `devkit::cargo_command()`, which drops the parent's
`CARGO_MANIFEST_DIR`, `CARGO_MANIFEST_PATH` and `CARGO_PKG_*` so they do not
invalidate build-script fingerprints.

`Run::nextest` passes Cargo's package selection unchanged to both the build
and execution commands. Use `-p <package>` to compile that package's test
targets and its dependencies. A nextest `-E 'package(...)'` filter selects
execution within the Cargo build scope; it does not replace `-p`. A product
keeps its full-workspace defaults for `check` and unscoped `test` runs, and
passes explicit package selectors through to Cargo.

## Local dependency links

`devkit::local_link::LocalLink` builds several Cargo workspaces against one
checkout of a Git dependency. The product supplies the repository URL, patch
path, and each workspace's existing lock and original-backup paths. Its CLI
owns checkout discovery and user-facing messages.

`link(checkout)` discovers the shared crates, writes the Cargo patch, keeps
the original locks and resolves every workspace. A crate still resolving to
Git is an error, including Cargo falling back to Git when the checkout's
version does not satisfy a workspace. Successful relinks keep the same
original backups. `unlink()` restores those locks, removes the patch and
backups. Linking happens only when the product CLI requests it.

Both operations restore the preceding file contents on a returned error,
including the preceding active link and any absent files. A rollback failure
is included in the error. Process termination can interrupt the operation;
the original-lock backups remain available for unlinking.

## Changed-package selection

`Run::nextest` accepts `--changed` (for example `cargo xtask test
--changed`); `xtask changed` prints the same selection as a nextest
filterset. The diff is taken against the merge base with `origin/master`, or
`--changed-base=<ref>`, and includes committed, staged, unstaged and
untracked files.

- Production changes select the owning packages and their reverse
  dependencies through nextest's `rdeps`.
- Changes confined to test functions select those tests within their package:
  every changed line must be inside a test in both versions, and the test's
  name must have no other use in the package's Rust sources. Calls between
  tests and partial overlaps keep package scope.
- A change to a Cargo-declared integration test file selects that package's
  test binary.
- Shared helpers and other package files select the package and its reverse
  dependencies; files outside packages select the workspace, except Markdown.
- Explicit `-E` filtersets intersect the selection. An unchanged tree succeeds
  without building tests.

## History

`cargo xtask status` shows the failing tests and compiler errors of this
checkout's last run. `cargo xtask history` queries the store; output is
formatted JSON, or one line per record with `--json`. Queries read the
current checkout by default; repeat `--checkout /absolute/path` to read
several worktrees of the same product, including ones that no longer exist.
Other products' runs are never included. An explicit run ID may select another
checkout of the invoking product; a foreign product's ID is refused before
its diagnostics or test output are read.

Each run records its HEAD, the Git tree object of that commit and a hash of
its uncommitted changes (tracked diffs and untracked files). If Git fails, an
untracked file cannot be read, or the checkout changes during the run, the run
records why in `observation_error` and has no tree.

| Query | Answers |
|---|---|
| `runs`, `failures [RUN]`, `diagnostics [RUN] [--level error]` | one checkout's runs, their failing tests and their compiler messages |
| `intermittent` | tests that passed in one run and failed in another at the same tree and uncommitted-changes hash; different commits with the same tree group together. Each group lists every run with its statuses, checkout, command, toolchain and host. A `flaky` status (failed, then passed on retry) is shown but counts as neither. `results_without_tree` counts results left out because their run has no tree |
| `first-failing SUITE TEST` | every recorded result of one test in run start order, and the first that failed or was flaky, with whether that run was at a clean HEAD, had uncommitted changes, or is unknown. This is the order runs happened in, not a bisection |
| `durations COMMAND [--args-json ARGS] [--stage NAME]` | minimum, median and maximum over passed runs of the command with exactly these arguments (default none), or over the successful stage. Every selected run is listed; one left out of the summary says why (`run not passed`, `stage not recorded`, `stage failed`, `duration not recorded`) |
| `diagnostic-groups [--level error\|warning]` | compiler messages grouped by text across worktrees, each occurrence with its run, checkout, HEAD, command, package and location |
| `compare BEFORE AFTER` | two passed runs with identical command, arguments, host and toolchain: checkout and Git identity, signed duration changes per run and stage, and slower, faster or stable trends. A stage on one side only shows its duration and an unknown change; a percentage against zero stays null; different scopes or a source observation error are refused |
| `explain RUN` | every test by duration, pass rate over passed, failed and flaky outcomes, stages, and the recorded runs that overlapped it on the same host. Parallel test durations are never added into wall time, and overlap alone is not reported as contention. The job runner's queue wait is linked through the recorded `DEVKIT_JOB_ID`, not measured here |
| `compare-days DAY DAY` | this checkout's runs on two UTC days, grouped by exact command, arguments, host and toolchain: run and pass counts and mean passed-run durations. A group present on one day only keeps that day's figures with null changes; failed source observations and invalid durations are left out of means |

Load and cache state at the time of a run are not recorded, so durations are
compared between runs, never asserted.

## Tests and fixtures

`#[devkit::test]` marks a test. A sync test without parameters is a plain
`#[test]`; an `async fn` test runs on a current-thread tokio runtime,
optionally with `timeout = "30s"`. `multi_thread` gives it a multi-thread
runtime, for a body that blocks a thread while spawned tasks must keep
running; the test crate then needs tokio's `rt-multi-thread` feature. Each
parameter is a fixture whose type implements `devkit::Fixture` and is created
before the body runs:

```rust,ignore
#[devkit::test(timeout = "30s")]
async fn writes_events(db: devkit::pg::TestDatabase, bus: devkit::nats::TestNats) {
    let url = db.url(None);
    let subject = bus.subject("events");
    // ...
}
```

A product can implement `Fixture` for its own types, such as a context that
wraps a database and a connection.

`Nextest { tmpfs: true, .. }` gives the tests a `TMPDIR` of their own: a child
of `DEVKIT_SCRATCH` when the job runner supplies one (and removes it after the
job, even a killed one), otherwise a child of `/dev/shm`. Tests of real
filesystem durability use `TEST_DURABLE_TMPDIR`. Nextest 0.9.143 or newer is
required.

### PostgreSQL

With the `pg` feature, devkit provides PostgreSQL for products with
compile-time-checked SQL, over a socket: a per-user service when one is
running (its socket in `$XDG_RUNTIME_DIR/devkit/pg`), otherwise an owned
throwaway cluster on tmpfs. The product's `xtask` chooses which.

`ClusterOptions.required_extensions` names the extensions schema bootstrap
needs. `Server::service_or_start` checks the running service's actual
`pg_available_extensions`, PostgreSQL major version and required preloaded
libraries before reusing it. An incompatible service stays running; devkit
starts an isolated cluster from the current development shell instead and
verifies the same requirements before returning it. A missing extension in
that shell fails by name before schema bootstrap. Further server settings
apply to newly started clusters.

An owned cluster runs PostgreSQL as a direct child. Normal shutdown stops
and waits for that child, so a container whose init does not reap orphans
does not accumulate exited postmasters. If the owner is killed, the existing
watchdog stops the cluster and removes its files; reaping processes orphaned
by abrupt owner death remains the platform init's responsibility.

- Each schema version, named by a hash of the schema sources, is built once
  into a template database and copied to the read-only **schema database**
  that `DATABASE_URL` points at. Its builder holds a file lock in the
  server's socket directory while it builds into `<project>_building_<hash>`
  and renames that into place; a builder that was killed leaves that name
  behind, and the next builder or cleanup, finding the lock free, drops it.
  A built template accepts no connections: a copy fails while any session is
  connected to its template, and autovacuum or a TimescaleDB scheduler
  would otherwise connect to it. Ensuring a version ends sessions a template
  still has from before this rule.
- A test takes a **slot**: one of a fixed number of test database names per
  project, `<project>_test<k>`. `test_database()` (or a `TestDatabase`
  fixture) locks the slot's file in the socket directory, trying the slots
  in order and waiting for any to come free when all are taken, then drops
  whatever database the slot holds and clones the template into it. A test
  process that already holds a slot of the family never waits: two tests
  each holding one and waiting for another would wait forever, so it takes
  the first free name past the configured count. A passing test drops it. The
  kernel releases the lock however the test process ends, so a killed test's
  database is simply replaced by the next taker. Tests whose databases grow
  large, such as crash tests, use `large_test_database()` and its own slots,
  `<project>_large<k>`. Every run on the server shares the slots, so waiting
  for one also bounds the load that runs together put on a shared server.
- A failing (panicking) test's database is renamed to one of a fixed set of
  kept failed databases, `<project>_failed<j>`, replacing the oldest failure,
  and its name printed with a `psql -h <socket> <name>` line that opens it.
- A project therefore never has more test databases on a server than its
  ordinary and large slots, one more for each test process that holds
  several of a family, and its kept failed databases, whatever happens to
  the tests. `Slots` holds the three numbers. Their defaults are the
  host's parallelism (nextest's default thread count), 4 and 5, and
  `DEVKIT_PG_SLOTS`, `DEVKIT_PG_LARGE_SLOTS` and `DEVKIT_PG_FAILED_KEPT`
  replace them, set by the product's `xtask` (through `Nextest::env`) or by
  the host. A slot's name is reused by later tests, so a test that needs a
  unique path makes its own rather than deriving it from the database name.
- Every copy uses PostgreSQL's `FILE_COPY` strategy. It forces a checkpoint
  of the whole server per copy, yet on a tmpfs server without fsync it
  copied about twice as fast as `WAL_LOG`, one copy at a time and from four
  slots at once, idle and beside a writer.
- Each server value that ensured a schema version holds a shared lock on it
  until dropped. Cleanup drops versions beyond the newest eight per project
  by last use, or unused for fourteen days, never one whose lock is held or
  that has a connection; it runs at the start of `Run::nextest` only when
  the run's environment names the server in `DEVKIT_PG_SOCKET`, so the
  library never discovers a service on its own during a test run. It also
  drops the uniquely named test databases of earlier devkit versions
  (`t_<pid>_<seconds>_<number>`) once nothing is connected to them.
- `xtask db ensure | url | psql [DATABASE] | exec | drop-unused | ready`
  drives it.

An ignored test, `benchmark_database_clones`, times `FILE_COPY` and
`WAL_LOG` in its own throwaway cluster with a synthetic template: 50 clones
one after another, then four slots each replacing its database twelve times
at once, checking clone contents and isolation; drop times are reported
apart. `DEVKIT_BENCH_ROUNDS`, `DEVKIT_BENCH_WRITES` (adds a half-gigabyte
working set and a background writer), `DEVKIT_BENCH_TEMPLATE_BYTES` and
`DEVKIT_BENCH_DIRECTORY` (default tmpfs) set it up.

### NATS

With the `nats` feature, devkit starts a throwaway `nats-server` with
JetStream on a free loopback port. A product's `xtask` starts one per test run
and passes its URL through `DEVKIT_NATS_URL`; `TestNats` gives each test that
server and a unique subject prefix, or a server of its own when run outside
`xtask`.

## Runners

**Bench** (`bench` feature). Draws a stratified sample of a corpus, written
down in a manifest; records each run with its metrics and the host load,
sampled every second; and compares a run with the previous quiet run on the
same sample and storage. A run is quiet when every load reading was below the
thresholds and none was more than two seconds late. Products report the
hardware-independent figures (bytes, syncs and rows per input, stage
utilisation) beside absolute rates, as paired runs. `linear_estimate` scales a
sample's time to the whole corpus by input bytes and labels itself
uncalibrated.

**Coverage.** Runs cargo-llvm-cov through `Run::nextest` with the caller's
package and test selection, separate instrumentation output, full LLVM JSON
and an optional HTML or LCOV report. It checks weighted line totals, report
existence and a caller-chosen threshold, and records the exact selection and
the line regions mapped through each function's file table. Build
diagnostics, stages and test outcomes enter the same history as ordinary
runs. An empty test selection fails. The instrumentation directory is created
exclusively before any build, so concurrent coverage invocations cannot share
one output. Failed runs retain their output; output is never reused or broadly
removed.

**Fuzz.** Saved inputs are replayed once as ordinary tests, so a crash found
once stays a test. Longer runs use the same harness: targets are discovered
from Cargo manifest metadata and a named target runs with explicit duration
and memory budgets. Corpus and crash files stay in caller-chosen directories
after a failure. No target is generated or silently skipped. cargo-fuzz needs
unstable sanitizer flags, so the fuzz command alone sets `RUSTC_BOOTSTRAP=1`;
`DEVKIT_FUZZ_RUSTFLAGS` adds experiment-only flags to its instrumentation
(for example
[`-Zexternal-clangrt`](https://doc.rust-lang.org/unstable-book/compiler-flags/external-clangrt.html)
with a matching Clang linker when the compiler bundles no sanitizer runtimes).

**VM scenarios.** `devkit::vm` validates the whole requested set of exported
NixOS scenarios before building, refuses uncommitted source input, retrieves
each scenario's log with `nix log` even for cached builds, writes build output
separately, continues through failures and writes per-scenario results. It
exclusively creates `results.json` before any scenario command, giving the
invocation ownership of its output directory even when concurrent requests
select different scenarios. Logs and partial output remain after failure. A
product supplies its scenario names.

**Development stacks.** `devkit::process` owns a checkout's development
services as process groups, forwards SIGINT and SIGTERM to them and stops
descendants when dropped. It observes child exits without reaping the leader
until the final group signal, so an exited leader's ID cannot be reused during
cleanup. Ordinary `Run` commands stay in the caller's group and receive
terminal signals directly. `development_lock` holds one stack per checkout
through a file lock on a persistent file under the checkout's ignored `target`
directory; the kernel releases it when the process exits. Source-content
hashing (shared with schema hashes) detects additions, removals and edits
while excluding build output. A product supplies service setup, executable
arguments and a real readiness probe.

## What a product supplies

Its `xtask` binary and own checks; schema sources and a schema build for the
`pg` feature; fixture types of its own; bench corpora and metrics; fuzz
targets; VM scenario names; development-stack services and readiness probes.
