//! Nesting is bounded by memory, not by the thread's stack: every walk over
//! the tree grows its stack as it goes. These run on a thread whose own
//! stack could not hold a few dozen parser levels.
use crate::support;
use proptest::prelude::*;
use query_language::{bind::bind, format::format_query, identity::normal_form, parse::parse};

const SMALL_STACK: usize = 64 * 1024;
const DEPTH: usize = 1000;

fn on_small_stack(work: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(work)
        .unwrap()
        .join()
        .unwrap();
}

/// Everything a product does with a parsed query, none of which may run out
/// of stack. Refusals are fine; crashes are not.
fn exercise(text: &str) {
    let catalog = support::polylogue();
    let Ok(query) = parse(text, &catalog) else {
        return;
    };
    let copy = query.clone();
    assert_eq!(copy, query);
    let _ = format!("{query:?}");
    let _ = serde_json::to_string(&query).unwrap();
    let _ = bind(&query, &catalog);
    if let Ok(rendered) = format_query(&query, &catalog) {
        let _ = parse(&rendered, &catalog);
    }
    // The normal form is a JSON tree as deep as the query: drop it a level
    // at a time.
    if let Ok(form) = normal_form(&query, &catalog) {
        query_language::depth::json::drop(form);
    }
}

fn repeat(prefix: &str, middle: &str, suffix: &str) -> String {
    format!("{}{middle}{}", prefix.repeat(DEPTH), suffix.repeat(DEPTH))
}

#[test]
fn deep_nesting_parses_and_walks_on_a_small_stack() {
    let texts = [
        repeat("(", "\"x\"", ")"),
        repeat("NOT ", "\"x\"", ""),
        repeat("-", "\"x\"", ""),
        repeat("\"x\" OR NOT (\"y\" AND (", "\"z\"", "))"),
        repeat("origin:(", "a", ")"),
        format!("words > 1{}", " + 1".repeat(DEPTH)),
        repeat("exists(messages where ", "\"x\"", ")"),
    ];
    on_small_stack(move || {
        for text in &texts {
            exercise(text);
        }
    });
}

#[test]
fn a_set_inside_a_set_adds_its_values() {
    let catalog = support::polylogue();
    let nested = parse("origin:(a|(b|(c)))", &catalog).unwrap();
    let flat = parse("origin:(a|b|c)", &catalog).unwrap();
    assert_eq!(
        normal_form(&nested, &catalog).unwrap(),
        normal_form(&flat, &catalog).unwrap()
    );
}

const TOKENS: &[&str] = &[
    "NOT ",
    "-",
    "(",
    ")",
    " OR ",
    " AND ",
    "\"x\"",
    "origin:",
    "(a|b)",
    "a",
    "words",
    " > ",
    "1",
    " + ",
    " * ",
    " | sort by words",
    " | limit 3",
    "messages",
    " where ",
    "exists(",
    "count(",
    "since:7d",
    "repo:x",
    "|",
    ",",
    "similar:\"y\"",
    " in (",
    "sessions",
];

proptest! {
    /// Random token sequences, many of them deeply nested or malformed: the
    /// whole pipeline answers or refuses, and never crashes.
    #[test]
    fn any_token_sequence_parses_or_refuses(picks in proptest::collection::vec(0..TOKENS.len(), 0..400)) {
        let text: String = picks.into_iter().map(|i| TOKENS[i]).collect();
        on_small_stack(move || exercise(&text));
    }
}
