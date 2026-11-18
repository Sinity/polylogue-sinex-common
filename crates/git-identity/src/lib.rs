//! Which Git repository a directory belongs to.
//!
//! Polylogue joins sessions to repositories and Sinex attributes shell
//! commands to projects by the same facts, so both products ask this crate.
//! When each product computed them itself they disagreed: one stored the
//! remote URL with its credentials, one took only the first-parent root
//! commit, and two counted shallow-clone boundaries as roots.
//!
//! The rules:
//!
//! - **Root commits** are every commit reachable from `HEAD` that has no
//!   parent, sorted and without repeats, leaving out the boundary commits of
//!   a shallow clone. A boundary commit has parents the clone did not fetch,
//!   so it is not a root, and a shallow clone of a repository must not look
//!   like a different repository. A set rather than one commit, because a
//!   set is stable when histories are merged in either direction, while the
//!   first-parent root changes with the direction of a merge. Two
//!   directories belong to the same repository when their sets share a
//!   commit. A shallow clone whose history reaches no real root has an empty
//!   set.
//! - **Remote** is the `origin` URL with any user and password removed from
//!   a `scheme://` URL. An scp-like `git@host:path` names no secret and is
//!   kept.
//! - Git runs with the repository environment variables removed and
//!   replacement objects, legacy grafts and lazy fetching off, so a caller's `GIT_DIR` or a
//!   partial clone cannot change the answer or reach the network. Output goes
//!   to a temporary file, so a large answer never blocks the child, and every
//!   query shares one deadline.

use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// What Git says about the repository holding a directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    /// The repository's common directory, absolute: the same for every
    /// worktree of one repository.
    pub common_dir: String,
    /// The root commits reachable from `HEAD`, sorted, shallow boundaries
    /// left out.
    pub root_commits: Vec<String>,
    /// The commit `HEAD` names.
    pub head: String,
    /// The checked-out branch; none when `HEAD` is detached.
    pub branch: Option<String>,
    /// The `origin` remote without credentials; none when there is none.
    pub remote: Option<String>,
}

/// Previously observed roots, reusable only with the same shallow state.
/// Branch and remote are always read again; these are not cached identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnownRoots {
    pub head: String,
    pub shallow: String,
    pub root_commits: Vec<String>,
}

/// Why no identity could be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// The directory is not inside a repository, or the repository has no
    /// commit yet.
    NotARepository,
    /// Git could not be started.
    GitMissing,
    /// The deadline passed before Git answered.
    DeadlinePassed,
    /// Git's answer was not text, or a temporary file could not be made.
    Unreadable,
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Unavailable::NotARepository => "not in a repository with a commit",
            Unavailable::GitMissing => "git could not be started",
            Unavailable::DeadlinePassed => "git did not answer before the deadline",
            Unavailable::Unreadable => "git's answer could not be read",
        })
    }
}

impl std::error::Error for Unavailable {}

/// Reads the identity of the repository holding `dir`, giving up at
/// `deadline`.
pub fn observe(dir: &Path, deadline: Instant) -> Result<Identity, Unavailable> {
    observe_reusing(dir, deadline, None).map(|(identity, _)| identity)
}

/// Observe current metadata, reusing roots or extending them after a fast
/// forward. A changed shallow file or unrelated history requires a full walk.
pub fn observe_reusing(
    dir: &Path,
    deadline: Instant,
    known: Option<&KnownRoots>,
) -> Result<(Identity, KnownRoots), Unavailable> {
    let run = |args: &[&str]| git(dir, args, deadline, None);
    let common_dir = required(run(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ]))?;
    let head = required(run(&["rev-parse", "--verify", "HEAD"]))?;
    let shallow = shallow_state(&common_dir)?;
    let root_commits = root_commits_of(dir, &head, &shallow, known, deadline)?;
    let branch =
        run(&["symbolic-ref", "--quiet", "--short", "HEAD"])?.filter(|branch| !branch.is_empty());
    let remote = remote(dir, deadline)?;
    let known = KnownRoots {
        head: head.clone(),
        shallow,
        root_commits: root_commits.clone(),
    };
    Ok((
        Identity {
            common_dir,
            root_commits,
            head,
            branch,
            remote,
        },
        known,
    ))
}

/// The root commits reachable from `HEAD` in the repository holding `dir`,
/// sorted, shallow boundaries left out. Empty when `HEAD` names no commit or
/// the history reaches no real root.
pub fn root_commits(dir: &Path, deadline: Instant) -> Result<Vec<String>, Unavailable> {
    match git(dir, &["rev-parse", "--verify", "HEAD"], deadline, None)? {
        Some(head) if !head.is_empty() => {
            let common = required(git(
                dir,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                deadline,
                None,
            ))?;
            root_commits_of(dir, &head, &shallow_state(&common)?, None, deadline)
        }
        _ => Ok(Vec::new()),
    }
}

