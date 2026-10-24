# Recorded development history

`cargo xtask history` reads the existing machine-wide SQLite store at `$XDG_STATE_HOME/devkit/history.db`. `runs`, `failures` and `diagnostics` retain their checkout/run queries. The aggregate commands below default to the invoking project's current checkout. Repeat `--checkout /absolute/historical/path` to select several worktrees explicitly; another project's rows remain excluded. Historical checkouts need not still exist. Reports print formatted JSON by default and one JSON value with `--json`.

## Outcomes at the same tree

```sh
cargo xtask history intermittent --checkout /work/project-a --checkout /work/project-b --json
```

A result requires the same binary ID and test name to have recorded `passed` and `failed` outcomes in different runs at equal committed Git tree objects and equal dirty-content hashes. Distinct commits with identical committed trees can therefore share a group. Every group's observed statuses, run IDs, HEADs, checkouts, command/arguments, toolchain and host remain visible. `flaky` means nextest recorded a failure followed by a successful retry; that status alone does not establish this across-run criterion. Ignored, filtered and skipped observations stay visible within qualifying groups. An absent test row supplies no outcome.

New runs record the committed Git tree object separately from their dirty hash. The corrected dirty-content encoding uses a `v2:` prefix, so it is not equated with older unversioned dirty observations. Failed Git commands or unreadable untracked content make observation unknown, rather than clean; such observations cannot supply equal-tree evidence. A checkout that changes or becomes unobservable during a run also invalidates that run's source identity. A tree object plus a dirty hash is recorded source identity, not proof that toolchains, test selections or runtime environments matched. `unknown_tree_observations` counts test rows excluded for unavailable identity.

Legacy runs have no recorded tree object. By default they remain unknown. `--repository /absolute/repository` permits read-only resolution of their recorded HEAD objects in that explicitly selected repository. Resolved groups mark each such observation `resolved legacy HEAD`. This verifies the committed object, not the completeness of the older dirty-content observer. Missing objects and explicitly failed observations remain excluded.

## First recorded failure

```sh
cargo xtask history first-failing suite-id exact-test-name --json
```

This returns the earliest recorded `failed` or retry-`flaky` observation for the exact binary/test identity, ordered by recorded run start time and then run ID. All selected observations remain visible, including passed, ignored, filtered and skipped rows. Dirty changes and failed observation are explicit; an unknown or dirty version is not attributed to a clean revision. No recorded failure returns null. This is a chronological history query, not a bisect result or evidence that a revision caused the failure.

## Duration samples

```sh
cargo xtask history durations check --json
cargo xtask history durations check --stage 'test build' --json
cargo xtask history durations test --args-json '["-p","example"]' --stage test --json
```

Selection matches the exact recorded command and argument array; omitted `--args-json` means `[]`. Stage names are exact. Without `--stage`, samples describe whole invocations. With a stage, each recorded stage occurrence has its position and duration, so build, setup and execution can be inspected separately without adding them twice.

Reports show the project/checkouts, selection, selected run count, sample count, start times, statuses, toolchain, host, dirty identity and each included or excluded observation. Min/median/max use passed runs only, and successful recorded stages when a stage is selected. Failed/running runs, missing stages and missing/invalid durations have explicit exclusion reasons; they are never zero-duration samples. Empty populations return null summaries. These are descriptive samples, with no calibration for current load or cache state and no promise of completion time.

## Compiler messages across worktrees

```sh
cargo xtask history diagnostic-groups --checkout /work/project-a --checkout /work/project-b --level error --json
```

Messages group by their recorded text hash. Each occurrence retains run, checkout, HEAD, dirty hash, command, argument selection, package, level and location. `--level warning` includes warnings and errors; `error` includes errors only. An empty report means no matching messages were recorded, not that compilation succeeded.

## Storage compatibility

The existing `runs` table gains nullable `git_tree` and `observation_error` columns through an additive migration; old rows remain unverified until explicitly resolved. The existing run/test/stage/diagnostic tables and output storage remain the query sources. The connected benchmark recorder also retains an unknown revision when source observation fails, using its existing nullable HEAD column. These commands add no scheduler, performance gate, history database or versioned SQL views. The wider shared-tooling task still owns view contracts, gate replacement and measured affected-test selection.
