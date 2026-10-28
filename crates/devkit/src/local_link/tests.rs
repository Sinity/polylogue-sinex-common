use super::LocalLink;
use std::{
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
            ".cargo/link.off",
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
            (paths[2].as_path(), paths[3].as_path()),
            (paths[4].as_path(), paths[5].as_path()),
        ];
        operation(LocalLink {
            repository: &self.repository,
            patch: &paths[0],
            disabled: &paths[1],
            locks: &locks,
        })
    }
}

#[test]
fn failed_relink_preserves_the_active_checkout_and_original_locks() {
    let fixture = Fixture::new();
    let original = fixture.state();
    let names = fixture.with_link(|link| link.link(&fixture.first)).unwrap();
    assert_eq!(names, ["shared"]);
    let linked = fixture.state();
    assert_ne!(linked[2], original[2]);
    assert_ne!(linked[4], original[4]);
    assert_eq!(linked[3], original[2]);
    assert_eq!(linked[5], original[4]);
    // The root accepts version 2, but the fuzz workspace requires version 1.
    let error = fixture
        .with_link(|link| link.link(&fixture.second))
        .unwrap_err();
    assert!(error.contains("still resolves to Git"), "{error}");
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
    assert_eq!(unlinked[2], original[2]);
    assert_eq!(unlinked[4], original[4]);
    assert!(unlinked[0].is_none());
    assert!(unlinked[3].is_none());
    assert!(unlinked[5].is_none());
    assert!(unlinked[1].is_some());
}

#[test]
fn failed_first_link_restores_absence_and_the_existing_opt_out() {
    let fixture = Fixture::new();
    fs::write(
        fixture.root().join(".cargo/link.off"),
        b"synthetic opt out\n",
    )
    .unwrap();
    let original = fixture.state();
    assert!(
        fixture
            .with_link(|link| link.link(&fixture.second))
            .is_err()
    );
    assert_eq!(fixture.state(), original);
    assert!(fixture.with_link(|link| link.unlink()).unwrap().is_empty());
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
