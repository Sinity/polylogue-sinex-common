# polylogue-sinex-common

See [README.md](README.md) for the crates and contributor entry points.

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

Follow the [release procedure](README.md#releasing).

The repository is public. Test fixtures are synthetic.

## Cloud sessions and changes the products need

Cloud sessions (Claude Code on the web, Codex cloud) run as root on a fresh
VM without AgentCTL or the operator's services; run `cargo xtask check` and
`cargo xtask test` there directly. The environment's setup script runs
`bash dev/cloud/setup.sh`, after which every shell is in the development
shell. devkit's throwaway PostgreSQL runs as `nobody` when started by root,
so the `pg` tests run there too. Push a branch and open a pull request; do
not merge.

Polylogue and Sinex follow this repository's `master`, and their Cargo.lock
pins the commit. To change a crate here together with its callers, check
the product out beside this repository (a cloud session includes both) and
run `cargo xtask link-common` in the product, which builds it against this
checkout. Merge the pull request here first. The `Update product locks`
workflow then opens a pull request in each product that moves its lock to
the new master; the callers are converted on that branch, or on a branch
that runs the same `cargo update`.
