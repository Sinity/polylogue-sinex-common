use super::*;

#[test]
fn declared_builtin_lookup_canonicalizes_and_refuses_case_collisions() {
    assert_eq!(builtin("DAY", &["day", "hour"], 7).unwrap(), Some("day"));
    assert_eq!(builtin("CUSTOM", &["day", "hour"], 7).unwrap(), None);
    let error = builtin("day", &["day", "DAY"], 7).unwrap_err();
    assert_eq!(error.kind, ErrorKind::Ambiguous);
    assert_eq!(error.position, 7);
}
