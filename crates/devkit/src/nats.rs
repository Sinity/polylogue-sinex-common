//! NATS for tests: a throwaway `nats-server` with JetStream, and a subject
//! prefix per test so many tests can share one server.
//!
//! - **Server.** [`Server::start`] runs `nats-server` on a free loopback port
//!   with JetStream stored on tmpfs, owned by the calling process and removed
//!   when dropped. A project's xtask starts one per test run and passes its
//!   URL to the tests through [`Server::test_env`].
//! - **Per test.** [`test_nats`] uses the run's server when the environment
//!   names one, and otherwise starts its own. Each test gets a unique
//!   [`TestNats::prefix`] for its subjects and stream names, so tests sharing a
//!   server never see each other's messages. A test that restarts or kills
//!   NATS starts its own [`Server`] instead.
//!
//! `nats-server` must be on PATH.
use std::{
    env, fs,
    io::{BufRead, BufReader},
    net::{TcpListener, TcpStream},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub type Result<T = ()> = std::result::Result<T, String>;

/// The environment variable that carries a run's server URL to its tests.
pub const URL_VAR: &str = "DEVKIT_NATS_URL";

/// How long a starting server may take to accept clients.
const START_TIMEOUT: Duration = Duration::from_secs(15);

/// Settings for a throwaway server.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Extra server configuration appended to the generated file, such as an
    /// `authorization { users = [...] }` block for credential tests.
    pub config: String,
}

/// A running NATS server reachable on a loopback port.
pub struct Server {
    url: String,
    port: u16,
    owned: Option<Owned>,
}

struct Owned {
    child: Child,
    dir: PathBuf,
}

impl Server {
    /// Starts a throwaway server with JetStream, stopped and removed when the
    /// returned value is dropped.
    pub fn start() -> Result<Self> {
        Self::start_with(&Options::default())
    }

    pub fn start_with(options: &Options) -> Result<Self> {
        // Another process can take the free port between choosing it and the
        // server binding it; a few attempts cover that race.
        let mut last = String::new();
        for _ in 0..3 {
            match Self::try_start(options) {
                Ok(server) => return Ok(server),
                Err(error) => last = error,
            }
        }
        Err(last)
    }

    fn try_start(options: &Options) -> Result<Self> {
        let base = if Path::new("/dev/shm").is_dir() {
            PathBuf::from("/dev/shm")
        } else {
            env::temp_dir()
        };
        // Servers left by killed runs whose watchdog died too.
        crate::owner::sweep(
            &base,
            "devkit-nats",
            r#"pkill -KILL -f "$1/nats.conf"; rm -rf "$1""#,
        );
        let dir = base.join(format!("devkit-nats-{}-{}", std::process::id(), unique()));
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let port = free_port()?;
        let config = dir.join("nats.conf");
        fs::write(
            &config,
            format!(
                "listen: \"127.0.0.1:{port}\"\nhttp: 0\njetstream {{\n  store_dir: \"{}\"\n}}\n{}\n",
                dir.join("js").display(),
                options.config
            ),
        )
        .map_err(|e| format!("{}: {e}", config.display()))?;
        let log = fs::File::create(dir.join("server.log")).map_err(|e| e.to_string())?;
        let child = Command::new("nats-server")
            .arg("-c")
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log)
            // Its own process group, so a test runner interrupt does not race
            // the server's shutdown with ours.
            .process_group(0)
            .spawn()
            .map_err(|e| format!("nats-server: {e}"))?;
        // The server has its own process group, led by itself.
        crate::owner::watch(
            std::process::id(),
            &dir,
            &format!(
                "kill -KILL -- -{}; rm -rf {}",
                child.id(),
                crate::owner::shell_quote(&dir.display().to_string())
            ),
        );
        let server = Self {
            url: format!("nats://127.0.0.1:{port}"),
            port,
            owned: Some(Owned { child, dir }),
        };
        server.wait_ready()?;
        Ok(server)
    }

    /// Waits until the server logs that it is ready and answers a client
    /// with its INFO line.
    fn wait_ready(&self) -> Result {
        let owned = self
            .owned
            .as_ref()
            .expect("only an owned server is started");
        let log = owned.dir.join("server.log");
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            let text = fs::read_to_string(&log).unwrap_or_default();
            if text.contains("Server is ready") && self.ready() {
                return Ok(());
            }
            if text.contains("[FTL]") || Instant::now() > deadline {
                return Err(format!(
                    "nats-server did not become ready on port {}:\n{}",
                    self.port,
                    text.trim()
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether the server answers a client connection with its INFO line.
    pub fn ready(&self) -> bool {
        let Ok(stream) = TcpStream::connect(("127.0.0.1", self.port)) else {
            return false;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).is_ok() && line.starts_with("INFO ")
    }

    /// `nats://127.0.0.1:<port>`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Environment that lets tests in a child process use this server through
    /// [`test_nats`].
    pub fn test_env(&self) -> Vec<(String, String)> {
        vec![(URL_VAR.into(), self.url.clone())]
    }

    /// The server's log, for a failing test's report.
    pub fn log(&self) -> Option<String> {
        let owned = self.owned.as_ref()?;
        fs::read_to_string(owned.dir.join("server.log")).ok()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(owned) = &mut self.owned {
            let _ = owned.child.kill();
            let _ = owned.child.wait();
            let _ = fs::remove_dir_all(&owned.dir);
        }
    }
}

/// NATS for one test: the run's shared server, or a server of its own when the
/// test runs outside xtask.
pub struct TestNats {
    url: String,
    prefix: String,
    _own: Option<Server>,
}

pub fn test_nats() -> Result<TestNats> {
    let prefix = format!("t{}x{:x}", std::process::id(), unique());
    match env::var(URL_VAR) {
        Ok(url) if !url.is_empty() => Ok(TestNats {
            url,
            prefix,
            _own: None,
        }),
        _ => {
            let server = Server::start()?;
            Ok(TestNats {
                url: server.url().to_owned(),
                prefix,
                _own: Some(server),
            })
        }
    }
}

/// `#[devkit::test] fn name(bus: TestNats)` gives the test its own subject prefix.
impl crate::Fixture for TestNats {
    fn create() -> Result<Self> {
        test_nats()
    }
}

impl TestNats {
    pub fn url(&self) -> &str {
        &self.url
    }

    /// A subject token unique to this test, usable as a subject prefix and,
    /// upper-cased, in stream and consumer names.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// `<prefix>.<name>`.
    pub fn subject(&self, name: &str) -> String {
        format!("{}.{name}", self.prefix)
    }

    /// `<PREFIX>_<NAME>`: a stream or consumer name unique to this test.
    pub fn name(&self, name: &str) -> String {
        format!("{}_{name}", self.prefix).to_ascii_uppercase()
    }
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("free port: {e}"))?;
    listener
        .local_addr()
        .map(|a| a.port())
        .map_err(|e| format!("free port: {e}"))
}

fn unique() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::from(d.subsec_nanos()));
    nanos ^ (COUNTER.fetch_add(1, Ordering::Relaxed) << 32)
}

#[cfg(test)]
mod tests;
