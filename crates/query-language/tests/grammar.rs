mod support;
use query_language::{ErrorKind, ast::*, catalog::*, parse::parse};
fn condition(text: &str, catalog: &Catalog) -> Condition {
    match parse(text, catalog).unwrap().source {
        Source::Unit {
            condition: Some(c), ..
        } => c,
        other => panic!("{other:?}"),
    }
}
#[test]
fn language_atoms_keep_their_meaning() {
    let c = support::polylogue();
    for text in [
        "origin:(a|b|c)",
        "cost:unknown",
        "cost:known",
        "kind:*",
        "cost >= 2.90usd",
        "started between 2026-10-02 and 2026-10-03",
        "title ~ \"two words\"",
        "cwd ^ \"/src/demo\"",
        "refactor*",
        "refactor~",
        "exists tool_calls(outcome:error)",
        "count(tool_calls where outcome:error) >= 3",
        "id in (messages where role:user | group by session)",
        "query:failing(repo=demo,phrase=\"two words\")",
        "seq all(tool:Bash -> [next] tool:Edit -> [within 5m] tool:Bash)",
        "similar:\"semantic question\"",
        "around:2026-10-01",
        "@3",
        "@last",
        "answer:01J0000000000000000000000",
        "https://example.invalid/query",
    ] {
        parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
    assert!(matches!(
        condition("refactor*", &c).kind,
        ConditionKind::Text {
            mode: TextMode::Prefix,
            ..
        }
    ));
    assert!(matches!(
        condition("refactor~", &c).kind,
        ConditionKind::Text {
            mode: TextMode::Approximate,
            ..
        }
    ));
    assert!(matches!(
        condition("similar:\"sx.events\"", &c).kind,
        ConditionKind::Rank {
            list: RankList::Similar,
            ..
        }
    ));
    assert!(matches!(
        condition("cost:known", &c).kind,
        ConditionKind::Match {
            value: Value::Known,
            ..
        }
    ));
    assert!(matches!(
        condition("cwd ^ \"/src/demo\"", &c).kind,
        ConditionKind::Compare { op: Op::Prefix, .. }
    ));
    let q = parse(
        "origin:demo with messages(id,role)[role:assistant,last:2] | fields id,title",
        &c,
    )
    .unwrap();
    assert_eq!(q.attach.len(), 1);
}
#[test]
fn both_catalogs_spell_grouped_relations_alike() {
    for (catalog, unit, key) in [
        (support::polylogue(), "tool_calls", "tool"),
        (support::sinex(), "commands", "cwd"),
    ] {
        let text = format!(
            "{unit} | group by {key} | agg count, sum(duration) | where count >= 2 | sort by sum_duration desc | limit 5"
        );
        let q = parse(&text, &catalog).unwrap_or_else(|e| panic!("{text}: {e}"));
        assert!(matches!(
            q.transforms.last().unwrap().kind,
            TransformKind::Limit { count: 5 }
        ));
        let TransformKind::Aggregate { aggregates } = &q.transforms[1].kind else {
            panic!()
        };
        assert_eq!(aggregates[1].column(), "sum_duration");
    }
    assert!(
        parse(
            "commands | group by cwd | count | where exit_code = 0",
            &support::sinex()
        )
        .unwrap_err()
        .message
        .contains("grouped")
    );
}
#[test]
fn namespace_binding_ignores_literal_text() {
    let c = support::mixed();
    let q = parse(
        "pl.messages where text ~ \"sx.events\" | join near (sx.events where text ~ \"pl.sessions\") within 5m",
        &c,
    )
    .unwrap();
    assert_eq!(q.namespaces().into_iter().collect::<Vec<_>>(), ["pl", "sx"]);
    let q = parse("pl.messages where similar:\"sx.events\"", &c).unwrap();
    assert_eq!(q.namespaces().into_iter().collect::<Vec<_>>(), ["pl"]);
}
#[test]
fn first_words_are_never_guessed_as_commands_and_closed_sets_refuse() {
    let c = support::sinex();
    // No command words: a bare word is a search, and a near-miss of a unit
    // name standing alone is a search too.
    for text in ["related reference", "comands", "\"related\""] {
        parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
    assert!(matches!(
        condition("comands", &c).kind,
        ConditionKind::Text { .. }
    ));
    assert!(parse("source", &c).unwrap_err().message.contains("field"));
    let e = parse("runs where status = hide", &c).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BadValue);
    assert!(e.message.contains("archive.reason"));
    let e = parse("commands where exit_code:>=2", &c).unwrap_err();
    assert_eq!(
        e.suggestion.as_deref(),
        Some("commands where exit_code >= 2")
    );
    assert!(parse(e.suggestion.as_deref().unwrap(), &c).is_ok());
}
#[test]
fn unit_names_have_no_aliases() {
    let c = support::polylogue();
    let e = parse("tool_call where outcome:error", &c).unwrap_err();
    assert_eq!(
        e.suggestion.as_deref(),
        Some("tool_calls where outcome:error")
    );
}
#[test]
fn positional_keywords_and_escaping() {
    let mut c = support::polylogue();
    c.units
        .iter_mut()
        .find(|u| u.name == "sessions")
        .unwrap()
        .fields
        .extend([
            Field {
                name: "count".into(),
                kind: FieldKind::Integer,
                ..Field::default()
            },
            Field {
                name: "where".into(),
                ..Field::default()
            },
        ]);
    assert!(parse("count words", &c).is_err()); // count is a declared field in this synthetic catalog.
    assert!(
        matches!(condition("cats \"and\" dogs",&c).kind,ConditionKind::And{conditions} if conditions.len()==3)
    );
    assert!(matches!(
        condition("where:yes count >= 1", &c).kind,
        ConditionKind::And { .. }
    ));
    let quoted = condition(r#""a\"b\\c\nline""#, &c);
    assert!(matches!(quoted.kind,ConditionKind::Text{text,..} if text=="a\"b\\c\nline"));
}
#[test]
fn order_relations_and_as_of_preserve_inner_context() {
    let c = support::sinex();
    let q = parse(
        "events | join near (commands | as of seq 42) within 5m at end | fields match.distance",
        &c,
    )
    .unwrap();
    let TransformKind::Join {
        query,
        on: JoinOn::Order {
            at: TimePoint::End, ..
        },
    } = &q.transforms[0].kind
    else {
        panic!()
    };
    assert!(matches!(
        query.transforms[0].kind,
        TransformKind::AsOf { sequence: true, .. }
    ));
    for text in [
        "events where when in last 7d | group by day(when) | count | fill",
        "events where material:known",
        "events where seen - when > 1d",
        "events where event_type:(command.executed|command.imported)",
        "events where near(commands where exit_code != 0) within 5m",
    ] {
        parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
}
#[test]
fn typed_extensions_are_walked_not_hidden() {
    let mut c = support::mixed();
    c.extensions.push(Extension {
        name: "correlate".into(),
        position: ExtensionPosition::Transform,
        arguments: vec![ExtensionSlot::Keyword("with".into()), ExtensionSlot::Query],
        help: String::new(),
    });
    let q = parse("pl.messages | correlate with (sx.events)", &c).unwrap();
    assert_eq!(q.namespaces().len(), 2);
}
#[test]
fn valid_size_is_not_a_refusal() {
    let c = support::polylogue();
    let text = vec!["origin:demo"; 4096].join(" ");
    let q = parse(&text, &c).unwrap();
    assert!(
        matches!(q.source,Source::Unit{condition:Some(Condition{kind:ConditionKind::And{conditions},..}),..} if conditions.len()==4096)
    );
}

/// Polylogue's `measure` is a catalog extension, not a shared transform.
pub fn measure() -> Extension {
    Extension {
        name: "measure".into(),
        position: ExtensionPosition::Transform,
        arguments: vec![
            ExtensionSlot::List(Box::new(ExtensionSlot::Value)),
            ExtensionSlot::Optional(vec![
                ExtensionSlot::Keyword("by".into()),
                ExtensionSlot::List(Box::new(ExtensionSlot::Expression)),
            ]),
        ],
        help: String::new(),
    }
}

#[test]
fn product_extension_forms_are_structurally_declared() {
    let mut c = support::polylogue();
    assert!(parse("sessions | measure usage", &c).is_err());
    c.extensions.push(measure());
    for (name, arguments) in [
        ("series", vec![]),
        (
            "extract",
            vec![
                ExtensionSlot::Keyword("number".into()),
                ExtensionSlot::Keyword("before".into()),
                ExtensionSlot::Value,
                ExtensionSlot::Keyword("as".into()),
                ExtensionSlot::Name,
            ],
        ),
        (
            "cluster",
            vec![
                ExtensionSlot::Keyword("by".into()),
                ExtensionSlot::Keyword("similar".into()),
                ExtensionSlot::Optional(vec![
                    ExtensionSlot::Keyword("[".into()),
                    ExtensionSlot::Value,
                    ExtensionSlot::Keyword("]".into()),
                ]),
            ],
        ),
        (
            "let",
            vec![
                ExtensionSlot::Name,
                ExtensionSlot::Keyword("=".into()),
                ExtensionSlot::Query,
            ],
        ),
    ] {
        c.extensions.push(Extension {
            name: name.into(),
            position: ExtensionPosition::Transform,
            arguments,
            help: String::new(),
        });
    }
    for text in [
        "sessions | series",
        r#"sessions | extract number before "MB/s" as rate"#,
        "sessions | cluster by similar",
        "sessions | cluster by similar [k:20]",
        "sessions | let failures = (tool_calls where outcome:error)",
        "sessions | measure usage",
        "sessions | measure compare:origin, words by day(started), origin",
    ] {
        parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
    let q = parse(
        "sessions | measure compare:origin, words by day(started)",
        &c,
    )
    .unwrap();
    let TransformKind::Extension { arguments, .. } = &q.transforms[0].kind else {
        panic!()
    };
    assert!(matches!(&arguments[0], ExtensionArgument::List(items) if items.len() == 2));
    let rendered = query_language::format::format_query(&q, &c).unwrap();
    assert_eq!(
        query_language::identity::normal_form(&parse(&rendered, &c).unwrap(), &c).unwrap(),
        query_language::identity::normal_form(&q, &c).unwrap()
    );
}

#[test]
fn product_functions_are_declared_by_the_catalog() {
    let mut c = support::polylogue();
    assert!(parse("tool_calls | group by program(command)", &c).is_err());
    c.functions.push(Function {
        name: "program".into(),
        arguments: 1,
        result: FieldKind::Text,
        help: String::new(),
    });
    let q = parse("tool_calls | group by program(command) | count", &c).unwrap();
    let TransformKind::Group { keys } = &q.transforms[0].kind else {
        panic!()
    };
    assert!(matches!(&keys[0].expression, Expression::Function { name, .. } if name == "program"));
    let e = parse("tool_calls | group by program(command, tool)", &c).unwrap_err();
    assert_eq!(e.kind, ErrorKind::BadValue);
}

#[test]
fn product_reference_forms_stay_literal_nodes() {
    let c = support::polylogue();
    for text in [
        "session:origin:synthetic-id",
        "message:synthetic-id",
        "tool:session#call",
        "span:message#c=1-2",
        "https://example.invalid/?x=y",
        "sinex:ev/synthetic-id",
        "polylogue:session/synthetic-id",
        "self",
        "last",
        "7f3a2c10-4b5e-4c1d",
        "@3",
        "answer:01J0000000000000000000000",
    ] {
        assert!(
            matches!(condition(text, &c).kind, ConditionKind::Reference { .. }),
            "{text}"
        );
    }
    assert!(matches!(
        condition("tool:Bash", &c).kind,
        ConditionKind::Match { .. }
    ));
    // Without a declaration, a product prefix is not a reference.
    let mut bare = support::polylogue();
    bare.references.clear();
    assert!(!matches!(
        condition("self", &bare).kind,
        ConditionKind::Reference { .. }
    ));
    assert!(matches!(
        condition("@3", &bare).kind,
        ConditionKind::Reference { .. }
    ));
}

#[test]
fn custom_unknown_words_are_unknown_unless_quoted() {
    let mut catalog = support::polylogue();
    catalog
        .units
        .iter_mut()
        .find(|u| u.name == "sessions")
        .unwrap()
        .fields
        .iter_mut()
        .find(|f| f.name == "cost")
        .unwrap()
        .unknown = UnknownRule::Word("not-recorded".into());
    for text in ["cost:not-recorded", "cost = not-recorded"] {
        assert!(matches!(
            condition(text, &catalog).kind,
            ConditionKind::Match {
                value: Value::Unknown,
                ..
            } | ConditionKind::Compare {
                right: Expression::Value {
                    value: Value::Unknown
                },
                ..
            }
        ));
    }
    assert!(matches!(
        condition("cost:\"not-recorded\"", &catalog).kind,
        ConditionKind::Match {
            value: Value::Scalar { .. },
            ..
        }
    ));
}

#[test]
fn closed_sets_use_their_declared_spelling_in_the_bound_tree() {
    let c = support::sinex();
    let query = parse("runs where status:RUNNING", &c).unwrap();
    let Source::Unit {
        condition: Some(condition),
        ..
    } = query.source
    else {
        panic!("unit filter")
    };
    let ConditionKind::Match {
        value: Value::Scalar { text, .. },
        ..
    } = condition.kind
    else {
        panic!("closed-set match")
    };
    assert_eq!(text, "running");
}

#[test]
fn mixed_sequences_bind_each_scopes_own_relation_domain() {
    let catalog = support::mixed();
    let query=parse("pl.sessions where seq(tool:Bash -> outcome:error) AND exists sx.events(seq(source:demo -> source:other))",&catalog).unwrap();
    let Source::Unit {
        condition:
            Some(Condition {
                kind: ConditionKind::And { conditions },
                ..
            }),
        ..
    } = &query.source
    else {
        panic!()
    };
    assert!(conditions.iter().any(|condition|matches!(&condition.kind,ConditionKind::Sequence{unit,..} if unit=="pl.tool_calls")));
    assert!(conditions.iter().any(|condition|matches!(&condition.kind,ConditionKind::Exists{unit,condition:Some(inner)} if unit=="sx.events"&&matches!(&inner.kind,ConditionKind::Sequence{unit,..} if unit=="sx.events"))));
    assert_eq!(
        query.namespaces().into_iter().collect::<Vec<_>>(),
        ["pl", "sx"]
    );
    let rendered = query_language::format::format_query(&query, &catalog).unwrap();
    let parsed = parse(&rendered, &catalog).unwrap();
    assert_eq!(
        query_language::identity::normal_form(&query, &catalog).unwrap(),
        query_language::identity::normal_form(&parsed, &catalog).unwrap()
    );
}

#[test]
fn both_product_catalogs_share_builtin_spelling_and_reference_spacing() {
    use query_language::{format::format_query, identity::question_id};
    use std::collections::BTreeMap;
    for catalog in [support::polylogue(), support::sinex()] {
        let unit = catalog
            .units
            .iter()
            .find(|unit| unit.main_time.is_some())
            .unwrap();
        let time = unit.main_time.as_ref().unwrap();
        let lower = parse(&format!("{} | group by day({time})", unit.name), &catalog).unwrap();
        let upper = parse(&format!("{} | group by DAY({time})", unit.name), &catalog).unwrap();
        assert_eq!(
            question_id(&lower, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&upper, &catalog, &BTreeMap::new()).unwrap()
        );
        for (attached, spaced) in [
            ("query:Case", "QUERY : Case"),
            ("result:Case", "RESULT : Case"),
        ] {
            let a = parse(attached, &catalog).unwrap();
            let b = parse(spaced, &catalog).unwrap();
            assert_eq!(
                format_query(&a, &catalog).unwrap(),
                format_query(&b, &catalog).unwrap()
            );
        }
    }
}

#[test]
fn matched_time_selectors_are_explicit_and_round_trip() {
    let catalog = support::sinex();
    for (word, point) in [
        ("match.when", TimePoint::MatchWhen),
        ("match.start", TimePoint::MatchStart),
        ("match.end", TimePoint::MatchEnd),
    ] {
        let text = format!("events | join near (commands) within 5m at {word}");
        let query = parse(&text, &catalog).unwrap();
        let TransformKind::Join {
            on: JoinOn::Order { at, .. },
            ..
        } = query.transforms[0].kind
        else {
            panic!()
        };
        assert_eq!(at, point);
        let encoded = serde_json::to_value(&query).unwrap();
        let decoded: Query = serde_json::from_value(encoded).unwrap();
        let rendered = query_language::format::format_query(&decoded, &catalog).unwrap();
        assert!(rendered.contains(&format!(" at {word}")));
        let bound = query_language::bind::bind(&decoded, &catalog).unwrap();
        let reparsed = parse(&rendered, &catalog).unwrap();
        assert_eq!(bound.query().transforms, reparsed.transforms);
        let upper = parse(&text.to_uppercase(), &catalog).unwrap();
        let TransformKind::Join {
            on: JoinOn::Order {
                at: upper_point, ..
            },
            ..
        } = upper.transforms[0].kind
        else {
            panic!()
        };
        assert_eq!(upper_point, point);
    }
}

#[test]
fn grouped_first_restores_the_input_columns_and_provenance() {
    let catalog = support::polylogue();
    let text = "messages | fields id as original, session | group by session | first 2 | fields original, session";
    let query = parse(text, &catalog).unwrap();
    let bound = query_language::bind::bind(&query, &catalog).unwrap();
    let scopes = &bound
        .typing()
        .query(&query_language::scope::NodePath::default())
        .unwrap()
        .transforms;
    assert!(scopes[2].after.field("original").is_some());
    assert!(scopes[2].after.field("count").is_none());
    assert!(scopes[3].after.field("original").is_some());
}
