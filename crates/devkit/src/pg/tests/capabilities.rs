use super::*;

fn selection(service: &Server) -> PathBuf {
    let runtime = tempfile::tempdir_in(service.socket()).unwrap();
    fs::create_dir(runtime.path().join("devkit")).unwrap();
    std::os::unix::fs::symlink(service.socket(), runtime.path().join("devkit/pg")).unwrap();
    let marker = runtime.path().join("selection");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "pg::tests::capabilities::selection_child"])
        .env("DEVKIT_PG_SELECTION_PROBE", &marker)
        .env("XDG_RUNTIME_DIR", runtime.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    PathBuf::from(fs::read_to_string(marker).unwrap())
}

#[test]
fn matching_service_is_reused_and_left_running() {
    let service = Server::start(&ClusterOptions::default()).unwrap();
    let selected = selection(&service);
    assert_eq!(selected, service.socket());
    assert!(service.ready());
}

#[test]
fn missing_service_extension_uses_a_capable_owned_cluster() {
    // PostgreSQL 18 can select its extension controls independently of the
    // server binary. This fixture is a real older-capability service without
    // changing the installed tools or the operator's service.
    let controls = tempfile::tempdir_in("/dev/shm").unwrap();
    if crate::owner::running_as_root() {
        give_to_nobody(controls.path()).unwrap();
    }
    let service = Server::start(&ClusterOptions {
        settings: vec![(
            "extension_control_path".into(),
            controls.path().display().to_string(),
        )],
        ..Default::default()
    })
    .unwrap();
    assert!(
        service
            .sql(
                "postgres",
                "SELECT default_version FROM pg_available_extensions WHERE name='plpgsql'"
            )
            .unwrap()
            .trim()
            .is_empty()
    );
    let selected = selection(&service);
    assert_ne!(selected, service.socket());
    assert!(!selected.exists(), "the owned fallback is removed on drop");
    assert!(service.ready(), "the incompatible service is untouched");
}

#[test]
fn selection_child() {
    let Some(marker) = std::env::var_os("DEVKIT_PG_SELECTION_PROBE") else {
        return;
    };
    let selected = Server::service_or_start(&ClusterOptions {
        required_extensions: vec!["plpgsql".into()],
        ..Default::default()
    })
    .unwrap();
    assert!(
        !selected
            .sql(
                "postgres",
                "SELECT default_version FROM pg_available_extensions WHERE name='plpgsql'"
            )
            .unwrap()
            .trim()
            .is_empty()
    );
    fs::write(
        marker,
        selected.socket().canonicalize().unwrap().display().to_string(),
    )
    .unwrap();
}
