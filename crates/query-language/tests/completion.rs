mod support;
use query_language::complete::complete;
fn values(text: &str, c: &query_language::catalog::Catalog) -> Vec<String> {
    complete(text, text.len(), c)
        .into_iter()
        .map(|s| s.value)
        .collect()
}
#[test]
fn units_fields_values_stages_and_operators() {
    let c = support::sinex();
    assert!(values("comm", &c).contains(&"commands".into()));
    assert!(values("commands where exit_c", &c).contains(&"exit_code".into()));
    assert!(values("runs where status:r", &c).contains(&"running".into()));
    assert!(values("commands | gro", &c).contains(&"group by".into()));
    assert!(values("commands where exit_code ", &c).contains(&">=".into()));
}
#[test]
fn grouped_completion_uses_output_columns() {
    let c = support::polylogue();
    let text = "tool_calls | group by tool | agg count, sum:duration | where ";
    let got = values(text, &c);
    assert!(got.contains(&"count".into()), "{got:?}");
    assert!(got.contains(&"sum_duration".into()));
    assert!(!got.contains(&"exit_code".into()));
}
#[test]
fn nested_completion_binds_inner_unit() {
    let c = support::polylogue();
    let got = values("sessions where exists tool_call(", &c);
    assert!(got.contains(&"outcome".into()), "{got:?}");
    let got = values("sessions where id in (messages where ", &c);
    assert!(got.contains(&"role".into()), "{got:?}");
}
#[test]
fn quoted_values_do_not_become_grammar() {
    let c = support::mixed();
    assert!(values("pl.messages where text ~ \"sx.ev", &c).is_empty());
    let completion = complete("runs where status:ru", 20, &support::sinex());
    assert!(
        completion
            .iter()
            .any(|s| s.value == "running" && s.replace.start == 18)
    );
}

#[test]
fn closed_set_values_complete_inside_alternatives() {
    let c = support::sinex();
    assert!(values("runs where status:(running|f", &c).contains(&"finished".into()));
}

#[test]
fn completion_shares_every_declared_stage() {
    let c = support::polylogue();
    let got = values("sessions | ", &c);
    for stage in query_language::catalog::STAGES {
        assert!(
            got.contains(&stage.completion.to_string()),
            "{}",
            stage.name
        );
    }
    assert!(values("sessions | group by origin | fi", &c).contains(&"first".into()));
}

#[test]
fn closed_set_literal_completion_preserves_reserved_words_and_query_punctuation() {
    use query_language::{
        ast::{ConditionKind, Source, Value},
        parse::parse,
    };
    let mut catalog = support::sinex();
    catalog
        .units
        .iter_mut()
        .find(|u| u.name == "runs")
        .unwrap()
        .fields
        .iter_mut()
        .find(|f| f.name == "status")
        .unwrap()
        .values = vec![
        "unknown".into(),
        "a|b".into(),
        "two words".into(),
        "$parameter".into(),
    ];
    for (prefix, literal) in [
        ("runs where status:unk", "unknown"),
        ("runs where status:a", "a|b"),
        ("runs where status:t", "two words"),
        ("runs where status:$", "$parameter"),
    ] {
        let completion = complete(prefix, prefix.len(), &catalog)
            .into_iter()
            .find(|c| c.value.starts_with('"'))
            .unwrap();
        let text = format!(
            "{}{}{}",
            &prefix[..completion.replace.start],
            completion.value,
            &prefix[completion.replace.end..]
        );
        let Source::Unit {
            condition: Some(condition),
            ..
        } = parse(&text, &catalog).unwrap().source
        else {
            panic!("unit filter")
        };
        let ConditionKind::Match {
            value: Value::Scalar { text, .. },
            ..
        } = condition.kind
        else {
            panic!("closed-set literal must remain a scalar")
        };
        assert_eq!(text, literal);
    }
}

