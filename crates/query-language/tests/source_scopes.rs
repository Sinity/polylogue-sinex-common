use query_language::{
    ErrorKind, ast::*, bind::bind, catalog::*, format::format_query, identity::question_id,
    parse::parse,
};
use std::collections::BTreeMap;
fn field(name: &str, kind: FieldKind) -> Field {
    Field {
        name: name.into(),
        kind,
        ..Field::default()
    }
}
fn catalog() -> Catalog {
    let mut catalog = Catalog::new("sessions");
    catalog.units.push(Unit {
        name: "sessions".into(),
        fields: vec![
            field("default_only", FieldKind::Text),
            field("id", FieldKind::Reference),
        ],
        ..Unit::default()
    });
    catalog
}
fn saved(name: &str, fields: Vec<Field>) -> SourceOutput {
    SourceOutput {
        source: OutputSource::Saved {
            name: name.into(),
            arguments: vec![],
        },
        schema: OutputSchema {
            match_units: None,
            unit: Some("sessions".into()),
            fields,
            main_time: None,
        },
    }
}
fn result(reference: &str, part: ResultPart, fields: Vec<Field>) -> SourceOutput {
    SourceOutput {
        source: OutputSource::Result {
            reference: reference.into(),
            part,
        },
        schema: OutputSchema {
            match_units: None,
            unit: Some("sessions".into()),
            fields,
            main_time: None,
        },
    }
}
#[test]
fn saved_suffix_columns_are_unknown_then_bound_to_actual_typed_output() {
    let mut catalog = catalog();
    let parsed = parse(
        "query:failing session.repo:morrow | sort by count desc",
        &catalog,
    )
    .unwrap();
    assert!(matches!(
        parsed.source,
        Source::Saved {
            output: SourceColumns::Unknown,
            ..
        }
    ));
    assert_eq!(
        bind(&parsed, &catalog).unwrap_err().kind,
        ErrorKind::BadValue
    );
    catalog.source_outputs.push(saved(
        "failing",
        vec![
            field("session.repo", FieldKind::Text),
            field("count", FieldKind::Integer),
        ],
    ));
    let bound = bind(&parsed, &catalog).unwrap();
    assert!(matches!(
        bound.query().source,
        Source::Saved {
            output: SourceColumns::Known,
            ..
        }
    ));
    assert!(parse("query:failing default_only:no", &catalog).is_err());
    assert_eq!(
        parse("query:failing | where count > nope", &catalog)
            .unwrap_err()
            .kind,
        ErrorKind::BadValue
    );
    assert_eq!(
        parse("query:failing | sort by default_only", &catalog)
            .unwrap_err()
            .kind,
        ErrorKind::UnknownField
    );
}
#[test]
fn grouped_promoted_aliases_types_and_closed_sets_come_from_output_snapshot() {
    let mut catalog = catalog();
    let mut state = field("state", FieldKind::Exact);
    state.values = vec!["Open".into(), "Closed".into()];
    catalog.source_outputs.push(saved(
        "grouped",
        vec![
            field("count", FieldKind::Integer),
            field("promoted_alias", FieldKind::Money),
            state,
        ],
    ));
    let a = parse(
        "query:grouped | where promoted_alias = 02.900usd AND state:open | sort by count",
        &catalog,
    )
    .unwrap();
    let b = parse(
        "query:grouped | where state:Open AND promoted_alias = 2.9usd | sort by count",
        &catalog,
    )
    .unwrap();
    assert_eq!(
        question_id(&a, &catalog, &BTreeMap::new()).unwrap(),
        question_id(&b, &catalog, &BTreeMap::new()).unwrap()
    );
    assert!(bind(&a, &catalog).is_ok());
    assert_eq!(
        parse("query:grouped | where state:absent", &catalog)
            .unwrap_err()
            .kind,
        ErrorKind::BadValue
    );
    assert!(
        parse(
            "query:grouped | fields count | sort by promoted_alias",
            &catalog
        )
        .is_err()
    );
    let regrouped = parse(
        "query:grouped | group by state | agg count | where count > 1",
        &catalog,
    )
    .unwrap();
    assert!(bind(&regrouped, &catalog).is_ok());
    assert_eq!(
        parse(
            "query:grouped | group by state | agg count | where count > nope",
            &catalog
        )
        .unwrap_err()
        .kind,
        ErrorKind::BadValue
    );
}
#[test]
fn kept_columns_and_quoted_literal_ids_keep_rows_and_matches_distinct() {
    let mut catalog = catalog();
    let reference = "result:source:λ | \"reader\".matches";
    catalog.source_outputs.push(result(
        reference,
        ResultPart::Rows,
        vec![field("id", FieldKind::Reference)],
    ));
    let query = parse(
        r#"result:"source:λ | \"reader\".matches" | sort by id"#,
        &catalog,
    )
    .unwrap();
    assert!(
        matches!(&query.source, Source::Result { reference: value, part: ResultPart::Rows, output: SourceColumns::Known, .. } if value == reference)
    );
    let text = format_query(&query, &catalog).unwrap();
    assert_eq!(
        query_language::identity::normal_form(&parse(&text, &catalog).unwrap(), &catalog).unwrap(),
        query_language::identity::normal_form(&query, &catalog).unwrap()
    );
    assert!(bind(&query, &catalog).is_ok());
    let matches = parse("result:synthetic.matches | sort by count", &catalog).unwrap();
    assert!(matches!(
        matches.source,
        Source::Result {
            part: ResultPart::Matches,
            output: SourceColumns::Unknown,
            ..
        }
    ));
    assert!(bind(&matches, &catalog).is_err());
    catalog.source_outputs.push(result(
        "result:synthetic",
        ResultPart::Matches,
        vec![field("count", FieldKind::Integer)],
    ));
    assert!(bind(&matches, &catalog).is_ok());
    let rows = parse("result:synthetic.rows | sort by count", &catalog).unwrap();
    assert!(bind(&rows, &catalog).is_err());
}
#[test]
fn binding_checks_nested_sources_and_never_trusts_serialized_known_marker() {
    let mut catalog = catalog();
    catalog
        .source_outputs
        .push(saved("outer", vec![field("id", FieldKind::Reference)]));
    let query = parse(
        "query:outer | where id in (query:inner | fields id)",
        &catalog,
    )
    .unwrap();
    assert!(bind(&query, &catalog).is_err());
    catalog
        .source_outputs
        .push(saved("inner", vec![field("id", FieldKind::Reference)]));
    assert!(bind(&query, &catalog).is_ok());
    let mut forged = parse("result:missing", &catalog).unwrap();
    if let Source::Result { output, .. } = &mut forged.source {
        *output = SourceColumns::Known;
    }
    let forged: Query = serde_json::from_value(serde_json::to_value(forged).unwrap()).unwrap();
    assert!(bind(&forged, &catalog).is_err());
}

