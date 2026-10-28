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

Products follow `master`; each product's `Cargo.lock` pins the exact commit:

```toml
hashed-blobs = { git = "https://github.com/Sinity/polylogue-sinex-common", branch = "master" }
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

Merge through a PR. The `Update product locks` workflow then opens or updates
a pull request in each product that moves its lock to the new `master` commit.
It needs the repository secret `COMMON_BUMP_TOKEN`, with Contents and Pull
requests read/write access to both products. Without it, update the product
locks with `cargo update -p devkit -p hashed-blobs -p credential-masking` and
open those pull requests manually. Convert affected callers and run the
product's checks before merging its lock update. Tags are optional release
markers; the products do not depend on them.

For a change shared with a product, check the repositories out side by side
and run `cargo xtask link-common` in the product. Its git-ignored
`.cargo/local.toml` points to this checkout, and `cargo xtask unlink-common`
restores the product's original lock. Merge the common change first, then
take its exact commit in the product's lock update.

The repository is public. Test fixtures are synthetic.
