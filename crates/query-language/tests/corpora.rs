//! Synthetic predecessor examples from both products, each with its one
//! shared spelling. A former spelling that changed is refused; it never
//! parses to a different question.
mod support;
use query_language::{ast::*, catalog::Catalog, parse::parse};

/// Every `(former, shared)` pair: the shared spelling parses; a former
/// spelling that differs is refused.
fn check(catalog: &Catalog, pairs: &[(&str, &str)]) {
    for (former, shared) in pairs {
        parse(shared, catalog).unwrap_or_else(|e| panic!("{former} => {shared}: {e}"));
        if former != shared {
            assert!(
                parse(former, catalog).is_err(),
                "the former spelling {former:?} must be refused, not reread"
            );
        }
    }
}

#[test]
fn polylogue_predecessor_corpus() {
    check(
        &support::polylogue(),
        &[
            ("", ""),
            ("   ", "   "),
            ("-tag:x", "-tag:x"),
            ("@3", "@3"),
            ("NOT tag:x", "NOT tag:x"),
            ("a (b OR c)", "a (b OR c)"),
            ("a OR b", "a OR b"),
            ("cats and dogs", "cats AND dogs"),
            (
                "connected:codex:01a20000-0005-7000-8000-000000000005",
                "connected:codex:01a20000-0005-7000-8000-000000000005",
            ),
            ("cost:unknown", "cost:unknown"),
            (
                "count(tool_call where outcome:error) >= 3",
                "count(tool_calls where outcome:error) >= 3",
            ),
            ("has:compaction", "has:compaction"),
            ("https://chatgpt.com/c/abc", "https://chatgpt.com/c/abc"),
            (
                "id in (messages where chrono | group by session)",
                "id in (messages where chrono | group by session)",
            ),
            ("kind:*", "kind:*"),
            (
                "kind:* | agg count, sum:cost",
                "kind:* | agg count, sum(cost)",
            ),
            ("link:fork", "link:fork"),
            (
                "links where kind:continuation",
                "links where kind:continuation",
            ),
            ("messages", "messages"),
            ("messages >= 10", "messages >= 10"),
            ("messages where active:false", "messages where active:false"),
            (
                "messages where written_by:human \"chrono\"",
                "messages where written_by:human \"chrono\"",
            ),
            (
                "messages where written_by:human duplicate:* \"chrono\"",
                "messages where written_by:human duplicate:* \"chrono\"",
            ),
            (
                "messages where written_by:human | group by session.origin | count",
                "messages where written_by:human | group by session.origin | count",
            ),
            ("model:claude-opus-5-5", "model:claude-opus-5-5"),
            (
                "near:\"ambiguous date formats\"",
                "near:\"ambiguous date formats\"",
            ),
            ("notes where kind:decision", "notes where kind:decision"),
            ("origin:(a|b|c)", "origin:(a|b|c)"),
            ("origin:(chatgpt|claude-ai)", "origin:(chatgpt|claude-ai)"),
            ("origin:claude-code", "origin:claude-code"),
            (
                "origin:claude-code kind:subagent",
                "origin:claude-code kind:subagent",
            ),
            (
                "origin:codex with messages(id, role)[role:assistant, last:1]",
                "origin:codex with messages(id, role)[role:assistant, last:1]",
            ),
            (
                "origin:codex | union (repo:morrow | where cost > 1) | except (repo:other)",
                "origin:codex | union (repo:morrow | where cost > 1) | except (repo:other)",
            ),
            (
                "parent:claude-code:7f3a2c10-0002-4000-8000-000000000002",
                "parent:claude-code:7f3a2c10-0002-4000-8000-000000000002",
            ),
            ("parse(x) fails", "parse(x) fails"),
            ("query:failing", "query:failing"),
            (
                "query:failing(repo=morrow, phrase=\"two words\")",
                "query:failing(repo=morrow, phrase=\"two words\")",
            ),
            ("repo:morrow", "repo:morrow"),
            ("repo:x \"a b\" refactor*", "repo:x \"a b\" refactor*"),
            ("sessions where cost > 1", "sessions where cost > 1"),
            (
                "sessions where exists tool_call(action:shell AND outcome:error)",
                "sessions where exists tool_calls(action:shell AND outcome:error)",
            ),
            (
                "sessions where human_prompts >= 3",
                "sessions where human_prompts >= 3",
            ),
            (
                "sessions where seq(command:\"cargo test\" AND outcome:error -> tool:Edit -> command:\"cargo test\" AND outcome:ok)",
                "sessions where seq(command:\"cargo test\" AND outcome:error -> tool:Edit -> command:\"cargo test\" AND outcome:ok)",
            ),
            (
                "sessions | group by origin | count",
                "sessions | group by origin | count",
            ),
            (
                "sessions | sort by random(7)",
                "sessions | sort by random(7)",
            ),
            ("since:2026-10-05", "since:2026-10-05"),
            ("started > last week", "started > last week"),
            (
                "started between 2026-10-02 and 2026-10-03",
                "started between 2026-10-02 and 2026-10-03",
            ),
            ("tag:project:morrow", "tag:project:morrow"),
            (
                "tag:project:morrow | tool_calls where action:shell | group by tool | count",
                "tag:project:morrow | tool_calls where action:shell | group by tool | count",
            ),
            (
                "tool_calls where outcome:error | group by session | count",
                "tool_calls where outcome:error | group by session | count",
            ),
            (
                "tool_calls where seq(tool:Bash -> [next] tool:Edit -> [within:5m] tool:Bash)",
                "tool_calls where seq(tool:Bash -> [next] tool:Edit -> [within 5m] tool:Bash)",
            ),
            (
                "tool_calls where tool:Bash | agg count",
                "tool_calls where tool:Bash | agg count",
            ),
            (
                "tool_calls | group by day(at), tool | agg count, sum(duration), p90:duration, count_distinct:session | sort by count desc | where count >= 3 | limit 5 | offset 10",
                "tool_calls | group by day(at), tool | agg count, sum(duration), p90(duration), count_distinct(session) | sort by count desc | where count >= 3 | limit 5 | offset 10",
            ),
            ("tree_cost > 2.90", "tree_cost > 2.90"),
            ("weekday_offset()", "weekday_offset()"),
        ],
    );
}

