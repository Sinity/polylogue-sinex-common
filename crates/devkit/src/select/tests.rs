use super::*;

fn packages() -> Vec<Package> {
    vec![
        package("model", "/w/crates/model", &[]),
        package("store", "/w/crates/store", &["model"]),
        package("xtask", "/w/xtask", &[]),
    ]
}

fn package(name: &str, directory: &str, dependencies: &[&str]) -> Package {
    Package {
        name: name.into(),
        directory: PathBuf::from(directory),
        tests: vec![],
        roots: vec![PathBuf::from(directory).join("src/lib.rs")],
        dependencies: dependencies.iter().map(|d| (*d).to_owned()).collect(),
        read_by_tests: vec![],
    }
}

fn no_functions(_: &str, _: &Package) -> Option<BTreeSet<String>> {
    None
}

fn tests(
    with_dependents: &[&str],
    packages: &[&str],
    binaries: &[(&str, &str)],
    functions: &[(&str, &[&str])],
) -> Selection {
    Selection::Tests(Tests {
        with_dependents: with_dependents.iter().map(|p| (*p).to_owned()).collect(),
        packages: packages.iter().map(|p| (*p).to_owned()).collect(),
        binaries: binaries
            .iter()
            .map(|(p, b)| ((*p).to_owned(), (*b).to_owned()))
            .collect(),
        functions: functions
            .iter()
            .map(|(p, names)| {
                (
                    (*p).to_owned(),
                    names.iter().map(|n| (*n).to_owned()).collect(),
                )
            })
            .collect(),
    })
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
        no_functions,
    );
    assert_eq!(selection, tests(&["model", "store"], &[], &[], &[]));
    assert_eq!(
        selection.filterset().unwrap(),
        "rdeps(=model) | rdeps(=store)"
    );
}

#[test]
fn files_outside_packages_select_the_workspace_except_markdown() {
    let root = Path::new("/w");
    assert_eq!(
        select(
            root,
            &files(&["crates/store/src/lib.rs", "flake.nix"]),
            &packages(),
            no_functions
        ),
        Selection::Workspace
    );
    assert_eq!(
        select(
            root,
            &files(&["scripts/setup.sh"]),
            &packages(),
            no_functions
        ),
        Selection::Workspace
    );
    let nothing = select(
        root,
        &files(&["docs/development.md", "README.md"]),
        &packages(),
        no_functions,
    );
    assert!(nothing.is_empty());
    assert_eq!(nothing.filterset().unwrap(), "none()");
}

#[test]
fn a_file_declared_read_by_tests_selects_those_tests() {
    let root = Path::new("/w");
    let mut packages = packages();
    packages[1].read_by_tests.push((
        lexical(Path::new("/w/crates/store/../../README.md")),
        ["readme_examples".to_owned()].into(),
    ));
    let selected = select(
        root,
        &files(&["README.md", "docs/testing.md"]),
        &packages,
        no_functions,
    );
    assert_eq!(
        selected,
        tests(&[], &[], &[], &[("store", &["readme_examples"])])
    );
    assert_eq!(
        selected.filterset().unwrap(),
        "(package(=store) & (test(readme_examples)))"
    );
}

