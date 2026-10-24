# Queries across recorded runs

`cargo xtask history` reads the machine-wide store at `$XDG_STATE_HOME/devkit/history.db`. `runs`, `failures` and `diagnostics` read one checkout's runs. The commands below read the current checkout by default; repeat `--checkout /absolute/path` to read several worktrees of the same project, including ones that no longer exist. Other projects' runs are never included. Output is formatted JSON, or one line with `--json`.

Each run records its HEAD, the Git tree object of that commit, and a hash of its uncommitted changes (tracked diffs and untracked files). If Git fails, an untracked file cannot be read, or the checkout changes during the run, the run records why in `observation_error` and has no tree.

## Intermittent tests

```sh
cargo xtask history intermittent --checkout /work/project-a --checkout /work/project-b
```

Lists tests that passed in one run and failed in another at the same Git tree and the same uncommitted-changes hash. Different commits with the same tree group together. Each group lists every run with its statuses, checkout, command, toolchain and host. A `flaky` status (failed, then passed on retry) is shown but does not count as a pass or a failure. `results_without_tree` counts the test results left out because their run has no tree.

## First failure

```sh
cargo xtask history first-failing suite-id exact-test-name
```

Lists every recorded result of one test in run start order, and the first that failed or was flaky. `revision` says whether that run was at a clean HEAD, had uncommitted changes, or is unknown. This is the order runs happened in, not a bisection.

## Durations

```sh
cargo xtask history durations check
cargo xtask history durations check --stage 'test build'
cargo xtask history durations test --args-json '["-p","example"]' --stage test
```

Selects runs of the command with exactly these arguments (default: none). The minimum, median and maximum cover passed runs, or the successful stage when `--stage` is given. Every selected run is listed; one left out of the summary says why (`run not passed`, `stage not recorded`, `stage failed`, `duration not recorded`). Load and cache state at the time are not recorded.

## Compiler messages across worktrees

```sh
cargo xtask history diagnostic-groups --checkout /work/project-a --checkout /work/project-b --level error
```

Groups recorded compiler messages by text. Each occurrence lists its run, checkout, HEAD, command, package and location. `--level warning` includes warnings and errors.
