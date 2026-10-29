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
