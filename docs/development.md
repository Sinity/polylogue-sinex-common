# Development

`rust-toolchain.toml` selects Rust and its components for rustup and the Nix
shell. The flake reads that file through its pinned rust-overlay input.
Changing the compiler version means changing this file in common and both
products together.

```sh
nix develop
cargo xtask check
cargo xtask test -p devkit --all-features
```

`direnv allow` loads the same shell through `.envrc`. Its ignored
`.envrc.local` is the place for personal build-output placement, job counts
and compiler wrappers. Cargo's default output directory is `target/`; a
caller may set `CARGO_TARGET_DIR` explicitly.

Declared AgentCTL operations clear an ambient `CARGO_TARGET_DIR` before entering
the actual workspace's shell. The shell selects that checkout's isolated host
target when `sinnix-cargo-target` is available, with dependency artifacts seeded
by the host helper. For an explicit target experiment, pass
`exec -- env CARGO_TARGET_DIR=... cargo ...`; this assignment runs after shell
selection. Keep the experimental target exclusive to the same checkout.

`nix flake check` builds sealed formatting, Clippy, nextest and browser-shell
checks. Clippy and nextest reuse one Crane dependency artifact build with the
same Rust toolchain and all workspace features. The source includes fixture
JSON and browser assets as well as Rust files.

The nextest check excludes devkit's `pg::` and `nats::` service test groups
and the two GNUtar extraction oracles that call filesystem ACL/xattr syscalls.
The sealed builder rejects those syscalls. The PAX subset is five synthetic
parser, metadata and content tests in the sealed check, plus two GNUtar
filesystem oracles in the native check. `cargo xtask check` also runs the two
filesystem oracles and service groups, retaining every native assertion. Database service tools stay in the shell:
PostgreSQL 18 with TimescaleDB and pgvector, and NATS. DBus and the frontend
tools support the existing notification and browser checks.

The shell installs this checkout's Git hooks when `core.hooksPath` is unset.
An explicitly configured hook path is preserved. The hooks check staged
whitespace and staged Rust formatting before a commit, with the loaded shell's
tools when available or by entering the declared shell. There is no push hook:
Clippy and tests remain in the native full check, which GitHub Actions runs for
every pull request. Timezone tests use the shell's declared tzdata,
and notification tests start their own configured private D-Bus service.

`nix fmt` uses nixfmt. The shell also provides cargo-llvm-cov, cargo-fuzz,
cargo-deny, mold, sqlite and jq. Coverage uses the LLVM tools from the declared
Rust toolchain.

A host may queue these commands through its own job runner.

For a change spanning common and a product, use the explicit
[local linking command](../README.md#depending-on-this-repository).

The common `cargo xtask test` command enables all features of the selected
packages by default, including focused runs such as `-p query-language`, so
the conformance tests and generators stay in scope. Explicit `--features`, `-F`,
`--all-features` or `--no-default-features` options are passed through unchanged
and suppress that default. `cargo xtask check` retains the complete feature
suite and separately compiles runtime and fixture feature combinations.

## Properties tested

These properties hold for every input and are checked by generated or exhaustive tests. Each crate's README describes the behaviour; the tests named here are where to look when one of these changes.

- Query filters compose: `Q | where A | where B` equals `Q | where A AND B` (`query-language`, `filter_composition`).
- An unknown value satisfies neither a condition nor its negation (`filter_three_valued`).
- Formatting a parsed question parses back to the same question, in both products' spellings (`roundtrip`, `polylogue_forms_roundtrip`, `sinex_forms_roundtrip`).
- Spellings with one normal form share one identity (`duration_gap_spellings_and_catalog_unknown_words_share_identity`, `associative_and_idempotent_conditions_share_identity`).
- Fused rankings equal an independent oracle, and reading only as deep as needed gives the exact answer (`rank_fusion_oracle`, `rank_early_stop_exact`).
- The current revision of authored records is a pure function of their revision links (`authored`, `currency_pure_function`, `currency_middle_withdrawal`).
- Order keys stay distinct and ordered under any sequence of insertions (`order_dense`), and a repeated request is recognised as a repeat (`request_idempotent`).
- Chunk boundaries depend only on content, and a local edit changes only nearby chunks (`hashed-blobs`, `cut_points_depend_only_on_content`, `an_edit_changes_only_nearby_chunks`).
- A crash at append, sync, commit or truncate leaves only a tail the next open removes (`crash_leaves_only_a_tail_at_append_sync_commit_and_truncate`).
- References and selectors print and parse back unchanged, and canonical locators are stable (`references`, `round_trip`, `selector_round_trip`, `locator_idempotent`).
- A notice that could not be sent is retried once under the same identity, and an unanswered one is never resent (`signals`, `not_sent_retried_once_unknown_never_and_restart_keeps_identity`).
