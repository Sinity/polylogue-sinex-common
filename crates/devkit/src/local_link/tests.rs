use super::LocalLink;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture {
    directory: tempfile::TempDir,
    repository: String,
    first: PathBuf,
    second: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let remote = directory.path().join("remote");
        package(&remote, "shared", "1.0.0", "");
        command(&remote, "git", &["init", "-q", "--initial-branch=master"]);
        command(&remote, "git", &["add", "Cargo.toml", "src/lib.rs"]);
        command(
            &remote,
            "git",
            &[
                "-c",
                "user.name=Synthetic",
                "-c",
                "user.email=synthetic@example.invalid",
                "commit",
                "-qm",
                "shared crate",
            ],
        );
        let repository = format!("file://{}", remote.display());
        let quoted = serde_json::to_string(&repository).unwrap();
        let consumer = directory.path().join("consumer");
        package(
            &consumer,
            "consumer-root",
            "0.1.0",
            &format!(
                "[dependencies]\nshared = {{ git = {quoted}, branch = \"master\", version = \">=1, <3\" }}\n"
            ),
        );
        package(
            &consumer.join("fuzz"),
            "consumer-fuzz",
            "0.1.0",
            &format!(
                "[dependencies]\nshared = {{ git = {quoted}, branch = \"master\", version = \"=1.0.0\" }}\n"
            ),
        );
        cargo(&consumer, &["metadata", "--format-version", "1"]);
        cargo(
            &consumer.join("fuzz"),
            &["metadata", "--format-version", "1"],
        );
        fs::create_dir_all(consumer.join(".cargo")).unwrap();
        fs::write(
            consumer.join(".cargo/config.toml"),
            "include = [{ path = \"local.toml\", optional = true }]\n[net]\noffline = true\n",
        )
        .unwrap();
        let first = directory.path().join("checkout with \"quotes\"");
        let second = directory.path().join("new checkout");
        package(&first, "shared", "1.0.0", "");
        package(&second, "shared", "2.0.0", "");
        Self {
            directory,
            repository,
            first,
            second,
        }
    }

    fn root(&self) -> PathBuf {
        self.directory.path().join("consumer")
    }

    fn paths(&self) -> Vec<PathBuf> {
        [
            ".cargo/local.toml",
            "Cargo.lock",
            ".cargo/root.lock",
            "fuzz/Cargo.lock",
            ".cargo/fuzz.lock",
        ]
        .into_iter()
        .map(|path| self.root().join(path))
        .collect()
    }

    fn state(&self) -> Vec<Option<Vec<u8>>> {
        self.paths()
            .iter()
            .map(|path| match fs::read(path) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("{}: {error}", path.display()),
            })
            .collect()
    }

    fn with_link<T>(&self, operation: impl FnOnce(LocalLink<'_>) -> T) -> T {
        let paths = self.paths();
        let locks = [
            (paths[1].as_path(), paths[2].as_path()),
            (paths[3].as_path(), paths[4].as_path()),
        ];
        operation(LocalLink {
            repository: &self.repository,
            patch: &paths[0],
            locks: &locks,
        })
    }
}

#[test]
fn link_roundtrip_restores_both_locks_without_leaving_auxiliary_state() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join(".cargo/consumer-settings.toml"),
        b"synthetic consumer settings\n",
    )
    .unwrap();
    let files = || {
        fs::read_dir(fixture.root().join(".cargo"))
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                (entry.file_name(), fs::read(entry.path()).unwrap())
            })
            .collect::<BTreeMap<_, _>>()
    };
    let original_files = files();
    let locks = ["Cargo.lock", "fuzz/Cargo.lock"];
    let original_locks = locks.map(|path| fs::read(fixture.root().join(path)).unwrap());
    fixture.with_link(|link| link.link(&fixture.first)).unwrap();
    fixture.with_link(|link| link.unlink()).unwrap();
    assert_eq!(files(), original_files);
    assert_eq!(
        locks.map(|path| fs::read(fixture.root().join(path)).unwrap()),
        original_locks
    );
}

#[test]
fn a_shared_crate_reached_only_through_another_dependency_is_linked() {
    let fixture = Fixture::new();
    let bridge = fixture.directory.path().join("bridge");
    package(
        &bridge,
        "bridge",
        "0.1.0",
        &format!(
            "[dependencies]\nshared = {{ git = {}, branch = \"master\", version = \"1.0.0\" }}\n",
            serde_json::to_string(&fixture.repository).unwrap()
        ),
    );
    package(
        &fixture.root(),
        "consumer-root",
        "0.1.0",
        "[dependencies]\nbridge = { path = \"../bridge\" }\n",
    );
    cargo(&fixture.root(), &["metadata", "--format-version", "1"]);
    let paths = fixture.paths();
    let original = fs::read(&paths[1]).unwrap();
    let locks = [(paths[1].as_path(), paths[2].as_path())];
    let link = LocalLink {
        repository: &fixture.repository,
        patch: &paths[0],
        locks: &locks,
    };
    assert_eq!(link.link(&fixture.first).unwrap(), ["shared"]);
    let metadata = super::metadata(&fixture.root(), false).unwrap();
    let shared = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "shared")
        .unwrap();
    assert!(shared["source"].is_null());
    assert_eq!(
        Path::new(shared["manifest_path"].as_str().unwrap()).parent(),
        Some(fixture.first.as_path())
    );
    link.unlink().unwrap();
    assert_eq!(fs::read(&paths[1]).unwrap(), original);
}

