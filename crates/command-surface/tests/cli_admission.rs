use command_surface::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Deserialize, Serialize, JsonSchema)]
struct Query {
    query: Option<String>,
}
#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct FixedResponse {
    count: u64,
}
static RENDERED: AtomicUsize = AtomicUsize::new(0);
fn render(value: &FixedResponse, _: TextStyle) -> String {
    RENDERED.fetch_add(1, Ordering::SeqCst);
    format!("count: {}\n", value.count)
}
fn words(input: &[&str]) -> Vec<String> {
    input.iter().map(|word| (*word).into()).collect()
}
fn builder(form: ArgumentForm) -> CatalogBuilder {
    let mut catalog = OperationCatalog::builder("example", Deadlines::default());
    catalog
        .declare_tool(ToolFamily {
            name: "query".into(),
            description: "Read values".into(),
            open_world: false,
            idempotent: true,
        })
        .unwrap();
    catalog
        .declare_cli_global("archive", "Archive directory")
        .unwrap();
    let mut spec = OperationSpec::request(
        "query",
        &["find"],
        "query",
        "Find values",
        "Exercise default admission",
    );
    spec.arguments
        .push(ArgumentSpec::new("query", form, "Query"));
    catalog
        .declare::<Query, FixedResponse>(spec, render)
        .unwrap();
    catalog
}
fn accepts_first(word: &str) -> bool {
    word.starts_with("unit:") || word.starts_with('(') || word.contains(' ')
}
#[test]
fn default_admission_checks_the_first_lexical_operand_before_words_join() {
    let mut builder = builder(ArgumentForm::Words);
    builder
        .set_default_operation("query", accepts_first)
        .unwrap();
    let catalog = builder.finish();
    for input in [
        words(&["typo"]),
        words(&["typo", "unit:events"]),
        words(&[
            "--archive",
            "store",
            "--deadline",
            "100",
            "typo",
            "unit:events",
        ]),
        words(&["--", "typo", "unit:events"]),
    ] {
        let failure = catalog.parse_cli(&input, "r").unwrap_err();
        assert_eq!(failure.kind, FailureKind::BadRequest);
        assert!(failure.message.contains("example find ..."));
    }
    for input in [
        words(&["unit:events", "plain"]),
        words(&["--json", "--archive=store", "unit:events", "plain"]),
        words(&["--", "unit:events", "plain"]),
        words(&["find", "unit:events", "plain"]),
    ] {
        assert_eq!(
            catalog.parse_cli(&input, "r").unwrap().request.arguments,
            json!({"query":"unit:events plain"})
        );
    }
    // Shell quoting supplies one lexical word, distinct from two bare words.
    assert_eq!(
        catalog
            .parse_cli(&words(&["quoted words"]), "r")
            .unwrap()
            .request
            .arguments,
        json!({"query":"quoted words"})
    );
    assert!(
        catalog
            .parse_cli(&words(&["quoted", "words"]), "r")
            .is_err()
    );
    // An explicit command does not need the default operation's first-word signal.
    assert_eq!(
        catalog
            .parse_cli(&words(&["find", "plain"]), "r")
            .unwrap()
            .request
            .arguments,
        json!({"query":"plain"})
    );
    // Orientation and explicit structured input have no first lexical operand.
    assert_eq!(
        catalog.parse_cli(&[], "r").unwrap().request.arguments,
        json!({})
    );
    assert_eq!(
        catalog
            .parse_cli(&words(&["--archive", "store"]), "r")
            .unwrap()
            .request
            .arguments,
        json!({})
    );
    assert_eq!(
        catalog
            .parse_cli(&words(&["--request", "{\"query\":\"plain\"}"]), "r")
            .unwrap()
            .request
            .arguments,
        json!({"query":"plain"})
    );
}
#[test]
fn default_admission_leaves_rest_and_literal_separator_words_intact() {
    let mut builder = builder(ArgumentForm::Rest);
    builder
        .set_default_operation("query", accepts_first)
        .unwrap();
    let catalog = builder.finish();
    for input in [
        words(&[
            "--archive",
            "store",
            "--",
            "unit:events",
            "--archive",
            "literal",
        ]),
        words(&["unit:events", "--archive", "literal"]),
    ] {
        assert_eq!(
            catalog.parse_cli(&input, "r").unwrap().request.arguments,
            json!({"query":"unit:events --archive literal"})
        );
    }
    assert_eq!(
        catalog
            .parse_cli(
                &words(&["find", "plain", "--", "--archive", "literal"]),
                "r"
            )
            .unwrap()
            .request
            .arguments,
        json!({"query":"plain -- --archive literal"})
    );
}
#[test]
fn response_validation_decodes_without_rendering_or_changing_the_answer() {
    let catalog = builder(ArgumentForm::Words).finish();
    RENDERED.store(0, Ordering::SeqCst);
    for mut answer in [
        Answer::ok(json!({"count":3})),
        Answer::empty(json!({"count":0})),
        Answer::degraded(json!({"count":2}), vec!["incomplete source".into()]),
        Answer::error(Failure::new(FailureKind::Io, "unavailable")),
    ] {
        answer.envelope.used_tokens = Some(77);
        answer.envelope.notes.push("preserved".into());
        let original = answer.clone();
        catalog.validate_answer("query", &answer).unwrap();
        assert_eq!(answer, original);
    }
    assert_eq!(RENDERED.load(Ordering::SeqCst), 0);
    for value in [
        json!({"count":"wrong"}),
        json!({}),
        json!({"count":3,"extra":true}),
        Value::Null,
    ] {
        for answer in [
            Answer::ok(value.clone()),
            Answer::empty(value.clone()),
            Answer::degraded(value, vec!["incomplete source".into()]),
        ] {
            assert_eq!(
                catalog.validate_answer("query", &answer).unwrap_err().kind,
                FailureKind::Io
            );
            assert!(
                catalog
                    .render_text("query", &answer, TextStyle::Plain)
                    .is_err()
            );
        }
    }
    assert_eq!(RENDERED.load(Ordering::SeqCst), 0);
    let answer = Answer::ok(json!({"count":3})).with_note("note");
    assert_eq!(
        catalog
            .render_text("query", &answer, TextStyle::Plain)
            .unwrap(),
        "count: 3\n"
    );
    assert_eq!(RENDERED.load(Ordering::SeqCst), 1);
    let footer = catalog.render_footer(Via::Cli, &answer);
    assert!(footer.contains("note"));
    assert_eq!(RENDERED.load(Ordering::SeqCst), 1);
    let request = RunnableRequest::new("query", json!({}), "r");
    let rendered = catalog.render_answer(Via::Cli, Some(&request), answer, TextStyle::Plain);
    assert_eq!(rendered.footer, footer);
    assert_eq!(RENDERED.load(Ordering::SeqCst), 2);
    assert!(
        catalog
            .validate_answer("unknown", &Answer::ok(json!({"count":3})))
            .is_err()
    );
}
