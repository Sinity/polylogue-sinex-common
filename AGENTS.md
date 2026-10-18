# polylogue-sinex-common

Rust crates and development tooling that Polylogue and Sinex both use.

| Crate | What it is |
|---|---|
| `hashed-blobs` | Content-addressed file store: plain, delta and sealed blobs with checked range reads |
| `credential-masking` | Streaming credential masking that never refuses input |
| `devkit` | The core of each project's `xtask`: test and check runs, their recorded history, changed-package selection |
| `devkit-macros` | The `#[devkit::test]` attribute |

## Checking

Inside `nix develop`, `cargo xtask check` runs formatting, clippy with warnings
as errors, and every test under nextest. `cargo xtask status` shows what failed
in this checkout's last run; `cargo xtask history --help` lists the queries.

## Releasing

Both projects pin this repository by tag (`vX.Y.Z`), one tag for every crate.
Merge through a PR, tag the merge commit, push the tag, then move each
project's pin with a normal PR in that project. A tag never moves; a fix is a
new version.

The repository is public. Test fixtures are synthetic.
