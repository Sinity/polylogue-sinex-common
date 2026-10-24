use super::Tree;
use std::{fs, path::Path, process::Command};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}
fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "first",
        ],
    );
    dir
}

#[test]
fn distinct_commits_with_equal_git_trees_keep_identity_separate_from_dirty_content() {
    let dir = repository();
    let root = dir.path();
    let first = Tree::observe(root);
    git(
        root,
        &[
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "second",
        ],
    );
    let second = Tree::observe(root);
    assert_ne!(first.head, second.head);
    assert_eq!(first.git_tree, second.git_tree);
    assert!(second.dirty.is_none());
    assert!(second.observation_error.is_none());
    fs::write(root.join("new.txt"), b"synthetic bytes").unwrap();
    let dirty = Tree::observe(root);
    assert_eq!(dirty.git_tree, second.git_tree);
    assert!(dirty.dirty.is_some());
    assert!(dirty.observation_error.is_none());
}

#[test]
fn failed_git_or_untracked_observation_is_unknown_instead_of_clean() {
    let dir = tempfile::tempdir().unwrap();
    let missing = Tree::observe(dir.path());
    assert!(missing.git_tree.is_none());
    assert!(missing.observation_error.is_some());
    let repo = repository();
    fs::write(repo.path().join(".git/index"), b"not an index").unwrap();
    let failed = Tree::observe(repo.path());
    assert!(failed.head.is_some());
    assert!(failed.git_tree.is_none());
    assert!(failed.observation_error.is_some());
    use std::os::unix::fs::PermissionsExt;
    let repo = repository();
    let path = repo.path().join("unreadable.txt");
    fs::write(&path, b"synthetic bytes").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o0)).unwrap();
    let readable = fs::read(&path);
    let unsupported = Tree::observe(repo.path());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(readable.is_err(), "requires actual file read denial");
    assert!(unsupported.git_tree.is_none());
    assert!(unsupported.observation_error.is_some());
}

#[test]
fn untracked_symlink_target_and_raw_filename_are_observed_without_following_the_link() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let repo = repository();
    let root = repo.path();
    let name = std::ffi::OsString::from_vec(b"non-utf8-\xff".to_vec());
    fs::write(root.join(name), b"synthetic bytes").unwrap();
    symlink("missing-target-a", root.join("link")).unwrap();
    let a = Tree::observe(root);
    assert!(a.observation_error.is_none());
    assert!(a.dirty.is_some());
    fs::remove_file(root.join("link")).unwrap();
    symlink("missing-target-b", root.join("link")).unwrap();
    let b = Tree::observe(root);
    assert!(b.observation_error.is_none());
    assert_ne!(a.dirty, b.dirty);
}

#[cfg(feature = "history")]
#[test]
fn a_changed_checkout_invalidates_recorded_tree_identity_at_finish() {
    let repo = repository();
    let database = tempfile::tempdir().unwrap();
    let path = database.path().join("history.db");
    let store = crate::history::Store::open_at(&path).unwrap();
    let mut run = super::Run::unrecorded("demo", "test", &[]);
    run.root = repo.path().to_path_buf();
    run.tree = Tree::observe(repo.path());
    assert!(run.tree.git_tree.is_some());
    let id = store.begin(&run).unwrap();
    run.record = Some((store, id));
    run.tests.push(super::TestOutcome {
        binary: "suite".into(),
        name: "passed-before-source-change".into(),
        status: super::TestStatus::Passed,
        duration: std::time::Duration::from_millis(1),
        output: None,
    });
    fs::write(repo.path().join("new.txt"), b"changed during run").unwrap();
    assert_eq!(run.finish(), std::process::ExitCode::FAILURE);
    let store = crate::history::Store::open_at(&path).unwrap();
    let row = store.run(id).unwrap().unwrap();
    assert_eq!(row.status, "failed");
    assert_eq!(store.test_counts(id).unwrap(), vec![("passed".into(), 1)]);
    assert!(row.git_tree.is_none());
    assert!(
        row.observation_error
            .as_deref()
            .unwrap()
            .contains("during the run")
    );
}
