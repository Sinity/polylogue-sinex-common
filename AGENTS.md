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
