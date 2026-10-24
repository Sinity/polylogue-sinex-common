# polylogue-sinex-common

Rust crates and development tooling that Polylogue and Sinex both use.

| Crate | What it is |
|---|---|
| `hashed-blobs` | Content-addressed file store: plain, delta and sealed blobs with checked range reads |
| `credential-masking` | Streaming credential masking that never refuses input |
| `devkit` | The core of each project's `xtask`: test and check runs, their recorded history, changed-package selection |
| `devkit-macros` | The `#[devkit::test]` attribute and its fixture parameters |

## Names

Names follow [`docs/vocabulary.md`](docs/vocabulary.md): a name says what the
thing is in ordinary words, and each concept has one name. This holds for
identifiers, local variables, comments, docs, error text and tests.

## Checking

`cargo xtask check` runs formatting, clippy with warnings as errors, and every
test under nextest. Run it, and every other build or test, as an AgentCTL job:
`agentctl job start --project <checkout> check`, or `focused`, `compile` and
`exec` with arguments (`.agentctl/project.toml`). `cargo xtask status` shows what failed
in this checkout's last run; `cargo xtask history --help` lists the queries.

## Releasing

Both projects pin this repository by tag (`vX.Y.Z`), one tag for every crate.
Merge through a PR, tag the merge commit, push the tag, then move each
project's pin with a normal PR in that project. A tag never moves; a fix is a
new version.

The repository is public. Test fixtures are synthetic.