#[test]
fn failed_relink_preserves_the_active_checkout_and_original_locks() {
    let fixture = Fixture::new();
    let original = fixture.state();
    let names = fixture.with_link(|link| link.link(&fixture.first)).unwrap();
    assert_eq!(names, ["shared"]);
    let linked = fixture.state();
    assert_ne!(linked[1], original[1]);
    assert_ne!(linked[3], original[3]);
    assert_eq!(linked[2], original[1]);
    assert_eq!(linked[4], original[3]);
    // The root accepts version 2, but the fuzz workspace requires version 1.
    let error = fixture
        .with_link(|link| link.link(&fixture.second))
        .unwrap_err();
    assert!(error.contains("still resolves to Git"), "{error}");
    assert!(error.contains("fuzz/Cargo.lock"), "{error}");
    assert_eq!(fixture.state(), linked);
    let restored = fixture.with_link(|link| link.unlink()).unwrap();
    assert_eq!(
        restored,
        [
            fixture.root().join("Cargo.lock"),
            fixture.root().join("fuzz/Cargo.lock")
        ]
    );
    let unlinked = fixture.state();
    assert_eq!(unlinked[1], original[1]);
    assert_eq!(unlinked[3], original[3]);
    assert!(unlinked[0].is_none());
    assert!(unlinked[2].is_none());
    assert!(unlinked[4].is_none());
}

#[test]
fn failed_first_link_preserves_original_locks_and_absent_overrides() {
    let fixture = Fixture::new();
    let original = fixture.state();
    assert!(
        fixture
            .with_link(|link| link.link(&fixture.second))
            .is_err()
    );
    assert_eq!(fixture.state(), original);
    assert!(fixture.with_link(|link| link.unlink()).unwrap().is_empty());
}

fn add_new_packages(checkout: &Path, version: &str) {
    let manifest = checkout.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap().replace(
        "[workspace]",
        "[workspace]\nmembers = [\"new-package\", \"unused-package\"]",
    );
    fs::write(manifest, text).unwrap();
    for name in ["new-package", "unused-package"] {
        let directory = checkout.join(name);
        package(&directory, name, version, "");
        let manifest = directory.join("Cargo.toml");
        let text = fs::read_to_string(&manifest)
            .unwrap()
            .replace("[workspace]\n", "");
        fs::write(manifest, text).unwrap();
    }
}

fn require_new_package(fixture: &Fixture, directory: &Path, requirement: &str) {
    let manifest = directory.join("Cargo.toml");
    let mut text = fs::read_to_string(&manifest).unwrap();
    text.push_str(&format!(
        "new-package = {{ git = {}, branch = \"master\", version = {} }}\n",
        serde_json::to_string(&fixture.repository).unwrap(),
        serde_json::to_string(requirement).unwrap()
    ));
    fs::write(manifest, text).unwrap();
}

#[test]
fn a_new_shared_package_links_before_it_exists_in_the_git_source() {
    let fixture = Fixture::new();
    add_new_packages(&fixture.first, "1.0.0");
    require_new_package(&fixture, &fixture.root(), "1.0.0");
    require_new_package(&fixture, &fixture.root().join("fuzz"), "1.0.0");
    let original = fixture.state();
    let names = fixture.with_link(|link| link.link(&fixture.first)).unwrap();
    assert_eq!(names, ["new-package", "shared"]);
    let patch = fs::read_to_string(&fixture.paths()[0]).unwrap();
    assert!(patch.contains("new-package"));
    assert!(!patch.contains("unused-package"));
    for directory in [fixture.root(), fixture.root().join("fuzz")] {
        let graph = super::metadata(&directory, false).unwrap();
        let package = graph["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|package| package["name"] == "new-package")
            .unwrap();
        assert!(package["source"].is_null());
    }
    fixture.with_link(|link| link.unlink()).unwrap();
    assert_eq!(fixture.state(), original);
}

#[test]
fn unpublished_package_failure_restores_an_active_link_and_both_original_locks() {
    let fixture = Fixture::new();
    add_new_packages(&fixture.first, "1.0.0");
    add_new_packages(&fixture.second, "2.0.0");
    require_new_package(&fixture, &fixture.root(), ">=1, <3");
    require_new_package(&fixture, &fixture.root().join("fuzz"), "=1.0.0");
    let original = fixture.state();
    fixture.with_link(|link| link.link(&fixture.first)).unwrap();
    let linked = fixture.state();
    assert!(
        fixture
            .with_link(|link| link.link(&fixture.second))
            .is_err()
    );
    assert_eq!(fixture.state(), linked);
    fixture.with_link(|link| link.unlink()).unwrap();
    assert_eq!(fixture.state(), original);
}

fn package(root: &Path, name: &str, version: &str, dependencies: &str) {
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 1 }\n").unwrap();
    fs::write(root.join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2024\"\n[workspace]\n{dependencies}")).unwrap();
}

fn command(root: &Path, executable: &str, args: &[&str]) {
    let output = Command::new(executable)
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cargo(root: &Path, args: &[&str]) {
    let output = crate::cargo_command()
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
