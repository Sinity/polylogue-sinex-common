use super::*;

#[test]
fn package_names_from_ids() {
    assert_eq!(package_name("path+file:///x/xtask#0.1.0"), "xtask");
    assert_eq!(
        package_name("path+file:///x/crates/model#polylogue-model@0.1.0"),
        "polylogue-model"
    );
    assert_eq!(
        package_name("registry+https://example.org/index#serde@1.0.0"),
        "serde"
    );
}

#[test]
fn ansi_is_stripped() {
    assert_eq!(
        strip_ansi("\u{1b}[1m\u{1b}[91merror\u{1b}[0m: x"),
        "error: x"
    );
}

#[test]
fn diagnostics_keep_warnings_and_errors_only() {
    let message: Value = serde_json::json!({
        "reason": "compiler-message",
        "package_id": "path+file:///x/crates/a#a@0.1.0",
        "message": {
            "level": "warning",
            "code": {"code": "unused_variables"},
            "rendered": "warning: unused",
            "spans": [{"is_primary": true, "file_name": "src/lib.rs", "line_start": 3}]
        }
    });
    let d = diagnostic(&message).unwrap();
    assert_eq!(
        (d.package.as_str(), d.line, d.code.as_deref()),
        ("a", Some(3), Some("unused_variables"))
    );
    let mut note = message.clone();
    note["message"]["level"] = "note".into();
    assert!(diagnostic(&note).is_none());
}
