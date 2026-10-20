use std::{env, ffi::OsStr, process::Command};

/// A child Cargo command without the calling package's metadata.
///
/// Cargo sets package context for the child itself. Inheriting the xtask's
/// context can invalidate build scripts that track those variables when two
/// checkouts share a target directory. Toolchain, target, compiler and job
/// configuration remain inherited; callers may add explicit overrides.
pub fn cargo_command() -> Command {
    let mut command = Command::new("cargo");
    remove_package_context(&mut command);
    command
}

fn remove_package_context(command: &mut Command) {
    for (key, _) in env::vars_os() {
        if is_package_context(&key) {
            command.env_remove(key);
        }
    }
}

fn is_package_context(key: &OsStr) -> bool {
    key == "CARGO_MANIFEST_DIR"
        || key == "CARGO_MANIFEST_PATH"
        || key.as_encoded_bytes().starts_with(b"CARGO_PKG_")
}

#[cfg(test)]
mod tests;
