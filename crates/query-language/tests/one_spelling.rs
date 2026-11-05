//! The one spelling, against both catalogs: the real-question examples of
//! both products parse with the same grammar, and every refusal of a former
//! or mistaken spelling carries a suggestion that parses as given.
mod support;
use proptest::prelude::*;
use query_language::{
    ErrorKind, ast::*, bind::bind, catalog::Catalog, format::format_query, identity::normal_form,
    parse::parse,
};

fn parses(catalog: &Catalog, text: &str) -> Query {
    let query = parse(text, catalog).unwrap_or_else(|e| panic!("{text}: {e}"));
    bind(&query, catalog).unwrap_or_else(|e| panic!("{text} does not bind: {e}"));
    let rendered = format_query(&query, catalog).unwrap_or_else(|e| panic!("{text}: {e}"));
    assert_eq!(
        normal_form(&query, catalog).unwrap(),
        normal_form(&parse(&rendered, catalog).unwrap(), catalog).unwrap(),
        "{text} => {rendered}"
    );
    query
}
/// The refusal's suggestion, which must itself parse, bind and format back
/// to the same normal form on the same catalog. Running it is each product's
/// half of the law, through its own compiler.
fn suggestion(catalog: &Catalog, text: &str) -> String {
    let error = parse(text, catalog).expect_err(text);
    let suggestion = error
        .suggestion
        .unwrap_or_else(|| panic!("{text}: no suggestion for {}", error.message));
    parses(catalog, &suggestion);
    suggestion
}

/// Law `ql.one-spelling`: the real-question examples of both products parse,
/// bind and round-trip with one grammar; nothing in a catalog selects a dialect.
#[test]
fn one_spelling() {
    let polylogue = support::polylogue();
    for text in [
        "last with messages(text)[written_by:model, last:1]",
        "tool_calls where session:last tool:(TodoWrite|update_plan) | sort by at desc | limit 1 | fields input",
        "messages where written_by:model text ~ \"7/10\" at between 2026-03-20 and 2026-04-15",
        "\"topic\" since:2026-07-01 | group by repo | count",
        "sessions where origin:claude-code count(messages) >= 200 | sort by random | limit 1",
        "messages where written_by:model \"top 5\" cwd:/realm/project/demo",
        "messages where written_by:human since:2026-03-01 similar:\"what I am building and why\"",
        "messages where written_by:model \"tests pass\" NOT after(tool_calls where command ~ \"cargo test\" outcome:ok) within 10 steps",
        "tool_calls where action:file_edit path:AGENTS.md | sort by at",
        "tool_calls where outcome:error | group by next.action | count",
        "tool_calls where tool ^ \"mcp__server__\" | group by tool | count",
        "sessions where last_active between 2026-05-24T17:08 and 2026-05-24T17:13 | sort by cost desc",
        "since:1d | group by origin, model | agg count, sum(cost)",
        "sessions | group by origin, month(started) | count",
        "since:2026-04-24 | group by day(started) | agg count, sum(cost) | fill",
    ] {
        parses(&polylogue, text);
    }
    // Saved and kept sources bind once the product supplies their output.
    parse("query:open-questions | diff (result:synthetic)", &polylogue).unwrap();
    let sinex = support::sinex();
    for text in [
        "commands where exit_code != 0 since:7d | group by day(when) | count | fill",
        "events where source != terminal.* near(commands where command ~ \"cargo test\" exit_code != 0) within 5m",
        "visits where during(focus where app:chromium)",
        "shell-sessions where containing(commands where command ~ \"git push\")",
        "events where month_day(when) = month_day(today)",
        "commands where when > 2026-02-01 | as of 2026-03-01",
        "events where seen - when > 1d | group by source | count",
        "commands where exit_code != 0 command ~ \"cargo build\" | fields command, next.command",
        "events where hour(when) >= 1 AND hour(when) < 5 | group by source | count",
    ] {
        parses(&sinex, text);
    }
    let mixed = support::mixed();
    for text in [
        "pl.tool_calls where session:self outcome:error command ~ \"cargo test\" | join near (sx.events where event_type:resource.sampled) within 1m",
        "pl.sessions where since:7d | fields repo, started as start, last_active as end, \"agent\" as who | union (sx.shell-sessions where since:7d | fields host as repo, start, end, \"operator\" as who) | group by repo, who | agg covered(start, end)",
        "sx.visits where during(pl.sessions where self)",
        "pl.tool_calls where command ~ \"git commit\" | join (sx.events where event_type:commit.created) on commit = key",
        "pl.sessions where cwd in (sx.shell-sessions where when in yesterday | group by cwds)",
    ] {
        let query = parses(&mixed, text);
        assert_eq!(
            query.namespaces().into_iter().collect::<Vec<_>>(),
            ["pl", "sx"],
            "{text}"
        );
    }
}

