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
#[test]
fn polylogue_forms_roundtrip() {
    let c = support::polylogue();
    for text in [
        "",
        "origin:demo NOT tag:old OR title:demo",
        "origin:(a|b|unknown)",
        "kind:*",
        "cost >= 2.90usd",
        "started between 2026-10-02 and 2026-10-03",
        "title ~ \"contains text\"",
        "a refactor* refactor~",
        "weekday_offset()",
        "parse(x) fails",
        "exists tool_call(outcome:error)",
        "exists tool_call()",
        "count(tool_call where outcome:error) >= 3",
        "id in (messages where role:user | group by session)",
        "similar:\"semantic question\"",
        "query:review(repo=demo,text=\"AND | punctuation\") | limit 4",
        "result:synthetic-id | distinct",
        "seq all(tool:Bash -> tool:Edit -> [next] outcome:error -> [within:5m] tool:Bash) | matches",
        "origin:demo with messages(id,role)[role:assistant,last:2],tool_calls(tool)[first:1] | fields id,title",
        "sessions | sort by random(7) | offset 2 | limit 10",
        "sessions where since:30d | measure cost by day",
        "sessions | sort by origin asc,title desc",
        "tool_calls | group by day(at),tool | agg count,sum:duration,p90:duration,count_distinct:session,histogram:duration,corr:duration,exit_code | where count > 1 | sort by sum_duration desc | first 2",
        "sessions | union (origin:demo) | intersect (origin:demo) | except (origin:other) | fuse (similar:demo)",
        "origin:demo | messages where role:user | measure \"synthetic measure\"",
        "session:origin:synthetic-id",
        "span:message#c=1-2",
        "https://example.invalid/?x=y",
        "origin:$provider",
    ] {
        roundtrip(text, &c);
    }
}
#[test]
fn sinex_forms_roundtrip() {
    let c = support::sinex();
    for text in [
        "commands where exit_code != 0 AND cwd ^ /src/demo sort when desc limit 20",
        "events where event_type = [command.executed,command.imported] AND NOT material exists",
        "events where when in last 7d group by day(when),source count fill where count > 0",
        "events where hour(when) >= 1 AND seen - when > 1d",
        "events where overlaps 2026-10-01 .. 2026-10-02",
        "events near (commands as of seq 42) at end within 5m before (commands) within 1m after (commands) within 2m during (intervals) containing (commands)",
        "recall",
        "recall yesterday",
        "recall 2026-10-02 14:05 ±20m",
        "recall mark 2 ±5m",
        "recall last where cwds ^ \"/src/demo (notes)\"",
        "trace sinex:ev/synthetic-id",
        "descendants sinex:ev/synthetic-id",
        "parents sinex:ev/synthetic-id",
        "events as of 2026-10-02",
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
            position: ExtensionPosition::Stage,
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
        "pl.messages where annotated text with \"AND | value\" | custom output sx.events (text ~ \"query\") (sx.events as of seq 42) with \"literal\"",
        "pl.messages | custom output sx.events (text ~ \"query\") (sx.events)",
    ] {
        roundtrip(text, &c);
    }
}
#[test]
fn unrepresentable_trees_are_refused() {
    let c = support::polylogue();
    let mut query = parse("origin:demo", &c).unwrap();
    query.source = Source::RecallLast {
        unit: "messages".into(),
        condition: None,
    };
    assert!(format_query(&query, &c).is_err());
    let mut query = parse("origin:demo", &c).unwrap();
    query.stages.push(Stage {
        span: Span::default(),
        kind: StageKind::Sort { keys: Vec::new() },
    });
    assert!(format_query(&query, &c).is_err());
}
proptest! {
 #[test]
 fn quoted_text_preserves_every_character(text in any::<String>()){
  let catalog=support::polylogue();let query=Query{source:Source::Unit{unit:"sessions".into(),implicit:false,condition:Some(Condition{span:Span::default(),kind:ConditionKind::Text{text,mode:TextMode::Phrase}})},attach:Vec::new(),stages:Vec::new()};
  let rendered=format_query(&query,&catalog).unwrap();let parsed=parse(&rendered,&catalog).unwrap();prop_assert_eq!(normal_form(&query,&catalog).unwrap(),normal_form(&parsed,&catalog).unwrap());
 }
}

proptest! {
    #[test]
    fn scalar_quoting_cannot_introduce_syntax(text in any::<String>()) {
        let catalog=support::polylogue();
        let query=Query{source:Source::Unit{unit:"sessions".into(),implicit:false,condition:Some(Condition{span:Span::default(),kind:ConditionKind::Match{field:"title".into(),value:Value::Scalar{text,quoted:false}}})},attach:Vec::new(),stages:Vec::new()};
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