#[test]
fn inline_saved_conditions_remain_distinct_from_piped_filters() {
    let mut catalog = catalog();
    catalog
        .source_outputs
        .push(saved("named", vec![field("hidden", FieldKind::Bool)]));
    let inline = parse("query:named hidden:true", &catalog).unwrap();
    let piped = parse("query:named | where hidden:true", &catalog).unwrap();
    assert!(matches!(
        inline.source,
        Source::Saved {
            condition: Some(_),
            ..
        }
    ));
    assert!(inline.transforms.is_empty());
    assert!(matches!(
        piped.source,
        Source::Saved {
            condition: None,
            ..
        }
    ));
    assert_eq!(piped.transforms.len(), 1);
    assert_ne!(
        question_id(&inline, &catalog, &BTreeMap::new()).unwrap(),
        question_id(&piped, &catalog, &BTreeMap::new()).unwrap()
    );
    assert!(matches!(
        bind(&inline, &catalog).unwrap().query().source,
        Source::Saved {
            condition: Some(_),
            ..
        }
    ));
}

#[test]
fn saved_argument_snapshot_matching_ignores_quoted_spelling_and_parameter_order() {
    let mut catalog = catalog();
    let parsed = parse(
        "QUERY:named(period=last 7d,kind=demo) | sort by count",
        &catalog,
    )
    .unwrap();
    catalog.source_outputs.push(SourceOutput {
        source: OutputSource::Saved {
            name: "named".into(),
            arguments: vec![
                Argument {
                    parameter: "kind".into(),
                    value: Value::scalar("demo"),
                },
                Argument {
                    parameter: "period".into(),
                    value: Value::scalar("last 7d"),
                },
            ],
        },
        schema: OutputSchema {
            match_units: None,
            unit: Some("sessions".into()),
            fields: vec![field("count", FieldKind::Integer)],
            main_time: None,
        },
    });
    assert!(bind(&parsed, &catalog).is_ok());
}