#[test]
fn packages_to_build_follow_reverse_dependencies_only_for_production_changes() {
    let workspace = Workspace {
        root: PathBuf::from("/w"),
        packages: vec![
            package("model", "/w/crates/model", &[]),
            package("store", "/w/crates/store", &["model"]),
            package("daemon", "/w/crates/daemon", &["store"]),
            package("other", "/w/crates/other", &[]),
        ],
    };
    let built = |selection: Selection| {
        workspace
            .packages_to_build(&selection)
            .map(|set| set.into_iter().collect::<Vec<_>>())
    };
    assert_eq!(
        built(tests(&["model"], &[], &[], &[])),
        Some(vec!["daemon".into(), "model".into(), "store".into()])
    );
    assert_eq!(
        built(tests(
            &[],
            &["store"],
            &[("other", "reads")],
            &[("model", &["a"])]
        )),
        Some(vec!["model".into(), "other".into(), "store".into()])
    );
    assert_eq!(built(tests(&[], &[], &[], &[])), Some(vec![]));
    assert_eq!(built(Selection::Workspace), None);
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
        no_functions,
    );
    assert_eq!(
        selected.filterset().unwrap(),
        "(package(=store) & binary(=reads))"
    );
    assert_eq!(
        select(
            Path::new("/w"),
            &files(&["crates/store/tests/reads.rs", "crates/model/src/lib.rs"]),
            &packages,
            no_functions
        )
        .filterset()
        .unwrap(),
        "rdeps(=model) | (package(=store) & binary(=reads))"
    );
    assert_eq!(
        select(
            Path::new("/w"),
            &files(&["crates/store/tests/reads.rs", "crates/store/src/lib.rs"]),
            &packages,
            no_functions
        ),
        tests(&["store"], &[], &[], &[])
    );
}

#[test]
fn explicit_filters_intersect_changes_and_test_arguments_stay_literal() {
    for args in [
        files(&["-E", "test(reads)"]),
        files(&["--filterset=test(reads)"]),
        files(&["--filter-expr", "test(reads)"]),
        files(&["--filter-expr=test(reads)"]),
        files(&["-Etest(reads)"]),
        files(&["-E=test(reads)"]),
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
    assert!(changed(root, "baseline").unwrap().is_empty());
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
        tests(&["base"], &[], &[], &[])
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
        tests(&["base", "other"], &[], &[], &[])
    );
    std::fs::write(root.join("new config\nfile.nix"), "{}\n").unwrap();
    assert_eq!(changed(root, "current").unwrap(), Selection::Workspace);
    assert!(changed(root, "missing-ref").is_err());
}

#[test]
fn git_diff_narrows_test_bodies_but_keeps_helpers_at_package_scope() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'test-selection'\nversion = '0.1.0'\nedition = '2024'\n[workspace]\n",
    )
    .unwrap();
    let source = "fn helper() -> u8 { 1 }\n#[test]\nfn reads() {\n    assert_eq!(helper(), 1);\n}\n#[test]\nfn writes() {}\n";
    std::fs::write(root.join("src/lib.rs"), source).unwrap();
    git_ok(root, &["init", "-b", "master"]);
    git_ok(root, &["add", "Cargo.toml", "src/lib.rs"]);
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
    std::fs::write(
        root.join("src/lib.rs"),
        source.replace("helper(), 1", "helper(), 2"),
    )
    .unwrap();
    assert_eq!(
        changed(root, "HEAD").unwrap().filterset().unwrap(),
        "(package(=test-selection) & (test(reads)))"
    );
    let mut relative = PathBuf::new();
    for _ in std::env::current_dir().unwrap().ancestors().skip(1) {
        relative.push("..");
    }
    relative.push(root.strip_prefix("/").unwrap());
    assert_eq!(
        changed(&relative, "HEAD").unwrap(),
        changed(root, "HEAD").unwrap()
    );
    let (args, unchanged) =
        nextest_args(root, &files(&["--base=HEAD", "-E", "test(reads)"])).unwrap();
    assert!(!unchanged);
    assert_eq!(
        args,
        files(&[
            "-E",
            "(test(reads)) & ((package(=test-selection) & (test(reads))))"
        ])
    );
    assert_eq!(
        nextest_args(root, &files(&["--base", "HEAD", "-E", "test(reads)"])).unwrap(),
        (args, false)
    );
    assert!(nextest_args(root, &files(&["--base"])).is_err());
    std::fs::write(
        root.join("src/lib.rs"),
        source.replace("u8 { 1 }", "u8 { 2 }"),
    )
    .unwrap();
    assert_eq!(
        changed(root, "HEAD").unwrap(),
        tests(&["test-selection"], &[], &[], &[])
    );
}