#[test]
fn sinex_predecessor_corpus() {
    check(
        &support::sinex(),
        &[
            ("\"cargo test\" deploy*", "\"cargo test\" deploy*"),
            ("\"related\"", "\"related\""),
            ("\"sort\" algorithm", "\"sort\" algorithm"),
            ("commands count", "commands | count"),
            (
                "commands during (intervals where state = focused)",
                "commands where during(intervals where state = focused)",
            ),
            (
                "commands group by cwd count where count > 1 sort count desc limit 5",
                "commands | group by cwd | count | where count > 1 | sort by count desc | limit 5",
            ),
            (
                "commands group by day(when) sum(duration)",
                "commands | group by day(when) | agg sum(duration)",
            ),
            (
                "commands where exit_code != 0 and cwd ^ /src/tide sort when desc limit 20",
                "commands where exit_code != 0 AND cwd ^ /src/tide | sort by when desc | limit 20",
            ),
            (
                "commands where exit_code != 0 sort when desc limit 5",
                "commands where exit_code != 0 | sort by when desc | limit 5",
            ),
            (
                "commands where when > 2026-02-01 as of 2026-03-01",
                "commands where when > 2026-02-01 | as of 2026-03-01",
            ),
            ("events as of seq 42", "events | as of seq 42"),
            (
                "events near (commands) within 5m after (commands) within 1m fields near_2, offset_2",
                "events where near(commands) within 5m after(commands) within 1m",
            ),
            (
                "events near (commands) within 5m during (commands) fields near, during_2",
                "events where near(commands) within 5m during(commands)",
            ),
            (
                "events near (commands) within 5m fields when, near, offset sort offset",
                "events | join near (commands) within 5m | fields when, offset | sort by offset",
            ),
            ("events since 7d", "events where since:7d"),
            (
                "events where event_type = [command.executed, command.imported] and not material exists",
                "events where event_type:(command.executed|command.imported) AND NOT material:known",
            ),
            (
                "events where hour(when) >= 1 and weekday(when) = Saturday and seen - when > 1d",
                "events where hour(when) >= 1 AND weekday(when) = Saturday AND seen - when > 1d",
            ),
            (
                "events where payload.exit_code > 0 and payload.url ~ github",
                "events where payload.exit_code > 0 AND payload.url ~ github",
            ),
            (
                "events where source != terminal.* near (commands where command ~ \"cargo test\") within 5m",
                "events where source != terminal.* near (commands where command ~ \"cargo test\") within 5m",
            ),
            (
                "events where source = a or source = b and event_type = c",
                "events where source = a OR source = b AND event_type = c",
            ),
            (
                "events where source:terminal.*",
                "events where source:terminal.*",
            ),
            (
                "events where when > 2026-09-01 and when < 2026-09-10",
                "events where when > 2026-09-01 AND when < 2026-09-10",
            ),
            (
                "events where when >= 2026-09-14 14:05",
                "events where when >= 2026-09-14 14:05",
            ),
            (
                "events where when in 2026-08",
                "events where when in 2026-08",
            ),
            (
                "events where when in last 7d",
                "events where when in last 7d",
            ),
            (
                "events where when in yesterday",
                "events where when in yesterday",
            ),
            (
                "shell-sessions where cwds ^ /src/tide sort start desc",
                "shell-sessions where cwds ^ /src/tide | sort by start desc",
            ),
            (
                "shell-sessions where cwds != /src/tide",
                "shell-sessions where cwds != /src/tide",
            ),
            (
                "events where payload.exit_code >= 0 and event_type = command.executed sum(payload.duration_ns)",
                "events where payload.exit_code >= 0 AND event_type = command.executed | agg sum(payload.duration_ns)",
            ),
            (
                "commands where when in last 3d group by day(when), cwd count fill where count >= 1 sort day",
                "commands where when in last 3d | group by day(when), cwd | count | fill | where count >= 1 | sort by day",
            ),
            (
                r#"query:by-directory(root="x\" or source exists", code=1) count"#,
                r#"query:by-directory(root="x\" or source exists", code=1) | count"#,
            ),
            ("tide", "tide"),
        ],
    );
}