/// The `origin` remote of the repository holding `dir`, with credentials
/// removed; none when there is no `origin`.
pub fn remote(dir: &Path, deadline: Instant) -> Result<Option<String>, Unavailable> {
    Ok(git(dir, &["remote", "get-url", "origin"], deadline, None)?
        .filter(|url| !url.is_empty())
        .map(|url| without_credentials(&url)))
}

fn shallow_state(common_dir: &str) -> Result<String, Unavailable> {
    match fs::read_to_string(Path::new(common_dir).join("shallow")) {
        Ok(shallow) => Ok(shallow),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(_) => Err(Unavailable::Unreadable),
    }
}

fn root_commits_of(
    dir: &Path,
    head: &str,
    shallow: &str,
    known: Option<&KnownRoots>,
    deadline: Instant,
) -> Result<Vec<String>, Unavailable> {
    if Instant::now() >= deadline {
        return Err(Unavailable::DeadlinePassed);
    }
    let known = known.filter(|known| known.shallow == shallow);
    if let Some(known) = known.filter(|known| known.head == head) {
        let mut roots = known.root_commits.clone();
        roots.sort();
        roots.dedup();
        return Ok(roots);
    }
    // Git must walk the captured boundary state, not a different file installed
    // by a concurrent fetch while the query runs.
    let mut snapshot = tempfile::NamedTempFile::new().map_err(|_| Unavailable::Unreadable)?;
    snapshot
        .write_all(shallow.as_bytes())
        .map_err(|_| Unavailable::Unreadable)?;
    let run = |args: &[&str]| git(dir, args, deadline, Some(snapshot.path()));
    let mut roots = Vec::new();
    let exclude = match known {
        Some(known) if run(&["merge-base", "--is-ancestor", &known.head, head])?.is_some() => {
            roots.extend(known.root_commits.iter().cloned());
            Some(format!("^{}", known.head))
        }
        _ => None,
    };
    let mut arguments = vec!["rev-list", "--max-parents=0", head];
    if let Some(exclude) = &exclude {
        arguments.push(exclude);
    }
    let observed = run(&arguments)?.ok_or(Unavailable::Unreadable)?;
    roots.extend(root_set(&observed, shallow));
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Reads the identity with a time budget instead of a deadline.
pub fn observe_within(dir: &Path, budget: Duration) -> Result<Identity, Unavailable> {
    observe(dir, Instant::now() + budget)
}

/// The sorted root set from `git rev-list --max-parents=0` output, leaving
/// out every commit the shallow file lists as a boundary.
pub fn root_set(rev_list: &str, shallow: &str) -> Vec<String> {
    let boundaries: std::collections::BTreeSet<&str> = shallow.lines().map(str::trim).collect();
    let roots: std::collections::BTreeSet<&str> = rev_list
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty() && !boundaries.contains(id))
        .collect();
    roots.into_iter().map(str::to_owned).collect()
}

/// A remote URL without the user and password a `scheme://` URL may carry.
/// An scp-like `git@host:path` names no secret and is kept.
pub fn without_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let host_end = rest.find('/').unwrap_or(rest.len());
    match rest[..host_end].rfind('@') {
        Some(at) => format!("{scheme}://{}", &rest[at + 1..]),
        None => url.to_owned(),
    }
}

/// The nearest directory at or above `dir` that holds `.git` (a directory,
/// or the file a worktree has).
pub fn repository_of(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

fn required(answer: Result<Option<String>, Unavailable>) -> Result<String, Unavailable> {
    answer?
        .filter(|text| !text.is_empty())
        .ok_or(Unavailable::NotARepository)
}

/// Runs one Git query. `Ok(None)` means Git refused; a successful answer can
/// be empty (as with --is-ancestor). `Err` means no answer could be obtained.
fn git(
    dir: &Path,
    args: &[&str],
    deadline: Instant,
    shallow: Option<&Path>,
) -> Result<Option<String>, Unavailable> {
    if Instant::now() >= deadline {
        return Err(Unavailable::DeadlinePassed);
    }
    let mut output = tempfile::tempfile().map_err(|_| Unavailable::Unreadable)?;
    let mut command = Command::new("git");
    command
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::from(
            output.try_clone().map_err(|_| Unavailable::Unreadable)?,
        ));
    for name in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_SHALLOW_FILE",
    ] {
        command.env_remove(name);
    }
    command
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env(
            "GIT_GRAFT_FILE",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        );
    if let Some(shallow) = shallow {
        command.env("GIT_SHALLOW_FILE", shallow);
    }
    let mut child = command.spawn().map_err(|_| Unavailable::GitMissing)?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(1));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Unavailable::DeadlinePassed);
            }
        }
    };
    if !status.success() {
        return Ok(None);
    }
    output.rewind().map_err(|_| Unavailable::Unreadable)?;
    let mut text = String::new();
    output
        .read_to_string(&mut text)
        .map_err(|_| Unavailable::Unreadable)?;
    let text = text.strip_suffix('\n').unwrap_or(&text).to_owned();
    Ok(Some(text))
}