#[test]
fn calls_to_a_changed_test_keep_the_package_and_its_dependents() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = 'test-calls'\nversion = '0.1.0'\nedition = '2024'\n[workspace]\n",
    )
    .unwrap();
    let source =
        "#[test]\nfn reads() {\n    assert_eq!(1, 1);\n}\n#[test]\nfn writes() { reads(); }\n";
    std::fs::write(root.join("src/lib.rs"), source).unwrap();
    git_ok(root, &["init", "-b", "master"]);
    git_ok(root, &["add", "Cargo.toml", "src/lib.rs"]);
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
    std::fs::write(root.join("src/lib.rs"), source.replace("1, 1", "2, 2")).unwrap();
    assert_eq!(
        changed(root, "HEAD").unwrap(),
        tests(&["test-calls"], &[], &[], &[])
    );
}

#[test]
fn workspace_settings_select_everything_even_when_a_package_lives_at_the_root() {
    let packages = vec![package("root", "/w", &[])];
    for file in [
        "Cargo.toml",
        "Cargo.lock",
        "flake.nix",
        "flake.lock",
        ".config/nextest.toml",
        ".cargo/config.toml",
    ] {
        assert_eq!(
            select(Path::new("/w"), &files(&[file]), &packages, no_functions),
            Selection::Workspace,
            "{file}"
        );
    }
}

#[test]
fn test_only_modules_select_their_package_without_dependents() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    for (name, dependencies) in [("core", ""), ("user", "core = { path = '../core' }")] {
        std::fs::create_dir_all(root.join(name).join("src")).unwrap();
        std::fs::write(
            root.join(name).join("Cargo.toml"),
            format!("[package]\nname = '{name}'\nversion = '0.1.0'\nedition = '2024'\n[dependencies]\n{dependencies}\n"),
        )
        .unwrap();
        std::fs::write(root.join(name).join("src/lib.rs"), "\n").unwrap();
    }
    std::fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers = ['core', 'user']\nresolver = '3'\n",
    )
    .unwrap();
    let write = |file: &str, text: &str| {
        let path = root.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "core/src/lib.rs",
        "pub mod parse;\nmod moved;\n#[cfg(all(test, unix))]\nmod tests;\n",
    );
    write("core/src/tests.rs", "fn helper() {}\n");
    write(
        "core/src/parse.rs",
        "pub fn parse() {}\n#[cfg(test)]\nmod tests;\n",
    );
    write("core/src/parse/tests.rs", "mod cases;\n");
    write("core/src/parse/tests/cases.rs", "fn case() {}\n");
    write(
        "core/src/moved.rs",
        "#[cfg(test)]\n#[path = \"elsewhere.rs\"]\nmod tests;\n",
    );
    write("core/src/moved/tests.rs", "fn unused() {}\n");
    write("core/src/elsewhere.rs", "fn shared() {}\n");
    write("core/tests/support/helpers.rs", "pub fn helper() {}\n");
    git_ok(root, &["init", "-b", "master"]);
    git_ok(root, &["add", "."]);
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
    let after_change = |file: &str| {
        let path = root.join(file);
        let original = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{original}fn added() {{}}\n")).unwrap();
        let selected = changed(root, "HEAD").unwrap();
        std::fs::write(&path, original).unwrap();
        selected
    };
    for file in [
        "core/src/tests.rs",
        "core/src/parse/tests.rs",
        "core/src/parse/tests/cases.rs",
        // A shared helper of the package's integration tests.
        "core/tests/support/helpers.rs",
    ] {
        assert_eq!(
            after_change(file),
            tests(&[], &["core"], &[], &[]),
            "{file}"
        );
    }
    // Production code, and a file reached through `#[path]`, reach dependents.
    for file in ["core/src/parse.rs", "core/src/elsewhere.rs"] {
        assert_eq!(
            after_change(file),
            tests(&["core"], &[], &[], &[]),
            "{file}"
        );
    }
    let workspace = Workspace::read(root).unwrap();
    assert_eq!(
        workspace.packages_to_build(&after_change("core/src/tests.rs")),
        Some(["core".to_owned()].into())
    );
    assert_eq!(
        workspace.packages_to_build(&after_change("core/src/parse.rs")),
        Some(["core".to_owned(), "user".to_owned()].into())
    );
}
