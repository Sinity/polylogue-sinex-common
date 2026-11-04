//! A query-language refusal becomes a `Failure` through one conversion.
use command_surface::{Failure, FailureKind, Position};
use query_language::{Error, ErrorKind};

#[test]
fn every_language_kind_converts_to_the_failure_kind_of_the_same_name() {
    for kind in ErrorKind::ALL {
        let failure = Failure::from(Error::new(kind, "refused", 3));
        let name = serde_json::to_value(failure.kind).unwrap();
        assert_eq!(name, kind.code(), "{kind:?}");
        assert!(FailureKind::ALL.contains(&failure.kind));
        assert_eq!(
            failure.kind.exit_code(),
            2,
            "a refused query is a usage error"
        );
    }
}

#[test]
fn position_suggestion_candidates_and_hint_survive_the_conversion() {
    let error = Error::new(ErrorKind::UnknownField, "unknown field `reop`", 15)
        .length(4)
        .suggestion("sessions where repo:sinex")
        .candidates(vec!["repo".into(), "role".into()])
        .hint("fields: repo, role");
    let failure = Failure::from(error);
    assert_eq!(failure.kind, FailureKind::UnknownField);
    assert_eq!(failure.message, "unknown field `reop`");
    assert_eq!(
        failure.position,
        Some(Position {
            offset: 15,
            length: 4
        })
    );
    assert_eq!(
        failure.suggestion.as_deref(),
        Some(&serde_json::json!("sessions where repo:sinex"))
    );
    assert_eq!(failure.candidates(), ["repo", "role"]);
    assert_eq!(failure.hint.as_deref(), Some("fields: repo, role"));
    assert!(!failure.retryable);
}

#[test]
fn a_parser_refusal_arrives_as_a_failure_whose_suggestion_runs() {
    let mut catalog = query_language::catalog::Catalog::new("sessions");
    catalog.units.push(query_language::catalog::Unit {
        name: "sessions".into(),
        fields: vec![query_language::catalog::Field {
            name: "repo".into(),
            ..Default::default()
        }],
        ..Default::default()
    });
    let error = query_language::parse::parse("sessions where reop:sinex", &catalog).unwrap_err();
    let failure = Failure::from(error);
    assert_eq!(failure.kind, FailureKind::UnknownField);
    let suggestion = failure.suggestion.as_deref().unwrap().as_str().unwrap();
    assert_eq!(suggestion, "sessions where repo:sinex");
    query_language::parse::parse(suggestion, &catalog).unwrap();
    assert_eq!(failure.candidates(), ["repo"]);
}
