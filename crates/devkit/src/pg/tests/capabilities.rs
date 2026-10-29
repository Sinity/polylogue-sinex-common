use super::*;

fn cluster_directory() -> tempfile::TempDir {
    tempfile::tempdir_in(if Path::new("/dev/shm").is_dir() {
        Path::new("/dev/shm")
    } else {
        Path::new("/tmp")
    })
    .unwrap()
}

fn selection(service: &Server, preload: &str) -> PathBuf {
    let runtime = tempfile::tempdir_in(service.socket()).unwrap();
    fs::create_dir(runtime.path().join("devkit")).unwrap();
    std::os::unix::fs::symlink(service.socket(), runtime.path().join("devkit/pg")).unwrap();
    let marker = runtime.path().join("selection");
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "pg::tests::capabilities::selection_child"])
        .env("DEVKIT_PG_SELECTION_PROBE", &marker)
        .env("DEVKIT_PG_PRELOAD_PROBE", preload)
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
    let selected = selection(&service, "");
    assert_eq!(selected, service.socket());
    assert!(service.ready());
}

#[test]
fn missing_service_extension_uses_a_capable_owned_cluster() {
    // PostgreSQL 18 can select its extension controls independently of the
    // server binary. This fixture is a real older-capability service without
    // changing the installed tools or the operator's service.
    let controls = cluster_directory();
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
    let selected = selection(&service, "");
    assert_ne!(selected, service.socket());
    assert!(!selected.exists(), "the owned fallback is removed on drop");
    assert!(service.ready(), "the incompatible service is untouched");
}

#[test]
fn matching_preload_is_reused_and_missing_preload_gets_an_owned_cluster() {
    for loaded in [false, true] {
        let options = ClusterOptions {
            preload: if loaded {
                vec!["pg_stat_statements".into()]
            } else {
                Vec::new()
            },
            ..Default::default()
        };
        let service = Server::start(&options).unwrap();
        let selected = selection(&service, "pg_stat_statements");
        if loaded {
            assert_eq!(selected, service.socket());
        } else {
            assert_ne!(selected, service.socket());
            assert!(!selected.exists());
        }
        assert!(service.ready());
    }
}

#[test]
fn an_incapable_owned_cluster_fails_by_extension_name_and_is_removed() {
    let base = cluster_directory();
    if crate::owner::running_as_root() {
        give_to_nobody(base.path()).unwrap();
    }
    let options = ClusterOptions {
        required_extensions: vec!["devkit_missing_fixture_extension".into()],
        ..Default::default()
    };
    let error = match Server::start_in(&options, base.path()) {
        Err(error) => error,
        Ok(_) => panic!("an incapable cluster must not reach schema bootstrap"),
    };
    assert!(
        error.contains("devkit_missing_fixture_extension"),
        "{error}"
    );
    assert_eq!(fs::read_dir(base.path()).unwrap().count(), 0);
}

#[test]
fn selection_child() {
    let Some(marker) = std::env::var_os("DEVKIT_PG_SELECTION_PROBE") else {
        return;
    };
    let preload = std::env::var("DEVKIT_PG_PRELOAD_PROBE").unwrap();
    let options = ClusterOptions {
        required_extensions: vec!["plpgsql".into()],
        preload: if preload.is_empty() {
            Vec::new()
        } else {
            vec![preload.clone()]
        },
        ..Default::default()
    };
    let selected = Server::service_or_start(&options).unwrap();
    if !preload.is_empty() {
        assert_eq!(
            selected
                .sql("postgres", "SHOW shared_preload_libraries")
                .unwrap()
                .trim(),
            preload
        );
    }
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
        selected
            .socket()
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
    )
    .unwrap();
}
