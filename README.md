# polylogue-sinex-common

Rust crates and development tooling shared by
[Polylogue](https://github.com/Sinity/polylogue) and
[Sinex](https://github.com/Sinity/sinex).

| Crate | What it is |
|---|---|
| [`hashed-blobs`](crates/hashed-blobs) | Content-addressed file store: plain, delta and sealed blobs with checked range reads |
| [`credential-masking`](crates/credential-masking) | Streaming credential masking that never refuses input ([behaviour](docs/credential-masking.md)) |
| [`devkit`](crates/devkit) | The core of each project's `xtask`: test and check runs, their recorded history, changed-package selection |
| [`devkit-macros`](crates/devkit-macros) | The `#[devkit::test]` attribute |

Projects depend on a tag:

```toml
hashed-blobs = { git = "https://github.com/Sinity/polylogue-sinex-common", tag = "v0.1.0" }
```

## devkit

A project's `xtask` starts a `devkit::Run`, adds its steps (`cargo` commands
with captured diagnostics, a nextest run with each test's outcome, its own
checks) and finishes it. Every run is recorded in one machine-wide SQLite
store at `$XDG_STATE_HOME/devkit/history.db`:

- `xtask status`: failing tests and compiler errors from this checkout's last run;
- `xtask history runs | failures [RUN] | diagnostics [RUN] [--level error]`, each with `--json`;
- `xtask changed`: the nextest filterset for packages changed since the merge base.

Passing tests keep no output and each diagnostic text is stored once, so the
store stays small without deleting anything.

With the `pg` feature, devkit also provides PostgreSQL for projects with
compile-time-checked SQL. The server is the per-user service on the socket in
`$XDG_RUNTIME_DIR/devkit/pg` (Sinnix's `devkit-pg`), or a throwaway cluster on
tmpfs when it is not running. Each schema version, named by a hash of the
schema sources, is built once into a template database and copied to the
read-only database `DATABASE_URL` points at; every test clones the template.
`xtask db ensure|url|psql|exec|drop-unused|ready` drives it.
