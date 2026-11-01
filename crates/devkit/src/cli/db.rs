//! `xtask db …`, for projects that use the `pg` feature.
use crate::pg::{ClusterOptions, Reap, Schema, SchemaDatabases, Server};
use std::process::{Command as Process, ExitCode};

#[derive(Debug, clap::Subcommand)]
pub enum Db {
    /// Build this schema version's databases if needed; print its `DATABASE_URL`.
    Ensure,
    /// Print this schema version's `DATABASE_URL` without building anything.
    Url,
    /// Open psql on this schema version's database, or on the named one,
    /// such as a kept failed test database.
    Psql { database: Option<String> },
    /// Run a command with `DATABASE_URL`, `PGHOST` and `PGPORT` set; without
    /// the per-user service, a throwaway server lives as long as the command.
    Exec {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    /// Drop schema versions nothing uses: those beyond the newest `--keep`
    /// per project or unused for `--days`, never one a running command holds
    /// or anything is connected to.
    DropUnused {
        #[arg(long, default_value_t = Reap::default().days)]
        days: u64,
        #[arg(long, default_value_t = Reap::default().schemas)]
        keep: usize,
    },
    /// Exit 0 if the per-user service accepts connections.
    Ready,
}

/// Runs a `db` command. `schema` describes the project's current schema;
/// `options` configure a throwaway server when the service is not running.
pub fn execute(command: Db, options: &ClusterOptions, schema: &Schema<'_>) -> ExitCode {
    match run(command, options, schema) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn service() -> Result<Server, String> {
    Server::service().ok_or_else(|| {
        format!(
            "the per-user PostgreSQL service is not running (socket {})",
            Server::service_socket().map_or("unknown".into(), |p| p.display().to_string())
        )
    })
}

fn run(command: Db, options: &ClusterOptions, schema: &Schema<'_>) -> Result<(), String> {
    match command {
        Db::Ready => service().map(drop),
        Db::Url => {
            let socket = Server::service_socket().ok_or("XDG_RUNTIME_DIR is not set")?;
            let names = SchemaDatabases::new(schema.project, &schema.hash)?;
            println!("postgresql:///{}?host={}", names.schema, socket.display());
            Ok(())
        }
        Db::Ensure => {
            let server = service()?;
            let names = server.ensure_schema(schema)?;
            println!("{}", server.url(&names.schema, None));
            Ok(())
        }
        Db::Psql { database } => {
            let server = service()?;
            let database = match database {
                Some(database) => database,
                None => server.ensure_schema(schema)?.schema,
            };
            exit(Process::new("psql").arg(server.url(&database, None)))
        }
        Db::Exec { command } => {
            let server = Server::service_or_start(options)?;
            let names = server.ensure_schema(schema)?;
            let (program, args) = command.split_first().ok_or("exec needs a command")?;
            exit(
                Process::new(program)
                    .args(args)
                    .env("DATABASE_URL", server.url(&names.schema, None))
                    .envs(server.pg_env())
                    .envs(server.test_env(&names.template)),
            )
        }
        Db::DropUnused { days, keep } => {
            let policy = Reap {
                schemas: keep,
                days,
            };
            for name in service()?.reap(&policy)? {
                println!("dropped {name}");
            }
            Ok(())
        }
    }
}

fn exit(command: &mut Process) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command.status().map_err(|e| format!("{program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}