/// Law `ql.lowercase-boolean`: a lowercase `and`, `or` or `not` between
/// conditions is refused with the capitalised query; quoted, it searches.
#[test]
fn lowercase_boolean() {
    for (catalog, text, expected) in [
        (
            support::polylogue(),
            "tool_calls where outcome:error and tool:Bash",
            "tool_calls where outcome:error AND tool:Bash",
        ),
        (support::polylogue(), "rock and roll", "rock AND roll"),
        (
            support::sinex(),
            "events where source = a or source = b",
            "events where source = a OR source = b",
        ),
        (
            support::sinex(),
            "events where source = a not source = b",
            "events where source = a NOT source = b",
        ),
    ] {
        assert_eq!(suggestion(&catalog, text), expected);
        assert_eq!(
            parse(text, &catalog).unwrap_err().kind,
            ErrorKind::InvalidQuery
        );
    }
    let c = support::polylogue();
    let quoted = parse("rock \"and\" roll", &c).unwrap();
    let Source::Unit {
        condition:
            Some(Condition {
                kind: ConditionKind::And { conditions },
                ..
            }),
        ..
    } = quoted.source
    else {
        panic!()
    };
    assert_eq!(conditions.len(), 3);
    // A trailing word is a search word: nothing stands after it.
    parse("rock and", &c).unwrap();
}

/// Law `ql.expr.dimensions`: arithmetic and comparison of incompatible
/// dimensions are refused when the query is read, in both catalogs;
/// compatible ones carry their dimension.
#[test]
fn expr_dimensions() {
    let polylogue = support::polylogue();
    let sinex = support::sinex();
    for (c, text, message) in [
        (
            &polylogue,
            "sessions | fields cost + tool_time as nonsense",
            "cannot combine",
        ),
        (
            &polylogue,
            "sessions | fields title * 2 as nonsense",
            "cannot combine",
        ),
        (
            &polylogue,
            "sessions where started + started > 1",
            "cannot combine",
        ),
        (
            &polylogue,
            "sessions | fields cost - 1d as nonsense",
            "cannot combine",
        ),
        (&polylogue, "sessions where cost * 2 > 1h", "cannot compare"),
        (
            &polylogue,
            "sessions where last_active - started > yesterday",
            "cannot compare",
        ),
        (
            &polylogue,
            "sessions where tool_time * 2 > 3",
            "cannot compare",
        ),
        (&polylogue, "sessions where cost + 1 > 1h", "cannot compare"),
        (
            &sinex,
            "events | fields seen + when as nonsense",
            "cannot combine",
        ),
        (&sinex, "commands where end - start > 5", "cannot compare"),
        (
            &sinex,
            "commands where end - start > yesterday",
            "cannot compare",
        ),
        (
            &sinex,
            "events where seen - when > 2026-01-01",
            "cannot compare",
        ),
    ] {
        let error = parse(text, c).unwrap_err();
        assert_eq!(error.kind, ErrorKind::InvalidQuery, "{text}");
        assert!(error.message.contains(message), "{text}: {}", error.message);
    }
    for (c, text) in [
        (
            &polylogue,
            "sessions | fields last_active - started as span",
        ),
        (&polylogue, "sessions | fields cost / messages as each"),
        (&polylogue, "sessions | fields cost / cost as ratio"),
        (&polylogue, "sessions where started + 1d > 2026-01-01"),
        (&polylogue, "sessions where tool_time * 2 > 1h"),
        (&polylogue, "sessions where cost - 2.90 > 0"),
        (&polylogue, "sessions where cost * 2 > 2.90"),
        (&sinex, "events where seen - when > 1d"),
        (&sinex, "commands where end - start > 5m"),
        (&sinex, "commands | fields end - start as took"),
    ] {
        parses(c, text);
    }
}

