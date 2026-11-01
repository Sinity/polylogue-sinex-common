mod support;
use proptest::prelude::*;
use query_language::{
    ast::*, catalog::*, format::format_query, identity::normal_form, parse::parse,
};
fn roundtrip(text: &str, catalog: &Catalog) {
    let query = parse(text, catalog).unwrap_or_else(|e| panic!("{text}: {e}"));
    let rendered = format_query(&query, catalog).unwrap_or_else(|e| panic!("{text}: {e}"));
    let parsed = parse(&rendered, catalog).unwrap();
    assert_eq!(
        normal_form(&query, catalog).unwrap(),
        normal_form(&parsed, catalog).unwrap(),
        "{text} => {rendered}"
    );
    assert_eq!(rendered, format_query(&parsed, catalog).unwrap());
}
/// Law `ql.format.roundtrip`: formatting a parsed question reparses to the
/// same normal form, and formatting again is stable.
#[test]
fn polylogue_forms_roundtrip() {
    let c = support::polylogue();
    for text in [
        "",
        "origin:demo NOT tag:old OR title:demo",
        "origin:(a|b|unknown)",
        "kind:*",
        "cost:known",
        "cost >= 2.90usd",
        "started between 2026-10-02 and 2026-10-03",
        "title ~ \"contains text\"",
        "cwd ^ \"/src/demo\"",
        "a refactor* refactor~",
        "weekday_offset()",
        "parse(x) fails",
        "exists tool_calls(outcome:error)",
        "exists tool_calls()",
        "count(tool_calls where outcome:error) >= 3",
        "id in (messages where role:user | group by session)",
        "similar:\"semantic question\"",
        "around:2026-10-01",
        "query:review(repo=demo,text=\"AND | punctuation\") | limit 4",
        "result:synthetic-id | distinct",
        "seq all(tool:Bash -> tool:Edit -> [next] outcome:error -> [within 5m] tool:Bash) | matches",
        "seq(tool:Bash -> [within 3 steps] tool:Edit)",
        "origin:demo with messages(id,role)[role:assistant,last:2],tool_calls(tool)[first:1] | fields id,title",
        "sessions | sort by random(7) | offset 2 | limit 10",
        "sessions | sort by origin asc,title desc",
        "tool_calls | group by day(at),tool | agg count,sum(duration),p90(duration),count_distinct(session),histogram(duration),corr(duration,exit_code) | where count > 1 | sort by sum_duration desc | first 2",
        "tool_calls | agg sum(duration) as total, covered(at, at) as busy",
        "sessions | union (origin:demo) | intersect (origin:demo) | except (origin:other) | fuse (similar:demo)",
        "sessions | fields title as name, cost * 2 as doubled, \"agent\" as who",
        "sessions where cost / 2 > 1usd",
        "sessions | diff (origin:demo)",
        "tool_calls where outcome:error | group by next.action | count",
        "messages where NOT after(tool_calls where outcome:ok) within 10 steps",
        "messages | join near (tool_calls where outcome:error) within 2 steps | fields match.distance",
        "messages | join (tool_calls) on session = session",
        "sessions | group by day(started) | agg count, sum(cost) | fill",
        "session:origin:synthetic-id",
        "span:message#c=1-2",
        "https://example.invalid/?x=y",
        "sinex:ev/synthetic-id",
        "@3",
        "answer:synthetic-id",
        "origin:$provider",
    ] {
        roundtrip(text, &c);
    }
}
#[test]
fn sinex_forms_roundtrip() {
    let c = support::sinex();
    for text in [
        "commands where exit_code != 0 AND cwd ^ /src/demo | sort by when desc | limit 20",
        "events where event_type:(command.executed|command.imported) AND NOT material:known",
        "events where when in last 7d | group by day(when),source | count | fill | where count > 0",
        "events where hour(when) >= 1 AND seen - when > 1d",
        "events where month_day(when) = month_day(today)",
        "events | join near (commands | as of seq 42) within 5m at end",
        "events where before(commands) within 1m after(commands) within 2m during(intervals) containing(commands)",
        "commands where exit_code != 0 | fields command, next.command",
        "commands where after(commands where exit_code != 0) within 1 step",
        "events | as of 2026-10-02",
    ] {
        roundtrip(text, &c);
    }
}
#[test]
fn extension_nodes_roundtrip() {
    let mut c = support::mixed();
    c.extensions.extend([
        Extension {
            name: "annotated".into(),
            position: ExtensionPosition::Atom,
            arguments: vec![
                ExtensionSlot::Field,
                ExtensionSlot::Keyword("with".into()),
                ExtensionSlot::Value,
            ],
            help: String::new(),
        },
        Extension {
            name: "custom".into(),
            position: ExtensionPosition::Transform,
            arguments: vec![
                ExtensionSlot::Name,
                ExtensionSlot::Unit,
                ExtensionSlot::Condition,
                ExtensionSlot::Query,
                ExtensionSlot::Optional(vec![
                    ExtensionSlot::Keyword("with".into()),
                    ExtensionSlot::Value,
                ]),
            ],
            help: String::new(),
        },
    ]);
    for text in [
        "pl.messages where annotated text with \"AND | value\" | custom output sx.events (text ~ \"query\") (sx.events | as of seq 42) with \"literal\"",
        "pl.messages | custom output sx.events (text ~ \"query\") (sx.events)",
    ] {
        roundtrip(text, &c);
    }
}
#[test]
fn unrepresentable_trees_are_refused() {
    let c = support::polylogue();
    let mut query = parse("origin:demo", &c).unwrap();
    query.transforms.push(Transform {
        span: Span::default(),
        kind: TransformKind::Sort { keys: Vec::new() },
    });
    assert!(format_query(&query, &c).is_err());
    // a - (b - c) needs parentheses the language does not have.
    let field = |name: &str| Box::new(Expression::field(name));
    let mut query = parse("cost > 1", &c).unwrap();
    let Source::Unit {
        condition:
            Some(Condition {
                kind: ConditionKind::Compare { left, .. },
                ..
            }),
        ..
    } = &mut query.source
    else {
        panic!()
    };
    *left = Expression::Arithmetic {
        operator: Arithmetic::Subtract,
        left: field("cost"),
        right: Box::new(Expression::Arithmetic {
            operator: Arithmetic::Subtract,
            left: field("cost"),
            right: field("cost"),
        }),
    };
    assert!(format_query(&query, &c).is_err());
}
proptest! {
 #[test]
 fn quoted_text_preserves_every_character(text in any::<String>()){
  let catalog=support::polylogue();let query=Query{source:Source::Unit{unit:"sessions".into(),implicit:false,condition:Some(Condition{span:Span::default(),kind:ConditionKind::Text{text,mode:TextMode::Phrase}})},attach:Vec::new(),transforms:Vec::new()};
  let rendered=format_query(&query,&catalog).unwrap();let parsed=parse(&rendered,&catalog).unwrap();prop_assert_eq!(normal_form(&query,&catalog).unwrap(),normal_form(&parsed,&catalog).unwrap());
 }
}

