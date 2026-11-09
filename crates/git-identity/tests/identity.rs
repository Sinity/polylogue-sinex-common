use std::path::Path;
use std::process::Command;
use std::time::Duration;

use git_identity::{Unavailable, observe_within, root_set, without_credentials};

const BUDGET: Duration = Duration::from_secs(20);

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn repo(dir: &Path) {
    git(dir, &["init", "-q", "-b", "main"]);
}

fn commit(dir: &Path, name: &str) -> String {
    std::fs::write(dir.join(name), name).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", name]);
    git(dir, &["rev-parse", "HEAD"])
}

#[test]
fn git_identity_strips_url_credentials() {
    assert_eq!(
        without_credentials("https://user:secret@github.com/a/b.git"),
        "https://github.com/a/b.git"
    );
    assert_eq!(
        without_credentials("ssh://git:token@host.example/a/b"),
        "ssh://host.example/a/b"
    );
    assert_eq!(
        without_credentials("git@github.com:a/b.git"),
        "git@github.com:a/b.git"
    );
    assert_eq!(
        without_credentials("https://github.com/a/b@v1"),
        "https://github.com/a/b@v1"
    );

    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    commit(dir.path(), "a");
    git(
        dir.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://user:secret@example.invalid/x.git",
        ],
    );
    let identity = observe_within(dir.path(), BUDGET).unwrap();
    assert_eq!(
        identity.remote.as_deref(),
        Some("https://example.invalid/x.git")
    );
}

#[test]
fn git_identity_reads_a_plain_repository() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let root = commit(dir.path(), "a");
    let head = commit(dir.path(), "b");
    let identity = observe_within(&dir.path().join("."), BUDGET).unwrap();
    assert_eq!(identity.root_commits, vec![root]);
    assert_eq!(identity.head, head);
    assert_eq!(identity.branch.as_deref(), Some("main"));
    assert_eq!(identity.remote, None);
    assert!(identity.common_dir.ends_with(".git"));
}

#[test]
fn git_identity_roots_are_every_root_sorted() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    let first = commit(dir.path(), "a");
    git(dir.path(), &["checkout", "-q", "--orphan", "other"]);
    git(dir.path(), &["rm", "-q", "-rf", "."]);
    let second = commit(dir.path(), "b");
    git(dir.path(), &["checkout", "-q", "main"]);
    git(
        dir.path(),
        &[
            "merge",
            "-q",
            "--allow-unrelated-histories",
            "-m",
            "m",
            "other",
        ],
    );
    let mut expected = vec![first, second];
    expected.sort();
    let identity = observe_within(dir.path(), BUDGET).unwrap();
    assert_eq!(identity.root_commits, expected);
}

#[test]
fn git_identity_roots_exclude_shallow_boundary() {
    let origin = tempfile::tempdir().unwrap();
    repo(origin.path());
    let root = commit(origin.path(), "a");
    commit(origin.path(), "b");
    commit(origin.path(), "c");
    let full = observe_within(origin.path(), BUDGET).unwrap();
    assert_eq!(full.root_commits, vec![root.clone()]);

    let clone = tempfile::tempdir().unwrap();
    let url = format!("file://{}", origin.path().display());
    git(clone.path(), &["clone", "-q", "--depth", "1", &url, "."]);
    let shallow = observe_within(clone.path(), BUDGET).unwrap();
    assert!(
        shallow.root_commits.is_empty(),
        "a boundary commit is not a root: {:?}",
        shallow.root_commits
    );
    git(clone.path(), &["fetch", "-q", "--unshallow"]);
    let unshallowed = observe_within(clone.path(), BUDGET).unwrap();
    assert_eq!(unshallowed.root_commits, vec![root]);
}

#[test]
fn root_set_drops_boundaries_and_repeats() {
    assert_eq!(root_set("b\na\nb\n\n", "a\n"), vec!["b".to_owned()]);
    assert_eq!(root_set("c\nb\n", ""), vec!["b".to_owned(), "c".to_owned()]);
}

#[test]
fn outside_a_repository_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        observe_within(dir.path(), BUDGET),
        Err(Unavailable::NotARepository)
    );
}

#[test]
fn a_passed_deadline_is_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    repo(dir.path());
    commit(dir.path(), "a");
    assert_eq!(
        observe_within(dir.path(), Duration::ZERO),
        Err(Unavailable::DeadlinePassed)
    );
}

#[test]
fn an_identity_serializes_with_plain_field_names() {
    let identity = git_identity::Identity {
        common_dir: "/r/.git".into(),
        root_commits: vec!["a".into()],
        head: "h".into(),
        branch: None,
        remote: None,
    };
    let value = serde_json::to_value(&identity).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"common_dir":"/r/.git","root_commits":["a"],"head":"h","branch":null,"remote":null})
    );
}