/// Law `ql.error.suggestion-runs`: the former spellings and common mistakes
/// are refused with a corrected query that parses as given.
#[test]
fn error_suggestion_runs() {
    let sinex = support::sinex();
    for (text, expected) in [
        (
            "commands where exit_code != 0 sort when desc",
            "commands where exit_code != 0 | sort by when desc",
        ),
        (
            "events where event_type = [command.executed, command.imported]",
            "events where event_type:(command.executed|command.imported)",
        ),
        (
            "events where material exists",
            "events where material:known",
        ),
        (
            "commands | group by cwd | sum(duration)",
            "commands | group by cwd | agg sum(duration)",
        ),
        (
            "commands | agg count, sum:duration",
            "commands | agg count, sum(duration)",
        ),
        ("events | since 7d", "events | where since:7d"),
        ("events where since 7d", "events where since:7d"),
        (
            "events where until yesterday",
            "events where until:yesterday",
        ),
        ("commands | sort when", "commands | sort by when"),
        (
            "commands where when > 2026-02-01 as of 2026-03-01",
            "commands where when > 2026-02-01 | as of 2026-03-01",
        ),
        ("commands | srot by when", "commands | sort by when"),
        (
            "comands where exit_code != 0",
            "commands where exit_code != 0",
        ),
        (
            "commands where exti_code != 0",
            "commands where exit_code != 0",
        ),
        ("events where when > 7d", "events where when in last 7d"),
    ] {
        assert_eq!(suggestion(&sinex, text), expected, "{text}");
    }
    let polylogue = support::polylogue();
    for (text, expected) in [
        (
            "seq(tool:Bash -> [within:5m] tool:Edit)",
            "seq(tool:Bash -> [within 5m] tool:Edit)",
        ),
        (
            "messages where similar:\"x\" OR origin:codex",
            "messages where origin:codex similar:\"x\"",
        ),
        ("sessions where reop:demo", "sessions where repo:demo"),
        ("title", "\"title\""),
    ] {
        assert_eq!(suggestion(&polylogue, text), expected, "{text}");
    }
    // Only a value that reads as a time is joined to `since:`.
    for text in ["events where since cost", "events | since cost"] {
        if let Err(error) = parse(text, &sinex) {
            assert!(
                !error.suggestion.as_deref().unwrap_or("").contains("since:"),
                "{text}: {:?}",
                error.suggestion
            );
        }
    }
}

proptest! {
    /// Law `ql.error.suggestion-runs`, generated against both catalogs:
    /// whatever mistakes a query carries, a suggestion parses and binds.
    #[test]
    fn suggestions_always_parse(
        sinex in any::<bool>(),
        words in prop::collection::vec(prop::sample::select(vec![
            "origin:demo", "and", "or", "not", "cost", ">", "1", "sort", "by", "|", "group",
            "agg", "sum:cost", "count", "where", "(", ")", "[", "]", ",", "exists", "repo:x",
            "reop:x", "sesions", "similar:x", "OR", "NOT", "-", "*", "known", "within", "near",
            "since", "7d", "events", "commands", "exit_code", "=", "source", "a", "when",
            "duration", "comands", "sum(duration)",
        ]), 1..9),
    ) {
        let catalog = if sinex { support::sinex() } else { support::polylogue() };
        let text = words.join(" ");
        if let Err(error) = parse(&text, &catalog)
            && let Some(suggestion) = error.suggestion
        {
            let parsed = parse(&suggestion, &catalog);
            prop_assert!(parsed.is_ok(), "{} suggested {}: {:?}", text, suggestion, parsed.err());
            let bound = bind(&parsed.unwrap(), &catalog);
            prop_assert!(bound.is_ok(), "{} suggested {}: {:?}", text, suggestion, bound.err());
        }
    }
}

