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
