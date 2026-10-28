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
        "kind:*",
        "cost >= 2.90usd",
        "started between 2026-10-02 and 2026-10-03",
        "title ~ \"two words\"",
        "refactor*",
        "refactor~",
        "exists tool_call(outcome:error)",
        "count(tool_call where outcome:error) >= 3",
        "id in (messages where role:user | group by session)",
        "query:failing(repo=demo,phrase=\"two words\")",
        "seq all(tool:Bash -> [next] tool:Edit -> [within:5m] tool:Bash)",
        "similar:\"semantic question\"",
        "@3.2",
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
        ConditionKind::Similar { .. }
    ));
    let q = parse(
        "origin:demo with messages(id,role)[role:assistant,last:2] | fields id,title",
        &c,
    )
    .unwrap();
    assert_eq!(q.attach.len(), 1);
}
#[test]
fn both_surfaces_preserve_grouped_relations() {
    for (catalog, text) in [
        (
            support::polylogue(),
            "tool_calls | group by tool | agg count, sum:duration | where count >= 2 | sort by sum_duration desc | limit 5",
        ),
        (
            support::sinex(),
            "commands group by cwd count, sum(duration) where count >= 2 sort sum_duration desc limit 5",
        ),
    ] {
        let q = parse(text, &catalog).unwrap();
        assert!(matches!(
            q.stages.last().unwrap().kind,
            StageKind::Limit { count: 5 }
        ));
    }
    assert!(
        parse(
            "commands group by cwd count where exit_code = 0",
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
    let q=parse("pl.messages where text ~ \"sx.events\" | near (sx.events where text ~ \"pl.sessions\") within 5m",&c).unwrap();
    assert_eq!(q.namespaces().into_iter().collect::<Vec<_>>(), ["pl", "sx"]);
    let q = parse("pl.messages where similar:\"sx.events\"", &c).unwrap();
    assert_eq!(q.namespaces().into_iter().collect::<Vec<_>>(), ["pl"]);
}
#[test]
fn command_words_and_closed_sets_refuse() {
    let c = support::sinex();
    assert_eq!(
        parse("related reference", &c).unwrap_err().kind,
        ErrorKind::NotAQuery
    );
    assert!(
        parse("comands", &c)
            .unwrap_err()
            .suggestion
            .unwrap()
            .contains("commands")
    );
    assert!(parse("\"related\"", &c).is_ok());
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
        matches!(condition("cats and dogs",&c).kind,ConditionKind::And{conditions} if conditions.len()==3)
    );
    assert!(matches!(
        condition("where:yes count >= 1", &c).kind,
        ConditionKind::And { .. }
    ));
    let quoted = condition(r#""a\"b\\c\nline""#, &c);
    assert!(matches!(quoted.kind,ConditionKind::Text{text,..} if text=="a\"b\\c\nline"));
    let e = parse("origin:demo and title:demo", &c).unwrap_err();
    assert!(e.suggestion.unwrap().contains("AND"));
}
#[test]
fn temporal_relations_preserve_inner_context() {
    let c = support::sinex();
    let q = parse(
        "events near (commands as of seq 42) at end within 5m fields near,offset",
        &c,
    )
    .unwrap();
    let StageKind::Temporal {
        query,
        at: TimePoint::End,
        ..
    } = &q.stages[0].kind
    else {
        panic!()
    };
    assert!(matches!(
        query.stages[0].kind,
        StageKind::AsOf { sequence: true, .. }
    ));
    for text in [
        "events where when in last 7d group by day(when) count fill",
        "events where material exists",
        "events where seen - when > 1d",
        "events where event_type = [command.executed, command.imported]",
        "recall yesterday",
        "trace sinex:ev/0190a0b0-0000-7000-8000-000000000001",
    ] {
        parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
}
#[test]
fn typed_extensions_are_walked_not_hidden() {
    let mut c = support::mixed();
    c.extensions.push(Extension {
        name: "correlate".into(),
        position: ExtensionPosition::Stage,
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

#[test]
fn product_extension_forms_are_structurally_declared() {
    let mut c = support::polylogue();
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
            position: ExtensionPosition::Stage,
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
    ] {
        parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
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
        "sx:sinex:ev/synthetic-id",
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
}