#[test]
fn closed_literal_completion_respects_custom_unknown_words_and_calendar_keywords() {
    use query_language::{
        ast::{ConditionKind, Source, Value},
        catalog::UnknownRule,
        parse::parse,
    };
    let mut catalog = support::sinex();
    let field = catalog
        .units
        .iter_mut()
        .find(|u| u.name == "runs")
        .unwrap()
        .fields
        .iter_mut()
        .find(|f| f.name == "status")
        .unwrap();
    field.values = vec!["missing".into(), "last".into(), "this".into()];
    field.unknown = UnknownRule::Word("missing".into());
    for (prefix, tail, literal) in [
        ("runs where status:m", "", "missing"),
        ("runs where status = l", " sort started", "last"),
        ("runs where status = t", " sort started", "this"),
    ] {
        let text = format!("{prefix}{tail}");
        let completion = complete(&text, prefix.len(), &catalog)
            .into_iter()
            .find(|c| c.value.starts_with('"'))
            .unwrap();
        let completed = format!(
            "{}{}{}",
            &text[..completion.replace.start],
            completion.value,
            &text[completion.replace.end..]
        );
        let Source::Unit {
            condition: Some(condition),
            ..
        } = parse(&completed, &catalog).unwrap().source
        else {
            panic!("unit condition")
        };
        let value = match condition.kind {
            ConditionKind::Match { value, .. } | ConditionKind::Compare { value, .. } => value,
            _ => panic!("scalar comparison"),
        };
        assert!(matches!(value,Value::Scalar{text,..} if text==literal));
    }
}