proptest! {
    #[test]
    fn scalar_quoting_cannot_introduce_syntax(text in any::<String>()) {
        let catalog=support::polylogue();
        let query=Query{source:Source::Unit{unit:"sessions".into(),implicit:false,condition:Some(Condition{span:Span::default(),kind:ConditionKind::Match{field:"title".into(),value:Value::Scalar{text,quoted:false}}})},attach:Vec::new(),transforms:Vec::new()};
        let rendered=format_query(&query,&catalog).unwrap();let parsed=parse(&rendered,&catalog).unwrap();prop_assert_eq!(normal_form(&query,&catalog).unwrap(),normal_form(&parsed,&catalog).unwrap());
    }
}

#[test]
fn quoted_comparison_symbols_remain_scalar_values() {
    let catalog = support::polylogue();
    for text in ["<", ">", "<=", ">=", "=", "!=", "~", "^"] {
        let source = format!("title:\"{text}\"");
        roundtrip(&source, &catalog);
        let query = parse(&source, &catalog).unwrap();
        assert!(
            matches!(query.source,Source::Unit{condition:Some(Condition{kind:ConditionKind::Match{value:Value::Scalar{text:ref literal,..},..},..}),..} if literal.as_str()==text)
        );
    }
}

#[test]
fn format_roundtrip_time_bounds_precede_initial_field_guard() {
    for catalog in [support::polylogue(), support::sinex()] {
        let main_time = catalog
            .unit(&catalog.default_unit)
            .unwrap()
            .main_time
            .clone()
            .unwrap();
        for (keyword, upper, until) in [("since", "SINCE", false), ("until", "UNTIL", true)] {
            // The one spelling is `since:T` (README grammar, `"since:" time`);
            // the spaced form is refused with the joined one.
            for spaced in [
                format!("{keyword} 2026-10-02"),
                format!("{upper} 2026-10-02"),
            ] {
                let error = parse(&spaced, &catalog).unwrap_err();
                assert_eq!(
                    error.suggestion.as_deref(),
                    Some(format!("{keyword}:2026-10-02").as_str())
                );
            }
            for spelling in [
                format!("{keyword}:2026-10-02"),
                format!("{upper}:2026-10-02"),
            ] {
                let query = parse(&spelling, &catalog).unwrap();
                let Source::Unit {
                    condition: Some(condition),
                    implicit: true,
                    ..
                } = &query.source
                else {
                    panic!("{spelling}")
                };
                let (field, value) = match &condition.kind {
                    ConditionKind::Since { field, value } if !until => (field, value),
                    ConditionKind::Until { field, value } if until => (field, value),
                    other => panic!("wrong bound: {other:?}"),
                };
                assert_eq!(field, &main_time);
                assert!(matches!(value, Value::Scalar { text, .. } if text == "2026-10-02"));
                roundtrip(&spelling, &catalog);
            }
            // Values keep their spelling even when the positional keyword is folded.
            let query = parse(&format!("{upper} $ExactCase"), &catalog).unwrap();
            let rendered = format_query(&query, &catalog).unwrap();
            assert_eq!(rendered, format!("{keyword} $ExactCase"));
        }
    }
    let catalog = support::polylogue();
    roundtrip("since = literal", &catalog);
    roundtrip("\"since\"", &catalog);
    assert!(parse("title", &catalog).is_err());
    let mut no_time = catalog.clone();
    no_time
        .units
        .iter_mut()
        .for_each(|unit| unit.main_time = None);
    assert!(parse("since:2026-10-02", &no_time).is_err());
}

