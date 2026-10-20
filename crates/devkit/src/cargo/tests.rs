use super::*;

#[test]
fn package_context_keys_do_not_include_build_configuration() {
    for key in [
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
        "CARGO_PKG_NAME",
        "CARGO_PKG_VERSION",
        "CARGO_PKG_CUSTOM",
    ] {
        assert!(is_package_context(OsStr::new(key)), "{key}");
    }
    for key in [
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_JOBS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTFLAGS",
        "RUSTC_WRAPPER",
        "CARGO_BUILD_TARGET",
        "CARGO_MANIFEST_DIR_EXTRA",
    ] {
        assert!(!is_package_context(OsStr::new(key)), "{key}");
    }
}

#[test]
fn child_package_context() {
    if env::var_os("DEVKIT_CONTEXT_PROBE").is_none() {
        return;
    }
    let mut command = Command::new("env");
    remove_package_context(&mut command);
    let output = command.output().unwrap();
    assert!(output.status.success());
    let output = String::from_utf8(output.stdout).unwrap();
    let vars: std::collections::BTreeMap<_, _> = output
        .lines()
        .filter_map(|line| line.split_once('='))
        .collect();
    for key in [
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
        "CARGO_PKG_NAME",
        "CARGO_PKG_CUSTOM",
    ] {
        assert!(!vars.contains_key(key), "{key} leaked");
    }
    for key in [
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_JOBS",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
    ] {
        assert_eq!(vars.get(key), Some(&"preserved"), "{key}");
    }
}

#[test]
fn removes_inherited_package_context_in_child_without_changing_parent() {
    let mut command = Command::new(env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "cargo::tests::child_package_context",
            "--nocapture",
        ])
        .env("DEVKIT_CONTEXT_PROBE", "1");
    for key in [
        "CARGO_MANIFEST_DIR",
        "CARGO_MANIFEST_PATH",
        "CARGO_PKG_NAME",
        "CARGO_PKG_CUSTOM",
    ] {
        command.env(key, "parent-package");
    }
    for key in [
        "CARGO_HOME",
        "CARGO_TARGET_DIR",
        "CARGO_BUILD_JOBS",
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTC_WRAPPER",
    ] {
        command.env(key, "preserved");
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
