use super::*;

fn packages() -> Vec<Package> {
    vec![
        Package {
            name: "model".into(),
            directory: PathBuf::from("/w/crates/model"),
            tests: vec![],
        },
        Package {
            name: "store".into(),
            directory: PathBuf::from("/w/crates/store"),
            tests: vec![],
        },
        Package {
            name: "xtask".into(),
            directory: PathBuf::from("/w/xtask"),
            tests: vec![],
        },
    ]
}

fn files(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn changed_files_map_to_their_packages() {
    let root = Path::new("/w");
    let selection = select(
        root,
        &files(&["crates/store/src/lib.rs", "crates/model/README.md"]),
        &packages(),
    );
    assert_eq!(
        selection,
        Selection::Packages(vec!["model".into(), "store".into()])
    );
    assert_eq!(
        selection.filterset().unwrap(),
        "rdeps(=model) | rdeps(=store)"
    );
}

#[test]
fn a_file_outside_every_package_selects_the_workspace() {
    let root = Path::new("/w");
    assert_eq!(
        select(
            root,
            &files(&["crates/store/src/lib.rs", "flake.nix"]),
            &packages()
        ),
        Selection::Workspace
    );
    assert_eq!(
        select(root, &files(&["docs/development.md"]), &packages()),
        Selection::Nothing
    );
}

#[test]
fn integration_test_changes_narrow_to_the_declared_binary() {
    let mut packages = packages();
    packages[1].tests.push((
        "reads".into(),
        PathBuf::from("/w/crates/store/tests/reads.rs"),
    ));
    let selected = select(
        Path::new("/w"),
        &files(&["crates/store/tests/reads.rs"]),
        &packages,
    );
    assert_eq!(
        selected.filterset().unwrap(),
        "(package(=store) & binary(=reads))"
    );
    assert_eq!(
        select(
            Path::new("/w"),
            &files(&["crates/store/tests/reads.rs", "crates/model/src/lib.rs"]),
            &packages
        )
        .filterset()
        .unwrap(),
        "rdeps(=model) | (package(=store) & binary(=reads))"
    );
    // A shared test helper may be read by several binaries.
    assert_eq!(
        select(
            Path::new("/w"),
            &files(&["crates/store/tests/common.rs"]),
            &packages
        ),
        Selection::Packages(vec!["store".into()])
    );
    assert_eq!(
        select(
            Path::new("/w"),
            &files(&["crates/store/tests/reads.rs", "crates/store/src/lib.rs"]),
            &packages
        ),
        Selection::Packages(vec!["store".into()])
    );
}

#[test]
fn explicit_filters_intersect_changes_and_test_arguments_stay_literal() {
    for args in [
        files(&["-E", "test(reads)"]),
        files(&["--filterset=test(reads)"]),
        files(&["-Etest(reads)"]),
    ] {
        assert!(
            intersect_filter(&args, "rdeps(=store)")
                .unwrap()
                .last()
                .unwrap()
                .contains("(test(reads)) & (rdeps(=store))")
        );
    }
    assert_eq!(
        intersect_filter(&files(&["--", "--changed"]), "none()").unwrap(),
        files(&["-E", "none()", "--", "--changed"])
    );
    assert!(intersect_filter(&files(&["-E"]), "none()").is_err());
}

fn git_ok(root: &Path, args: &[&str]) {
    git(root, args).unwrap();
}

#[test]
fn diff_includes_commits_index_working_files_untracked_and_both_sides_of_renames() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    for name in ["base", "other"] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
        std::fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n"),
        )
        .unwrap();
        std::fs::write(root.join(name).join("src/lib.rs"), "pub fn value() {}\n").unwrap();
    }
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = ['base', 'other']\nresolver = '3'\n",
    )
    .unwrap();
    git_ok(root, &["init", "-b", "master"]);
    git_ok(root, &["add", "Cargo.toml", "base", "other"]);
    git_ok(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    );
    git_ok(root, &["branch", "baseline"]);
    assert_eq!(changed(root, "baseline").unwrap(), Selection::Nothing);
    std::fs::write(root.join("base/src/lib.rs"), "pub fn changed() {}\n").unwrap();
    git_ok(root, &["add", "base/src/lib.rs"]);
    git_ok(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "changed",
        ],
    );
    assert_eq!(
        changed(root, "baseline").unwrap(),
        Selection::Packages(vec!["base".into()])
    );
    // metadata writes Cargo.lock; include it in the baseline before later checks.
    git_ok(root, &["add", "Cargo.lock"]);
    git_ok(
        root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "lock",
        ],
    );
    git_ok(root, &["branch", "current"]);
    std::fs::write(root.join("base/src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::rename(
        root.join("base/src/lib.rs"),
        root.join("other/src/moved.rs"),
    )
    .unwrap();
    git_ok(
        root,
        &[
            "add",
            "base/src/lib.rs",
            "base/src/main.rs",
            "other/src/moved.rs",
        ],
    );
    assert_eq!(
        changed(root, "current").unwrap(),
        Selection::Packages(vec!["base".into(), "other".into()])
    );
    std::fs::write(root.join("new config\nfile.nix"), "{}\n").unwrap();
    assert_eq!(changed(root, "current").unwrap(), Selection::Workspace);
    assert!(changed(root, "missing-ref").is_err());
}