#[test]
fn nested_implicit_units_inherit_the_row_domain_and_explicit_units_keep_their_own() {
    let mut catalog = catalog();
    catalog.units.push(Unit {
        name: "commands".into(),
        fields: vec![
            field("id", FieldKind::Reference),
            field("repo", FieldKind::Text),
            field("outcome", FieldKind::Text),
        ],
        ..Unit::default()
    });
    let query = parse(
        "commands | group by repo | where repo in (outcome:error)",
        &catalog,
    )
    .unwrap();
    let TransformKind::Where { condition } = &query.transforms[1].kind else {
        panic!("where")
    };
    let ConditionKind::In { query: inner, .. } = &condition.kind else {
        panic!("in")
    };
    assert!(
        matches!(&inner.source, Source::Unit { unit, implicit: true, .. } if unit == "commands")
    );
    let text = format_query(&query, &catalog).unwrap();
    assert!(!text.contains("(commands where"));
    assert!(bind(&query, &catalog).is_ok());
    let explicit = parse(
        "commands where id in (sessions where default_only:yes)",
        &catalog,
    )
    .unwrap();
    let Source::Unit {
        condition: Some(condition),
        ..
    } = &explicit.source
    else {
        panic!("condition")
    };
    let ConditionKind::In { query: inner, .. } = &condition.kind else {
        panic!("in")
    };
    assert!(
        matches!(&inner.source, Source::Unit { unit, implicit: false, .. } if unit == "sessions")
    );
    assert!(bind(&explicit, &catalog).is_ok());

    let unbound = parse("result:kept | where id in (outcome:error)", &catalog).unwrap();
    assert!(bind(&unbound, &catalog).is_err());
    catalog.source_outputs.push(SourceOutput {
        source: OutputSource::Result {
            reference: "result:kept".into(),
            part: ResultPart::Rows,
        },
        schema: OutputSchema {
            match_units: None,
            unit: Some("commands".into()),
            fields: vec![field("id", FieldKind::Reference)],
            main_time: None,
        },
    });
    let bound = bind(&unbound, &catalog).unwrap();
    let TransformKind::Where { condition } = &bound.query().transforms[0].kind else {
        panic!("where")
    };
    let ConditionKind::In { query: inner, .. } = &condition.kind else {
        panic!("in")
    };
    assert!(
        matches!(&inner.source, Source::Unit { unit, implicit: true, .. } if unit == "commands")
    );
}

#[test]
fn explicit_units_keep_saved_and_result_predicates_distinct_from_sources() {
    let catalog = catalog();
    for (literal, saved) in [("query:by-action", true), ("result:synthetic", false)] {
        let source = parse(literal, &catalog).unwrap();
        assert!(if saved {
            matches!(source.source, Source::Saved { .. })
        } else {
            matches!(source.source, Source::Result { .. })
        });
        let implicit = parse(&format!("({literal})"), &catalog).unwrap();
        assert_eq!(
            question_id(&source, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&implicit, &catalog, &BTreeMap::new()).unwrap()
        );
        for text in [
            format!("sessions where {literal}"),
            format!("sessions where ({literal})"),
        ] {
            let explicit = parse(&text, &catalog).unwrap();
            let Source::Unit {
                unit,
                implicit: false,
                condition: Some(condition),
            } = &explicit.source
            else {
                panic!("explicit source was lost: {explicit:?}");
            };
            assert_eq!(unit, "sessions");
            assert!(if saved {
                matches!(condition.kind, ConditionKind::Saved { .. })
            } else {
                matches!(condition.kind, ConditionKind::Reference { .. })
            });
            assert_ne!(
                question_id(&source, &catalog, &BTreeMap::new()).unwrap(),
                question_id(&explicit, &catalog, &BTreeMap::new()).unwrap()
            );
            let rendered = format_query(&explicit, &catalog).unwrap();
            let parsed = parse(&rendered, &catalog).unwrap();
            assert!(matches!(
                parsed.source,
                Source::Unit {
                    implicit: false,
                    ..
                }
            ));
            assert_eq!(
                question_id(&explicit, &catalog, &BTreeMap::new()).unwrap(),
                question_id(&parsed, &catalog, &BTreeMap::new()).unwrap()
            );
        }
    }
}

