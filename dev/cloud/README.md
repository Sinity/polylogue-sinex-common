# Cloud development environments

All three repositories use this directory's setup implementation. Include
`polylogue-sinex-common` in a product environment; Polylogue also needs Sinex
for its Git dependencies. The product's `dev/cloud/setup.sh` delegates here.
Run setup from the checkout you want to prepare:

```sh
bash dev/cloud/setup.sh
```

Command sandbox permissions and environment capabilities are separate.
Dependency downloads may require the command tool's normal reviewed approval
path, while retaining the inherited proxy and configured host allowlist.
Diagnose network and namespace errors in that command's effective sandbox
context. A proxy host-policy denial still requires the environment's normal
review/save path for the required hosts before retrying.

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
pkg-config, Perl with its standard generator modules, Node.js 24 or newer and
`dbus-daemon`. Signals tests create their own private D-Bus session; they do
not need D-Bus development headers or an operator session bus.
The native backend downloads the SHA256-pinned artifacts in
`pins.json`: Rust with rustfmt and Clippy, nextest, CMake, TypeScript 7.0.2,
esbuild 0.27.2, PostgreSQL 18, TimescaleDB and NATS. The bootstrap also pins M4, Bison and Flex, which
PostgreSQL 18 requires even when building its release archives. Rust matches the product's `rust-toolchain.toml`.
PostgreSQL and TimescaleDB build into the user's prefix. The toolchain does
not need root, a writable `/nix`, user namespaces or PRoot.

On Debian 13 amd64, `CLOUD_NATIVE_BOOTSTRAP=debian13` selects the official
Debian generator packages pinned in `pins.json` and the official PostgreSQL
GitHub release tag archive instead of the GNU/PostgreSQL FTP artifacts. This
route needs `dpkg-deb` in the base image. For example:

```sh
CLOUD_BACKEND=native CLOUD_NATIVE_BOOTSTRAP=debian13 bash dev/cloud/setup.sh
```

It downloads exact package artifacts,
checks their SHA256 from the signed Debian index, and extracts files into the
owned prefix; it runs no package scripts or system installation. Bison's data
directory and M4 path are exported for the relocated tools. The default
`CLOUD_NATIVE_BOOTSTRAP=source` compiles the generator release archives.
Both routes use the same PostgreSQL/Timescale/NATS smoke checks.

`CLOUD_PREFIX` selects the absolute writable prefix; its default is `.cloud`
beside the checkout. Downloads and build trees stay there for repeat setup.
`CLOUD_BUILD_JOBS` controls native compilation concurrency (default 2).
The frontend tools use the upstream npm tarballs directly, without npm install
scripts or changing workspace manifests. Their versions match the shared
development shell so the committed web-shell bundle remains reproducible.
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
