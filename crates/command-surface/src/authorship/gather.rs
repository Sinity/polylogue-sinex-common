//! Gathering the caller's facts from the process tree (Linux `/proc`).
//!
//! A harness is recognised by the variables it sets in the processes it
//! starts and by its executable. An ancestor is a harness when its
//! executable names one, or when its own environment lacks a harness's
//! marker variable that the processes below it carry: it set that marker in
//! the processes it started. The nearest harness ancestor of the caller is
//! the one that launched it, so it wins over variables a launcher forwarded
//! from an outer harness. No job runner is recognised by name.
use super::{AgentSession, CallerEvidence, Harness};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// The variables gathering reads: each harness's session and marker
/// variables. Nothing else from the caller's environment is kept.
pub const HARNESS_VARIABLES: [&str; 7] = [
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
    "CLAUDECODE",
    "CODEX_THREAD_ID",
    "CODEX_TOOL_CALL_ID",
    "HERMES_SESSION_ID",
    "GEMINI_CLI",
];

/// What each harness sets: its name, the variable holding its session, the
/// variable whose presence marks it, the variable holding its process ID,
/// and the variable holding the current tool call.
struct HarnessVariables {
    harness: &'static str,
    session: Option<&'static str>,
    marker: &'static str,
    marker_value: Option<&'static str>,
    pid: Option<&'static str>,
    tool_call: Option<&'static str>,
}
const HARNESSES: [HarnessVariables; 4] = [
    HarnessVariables {
        harness: "claude-code",
        session: Some("CLAUDE_CODE_SESSION_ID"),
        marker: "CLAUDECODE",
        marker_value: None,
        pid: Some("CLAUDE_PID"),
        tool_call: None,
    },
    HarnessVariables {
        harness: "codex",
        session: Some("CODEX_THREAD_ID"),
        marker: "CODEX_THREAD_ID",
        marker_value: None,
        pid: None,
        tool_call: Some("CODEX_TOOL_CALL_ID"),
    },
    HarnessVariables {
        harness: "hermes",
        session: Some("HERMES_SESSION_ID"),
        marker: "HERMES_SESSION_ID",
        marker_value: None,
        pid: None,
        tool_call: None,
    },
    HarnessVariables {
        harness: "gemini-cli",
        session: None,
        marker: "GEMINI_CLI",
        marker_value: Some("1"),
        pid: None,
        tool_call: None,
    },
];

/// The facts about the current process: `variable` reads its environment
/// (a product passes its own snapshot), and the process tree, terminal and
/// cgroup come from `/proc/self`.
pub fn gather_current(variable: impl Fn(&str) -> Option<OsString>) -> CallerEvidence {
    let environment = HARNESS_VARIABLES
        .into_iter()
        .filter_map(|name| variable(name).map(|value| (name.to_owned(), value)))
        .collect();
    let ancestors = harness_ancestors(std::os::unix::process::parent_id(), &environment);
    let unit = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|cgroup| cgroup_unit(&cgroup));
    let terminal = std::fs::OpenOptions::new()
        .read(true)
        .open("/dev/tty")
        .is_ok();
    select(&environment, &ancestors, terminal, unit)
}

/// The facts about a socket peer, from its `/proc/<pid>` entry: its
/// harness variables, its harness ancestors, whether it has a controlling
/// terminal, and its cgroup unit.
pub fn gather_peer(pid: u32) -> std::io::Result<CallerEvidence> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let stat = std::fs::read_to_string(root.join("stat"))?;
    let terminal = stat
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(4))
        .and_then(|tty| tty.parse::<i64>().ok())
        .is_some_and(|tty| tty != 0);
    let environment = harness_variables(&std::fs::read(root.join("environ"))?);
    let unit = std::fs::read_to_string(root.join("cgroup"))
        .ok()
        .and_then(|cgroup| cgroup_unit(&cgroup));
    let ancestors =
        parent_pid(&stat).map_or_else(Vec::new, |parent| harness_ancestors(parent, &environment));
    Ok(select(&environment, &ancestors, terminal, unit))
}

/// Decide the caller's harness and session from its harness variables, its
/// harness ancestors (nearest first), its terminal and its unit.
///
/// The nearest harness ancestor names the harness; its variables, when the
/// caller has that harness's, name the session. With no harness ancestor,
/// the variables of exactly one harness name it; variables of several leave
/// the caller ambiguous.
pub fn select(
    environment: &BTreeMap<String, OsString>,
    ancestors: &[(u32, Harness)],
    controlling_terminal: bool,
    unit: Option<String>,
) -> CallerEvidence {
    let text = |name: &str| {
        environment
            .get(name)
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
    };
    struct Found {
        harness: Harness,
        session: Option<AgentSession>,
        tool_call_id: Option<String>,
        pid: Option<u32>,
    }
    let mut found: Vec<Found> = marked(environment)
        .map(|declared| {
            let harness = Harness::new(declared.harness).expect("declared harness names are valid");
            Found {
                session: declared
                    .session
                    .and_then(text)
                    .and_then(|native| AgentSession::new(harness.clone(), native)),
                harness,
                tool_call_id: declared.tool_call.and_then(text).map(str::to_owned),
                pid: declared.pid.and_then(text).and_then(|pid| pid.parse().ok()),
            }
        })
        .collect();
    let mut evidence = CallerEvidence {
        controlling_terminal,
        unit,
        ambiguous_harness: ancestors.is_empty() && found.len() > 1,
        ..CallerEvidence::default()
    };
    let chosen = if let Some((pid, harness)) = ancestors.first() {
        let mut chosen = found
            .into_iter()
            .find(|found| &found.harness == harness)
            .unwrap_or(Found {
                harness: harness.clone(),
                session: None,
                tool_call_id: None,
                pid: None,
            });
        chosen.pid = Some(*pid);
        Some(chosen)
    } else if found.len() == 1 {
        found.pop()
    } else {
        None
    };
    if let Some(chosen) = chosen {
        evidence.harness = Some(chosen.harness);
        evidence.session = chosen.session;
        evidence.tool_call_id = chosen.tool_call_id;
        evidence.harness_pid = chosen.pid;
    }
    evidence
}

