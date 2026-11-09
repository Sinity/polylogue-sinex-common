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
//!   replacement objects and lazy fetching off, so a caller's `GIT_DIR` or a
//!   partial clone cannot change the answer or reach the network. Output goes
//!   to a temporary file, so a large answer never blocks the child, and every
//!   query shares one deadline.

use std::fs;
use std::io::{Read, Seek};
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
    let run = |args: &[&str]| git(dir, args, deadline);
    let common_dir = required(run(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-common-dir",
    ]))?;
    let head = required(run(&["rev-parse", "--verify", "HEAD"]))?;
    let roots = required(run(&["rev-list", "--max-parents=0", &head]))?;
    let shallow = match run(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "shallow",
    ]) {
        Ok(Some(path)) => fs::read_to_string(PathBuf::from(path)).unwrap_or_default(),
        Ok(None) => String::new(),
        Err(reason) => return Err(reason),
    };
    let root_commits = root_set(&roots, &shallow);
    let branch = run(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let remote = run(&["remote", "get-url", "origin"])?.map(|url| without_credentials(&url));
    Ok(Identity {
        common_dir,
        root_commits,
        head,
        branch,
        remote,
    })
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
    answer?.ok_or(Unavailable::NotARepository)
}

/// Runs one Git query. `Ok(None)` is an answer that Git refused or left
/// empty; `Err` means no answer could be had at all.
fn git(dir: &Path, args: &[&str], deadline: Instant) -> Result<Option<String>, Unavailable> {
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
    ] {
        command.env_remove(name);
    }
    command
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1");
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
    Ok((!text.is_empty()).then_some(text))
}