#[test]
fn ranking_atoms_stand_only_in_a_top_level_and() {
    let c = support::polylogue();
    for text in [
        "messages where NOT similar:\"x\"",
        "messages where exists tool_calls(similar:\"x\")",
        "messages where origin:a OR (origin:b similar:\"x\")",
    ] {
        let error = parse(text, &c).unwrap_err();
        assert!(error.message.contains("ranks the whole query"), "{text}");
    }
    for text in [
        "messages where similar:\"x\" origin:codex",
        "messages | where around:2026-10-01",
        "sessions | fuse (similar:\"x\")",
    ] {
        parses(&c, text);
    }
}

#[test]
fn order_relations_count_steps_only_inside_one_order_scope() {
    let c = support::polylogue();
    for (text, fragment) in [
        (
            "sessions where after(sessions) within 1 step",
            "no order scope",
        ),
        (
            "messages where during(tool_calls) within 2 steps",
            "compares times",
        ),
        ("messages where near(tool_calls)", "needs a distance"),
        (
            "messages where after(tool_calls) within 2 steps at end",
            "steps count positions",
        ),
    ] {
        let error = parse(text, &c).unwrap_err();
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
    }
    let mixed = support::mixed();
    let error = parse("pl.messages where after(sx.events) within 1 step", &mixed).unwrap_err();
    assert!(error.message.contains("never cross"), "{}", error.message);
    let q = parses(
        &c,
        "messages where after(tool_calls) within 3 steps in session",
    );
    let Source::Unit {
        condition:
            Some(Condition {
                kind:
                    ConditionKind::Order {
                        within: Some(Distance::Steps { count: 3 }),
                        scope: Some(scope),
                        ..
                    },
                ..
            }),
        ..
    } = &q.source
    else {
        panic!("{q:?}")
    };
    assert_eq!(scope, "session");
    assert!(
        parse(
            "messages where after(tool_calls) within 3 steps in turn",
            &c
        )
        .is_err()
    );
}

#[test]
fn transforms_declare_the_fields_they_add() {
    let c = support::polylogue();
    let typed = |text: &str| {
        let bound = bind(&parse(text, &c).unwrap(), &c).unwrap();
        bound.typing().queries[0]
            .transforms
            .last()
            .unwrap()
            .after
            .clone()
    };
    let scope = typed("sessions where seq(tool:Bash -> tool:Edit) | matches");
    for name in [
        "parent",
        "match",
        "position",
        "step",
        "step.tool",
        "step.outcome",
    ] {
        assert!(scope.field(name).is_some(), "{name}");
    }
    let scope = typed("sessions | diff (origin:demo)");
    assert_eq!(
        scope.field("change").unwrap().field.values,
        ["joined", "left", "moved"]
    );
    assert!(scope.field("old_rank").is_some() && scope.field("title").is_some());
    let scope = typed("messages | join near (tool_calls) within 2 steps");
    assert!(scope.field("match.tool").is_some() && scope.field("match.distance").is_some());
    assert!(scope.field("text").is_some());
    let scope = typed("since:7d | group by day(started) | count | fill");
    assert_eq!(
        scope.field("coverage").unwrap().field.values,
        query_language::coverage::CoverageState::STATES
    );
    // One top-level order relation exposes its nearest inner row.
    let scope = typed("messages where after(tool_calls) within 2 steps | sort by match.distance");
    assert!(scope.field("match.tool").is_some());
    for (text, fragment) in [
        ("query:saved | matches", "hides its own"),
        ("sessions | matches", "one top-level"),
        (
            "sessions where exists tool_calls() exists messages() | matches",
            "exactly one",
        ),
        ("sessions | diff (messages)", "one unit"),
        ("sessions | union (messages)", "one unit"),
        (
            "sessions | agg covered(started, cost)",
            "one time or number kind",
        ),
        ("sessions | fill", "time bucket"),
    ] {
        let error = parse(text, &c).unwrap_err();
        assert!(
            error.message.contains(fragment),
            "{text}: {}",
            error.message
        );
    }
}