/// The harnesses whose variables `environment` holds.
fn marked(
    environment: &BTreeMap<String, OsString>,
) -> impl Iterator<Item = &'static HarnessVariables> {
    let value = |name: &str| environment.get(name).and_then(|value| value.to_str());
    let present = move |name: &str| value(name).is_some_and(|value| !value.is_empty());
    HARNESSES
        .iter()
        .filter(move |declared| match declared.marker_value {
            Some(expected) => value(declared.marker) == Some(expected),
            None => declared.session.is_some_and(present) || present(declared.marker),
        })
}

/// The harness variables of a `/proc/<pid>/environ` file.
fn harness_variables(environ: &[u8]) -> BTreeMap<String, OsString> {
    environ
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let split = entry.iter().position(|byte| *byte == b'=')?;
            let name = std::str::from_utf8(&entry[..split]).ok()?;
            HARNESS_VARIABLES.contains(&name).then(|| {
                (
                    name.to_owned(),
                    OsString::from_vec(entry[split + 1..].to_vec()),
                )
            })
        })
        .collect()
}

/// One process as the walk up the tree reads it.
#[derive(Debug, Clone, Default)]
pub struct Process {
    pub executable: Option<PathBuf>,
    pub arguments: Option<Vec<u8>>,
    /// Its harness variables; `None` when its environment is unreadable.
    pub environment: Option<BTreeMap<String, OsString>>,
    pub parent: Option<u32>,
}

impl Process {
    /// `/proc/<pid>`, or `None` when the process is gone.
    pub fn read(pid: u32) -> Option<Self> {
        let root = PathBuf::from(format!("/proc/{pid}"));
        let stat = std::fs::read_to_string(root.join("stat")).ok()?;
        Some(Self {
            executable: std::fs::read_link(root.join("exe")).ok(),
            arguments: std::fs::read(root.join("cmdline")).ok(),
            environment: std::fs::read(root.join("environ"))
                .ok()
                .map(|environ| harness_variables(&environ)),
            parent: parent_pid(&stat),
        })
    }
}

/// The harness processes from `pid` upward, nearest first, with their IDs.
/// `below` holds the harness variables of the process `pid` started (the
/// caller).
pub fn harness_ancestors(pid: u32, below: &BTreeMap<String, OsString>) -> Vec<(u32, Harness)> {
    harness_chain(pid, below, Process::read)
}

/// [`harness_ancestors`] over any process table.
pub fn harness_chain(
    mut pid: u32,
    below: &BTreeMap<String, OsString>,
    process: impl Fn(u32) -> Option<Process>,
) -> Vec<(u32, Harness)> {
    let mut found = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut marked_below: Vec<&str> = marked(below).map(|declared| declared.harness).collect();
    while pid > 1 && seen.insert(pid) {
        let Some(facts) = process(pid) else {
            break;
        };
        let marked_here: Option<Vec<&str>> = facts.environment.as_ref().map(|environment| {
            marked(environment)
                .map(|declared| declared.harness)
                .collect()
        });
        let by_environment = marked_here.as_ref().and_then(|here| {
            marked_below
                .iter()
                .find(|harness| !here.contains(harness))
                .and_then(|harness| Harness::new(*harness))
        });
        if let Some(harness) =
            harness_of(facts.executable.as_deref(), facts.arguments.as_deref()).or(by_environment)
        {
            found.push((pid, harness));
        }
        if let Some(here) = marked_here {
            marked_below = here;
        }
        let Some(parent) = facts.parent else {
            break;
        };
        pid = parent;
    }
    found
}

/// The harness a process is, from its executable and its first argument
/// (the script, for harnesses that run under an interpreter).
pub fn harness_of(executable: Option<&Path>, arguments: Option<&[u8]>) -> Option<Harness> {
    let name = executable
        .and_then(Path::file_name)
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let script = arguments
        .and_then(|arguments| arguments.split(|byte| *byte == 0).nth(1))
        .and_then(|value| std::str::from_utf8(value).ok())
        .unwrap_or_default();
    let harness = match name {
        "claude" | "claude-code" | "claude.exe" => "claude-code",
        "codex" => "codex",
        "hermes" => "hermes",
        "gemini" => "gemini-cli",
        "node" | "nodejs" if script.contains("/@anthropic-ai/claude-code/") => "claude-code",
        "node" | "nodejs" if script.contains("/@google/gemini-cli/") => "gemini-cli",
        "python" | "python3"
            if Path::new(script)
                .file_name()
                .is_some_and(|name| name == "hermes") =>
        {
            "hermes"
        }
        _ => return None,
    };
    Harness::new(harness)
}

/// The parent process ID from a `/proc/<pid>/stat` line, whose command
/// name may itself hold parentheses.
pub fn parent_pid(stat: &str) -> Option<u32> {
    stat.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// The innermost `.service` or `.scope` unit in a `/proc/<pid>/cgroup` file.
pub fn cgroup_unit(cgroup: &str) -> Option<String> {
    cgroup
        .lines()
        .filter_map(|line| line.splitn(3, ':').nth(2))
        .flat_map(|path| path.split('/'))
        .rfind(|part| part.ends_with(".service") || part.ends_with(".scope"))
        .map(str::to_owned)
}
