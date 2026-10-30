# polylogue-sinex-common

Rust crates and development tooling shared by
[Polylogue](https://github.com/Sinity/polylogue) and
[Sinex](https://github.com/Sinity/sinex).

| Crate | Holds |
|---|---|
| [`hashed-blobs`](crates/hashed-blobs/README.md) | content-addressed blobs in an append-only segment container, checked reads, PAX member access, the on-demand scan |
| [`query-language`](crates/query-language/README.md) | the shared grammar and spelling, the syntax tree's meaning, time expressions, the logical day, cursors, the completer and the shared law suite |
| [`command-surface`](crates/command-surface/README.md) | the `Operation` trait, failures with their exit and HTTP table, request and answer envelopes, latency classes and deadlines, server-sent-event framing, the status figure format |
| [`web-shell`](crates/web-shell/README.md) | the rendering model, visual tokens, the shared key table, palette, copy, page frame, live fragments and windowed lists |
| [`signals`](crates/signals/README.md) | the pure signal rule schema, evaluator, host policy and delivery checkpoint types |
| [`signal-delivery`](crates/signal-delivery/README.md) | the desktop and command worker over the pure signal checkpoint |
| [`devkit`](crates/devkit/README.md), [`devkit-macros`](crates/devkit-macros) | the core of each product's `xtask`: runs and their history, changed-package selection, test databases and NATS, fixtures, bench, coverage, fuzz and VM runners |

Each crate's `README.md` is its documentation. A crate that is not yet in the
workspace is described by its page under `docs/`, which becomes its README
when the crate is created. [`docs/vocabulary.md`](docs/vocabulary.md) names
the shared mechanics.

## What is shared

Code lives here only where the two products' mechanics are identical: storing
bytes, the query language's grammar and meaning, how operations travel and
fail, the web front end's shell, signal evaluation, and development runs.
Shared signals also mean one budget for the operator's attention on a host.

Concepts stay in their product even where the words are alike. Polylogue's
authored records and their currency, annotations, Pages and measures, and
Sinex's privacy rules, are different things, and neither product's concept is
bent to fit the other. Each product supplies its own catalog, failure kinds,
budgets, views, rules and occurrence keys to the shared mechanics; each
crate's page says what a product must supply.

The wire between the products stays at the wire level: queries travel as
text, failures as kind codes, references as strings. That is why each product
can take a new commit of this repository on its own schedule.

## Conventions

[Development](docs/development.md) describes the declared Rust toolchain,
development shell and local overrides.

- Every crate page ends with `## Laws`: one bullet per law, a stable key in
  bold, then the property and its oracle. A test that checks a law carries the
  key in its name (`cursor_refusals` for `cursor-refusals`), and
  `cargo xtask laws` reports each law's tests and the laws without one.
- The operator's rulings are quoted with their date and a locator, following
  [Polylogue's citation convention](https://github.com/Sinity/polylogue/blob/master/docs/README.md).
  Everything unlabelled is the design.

## Depending on this repository

Both products follow this repository's `master` branch; each workspace's
`Cargo.lock` pins the exact commit, so builds are reproducible:

```toml
hashed-blobs = { git = "https://github.com/Sinity/polylogue-sinex-common", branch = "master" }
```

When `master` moves, a workflow here opens a pull request in each product that
updates all its workspace locks to the new commit. The product's own checks
decide when to take it. The workflow needs the repository secret `COMMON_BUMP_TOKEN`,
with Contents and Pull requests read/write access to both products. Without
it, update their locks with `cargo update` selecting the shared crates
that product adopts and open those pull requests manually. Convert affected
callers and run the product's checks before merging its lock update.
Sinex also has `fuzz/Cargo.lock`: update it with
`cargo update --manifest-path fuzz/Cargo.toml -p devkit -p hashed-blobs`.
Use `--precise <commit>` to select the same revision in both locks.

To work on this repository and a product together, check both out side by
side and run `cargo xtask link-common` in the product: it writes a git-ignored
`.cargo/local.toml`, which the product's tracked `.cargo/config.toml` includes
optionally, so the product builds against the local checkout. Common is found
by argument, then `POLYLOGUE_SINEX_COMMON`, then `../polylogue-sinex-common`.
`cargo xtask unlink-common` restores the original workspace locks.
`devkit::local_link::LocalLink` owns patching, lock backups and error rollback;
each product supplies its checkout discovery and workspace paths.
A cross-repository change is two pull requests: merge the change here, then take the lock update
in the product, converting every caller in that pull request.

Tags mark releases only; nothing depends on them.

The repository is public. Test fixtures are synthetic and neutral.

Cloud preparation uses [one shared setup implementation](dev/cloud/README.md)
for this repository and both products, with Nix and an explicit rootless
native backend.
