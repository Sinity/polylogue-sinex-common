use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize, JsonSchema)]
struct Empty {}
#[derive(Deserialize, Serialize, JsonSchema)]
struct Ping {
    target: String,
    message: String,
}
fn words(input: &[&str]) -> Vec<String> {
    input.iter().map(|word| (*word).into()).collect()
}
fn spec(name: &str, command: &[&str]) -> OperationSpec {
    let mut spec =
        OperationSpec::request(name, command, "unused", "Example", "Exercise CLI globals");
    spec.kind = OperationKind::Local;
    spec.http_route = None;
    spec
}
fn globals(builder: &mut CatalogBuilder) {
    for (name, help) in [
        ("archive", "Archive directory"),
        ("config", "Configuration file"),
        ("listen", "HTTP listen address"),
        ("auth_token_file", "HTTP bearer token file"),
    ] {
        builder.declare_cli_global(name, help).unwrap();
    }
}
fn catalog() -> std::sync::Arc<OperationCatalog> {
    let mut builder = OperationCatalog::builder("example", Deadlines::default());
    globals(&mut builder);
    builder
        .declare_local::<Empty>(spec("daemon", &["daemon"]))
        .unwrap();
    builder
        .declare_local::<Empty>(spec("sink", &["sink"]))
        .unwrap();
    builder
        .declare_local::<Empty>(spec("add", &["sink", "add"]))
        .unwrap();
    let mut ping = spec("ping", &["ping"]);
    ping.arguments = vec![
        ArgumentSpec::new("target", ArgumentForm::Positional, "Target"),
        ArgumentSpec::new("message", ArgumentForm::Rest, "Message"),
    ];
    builder.declare_local::<Ping>(ping).unwrap();
    builder.set_default_operation("ping").unwrap();
    builder.finish()
}
#[test]
fn caller_globals_select_commands_and_preserve_envelope_options() {
    let catalog = catalog();
    let expected = catalog
        .parse_cli(
            &words(&[
                "sink",
                "add",
                "--archive",
                "/example",
                "--config",
                "settings",
                "--listen",
                "127.0.0.1:1234",
                "--auth-token-file",
                "token",
                "--json",
                "--deadline",
                "100",
                "--request-id",
                "chosen",
                "--budget",
                "30",
                "--cursor",
                "next",
                "--fresh",
                "--request",
                "{}",
            ]),
            "unused",
        )
        .unwrap();
    let interleaved = catalog
        .parse_cli(
            &words(&[
                "--archive=/example",
                "--json",
                "sink",
                "--config",
                "settings",
                "--deadline",
                "100",
                "--listen=127.0.0.1:1234",
                "--request-id",
                "chosen",
                "add",
                "--auth-token-file=token",
                "--budget",
                "30",
                "--cursor",
                "next",
                "--fresh",
                "--request",
                "{}",
            ]),
            "unused",
        )
        .unwrap();
    assert_eq!(interleaved, expected);
    assert_eq!(expected.request.arguments, json!({}));
    assert_eq!(expected.calling_process, json!({"archive":"/example", "config":"settings", "listen":"127.0.0.1:1234", "auth_token_file":"token"}).as_object().unwrap().clone());
    let daemon = catalog
        .parse_cli(&words(&["--archive", "/example", "daemon"]), "r")
        .unwrap();
    assert_eq!(daemon.request.operation, "daemon");
    assert_eq!(daemon.calling_process["archive"], "/example");
}
#[test]
fn caller_globals_do_not_consume_rest_or_literal_separator_words() {
    let catalog = catalog();
    for input in [
        words(&["ping", "target", "--", "literal", "--archive", "/example"]),
        words(&["ping", "target", "literal", "--archive", "/example"]),
    ] {
        let parsed = catalog.parse_cli(&input, "r").unwrap();
        assert_eq!(
            parsed.request.arguments,
            json!({"target":"target", "message":"literal --archive /example"})
        );
        assert!(parsed.calling_process.is_empty());
    }
    let parsed = catalog
        .parse_cli(
            &words(&[
                "--archive",
                "/actual",
                "ping",
                "target",
                "--",
                "--archive",
                "/literal",
            ]),
            "r",
        )
        .unwrap();
    assert_eq!(
        parsed.request.arguments,
        json!({"target":"target", "message":"--archive /literal"})
    );
    assert_eq!(parsed.calling_process["archive"], "/actual");
}
#[test]
fn global_projection_and_help_use_the_same_catalog_declarations() {
    let catalog = catalog();
    assert_eq!(catalog.cli_globals().len(), 4);
    for name in ["daemon", "add", "ping"] {
        let arguments = catalog.cli_arguments(name).unwrap();
        for global in catalog.cli_globals() {
            let projected = arguments
                .iter()
                .find(|argument| argument.spec.name == global.name)
                .unwrap();
            assert_eq!(&projected.spec, global);
            assert_eq!(projected.kind, ValueKind::Text);
            assert!(!projected.required);
            assert!(projected.spec.calling_process);
            assert!(catalog.help_for(name).unwrap().contains(&global.help));
        }
    }
    assert!(catalog.help().contains("--auth-token-file AUTH_TOKEN_FILE"));
}
#[derive(Deserialize, Serialize, JsonSchema)]
struct Collision {
    archive: String,
}
#[test]
fn global_collisions_are_rejected_in_both_declaration_orders() {
    let mut before = OperationCatalog::builder("example", Deadlines::default());
    before.declare_cli_global("archive", "Archive").unwrap();
    assert!(
        before
            .declare_local::<Collision>(spec("collision", &["collision"]))
            .is_err()
    );
    assert!(before.declare_cli_global("archive", "Duplicate").is_err());
    for name in ["json", "request_id", "not-plain"] {
        assert!(before.declare_cli_global(name, "Invalid").is_err());
    }
    let mut after = OperationCatalog::builder("example", Deadlines::default());
    after
        .declare_local::<Collision>(spec("collision", &["collision"]))
        .unwrap();
    assert!(after.declare_cli_global("archive", "Archive").is_err());
    // Failed declarations leave no partial metadata behind.
    assert!(after.cli_globals().is_empty());
    let mut caller = spec("caller", &["caller"]);
    caller
        .arguments
        .push(ArgumentSpec::new("config", ArgumentForm::Named, "Caller config").calling_process());
    after.declare_local::<Empty>(caller).unwrap();
    assert!(after.declare_cli_global("config", "Config").is_err());
}
#[test]
fn caller_global_values_are_text_and_duplicate_or_missing_values_fail() {
    let catalog = catalog();
    assert_eq!(
        catalog
            .parse_cli(&words(&["daemon", "--archive=123"]), "r")
            .unwrap()
            .calling_process["archive"],
        "123"
    );
    for input in [
        words(&["--archive"]),
        words(&["sink", "--archive"]),
        words(&["daemon", "--archive"]),
        words(&["--archive", "a", "daemon", "--archive", "b"]),
    ] {
        assert!(catalog.parse_cli(&input, "r").is_err());
    }
}