/// Sinex's `recall`, `trace`, `descendants` and `parents` are commands that
/// assemble queries; in the language their words are ordinary search terms.
#[test]
fn former_command_sources_are_search_words() {
    let c = support::sinex();
    for text in [
        "recall",
        "recall yesterday",
        "recall last",
        "trace sinex:ev/0190",
        "related sinex:ev/0190",
        "comands since:7d",
        "sort algorithm",
    ] {
        let query = parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
        let Source::Unit {
            implicit: true,
            condition: Some(condition),
            ..
        } = query.source
        else {
            panic!("{text} is a condition on the default unit")
        };
        let first = match &condition.kind {
            ConditionKind::And { conditions } => &conditions[0],
            _ => &condition,
        };
        assert!(
            matches!(first.kind, ConditionKind::Text { .. }),
            "{text}: {first:?}"
        );
    }
}

#[test]
fn refusals_from_both_predecessors() {
    for (catalog, text, reason) in [
        (
            support::polylogue(),
            "count words",
            "a declared field name is never treated as a text term",
        ),
        (
            support::sinex(),
            "archive where reason = unknown_reason",
            "a value outside a closed set",
        ),
        (
            support::sinex(),
            "commands group by cwd fill",
            "transforms follow |",
        ),
        (
            support::sinex(),
            "commands | group by cwd | where exit_code = 0",
            "a grouped relation shows only its columns",
        ),
        (
            support::sinex(),
            "commands where exit_cod = 0",
            "an unknown field",
        ),
        (
            support::sinex(),
            "events where near(commands)",
            "near needs a distance",
        ),
        (
            support::sinex(),
            "events where when > 7d",
            "a bare duration is not an instant",
        ),
        (
            support::sinex(),
            "materials where payload.x = 1",
            "an undeclared payload field",
        ),
        (
            support::sinex(),
            "runs where status = hide",
            "a value of another unit",
        ),
        (
            support::polylogue(),
            "kind:* where exists tool_calls(outcome:error)",
            "where inside conditions",
        ),
        (
            support::polylogue(),
            "kind:* NOT where",
            "where inside conditions",
        ),
        (support::polylogue(), "frobs where x:1", "an unknown unit"),
        (
            support::polylogue(),
            "sessions where exists frob(x:1)",
            "an unknown unit",
        ),
        (support::polylogue(), "repo:(a|", "an unclosed set"),
        (
            support::polylogue(),
            "\"unclosed phrase",
            "an unclosed quote",
        ),
        (support::polylogue(), "messages where", "an empty filter"),
        (support::polylogue(), "reop:morrow", "an unknown field"),
        (
            support::polylogue(),
            "messages:>=10",
            "a compact comparison",
        ),
    ] {
        assert!(parse(text, &catalog).is_err(), "{text}: {reason}");
    }
}

#[test]
fn a_bare_field_refuses_with_the_quoted_search() {
    let c = support::sinex();
    let error = parse("tide when", &c).unwrap_err();
    assert!(error.message.contains("field"));
    assert_eq!(error.suggestion.as_deref(), Some("tide \"when\""));
    parse(error.suggestion.as_deref().unwrap(), &c).unwrap();
}
