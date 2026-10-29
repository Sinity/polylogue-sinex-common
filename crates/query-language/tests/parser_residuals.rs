mod support;

use query_language::{
    ErrorKind,
    ast::{ConditionKind, Source, Value},
    catalog::{Catalog, Field, FieldKind, OutputSchema, OutputSource, SourceOutput},
    format::format_query,
    identity::normal_form,
    parse::parse,
};

fn catalogs() -> [(Catalog, &'static str); 2] {
    [
        (support::polylogue(), "origin"),
        (support::sinex(), "source"),
    ]
}

#[test]
fn leading_saved_or_is_the_same_boolean_tree_as_parenthesized_conditions() {
    for (catalog, field) in catalogs() {
        let text = format!("query:ExactCase(value=\"OR\") OR {field}:DemoCase");
        let query = parse(&text, &catalog).unwrap();
        let Source::Unit {
            unit,
            implicit: true,
            condition: Some(condition),
        } = &query.source
        else {
            panic!("{query:?}")
        };
        assert_eq!(unit, &catalog.default_unit);
        let ConditionKind::Or { conditions } = &condition.kind else {
            panic!("{condition:?}")
        };
        assert!(
            matches!(&conditions[0].kind, ConditionKind::Saved { name, arguments } if name == "ExactCase" && matches!(&arguments[0].value, Value::Scalar { text, .. } if text == "OR"))
        );
        assert!(
            matches!(&conditions[1].kind, ConditionKind::Match { field: actual, value: Value::Scalar { text, .. } } if actual == field && text == "DemoCase")
        );
        let grouped = parse(&format!("({text})"), &catalog).unwrap();
        assert_eq!(
            normal_form(&query, &catalog).unwrap(),
            normal_form(&grouped, &catalog).unwrap()
        );
        let rendered = format_query(&query, &catalog).unwrap();
        assert_eq!(
            normal_form(&query, &catalog).unwrap(),
            normal_form(&parse(&rendered, &catalog).unwrap(), &catalog).unwrap()
        );
    }
}

#[test]
fn saved_sources_keep_their_own_suffix_schema_outside_boolean_membership() {
    for (mut catalog, native_field) in catalogs() {
        catalog.source_outputs.push(SourceOutput {
            source: OutputSource::Saved {
                name: "ExactCase".into(),
                arguments: Vec::new(),
            },
            schema: OutputSchema {
                unit: Some(catalog.default_unit.clone()),
                fields: vec![Field {
                    name: "promoted".into(),
                    kind: FieldKind::Integer,
                    ..Field::default()
                }],
                main_time: None,
                match_patterns: Vec::new(),
            },
        });
        for text in [
            "query:ExactCase promoted > 0",
            "query:ExactCase | where promoted > 0",
        ] {
            let query = parse(text, &catalog).unwrap();
            assert!(matches!(query.source, Source::Saved { .. }));
            query_language::bind::bind(&query, &catalog).unwrap();
        }
        let query = parse(&format!("query:ExactCase OR {native_field}:demo"), &catalog).unwrap();
        assert!(matches!(query.source, Source::Unit { implicit: true, .. }));
        assert_eq!(
            parse("query:ExactCase OR promoted > 0", &catalog)
                .unwrap_err()
                .kind,
            ErrorKind::UnknownField
        );
        for text in [
            "\"query:ExactCase OR promoted\"".to_owned(),
            format!("{native_field}:\"query:ExactCase OR promoted\""),
        ] {
            let query = parse(&text, &catalog).unwrap();
            assert!(matches!(query.source, Source::Unit { .. }));
        }
    }
}

#[test]
fn missing_parentheses_offer_only_a_complete_executable_closure() {
    for (catalog, field) in catalogs() {
        for (text, suffix) in [
            (format!("({field}:demo OR {field}:other"), ")"),
            (format!("(({field}:demo"), "))"),
            (format!("({field}:\"zażółć (\""), ")"),
        ] {
            let error = parse(&text, &catalog).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidQuery);
            assert_eq!(error.position, text.len());
            assert_eq!(
                error.suggestion.as_deref(),
                Some(format!("{text}{suffix}").as_str())
            );
            parse(error.suggestion.as_deref().unwrap(), &catalog).unwrap();
        }
        for text in [
            "(".to_owned(),
            format!("({field}:"),
            format!("({field}:demo OR"),
            "| sort by random(".into(),
        ] {
            assert!(
                parse(&text, &catalog).unwrap_err().suggestion.is_none(),
                "{text}"
            );
        }
        parse("\"(\"", &catalog).unwrap();
    }
}

#[test]
fn incomplete_saved_boolean_completion_uses_the_enclosing_native_domain() {
    for (mut catalog, native) in catalogs() {
        catalog.source_outputs.push(SourceOutput {
            source: OutputSource::Saved {
                name: "ExactCase".into(),
                arguments: Vec::new(),
            },
            schema: OutputSchema {
                unit: Some(catalog.default_unit.clone()),
                fields: vec![Field {
                    name: "promoted".into(),
                    ..Field::default()
                }],
                ..OutputSchema::default()
            },
        });
        let text = "query:ExactCase OR ";
        let candidates = query_language::complete::complete(text, text.len(), &catalog);
        assert!(
            candidates
                .iter()
                .any(|candidate| candidate.value.starts_with(native))
        );
        assert!(
            !candidates
                .iter()
                .any(|candidate| candidate.value.starts_with("promoted"))
        );
    }
}
