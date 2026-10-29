# Cloud development environments

All three repositories use this directory's setup implementation. Include
`polylogue-sinex-common` in a product environment; Polylogue also needs Sinex
for its Git dependencies. The product's `dev/cloud/setup.sh` delegates here.
Run setup from the checkout you want to prepare:

```sh
bash dev/cloud/setup.sh
```

The default `CLOUD_BACKEND=nix` uses the repository's development shell. If
Nix is absent, installation requires a privileged Debian/Ubuntu setup phase.
An existing Nix installation can be used by an ordinary user. Local
`nix develop` remains the normal development route.

For a restricted x86_64 Linux cloud runtime with a read-only root filesystem,
select the native backend explicitly:

```sh
CLOUD_BACKEND=native bash dev/cloud/setup.sh
. ../.cloud/env.sh
```

The base image must provide Bash, Python 3.12 or newer, C and C++ compilers, Make,
and pkg-config. The native backend downloads the SHA256-pinned artifacts in
`pins.json`: Rust with rustfmt and Clippy, nextest, CMake, PostgreSQL 18,
TimescaleDB and NATS. The bootstrap also pins M4, Bison and Flex, which
PostgreSQL 18 requires even when building its release archives. Rust matches the product's `rust-toolchain.toml`.
PostgreSQL and TimescaleDB build into the user's prefix. The toolchain does
not need root, a writable `/nix`, user namespaces or PRoot.

`CLOUD_PREFIX` selects the absolute writable prefix; its default is `.cloud`
beside the checkout. Downloads and build trees stay there for repeat setup.
`CLOUD_BUILD_JOBS` controls native compilation concurrency (default 2).
Every native setup checks the installed tools, starts a throwaway PostgreSQL
cluster, creates and queries a TimescaleDB hypertable, and checks a throwaway
NATS JetStream server. Both services stop before setup returns. The native
PostgreSQL is for local test sockets and omits ICU, Readline, zlib and TLS;
it is not a production database installation.

Setup fetches each existing Cargo workspace with `--locked`. It prepares the
committed dependency graph, without auto-linking a sibling common checkout.
Use `cargo xtask link-common` explicitly when changing shared crates; that
operation owns its patch and lock changes.

Configure `BASH_ENV` to the printed absolute `env.sh` path so later Bash
commands load the prepared tools, and have the environment's start
instructions source it explicitly. The privileged Nix backend also keeps
`/etc/devenv.sh` for existing profiles. `CLOUD_PREBUILD=1` builds all test
binaries after fetching; the default leaves builds to the requested task.
Run `cargo xtask` commands directly in cloud sessions. Each test run owns its
throwaway services; no operator host services are required.

In current Codex Cloud, finish and test setup in the active environment,
then save and Publish. Publication snapshots the prepared filesystem; it
does not provide a privileged installer phase. Start a fresh task from the
published environment to verify reuse. See the [current product's lifecycle
documentation](https://learn.chatgpt.com/docs/environments/cloud-environments).
NixOS VM tests and operator-data benchmarks need their corresponding host
capabilities and data.
