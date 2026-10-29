use query_language::{
    ErrorKind,
    ast::*,
    catalog::*,
    complete::{complete, requirements},
    format::format_query,
    identity::question_id,
    parse::parse,
};
use std::collections::BTreeMap;

fn catalog() -> Catalog {
    let mut catalog = Catalog::new("events");
    catalog.units.push(Unit {
        name: "events".into(),
        main_time: Some("when".into()),
        fields: vec![
            Field {
                name: "when".into(),
                kind: FieldKind::Time,
                ..Field::default()
            },
            Field {
                name: "source".into(),
                kind: FieldKind::Match,
                ..Field::default()
            },
            Field {
                name: "query".into(),
                values: vec!["Écho".into(), "two words".into()],
                ..Field::default()
            },
            Field {
                name: "result".into(),
                values: vec!["Écho".into(), "two words".into()],
                ..Field::default()
            },
        ],
        ..Unit::default()
    });
    catalog
}

#[test]
fn reference_colon_spacing_preserves_identity_and_canonical_format() {
    let catalog = catalog();
    for namespace in ["query", "result"] {
        let original = parse(&format!("{namespace}:Écho"), &catalog).unwrap();
        for text in [
            format!("{namespace} :Écho"),
            format!("{namespace}: Écho"),
            format!("{} : Écho", namespace.to_uppercase()),
            format!("{namespace} : \"Écho\""),
        ] {
            let query = parse(&text, &catalog).unwrap();
            assert_eq!(
                question_id(&original, &catalog, &BTreeMap::new()).unwrap(),
                question_id(&query, &catalog, &BTreeMap::new()).unwrap(),
                "{text}"
            );
            assert_eq!(
                format_query(&query, &catalog).unwrap(),
                format_query(&original, &catalog).unwrap()
            );
        }
    }
    let query = parse("events where query : Écho", &catalog).unwrap();
    let Source::Unit {
        condition: Some(condition),
        ..
    } = query.source
    else {
        panic!()
    };
    assert_eq!(condition.span, Span { start: 13, end: 26 });
    assert!(matches!(condition.kind, ConditionKind::Saved { name, .. } if name == "Écho"));
    assert_ne!(
        question_id(
            &parse("query:Écho", &catalog).unwrap(),
            &catalog,
            &BTreeMap::new()
        )
        .unwrap(),
        question_id(
            &parse("query:écho", &catalog).unwrap(),
            &catalog,
            &BTreeMap::new()
        )
        .unwrap()
    );
}

#[test]
fn declared_builtin_names_are_case_insensitive_but_fields_and_text_are_exact() {
    let catalog = catalog();
    for name in KEY_FUNCTIONS {
        let lower = parse(&format!("events | group by {name}(when)"), &catalog).unwrap();
        let upper = parse(
            &format!("events | group by {}(when)", name.to_uppercase()),
            &catalog,
        )
        .unwrap();
        assert_eq!(
            question_id(&lower, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&upper, &catalog, &BTreeMap::new()).unwrap()
        );
        assert_eq!(
            format_query(&upper, &catalog).unwrap(),
            format_query(&lower, &catalog).unwrap()
        );
    }
    for text in ["TODO:", "events where Source:synthetic"] {
        assert_eq!(
            parse(text, &catalog).unwrap_err().kind,
            ErrorKind::UnknownField,
            "{text}"
        );
    }
    parse("\"TODO:\"", &catalog).unwrap();
    assert!(parse("events | group by UNKNOWN(when)", &catalog).is_err());
    assert!(parse("source:a and source:b", &catalog).is_err());
    assert!(parse("source:a AND source:b", &catalog).is_ok());
    assert_ne!(
        question_id(
            &parse("source:A", &catalog).unwrap(),
            &catalog,
            &BTreeMap::new()
        )
        .unwrap(),
        question_id(
            &parse("source:a", &catalog).unwrap(),
            &catalog,
            &BTreeMap::new()
        )
        .unwrap()
    );
}

#[test]
fn spaced_reference_completion_uses_original_utf8_spans_and_decoded_prefixes() {
    let catalog = catalog();
    for namespace in ["query", "result"] {
        for prefix in [
            format!("{namespace}:É"),
            format!("{namespace} :É"),
            format!("{namespace}: É"),
            format!("{} : É", namespace.to_uppercase()),
        ] {
            let text = format!("{prefix}old");
            let requirement = requirements(&text, prefix.len(), &catalog).value.unwrap();
            assert_eq!(
                (requirement.field.as_str(), requirement.prefix.as_str()),
                (namespace, "É")
            );
            let completion = complete(&text, prefix.len(), &catalog)
                .into_iter()
                .find(|value| value.value == "Écho")
                .unwrap();
            let mut applied = text.clone();
            applied.replace_range(
                completion.replace.start..completion.replace.end,
                &completion.value,
            );
            let query = parse(&applied, &catalog).unwrap();
            assert_eq!(
                format_query(&query, &catalog).unwrap(),
                format_query(
                    &parse(&format!("{namespace}:Écho"), &catalog).unwrap(),
                    &catalog
                )
                .unwrap()
            );
        }
        let prefix = format!("{} : \"two", namespace.to_uppercase());
        assert_eq!(
            requirements(&prefix, prefix.len(), &catalog)
                .value
                .unwrap()
                .prefix,
            "two"
        );
        let completion = complete(&prefix, prefix.len(), &catalog)
            .into_iter()
            .find(|value| value.value == "\"two words\"")
            .unwrap();
        let mut applied = prefix.clone();
        applied.replace_range(
            completion.replace.start..completion.replace.end,
            &completion.value,
        );
        let query = parse(&applied, &catalog).unwrap();
        let formatted = format_query(&query, &catalog).unwrap();
        assert_eq!(
            question_id(&query, &catalog, &BTreeMap::new()).unwrap(),
            question_id(
                &parse(&formatted, &catalog).unwrap(),
                &catalog,
                &BTreeMap::new()
            )
            .unwrap()
        );
        assert!(
            complete(&format!("{namespace} : missing | where "), 1000, &catalog)
                .into_iter()
                .all(|value| value.kind != query_language::complete::CompletionKind::Field)
        );
    }
}

#[test]
fn conditional_saved_and_kept_names_use_the_same_spacing_and_quoting() {
    let catalog = catalog();
    for namespace in ["query", "result"] {
        for name in ["Case", "two words", "$literal"] {
            let a = parse(
                &format!(
                    "events where {namespace}:{}",
                    serde_json::to_string(name).unwrap()
                ),
                &catalog,
            )
            .unwrap();
            let b = parse(
                &format!(
                    "events where {} : {}",
                    namespace.to_uppercase(),
                    serde_json::to_string(name).unwrap()
                ),
                &catalog,
            )
            .unwrap();
            assert_eq!(
                question_id(&a, &catalog, &BTreeMap::new()).unwrap(),
                question_id(&b, &catalog, &BTreeMap::new()).unwrap()
            );
            format_query(&b, &catalog).unwrap();
        }
    }
}