#[test]
fn declared_values_complete_inside_open_quotes_with_escaped_replacements() {
    use query_language::{
        ast::{ConditionKind, Source, Value},
        parse::parse,
    };
    for (mut catalog, unit, field) in [
        (support::polylogue(), "sessions", "origin"),
        (support::sinex(), "runs", "status"),
    ] {
        catalog
            .units
            .iter_mut()
            .find(|u| u.name == unit)
            .unwrap()
            .fields
            .iter_mut()
            .find(|f| f.name == field)
            .unwrap()
            .values = vec![
            "two words".into(),
            r#"a"quote\path"#.into(),
            "λ value".into(),
        ];
        for (prefix, wanted) in [
            (format!("{unit} where {field}:\"two"), "two words"),
            (format!("{unit} where {field} = \"two"), "two words"),
            (format!("{unit} where {field}:\"λ"), "λ value"),
            (format!(r#"{unit} where {field}:"a\"q"#), r#"a"quote\path"#),
        ] {
            let completion = complete(&prefix, prefix.len(), &catalog)
                .into_iter()
                .find(|c| c.kind == query_language::complete::CompletionKind::Value)
                .unwrap();
            assert_eq!(completion.replace.start, prefix.find('"').unwrap());
            assert_eq!(completion.replace.end, prefix.len());
            let applied = format!(
                "{}{}",
                &prefix[..completion.replace.start],
                completion.value
            );
            let parsed = parse(&applied, &catalog).unwrap();
            let Source::Unit {
                condition: Some(condition),
                ..
            } = parsed.source
            else {
                panic!("missing condition")
            };
            let value = match condition.kind {
                ConditionKind::Match { value, .. } | ConditionKind::Compare { value, .. } => value,
                other => panic!("{other:?}"),
            };
            assert_eq!(
                value,
                Value::Scalar {
                    text: wanted.into(),
                    quoted: true
                }
            );
        }
        let text = format!("{unit} where {field}:\"two wo\"");
        let cursor = text.find("two").unwrap() + 3;
        let completion = complete(&text, cursor, &catalog)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(completion.replace.end, text.len());
        let applied = format!(
            "{}{}{}",
            &text[..completion.replace.start],
            completion.value,
            &text[completion.replace.end..]
        );
        assert!(parse(&applied, &catalog).is_ok());
    }
}

#[test]
fn grouped_completion_includes_implicit_count_before_explicit_aggregation() {
    for (catalog, text) in [
        (
            support::polylogue(),
            "sessions | group by origin | where co",
        ),
        (
            support::sinex(),
            "events | group by event_type | sort by co",
        ),
    ] {
        assert!(
            complete(text, text.len(), &catalog)
                .iter()
                .any(|candidate| candidate.value == "count"
                    && candidate.kind == query_language::complete::CompletionKind::Field)
        );
    }
}

#[test]
fn saved_output_completion_uses_typed_snapshot_fields_and_every_value_path() {
    use query_language::{
        ast::OutputBinding,
        catalog::{Field, FieldKind, OutputSchema, OutputSource, SourceOutput},
        complete::CompletionKind,
        parse::parse,
    };
    for (mut catalog, unit) in [
        (support::polylogue(), "tool_calls"),
        (support::sinex(), "commands"),
    ] {
        let default = catalog
            .units
            .iter_mut()
            .find(|unit| unit.name == catalog.default_unit)
            .unwrap();
        default.fields.push(Field {
            name: "outcome".into(),
            values: vec!["wrong-default".into()],
            ..Field::default()
        });
        catalog.source_outputs.push(SourceOutput {
            relation_scope: None,
            source: OutputSource::Saved {
                name: "failing".into(),
                arguments: vec![],
            },
            schema: OutputSchema {
                unit: Some(unit.into()),
                main_time: None,
                fields: vec![
                    Field {
                        name: "command".into(),
                        kind: FieldKind::Text,
                        help: "saved command".into(),
                        ..Field::default()
                    },
                    Field {
                        name: "outcome".into(),
                        kind: FieldKind::Match,
                        values: vec![
                            "error".into(),
                            "error with spaces".into(),
                            "escaped\"error\\path".into(),
                        ],
                        help: "saved outcome".into(),
                        ..Field::default()
                    },
                    Field {
                        name: "promoted_alias".into(),
                        kind: FieldKind::Integer,
                        ..Field::default()
                    },
                ],
                match_patterns: Vec::new(),
            },
        });
        let fields: Vec<_> = complete("query:failing | where ", 22, &catalog)
            .into_iter()
            .filter(|value| value.kind == CompletionKind::Field)
            .map(|value| value.value)
            .collect();
        assert_eq!(fields, ["command", "outcome", "promoted_alias"]);
        assert_eq!(values("query:failing | where out", &catalog), ["outcome"]);
        for prefix in [
            "query:failing | where outcome:er",
            "query:failing | where outcome = er",
            "query:failing | where outcome:(er",
            "query:failing | where outcome:\"er",
            "query:failing | where outcome = \"er",
        ] {
            let got = complete(prefix, prefix.len(), &catalog);
            assert!(
                got.iter()
                    .any(|value| value.value == "error" || value.value == "\"error\""),
                "{prefix}: {got:?}"
            );
            assert!(got.iter().all(|value| value.value != "wrong-default"));
            assert!(
                got.iter()
                    .filter(|value| value.kind == CompletionKind::Value
                        && value.value.contains("error"))
                    .all(|value| value.help == "saved outcome")
            );
        }
        let prefix = r#"query:failing | where outcome:"escaped\"e"#;
        let completion = complete(prefix, prefix.len(), &catalog)
            .into_iter()
            .find(|value| value.kind == CompletionKind::Value)
            .unwrap();
        assert_eq!(completion.replace.start, prefix.find('"').unwrap());
        assert_eq!(completion.value, r#""escaped\"error\\path""#);
        let applied = format!(
            "{}{}",
            &prefix[..completion.replace.start],
            completion.value
        );
        assert!(matches!(
            parse(&applied, &catalog).unwrap().source,
            query_language::ast::Source::Saved {
                output: OutputBinding::Known,
                ..
            }
        ));
    }
}

#[test]
fn deferred_and_incomplete_saved_or_result_sources_never_offer_default_fields() {
    use query_language::complete::CompletionKind;
    for catalog in [support::polylogue(), support::sinex()] {
        for prefix in [
            "query:missing ",
            "query:missing | where ",
            "query:missing | where out",
            "query:missing(unbound=",
            "result:missing | where ",
            "result:missing.matches | sort by ",
            "result:\"unfinished",
        ] {
            let got = complete(prefix, prefix.len(), &catalog);
            assert!(
                got.iter().all(|value| value.kind != CompletionKind::Field),
                "{prefix}: {got:?}"
            );
        }
    }
}

#[test]
fn result_rows_and_matches_have_separate_field_and_value_completion_scopes() {
    use query_language::{
        ast::ResultPart,
        catalog::{Field, OutputSchema, OutputSource, SourceOutput},
        complete::CompletionKind,
    };
    for mut catalog in [support::polylogue(), support::sinex()] {
        for (part, name, value) in [
            (ResultPart::Rows, "row_alias", "row_value"),
            (ResultPart::Matches, "match_alias", "match_value"),
        ] {
            catalog.source_outputs.push(SourceOutput {
                relation_scope: None,
                source: OutputSource::Result {
                    reference: "result:kept".into(),
                    part,
                },
                schema: OutputSchema {
                    unit: None,
                    main_time: None,
                    fields: vec![Field {
                        name: name.into(),
                        values: vec![value.into()],
                        help: value.into(),
                        ..Field::default()
                    }],
                    match_patterns: Vec::new(),
                },
            });
        }
        for (source, field, value) in [
            ("result:kept.rows", "row_alias", "row_value"),
            ("result:kept.matches", "match_alias", "match_value"),
        ] {
            let prefix = format!("{source} | sort by ");
            let got: Vec<_> = complete(&prefix, prefix.len(), &catalog)
                .into_iter()
                .filter(|value| value.kind == CompletionKind::Field)
                .map(|value| value.value)
                .collect();
            assert_eq!(got, [field]);
            for tail in [
                format!("{field}:{}", &value[..3]),
                format!("{field}:\"{}", &value[..3]),
            ] {
                let prefix = format!("{source} | where {tail}");
                let got = values(&prefix, &catalog);
                assert!(
                    got.contains(&value.into()) || got.contains(&format!("\"{value}\"")),
                    "{prefix}: {got:?}"
                );
            }
        }
    }
}

#[test]
fn nested_implicit_completion_uses_saved_actual_row_domain_and_defers_unknown_domain() {
    use query_language::{
        catalog::{Field, OutputSchema, OutputSource, SourceOutput},
        complete::CompletionKind,
    };
    for (mut catalog, unit, underlying) in [
        (support::polylogue(), "tool_calls", "outcome"),
        (support::sinex(), "commands", "exit_code"),
    ] {
        catalog.source_outputs.push(SourceOutput {
            relation_scope: None,
            source: OutputSource::Saved {
                name: "grouped".into(),
                arguments: vec![],
            },
            schema: OutputSchema {
                unit: Some(unit.into()),
                fields: vec![Field {
                    name: "id".into(),
                    ..Field::default()
                }],
                main_time: None,
                match_patterns: Vec::new(),
            },
        });
        let prefix = "query:grouped | where id in (";
        let got: Vec<_> = complete(prefix, prefix.len(), &catalog)
            .into_iter()
            .filter(|value| value.kind == CompletionKind::Field)
            .map(|value| value.value)
            .collect();
        assert!(got.contains(&underlying.into()), "{unit}: {got:?}");
        let prefix = "query:missing | where id in (";
        assert!(
            complete(prefix, prefix.len(), &catalog)
                .iter()
                .all(|value| value.kind != CompletionKind::Field)
        );
    }
}

#[test]
fn requirements_load_active_source_schema_before_its_value_section() {
    use query_language::{
        catalog::{Field, OutputSchema, OutputSource, SourceOutput},
        complete::{ValueRequirement, requirements},
    };
    for (mut catalog, unit) in [
        (support::polylogue(), "tool_calls"),
        (support::sinex(), "commands"),
    ] {
        let text = "query:failing | where promoted_alias:pre";
        let source = OutputSource::Saved {
            name: "failing".into(),
            arguments: vec![],
        };
        let missing = requirements(text, text.len(), &catalog);
        assert_eq!(missing.sources.as_slice(), std::slice::from_ref(&source));
        assert!(missing.value.is_none());
        catalog.source_outputs.push(SourceOutput {
            relation_scope: None,
            source: source.clone(),
            schema: OutputSchema {
                unit: Some(unit.into()),
                main_time: None,
                fields: vec![Field {
                    name: "promoted_alias".into(),
                    values: vec!["prefix value".into()],
                    ..Field::default()
                }],
                match_patterns: Vec::new(),
            },
        });
        let loaded = requirements(text, text.len(), &catalog);
        assert_eq!(loaded.sources, [source]);
        assert_eq!(
            loaded.value,
            Some(ValueRequirement {
                unit: Some(unit.into()),
                field: "promoted_alias".into(),
                prefix: "pre".into()
            })
        );
        assert!(loaded.reference.is_none());
        assert!(values(text, &catalog).contains(&"\"prefix value\"".into()));
    }
}

#[test]
fn requirements_decode_quotes_and_stop_at_the_byte_cursor_in_every_value_position() {
    use query_language::{
        catalog::{Field, OutputSchema, OutputSource, SourceOutput},
        complete::requirements,
    };
    let mut catalog = support::polylogue();
    catalog.source_outputs.push(SourceOutput {
        relation_scope: None,
        source: OutputSource::Saved {
            name: "failing".into(),
            arguments: vec![],
        },
        schema: OutputSchema {
            unit: Some("tool_calls".into()),
            main_time: None,
            fields: vec![Field {
                name: "session.repo".into(),
                ..Field::default()
            }],
            match_patterns: Vec::new(),
        },
    });
    for (text, cursor, expected) in [
        (
            "query:failing | where session.repo:λtail",
            "query:failing | where session.repo:λ".len(),
            "λ",
        ),
        (
            "query:failing | where session.repo = λtail",
            "query:failing | where session.repo = λ".len(),
            "λ",
        ),
        (
            "query:failing | where session.repo:(λtail)",
            "query:failing | where session.repo:(λ".len(),
            "λ",
        ),
        (
            r#"query:failing | where session.repo:"a\"quote tail" | sort by id"#,
            r#"query:failing | where session.repo:"a\"q"#.len(),
            "a\"q",
        ),
        (
            r#"query:failing | where session.repo = "line\nλ tail""#,
            r#"query:failing | where session.repo = "line\nλ"#.len(),
            "line\nλ",
        ),
    ] {
        let request = requirements(text, cursor, &catalog);
        let value = request
            .value
            .as_ref()
            .unwrap_or_else(|| panic!("{text}: {request:?}"));
        assert_eq!(value.unit.as_deref(), Some("tool_calls"));
        assert_eq!(value.field, "session.repo");
        assert_eq!(value.prefix, expected);
        assert!(request.reference.is_none());
    }
    let text = "query:failing | where session.repo:λtail";
    let before_lambda = text.find('λ').unwrap();
    assert_eq!(
        requirements(text, before_lambda + 1, &catalog)
            .value
            .unwrap()
            .prefix,
        ""
    );
}

#[test]
fn requirements_keep_result_parts_and_nested_source_dependencies_typed() {
    use query_language::{
        ast::ResultPart,
        catalog::{Field, OutputSchema, OutputSource, SourceOutput},
        complete::requirements,
    };
    let mut catalog = support::polylogue();
    for (part, field) in [
        (ResultPart::Rows, "row_alias"),
        (ResultPart::Matches, "match_alias"),
    ] {
        catalog.source_outputs.push(SourceOutput {
            relation_scope: None,
            source: OutputSource::Result {
                reference: "result:kept".into(),
                part,
            },
            schema: OutputSchema {
                unit: Some("tool_calls".into()),
                main_time: None,
                fields: vec![Field {
                    name: field.into(),
                    ..Field::default()
                }],
                match_patterns: Vec::new(),
            },
        });
    }
    for (text, part, field) in [
        (
            "result:kept.rows | where row_alias:r",
            ResultPart::Rows,
            "row_alias",
        ),
        (
            "result:kept.matches | where match_alias:m",
            ResultPart::Matches,
            "match_alias",
        ),
    ] {
        let request = requirements(text, text.len(), &catalog);
        assert_eq!(
            request.sources,
            [OutputSource::Result {
                reference: "result:kept".into(),
                part
            }]
        );
        assert_eq!(request.value.unwrap().field, field);
    }
    let nested = "result:kept.matches | where match_alias in (outcome:er";
    let request = requirements(nested, nested.len(), &catalog);
    assert_eq!(
        request.sources,
        [OutputSource::Result {
            reference: "result:kept".into(),
            part: ResultPart::Matches
        }]
    );
    let value = request.value.unwrap();
    assert_eq!(value.unit.as_deref(), Some("tool_calls"));
    assert_eq!(value.field, "outcome");
    let literal = r#"result:"literal.matches" | where row_alias:r"#;
    assert_eq!(
        requirements(literal, literal.len(), &catalog).sources,
        [OutputSource::Result {
            reference: "result:literal.matches".into(),
            part: ResultPart::Rows
        }]
    );
}

#[test]
fn reference_requirements_are_neutral_and_replace_the_whole_middle_token() {
    use query_language::{
        ast::Span,
        complete::{ReferenceRequirement, requirements},
    };
    let catalog = support::polylogue();
    let text = "sessions where synthetic:native-tail | sort by id";
    let cursor = "sessions where synthetic:nat".len();
    let request = requirements(text, cursor, &catalog);
    assert_eq!(
        request.reference,
        Some(ReferenceRequirement {
            prefix: "synthetic:nat".into(),
            replace: Span {
                start: "sessions where ".len(),
                end: "sessions where synthetic:native-tail".len()
            }
        })
    );
    assert!(request.value.is_none());
    for text in [
        "sessions | sort by ",
        "sessions | fields ",
        "sessions where origin = ",
        "sessions where origin:\"",
        "query:fai",
        "sessions ",
    ] {
        assert!(
            requirements(text, text.len(), &catalog).reference.is_none(),
            "{text}"
        );
    }
    let empty = requirements("", 0, &catalog);
    assert_eq!(empty.reference.unwrap().prefix, "");
    assert_eq!(
        serde_json::from_value::<query_language::complete::Requirements>(
            serde_json::to_value(&request).unwrap()
        )
        .unwrap(),
        request
    );
}

#[test]
fn requirements_keep_group_aliases_and_exclude_stage_reference_requests() {
    use query_language::{
        catalog::{Field, OutputSchema, OutputSource, SourceOutput},
        complete::requirements,
    };
    let mut catalog = support::sinex();
    catalog.source_outputs.push(SourceOutput {
        relation_scope: None,
        source: OutputSource::Saved {
            name: "grouped".into(),
            arguments: vec![],
        },
        schema: OutputSchema {
            unit: Some("commands".into()),
            main_time: None,
            fields: vec![Field {
                name: "promoted_alias".into(),
                values: vec!["prefix".into()],
                ..Field::default()
            }],
            match_patterns: Vec::new(),
        },
    });
    for (text, field, prefix) in [
        (
            "query:grouped | group by promoted_alias | where promoted_alias:p",
            "promoted_alias",
            "p",
        ),
        (
            "query:grouped | group by promoted_alias | where count > 2",
            "count",
            "2",
        ),
    ] {
        let request = requirements(text, text.len(), &catalog);
        assert_eq!(request.sources.len(), 1);
        let value = request.value.unwrap();
        assert_eq!(value.unit.as_deref(), Some("commands"));
        assert_eq!(value.field, field);
        assert_eq!(value.prefix, prefix);
        assert!(request.reference.is_none());
    }
    for text in [
        "events limit 1 ",
        "events | limit 1 ",
        "events | sort by when ",
    ] {
        assert!(
            requirements(text, text.len(), &catalog).reference.is_none(),
            "{text}"
        );
    }
}
