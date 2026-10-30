# polylogue-sinex-common

This repository holds the Rust crates and development tooling that Polylogue
and Sinex share. Each product follows `master`, with the exact commit pinned
in its `Cargo.lock`. Code lives here only where the two products' mechanics
are identical; concepts stay in their product, and neither product's concept
is bent to fit the other.

This file explains how work is done here. Claude Code, Codex and any other
agent read it.

## The design of record

The design of this repository is its documentation: each crate's
`README.md`, and for a crate not yet in the workspace its page under `docs/`
(which becomes the crate's README when the crate is created).
[README.md](README.md) holds the crate table, what is shared and what each
product keeps as its own. Each page describes the design, not the current
code; module documentation (`//!`) describes each module as it is, and the
difference between the two is work tracked outside the repository.

Product behaviour is described in the products' own docs. Read the consumers
in both products before changing a shared interface.

- [`hashed-blobs`](crates/hashed-blobs/README.md): blobs in append-only
  segments; one sync per group; extents committed with the rows that cite
  them; a segment opens at its last committed extent, so a crash never needs a
  repair step.
- [`query-language`](crates/query-language/README.md): one surface language, two
  catalogs and two compilers; the language owns the laws that are properties
  of the language and runs them against both catalogs.
- [`command-surface`](crates/command-surface/README.md): one operation table per
  product, from which every transport is generated; one failure table.
- [`web-shell`](crates/web-shell/README.md): server-rendered HTML from typed answers,
  one TypeScript shell, one visual language.
- [`signals`](crates/signals/README.md): one rule schema and evaluator, one interrupt
  budget and pure delivery checkpoint types.
- [`signal-delivery`](crates/signal-delivery/README.md): desktop and command I/O
  over the shared signal checkpoint.
- [`devkit`](crates/devkit/README.md): the core of each product's `xtask`. It
  starts what a run needs and stops it on drop, and names its interfaces
  neutrally (`DEVKIT_JOB_ID`, `DEVKIT_SCRATCH`) so that any job runner can
  set them; nothing in it knows a particular runner or host.

## Building and testing

Enter the development shell with `direnv allow` (the `.envrc` loads the
flake) or `nix develop`. The flake reads `rust-toolchain.toml`, the same pin
rustup uses; it also carries nextest, PostgreSQL and `nats-server`.
[Development](docs/development.md) owns local overrides and plain commands.

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

**Review your own work before verifying it.** Walk through every changed file
and function and narrate, in your notes, what it does, checked against the
task and the design, and against how both products use it. Look for
everything that is wrong at once: a requirement not met, a mechanism
duplicated, a predecessor left in place, a caller not converted in either
product, a limit that refuses valid input, a test that cannot fail, a name
that does not say what the thing is. Fix all of it, narrate again, and repeat
until a full pass finds nothing. Then verify scoped to the change; the full
check runs once on the tree that lands.

**Git and pull requests.** Branch, commit with plain messages whose body says
why, stage explicit paths, rebase onto `origin/master` before the final
verification. PR descriptions are plain prose: what changed and why, the
decisions taken, what was verified and what was not, and what each product
must change to adopt it.

**Decide technical details yourself** from the principles here and record
them on the task and in the PR. The operator decides product scope, privacy,
money, irreversible actions on real data and anything published outward.

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
- **Tests with value:** independent oracles, law suites, crash tests at real
  durability boundaries, property tests at serialization boundaries. Tests
  live in their own files (`<module>/tests.rs` or the crate's `tests/`).
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
workflow then opens a pull request in each product that moves its lock to
the new master; the callers are converted on that branch, or on a branch
that runs the same `cargo update`.
