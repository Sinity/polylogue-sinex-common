# polylogue-sinex-common

Rust crates and development tooling shared by
[Polylogue](https://github.com/Sinity/polylogue) and
[Sinex](https://github.com/Sinity/sinex).

| Crate | Holds |
|---|---|
| [`hashed-blobs`](crates/hashed-blobs/README.md) | content-addressed blobs in an append-only segment container, content-defined chunking of rewritten streams, checked reads, the on-demand scan |
| [`canonical-json`](crates/canonical-json/README.md) | the one JSON encoding every identity hash reads: a domain tag, sorted object keys, numbers as spelled |
| [`git-identity`](crates/git-identity/README.md) | which Git repository a directory belongs to: root commits without shallow boundaries, the `origin` remote without credentials |
| [`query-language`](crates/query-language/README.md) | the one grammar and spelling, the syntax tree's meaning, order relations, the fusion of ranked lists, query objects and their hash, the federation planner's types, time expressions, the logical day, cursors, the completer and the shared conformance cases |
| [`command-surface`](crates/command-surface/README.md) | the `Operation` trait, the answer contract, failures with their exit and HTTP table, callers and computed authors, the MCP tool grouping, the seen mark, latency classes and deadlines, server-sent-event framing, the status format |
| [`web-shell`](crates/web-shell/README.md) | the rendering model, visual tokens, the shared key table with card layers, palette, copy, page frame, custom elements, live fragments over one connection per browser, windowed lists, the timeline element and its series, and the seen-mark display rule |
| [`authored`](crates/authored/README.md) | the authored-record header, currency by the descendant rule, stale-base checks, atomic requests, uniqueness slots, order keys |
| [`authored-generator`](crates/authored-generator/README.md) | generated revision and withdrawal histories for both products' conformance tests |
| [`ranking`](crates/ranking/README.md) | the Plackett–Luce fit of judgments with intervals, judge sets, components and warm starts, and next-set selection under three rules |
| [`references`](crates/references/README.md) | the reference grammar (global and local forms), selectors, locators with their versioned canonicalizer and catalog patterns, URL patterns, the route catalog |
| [`signals`](crates/signals/README.md) | the signal rule schema, evaluator, route function and host policy, outcomes, follow, wait and backtest, and desktop notification state |
| [`signal-delivery`](crates/signal-delivery/README.md) | the one task that shows notices on the desktop or sends them to the away command |
| [`devkit`](crates/devkit/README.md), [`devkit-macros`](crates/devkit-macros) | the core of each product's `xtask`: runs and their history, changed-package selection, test databases and NATS, fixtures, bench, coverage, fuzz and VM runners |

Each crate's `README.md` is its documentation. A crate that is not yet in the
workspace is described by its page under `docs/`, which becomes its README
when the crate is created. [`docs/vocabulary.md`](docs/vocabulary.md) names
the shared mechanics.

## What is shared

Code lives here only where the two products' mechanics are identical: storing
bytes, the query language's grammar and meaning, how things are referenced
and addressed, how operations travel and fail, the rules of authored records,
the web front end's shell, signal evaluation, and development runs. Shared
signals also mean one budget for the operator's attention on a host.

Concepts stay in their product even where the words are alike. Polylogue's
assertion kinds, annotations, Pages and measures, Sinex's library precedence
and privacy rules, are different things, and neither product's concept is
bent to fit the other. What both products' authored records share is their
header and rules (currency, stale base, atomic requests, uniqueness, order);
each keeps its own kinds and storage. Each product supplies its own catalog, failure kinds,
budgets, views, rules and occurrence keys to the shared mechanics; each
crate's page says what a product must supply.

The wire between the products stays at the wire level: queries travel as
text, failures as kind codes, references as strings. That is why each product
can take a new commit of this repository on its own schedule.

## Conventions

[Development](docs/development.md) describes the declared Rust toolchain,
development shell and local overrides.

- The operator's rulings are quoted with their date and a locator, following
  [Polylogue's citation convention](https://github.com/Sinity/polylogue/blob/master/docs/README.md).
  Everything unlabelled is the design.

## Depending on this repository

Both products follow this repository's `master` branch; each workspace's
`Cargo.lock` pins the exact commit, so builds are reproducible:

```toml
hashed-blobs = { git = "https://github.com/Sinity/polylogue-sinex-common", branch = "master" }
```

A pull request here shows, beside the required `check`, whether each
product still passes against its head: the `products` workflow checks out
Polylogue's and Sinex's `master`, links each to the pull request's head as
`cargo xtask link-common` does, and runs that product's formatting, Clippy and
tests, both products at once. Its jobs inform and are not required. A job's
summary also gives the product's own check of the same master commit, so a
product that was already failing is not mistaken for this change's fault.

When `master` moves, the `Update product locks` workflow opens or updates a
pull request in Sinex on its `update-polylogue-sinex-common` branch that
moves every workspace lock to the new commit (Sinex's `fuzz/Cargo.lock`
included). Polylogue also pins Sinex by revision, and its one lock must
resolve that Sinex revision's common dependencies and its own to the same
common commit, so Polylogue moves after Sinex: when Sinex's master lock
changes, Sinex's `Update Polylogue` workflow starts `Update product locks`
again, which opens or updates the pull request on Polylogue's
`update-polylogue-sinex-common` branch that moves its Sinex revision to Sinex's
master and its common lock to the commit that master pins. Each product's own
checks decide when to merge. Convert affected callers on the update branch;
later updates start from the branch's current tip and keep its caller commits
and pull request description, and a concurrent push rejects the update until
the next run starts from the new tip. The workflow here needs the repository
secret `COMMON_BUMP_TOKEN` with Contents and Pull requests read/write access to
both products; Sinex's workflow needs a secret of the same name with Actions
read/write access to this repository.

To work on this repository and a product together, check both out side by
side and run `cargo xtask link-common` in the product: it writes a git-ignored
`.cargo/local.toml`, which the product's tracked `.cargo/config.toml` includes
optionally, so the product builds against the local checkout. Common is found
by argument, then `POLYLOGUE_SINEX_COMMON`, then `../polylogue-sinex-common`.
`cargo xtask unlink-common` restores the original workspace locks.
`devkit::local_link::LocalLink` owns patching, lock backups and error rollback;
each product supplies its checkout discovery and workspace paths.
A cross-repository change merges here first, after its `products` jobs show
what each product needs; the callers are then converted on the update
branches the workflow opens.

Tags mark releases only; nothing depends on them.

The repository is public. Test fixtures are synthetic and neutral.

Cloud preparation uses [one shared setup implementation](dev/cloud/README.md)
for this repository and both products, with Nix and an explicit rootless
native backend.