#[test]
fn format_roundtrip_random_preserves_explicit_seed_presence() {
    for catalog in [support::polylogue(), support::sinex()] {
        let bare = parse("| sort by random", &catalog).unwrap();
        let empty = parse("| sort by RANDOM()", &catalog).unwrap();
        assert_eq!(
            normal_form(&bare, &catalog).unwrap(),
            normal_form(&empty, &catalog).unwrap()
        );
        let TransformKind::Sort { keys } = &empty.transforms[0].kind else {
            panic!()
        };
        assert!(
            matches!(&keys[0].expression, Expression::Function { name, arguments } if name == "random" && arguments.is_empty())
        );
        for text in [
            "| sort by random",
            "| sort by random()",
            "| sort by random(0)",
            "| sort by random(18446744073709551615) desc",
        ] {
            roundtrip(text, &catalog);
        }
        let seeded = parse("| sort by random(0)", &catalog).unwrap();
        assert_ne!(
            normal_form(&empty, &catalog).unwrap(),
            normal_form(&seeded, &catalog).unwrap()
        );
        for invalid in [
            "| sort by random(1,2)",
            "| sort by random(1",
            "| sort by random(-1)",
        ] {
            assert!(parse(invalid, &catalog).is_err(), "{invalid}");
        }
    }
}

proptest! {
    #[test]
    fn format_roundtrip_random_seed_is_not_replaced(seed in any::<u64>()) {
        for catalog in [support::polylogue(), support::sinex()] {
            let text = format!("| sort by random({seed})");
            let query = parse(&text, &catalog).unwrap();
            let TransformKind::Sort { keys } = &query.transforms[0].kind else { panic!() };
            let Expression::Function { arguments, .. } = &keys[0].expression else { panic!() };
            prop_assert_eq!(arguments, &vec![Expression::value(Value::scalar(seed.to_string()))]);
            roundtrip(&text, &catalog);
        }
    }
}