#[test]
fn captured_saved_match_provenance_reuses_the_shared_step_shape() {
    use query_language::scope::NodePath;
    let mut catalog = catalog();
    let definition = bind(
        &parse("sessions | where exists sessions()", &catalog).unwrap(),
        &catalog,
    )
    .unwrap();
    let scope = &definition
        .typing()
        .query(&NodePath::default())
        .unwrap()
        .transforms
        .last()
        .unwrap()
        .after;
    let schema = OutputSchema {
        unit: scope.unit.clone(),
        fields: scope.fields.iter().map(|info| info.field.clone()).collect(),
        main_time: None,
        match_units: scope.match_units.clone(),
    };
    assert_eq!(schema.match_units, Some(vec!["sessions".into()]));
    let unresolved = parse(
        "query:qualified | matches | fields parent, step.id",
        &catalog,
    )
    .unwrap();
    assert!(bind(&unresolved, &catalog).is_err());
    catalog.source_outputs.push(SourceOutput {
        source: OutputSource::Saved {
            name: "qualified".into(),
            arguments: vec![],
        },
        schema: schema.clone(),
    });
    assert!(bind(&unresolved, &catalog).is_ok());
    let saved = bind(
        &parse(
            "query:qualified | matches | fields parent, step.id",
            &catalog,
        )
        .unwrap(),
        &catalog,
    )
    .unwrap();
    let direct = bind(
        &parse(
            "sessions where exists sessions() | matches | fields parent, step.id",
            &catalog,
        )
        .unwrap(),
        &catalog,
    )
    .unwrap();
    let fields = |bound: &query_language::bind::BoundQuery| {
        bound
            .typing()
            .query(&NodePath::default())
            .unwrap()
            .transforms
            .last()
            .unwrap()
            .after
            .fields
            .iter()
            .map(|info| info.field.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(fields(&saved), fields(&direct));
    // An evaluated result does not gain access to hidden source patterns.
    catalog.source_outputs.push(SourceOutput {
        source: OutputSource::Result {
            reference: "result:held".into(),
            part: ResultPart::Rows,
        },
        schema,
    });
    assert!(parse("result:held | matches", &catalog).is_err());
    catalog.source_outputs[0].schema.match_units = None;
    assert!(parse("query:qualified | matches", &catalog).is_err());
    catalog.source_outputs[0].schema.match_units = Some(vec!["sessions".into(), "sessions".into()]);
    assert_eq!(
        parse("query:qualified | matches", &catalog)
            .unwrap_err()
            .kind,
        ErrorKind::Ambiguous
    );
}

#[test]
fn grouped_rows_do_not_claim_their_original_row_match_patterns() {
    let catalog = catalog();
    for suffix in ["group by default_only", "count"] {
        let text = format!("sessions where exists sessions() | {suffix} | matches");
        assert!(parse(&text, &catalog).is_err(), "{text}");
    }
}

#[test]
fn local_namespace_prefix_has_one_unit_identity() {
    let mut catalog = catalog();
    catalog.namespace = Some("pl".into());
    let canonical = parse("sessions", &catalog).unwrap();
    let id = question_id(&canonical, &catalog, &BTreeMap::new()).unwrap();
    for text in ["pl.sessions", "PL.SESSIONS"] {
        let parsed = parse(text, &catalog).unwrap();
        assert_eq!(parsed, canonical, "{text}");
        assert_eq!(
            question_id(&parsed, &catalog, &BTreeMap::new()).unwrap(),
            id
        );
    }
    catalog.units.push(Unit {
        name: "sx.events".into(),
        fields: vec![field("id", FieldKind::Reference)],
        ..Unit::default()
    });
    let remote = parse("sx.events", &catalog).unwrap();
    assert!(matches!(&remote.source, Source::Unit {unit, ..} if unit == "sx.events"));
    assert_eq!(remote.namespaces().into_iter().collect::<Vec<_>>(), ["sx"]);
}

#[test]
fn native_reference_prefixes_preserve_real_fields_in_the_current_scope() {
    let mut catalog = catalog();
    catalog.references.push(ReferenceForm::prefix("event:"));
    let opaque = parse("sessions where event:opaque", &catalog).unwrap();
    assert!(matches!(
        opaque.source,
        Source::Unit {
            condition: Some(Condition {
                kind: ConditionKind::Reference { .. },
                ..
            }),
            ..
        }
    ));
    catalog.units[0]
        .fields
        .push(field("event", FieldKind::Exact));
    let scoped = parse("sessions where event:started", &catalog).unwrap();
    assert!(
        matches!(&scoped.source, Source::Unit {condition:Some(Condition {kind:ConditionKind::Match {field,..},..}),..} if field == "event")
    );
    catalog.references.push(ReferenceForm::Prefix {
        prefix: "event:".into(),
        marker: Some("#".into()),
    });
    let marked = parse("sessions where event:owner#record", &catalog).unwrap();
    assert!(matches!(
        marked.source,
        Source::Unit {
            condition: Some(Condition {
                kind: ConditionKind::Reference { .. },
                ..
            }),
            ..
        }
    ));
}

#[test]
fn quoted_native_references_round_trip_without_stealing_scoped_fields() {
    let mut catalog = catalog();
    catalog.references.push(ReferenceForm::prefix("event:"));
    catalog.references.push(ReferenceForm::Prefix {
        prefix: "session:".into(),
        marker: Some(":".into()),
    });
    for reference in [
        "event:record with |,[] and \"quotes\"",
        "session:origin:native \\ id",
    ] {
        let query = Query {
            source: Source::Unit {
                unit: "sessions".into(),
                implicit: false,
                condition: Some(Condition {
                    span: Span::default(),
                    kind: ConditionKind::Reference {
                        reference: reference.into(),
                    },
                }),
            },
            attach: vec![],
            transforms: vec![],
        };
        let text = format_query(&query, &catalog).unwrap();
        let parsed = parse(&text, &catalog).unwrap();
        assert_eq!(
            question_id(&parsed, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&query, &catalog, &BTreeMap::new()).unwrap(),
            "{text}"
        );
    }
    catalog.units[0]
        .fields
        .push(field("event", FieldKind::Exact));
    let query = parse(r#"sessions where event:"a recorded value""#, &catalog).unwrap();
    assert!(matches!(
        query.source,
        Source::Unit {
            condition: Some(Condition {
                kind: ConditionKind::Match { .. },
                ..
            }),
            ..
        }
    ));
}

#[test]
fn source_unit_typos_offer_runnable_canonical_corrections_without_changing_literals() {
    let mut catalog = catalog();
    catalog.units.push(Unit {
        name: "messages".into(),
        fields: vec![field("text", FieldKind::Text)],
        ..Unit::default()
    });
    for (text, expected) in [
        ("messges cargo", "messages where cargo"),
        ("sesions where id:one", "sessions where id:one"),
        ("messges", "messages"),
        ("messges | count", "messages | count"),
    ] {
        let error = parse(text, &catalog).unwrap_err();
        assert_eq!(error.kind, ErrorKind::NotAQuery, "{text}: {error:?}");
        assert_eq!(error.position, 0);
        assert_eq!(error.suggestion.as_deref(), Some(expected));
        bind(&parse(expected, &catalog).unwrap(), &catalog).unwrap();
    }
    for text in [
        "\"messges\" cargo",
        "cargo messges",
        "brief yesterday",
        "status",
        "statsu",
    ] {
        let parsed = parse(text, &catalog).unwrap();
        assert!(
            matches!(parsed.source, Source::Unit { implicit: true, .. }),
            "{text}"
        );
    }
}
