//! Binding a parsed tree types it exactly as the parser typed the text: the
//! same scopes, the same operands, the same tree, and the same refusals when
//! a source's output becomes known only at binding.
use super::*;
use crate::catalog::{OutputSchema, OutputSource, SourceOutput};
use proptest::prelude::*;

#[path = "../../tests/support/mod.rs"]
mod support;

const SOURCES: &[&str] = &[
    "",
    "commands",
    "commands where",
    "events where",
    "query:recent",
    "query:recent where",
];
const CONDITIONS: &[&str] = &[
    "",
    "exit_code:0",
    "exit_code:(1|2)",
    "exit_code > 1",
    "duration * 2 > 10",
    "cwd:/tmp",
    "command ~ git",
    "since:7d",
    "until:2026-01-01",
    "exit_code between 1 and 3",
    "when in last week",
    "exit_code in (commands | fields exit_code)",
    "NOT host:a",
    "(exit_code:1 OR exit_code:2)",
    "abs(exit_code) > 1",
    "len(command) > 3",
    "case_eq(command, git)",
    "seq(exit_code:1 -> exit_code:2)",
    "exists events(source:x)",
    "before(commands where exit_code:1) within 5m",
    "near(commands) within 3 steps",
    "\"two words\"",
    "deploy*",
    "state:unknown",
    "source:x",
    "reop:x",
    "exit_code:many",
];
const TRANSFORMS: &[&str] = &[
    "| where exit_code:1",
    "| where match.exit_code:1",
    "| sort by when desc",
    "| sort by random",
    "| sort by exit_code, when",
    "| group by host",
    "| group by day(when) as day",
    "| agg count",
    "| agg sum(duration), max(exit_code)",
    "| agg covered(start, end)",
    "| count",
    "| fields command, exit_code as code",
    "| fields exit_code + 1 as next",
    "| limit 5",
    "| first 2",
    "| distinct",
    "| union (commands where exit_code:1)",
    "| except (commands)",
    "| diff (commands)",
    "| join near (commands) within 5m",
    "| join (events) on id = id",
    "| matches",
    "| fill",
    "| events where source:x",
    "| where code:1",
    "| where count > 2",
    "| sort by count desc",
];

/// The sinex synthetic catalog with one saved question's output declared.
fn with_outputs() -> Catalog {
    let mut catalog = support::sinex();
    let commands = catalog.unit("commands").unwrap().clone();
    catalog.source_outputs.push(SourceOutput {
        source: OutputSource::Saved {
            name: "recent".into(),
            arguments: Vec::new(),
        },
        schema: OutputSchema {
            unit: Some("commands".into()),
            fields: commands
                .fields
                .iter()
                .filter(|field| {
                    ["command", "exit_code", "when", "host", "id", "duration"]
                        .contains(&field.name.as_str())
                })
                .cloned()
                .collect(),
            main_time: Some("when".into()),
            match_units: Some(Vec::new()),
        },
    });
    catalog
}

fn text(source: &str, condition: &str, transforms: &[&str]) -> String {
    let mut parts = vec![source, condition];
    parts.extend(transforms);
    parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// One query: what binding the parsed tree gives against `bound_catalog`
/// is what parsing the text against `bound_catalog` gives. Returns whether
/// the text was a query.
fn agrees(text: &str, parse_catalog: &Catalog, bound_catalog: &Catalog) -> Result<bool, String> {
    let Ok(tree) = crate::parse::parse(text, parse_catalog) else {
        return Ok(false);
    };
    let bound = bind(&tree, bound_catalog);
    match crate::parse::parse_typed(text, bound_catalog) {
        Ok(typed) => {
            let bound = bound.map_err(|error| format!("{text}: bind refused: {error:?}"))?;
            if bound.typing() != &typed.typing {
                return Err(format!("{text}: typing differs"));
            }
            if bound.query() != &typed.query {
                return Err(format!(
                    "{text}: tree differs\n bound {:?}\n typed {:?}",
                    bound.query(),
                    typed.query
                ));
            }
            Ok(true)
        }
        Err(refusal) => {
            let error = match bound {
                Ok(_) => {
                    return Err(format!(
                        "{text}: bound although the text is refused: {refusal:?}"
                    ));
                }
                Err(error) => error,
            };
            if (error.kind, &error.message) != (refusal.kind, &refusal.message) {
                return Err(format!(
                    "{text}: bind refused {error:?}, parse refused {refusal:?}"
                ));
            }
            Ok(true)
        }
    }
}

/// Every source with every condition and every transform, and every pair of
/// transforms after a plain source: binding agrees with parsing, both when
/// the catalog is the one the text was parsed with and when a saved
/// question's output becomes known only at binding.
#[test]
fn binding_a_parsed_tree_agrees_with_parsing_its_text() {
    let plain = support::sinex();
    let known = with_outputs();
    let mut texts = Vec::new();
    for source in SOURCES {
        for condition in CONDITIONS {
            if source.ends_with("where") && condition.is_empty() {
                continue;
            }
            texts.push(text(source, condition, &[]));
            for transform in TRANSFORMS {
                texts.push(text(source, condition, &[transform]));
            }
        }
    }
    for first in TRANSFORMS {
        for second in TRANSFORMS {
            texts.push(text("commands", "", &[first, second]));
            texts.push(text("query:recent", "", &[first, second]));
        }
    }
    let mut queries = 0;
    let mut failures = Vec::new();
    for text in &texts {
        for (parse_catalog, bound_catalog) in [(&known, &known), (&plain, &known)] {
            match agrees(text, parse_catalog, bound_catalog) {
                Ok(true) => queries += 1,
                Ok(false) => {}
                Err(failure) => failures.push(failure),
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    // The fragments are mostly valid together; a grammar change that made
    // them fail to parse would leave this test checking nothing.
    assert!(
        queries > texts.len() / 2,
        "{queries} queries in two catalog pairings of {} texts",
        texts.len()
    );
}

proptest! {
    #[test]
    fn binding_a_generated_parsed_tree_agrees_with_parsing_its_text(
        source in prop::sample::select(SOURCES),
        condition in prop::sample::select(CONDITIONS),
        transforms in prop::collection::vec(prop::sample::select(TRANSFORMS), 0..5),
        output_known_when_parsed in any::<bool>(),
    ) {
        let known = with_outputs();
        let plain = support::sinex();
        let parse_catalog = if output_known_when_parsed { &known } else { &plain };
        let text = text(source, condition, &transforms);
        prop_assert_eq!(agrees(&text, parse_catalog, &known).err(), None);
    }
}

#[test]
fn polylogue_forms_bind_as_they_parse() {
    let catalog = support::polylogue();
    let mut queries = 0;
    for text in [
        "origin:demo NOT tag:old OR title:demo",
        "messages where written_by:model | group by session | agg count",
        "last with messages(text)[written_by:model, last:1]",
        "sessions | sort by rank",
        "sessions where similar:\"rate limit\"",
        "sessions | group by origin | where count > 1 | sort by count desc",
        "messages | fields text, session as s | where s:x",
        "sessions | diff (sessions where origin:demo)",
    ] {
        match agrees(text, &catalog, &catalog) {
            Ok(true) => queries += 1,
            Ok(false) => {}
            Err(failure) => panic!("{failure}"),
        }
    }
    assert!(queries >= 4, "{queries}");
}