/// Law `ql.identity.spelling`: spellings with one normal form share one
/// `query:<hash>` in both catalogs, and any changed condition changes it.
#[test]
fn identity_spelling_in_both_catalogs() {
    use query_language::identity::question_id;
    use std::collections::BTreeMap;
    let id = |catalog: &Catalog, text: &str| {
        question_id(&parses(catalog, text), catalog, &BTreeMap::new()).unwrap()
    };
    let polylogue = support::polylogue();
    let sinex = support::sinex();
    for (catalog, same, changed) in [
        (
            &polylogue,
            vec![
                "sessions where repo:demo cost > 2",
                "sessions where cost > 2 AND repo:demo",
                "sessions  where   (repo:demo)   cost >  2",
                "sessions where cost > 2.0 repo:demo",
            ],
            vec![
                "sessions where repo:demo cost > 3",
                "sessions where repo:other cost > 2",
                "sessions where repo:demo cost >= 2",
                "sessions where repo:demo OR cost > 2",
            ],
        ),
        (
            &sinex,
            vec![
                "commands where exit_code != 0 | sort by when desc",
                "commands where (exit_code != 0) | sort by when desc",
                "commands  where exit_code  !=  0 | sort by when desc",
            ],
            vec![
                "commands where exit_code != 1 | sort by when desc",
                "commands where exit_code != 0 | sort by when",
                "commands where exit_code = 0 | sort by when desc",
            ],
        ),
    ] {
        let first = id(catalog, same[0]);
        for text in &same[1..] {
            assert_eq!(
                id(catalog, text),
                first,
                "{text} is spelled differently, not asked differently"
            );
        }
        for text in changed {
            assert_ne!(id(catalog, text), first, "{text} asks something else");
        }
    }
}

/// Law `ql.generated-names`: a field the language generates never hides a
/// stored field, in either catalog; Sinex's stored `events.offset` stays
/// `offset` beside an order relation's `match.distance`.
#[test]
fn generated_names_never_hide_stored_fields() {
    use query_language::catalog::{Field, FieldKind};
    for catalog in [support::polylogue(), support::sinex()] {
        assert_eq!(catalog.hidden_fields(), Vec::<String>::new());
    }
    let sinex = support::sinex();
    let bound = bind(
        &parse(
            "events where near(commands) within 5m | sort by offset, match.distance",
            &sinex,
        )
        .unwrap(),
        &sinex,
    )
    .unwrap();
    let scope = &bound.typing().queries[0].transforms.last().unwrap().after;
    assert_eq!(
        scope.field("offset").unwrap().field.kind,
        FieldKind::Integer
    );
    assert_eq!(
        scope.field("match.distance").unwrap().field.kind,
        FieldKind::Duration
    );
    let mut clashing = support::sinex();
    for name in ["distance", "change"] {
        clashing.units[0].fields.push(Field {
            name: name.into(),
            kind: FieldKind::Integer,
            ..Field::default()
        });
    }
    let unit = &clashing.units[0].name;
    assert_eq!(
        clashing.hidden_fields(),
        vec![format!("{unit}.distance"), format!("{unit}.change")]
    );
}

/// More than one primitive correction is completed at the outer boundary.
#[test]
fn correction_chains_reach_a_runnable_question() {
    for catalog in [support::polylogue(), support::sinex()] {
        assert_eq!(
            suggestion(&catalog, "rock and roll and jazz"),
            "rock AND roll AND jazz"
        );
        assert_eq!(
            suggestion(&catalog, "rock or roll and jazz"),
            "rock OR roll AND jazz"
        );
    }
}
