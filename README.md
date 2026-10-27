# polylogue-sinex-common

Rust crates and development tooling shared by
[Polylogue](https://github.com/Sinity/polylogue) and
[Sinex](https://github.com/Sinity/sinex).

| Crate | What it is |
|---|---|
| [`hashed-blobs`](crates/hashed-blobs) | Content-addressed file store: plain, delta and sealed blobs with checked range reads |
| [`credential-masking`](crates/credential-masking) | Streaming credential masking that never refuses input ([behaviour](docs/credential-masking.md)) |
| [`devkit`](crates/devkit/README.md) | The core of each project's `xtask`: test and check runs, their recorded history, changed-package selection, test databases and NATS |
| [`devkit-macros`](crates/devkit-macros) | The `#[devkit::test]` attribute |

Projects depend on a tag:

```toml
hashed-blobs = { git = "https://github.com/Sinity/polylogue-sinex-common", tag = "v0.7.3" }
```

`hashed-blobs` owns blob storage; `credential-masking` owns display masking and credential scanning. Neither depends on devkit. `devkit` owns development runs and fixtures and uses `devkit-macros` for its test attribute.

The [devkit guide](crates/devkit/README.md) describes its APIs and fixtures; the [history guide](docs/history.md) describes run queries.

## Development runs

Run checks through the repository's declared AgentCTL operations:

```sh
agentctl job start --project /path/to/checkout check
agentctl job start --project /path/to/checkout focused -- -p devkit --all-features
agentctl job start --project /path/to/checkout compile -- -p devkit --all-features
```

An explicit `CARGO_TARGET_DIR` in the caller's environment is passed to the job.

## Releasing

Both projects pin this repository by tag (`vX.Y.Z`), one tag for every crate.
Merge through a PR, tag the merge commit, push the tag, then move each
project's pin with a normal PR in that project. A tag never moves; a fix is a
new version.

The repository is public. Test fixtures are synthetic.
