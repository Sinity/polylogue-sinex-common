# polylogue-sinex-common

This repository holds the Rust crates and development tooling that Polylogue
and Sinex share. Each product follows `master`, with the exact commit pinned
in its `Cargo.lock`. Code lives here only where the two products' mechanics
are identical; concepts stay in their product, and neither product's concept
is bent to fit the other.

This file explains how work is done here. Claude Code, Codex and any other
agent read it.

## The design of record

Each crate's `README.md` describes what the crate does, and module
documentation (`//!`) describes each module. Planned behaviour lives in Beads,
not in the pages; when a change ships, its page changes in the same commit.
[README.md](README.md) holds the crate table, what is shared and what each
product keeps as its own.

Product behaviour is described in the products' own docs. Read the consumers
in both products before changing a shared interface.

The crate table in [README.md](README.md) says what each crate holds.

## Building and testing

Enter the development shell with `direnv allow` (the `.envrc` loads the
flake) or `nix develop`. The flake reads `rust-toolchain.toml`, the same pin
rustup uses; it also carries nextest, PostgreSQL and `nats-server`.
[Development](docs/development.md) owns local overrides and plain commands.
Pull requests and master run `cargo xtask check` on GitHub Actions; read the
result with `gh pr checks`.

| Command | Does |
|---|---|
| `cargo xtask check` | formatting, clippy with warnings denied, and every test under nextest |
| `cargo xtask test -p <crate> [-E 'test(<name>)']` | selected tests |
| `cargo xtask test --changed` | tests affected by the current diff |
| `cargo xtask status` | what failed in this checkout's last run |
| `cargo xtask history --help` | the recorded run queries |

A host may queue these same commands through its own job runner; the
operator's host does so with AgentCTL, whose descriptor in `.agentctl/`
carries only host tuning. It is optional.

Check changes against both products with `cargo xtask link-common` and each
product's `cargo xtask check`. The
[dependency guide](README.md#depending-on-this-repository) owns local linking
and lock updates; a change reaches each product when its lock update lands.

## How work is done here

**Design first.** Read the crate's page and the consumers in both products
before changing a shared interface. Write the whole change from the design.

**Review your own work before verifying it.** Read the whole diff once
against the task and the design, and against how both products use it, and
list everything wrong in that one pass: a requirement not met, a mechanism
duplicated, a predecessor left in place, a caller not converted in either
product, a limit that refuses valid input, a test that cannot fail, a name
that does not say what the thing is. Fix the whole list, then read only what
the fixes changed. Then verify scoped to the change.

**Git and pull requests.** Branch from `origin/master`, commit with plain
messages whose body says why, stage explicit paths. A pull request is
verified by GitHub Actions; master requires the `check` status on the pull
request's head, branches need not be up to date, so rebase only to resolve a
conflict. Merge with `gh pr merge --auto --squash --match-head-commit <sha>`.
Development speed comes before a perfectly green master (operator,
2026-11-09): whoever's change breaks master fixes it next. PR descriptions are plain prose: what changed and why, the
decisions taken, what was verified and what was not, and what each product
must change to adopt it.

**Decide technical details yourself** from the principles here and record
them on the task and in the PR. The operator decides product scope and intent,
privacy, money (paid APIs, purchased vectors) and anything published outward.
Deploying, activating, restarting, rebuilding an archive and the release
cutover are yours: original inputs stay where their sources keep them and an
old archive is set aside, not deleted. No stored format or archive is deployed
in a sense that needs compatibility or migration; target one current
format.

## Engineering values

These are the operator's standing positions; the product repositories'
`AGENTS.md` files explain each at more length. References in the form
`cc:<session>:<message>` locate the operator's words in his session history.

- **Invariants instead of repair.** Code does not produce broken state;
  there are no repair or verify-and-fix steps, and a crash leaves only states
  the ordinary open path handles.
- **One mechanism per concept; a replacement removes its predecessor,** with
  its callers, docs and tests, in every repository, in the same change. No
  compatibility shims, aliases or old-format readers for the products' own
  formats. "No compatibility shims means things should be done right, not
  that there shoudl be no compatiblity and things coudl break" (2026-09-28,
  `cc:9ec284a7-6a6a-486b-949f-d8bbdb22ae1e:e645e5ee-d29a-402e-bd4a-a58c32ad0c88`).
- **Full features on deep abstractions,** not bare-bones versions to be
  replaced later. "We want powerful abstractions which make downstream
  features possible to have withotu writing tons of code" (2026-10-19,
  `cc:ee52794c-8a9e-43b6-9fbc-9749c7e78e8f:f3730131-5500-451b-aaa7-eb42ce91ce0a`).
- **Plain names everywhere,** local variables included; an opaque term is
  replaced throughout in one pass.
- **Good design instead of guard machinery:** no build-enforced bans, line
  budgets or word checks.
- **Valid input is never refused or truncated for size.**
- **Tests with value.** A test checks behaviour a caller can observe through
  a stable interface, against an expected result known independently of the
  code under test: independent oracles, shared conformance tests, crash tests at real
  durability boundaries, property tests at serialization boundaries. Code
  that was removed is gone because it was deleted, so a test describes what
  exists, never that a name, variant or format stays absent. Assertions are
  exact; a changed expectation follows a changed design page and cites it,
  and an assertion is never weakened to make a run pass. Tests live in their
  own files (`<module>/tests.rs` or the crate's `tests/`).
- **Hardware-independent design.** Devkit and the shared crates target data
  and guarantees, not one machine; host tuning belongs to the host.
  Measurements report hardware-independent figures alongside absolute rates,
  and no test asserts elapsed time.

## Vocabulary

[`docs/vocabulary.md`](docs/vocabulary.md) lists each concept and its one
name. A name says what the thing is in ordinary words; use the listed word,
or add the new concept in the same change.

## Data

The repository is public. Test fixtures are synthetic and neutral, and
nothing names a particular host, path or person.
Public docs state design intent in neutral language and cite rulings by date
and locator, never quoting the operator's private sessions; PR bodies, review
comments and commit messages carry no excerpts of captured data or private
analyses.

## Cloud sessions and changes the products need

In cloud sessions, check for `${CLOUD_PREFIX:-../.cloud}/env.sh` from the
checkout root before Cargo or other development commands. When present,
source it in every command shell:

```sh
. "${CLOUD_PREFIX:-../.cloud}/env.sh"
```

If absent, read [the cloud setup guide](dev/cloud/README.md) before choosing
a backend or installing tools. Preserve a resumed task's branch, commits and
uncommitted work; sourcing initializes tools. Run `cargo xtask` directly;
each run owns its services. Push a branch and open a pull request; do not merge.

Polylogue and Sinex follow this repository's `master`, and their Cargo.lock
pins the commit. To change a crate here together with its callers, check
the product out beside this repository (a cloud session includes both) and
run `cargo xtask link-common` in the product, which builds it against this
checkout. Merge the pull request here first. The `Update product locks`
workflow then opens a pull request in Sinex that moves its lock to the new
master; the callers are converted on that branch. Polylogue follows after
Sinex has adopted the commit, in one pull request that moves its Sinex
revision and its common lock together (README.md, "Depending on this
repository").
