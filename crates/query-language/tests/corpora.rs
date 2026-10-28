//! Preserved synthetic predecessor parser examples. Intentional spelling/binding
//! changes are recorded alongside their exact former inputs.
mod support;
use query_language::parse::parse;
#[test]
fn polylogue_predecessor_positive_corpus() {
    let c = support::polylogue();
    for text in [
        "",
        "   ",
        "-tag:x",
        "@3.2",
        "NOT tag:x",
        "a (b OR c)",
        "a OR b",
        "cats and dogs",
        "connected:codex:01a20000-0005-7000-8000-000000000005",
        "cost:unknown",
        "count(tool_call where outcome:error) >= 3",
        "has:compaction",
        "https://chatgpt.com/c/abc",
        "id in (messages where chrono | group by session)",
        "kind:*",
        "kind:* | agg count, sum:cost",
        "link:fork",
        "links where kind:continuation",
        "messages",
        "messages >= 10",
        "messages where active:false",
        "messages where written_by:human \"chrono\"",
        "messages where written_by:human duplicate:* \"chrono\"",
        "messages where written_by:human | group by session.origin | count",
        "model:claude-opus-5-5",
        "near:\"ambiguous date formats\"",
        "notes where kind:decision",
        "origin:(a|b|c)",
        "origin:(chatgpt|claude-ai)",
        "origin:claude-code",
        "origin:claude-code kind:subagent",
        "origin:codex with messages(id, role)[role:assistant, last:1]",
        "origin:codex | union (messages)",
        "origin:codex | union (repo:morrow | where cost > 1) | except (messages where chrono)",
        "parent:claude-code:7f3a2c10-0002-4000-8000-000000000002",
        "parse(x) fails",
        "query:failing",
        "query:failing(repo=morrow, phrase=\"two words\")",
        "repo:morrow",
        "repo:other",
        "repo:x \"a b\" refactor*",
        "sessions where cost > 1",
        "sessions where exists tool_call(action:shell AND outcome:error)",
        "sessions where human_prompts >= 3",
        "sessions where messages >= 10",
        "sessions where seq(command:\"cargo test\" AND outcome:error -> tool:Edit -> command:\"cargo test\" AND outcome:ok)",
        "sessions where tool_calls > 5",
        "sessions | group by origin | count",
        "sessions | sort by random(7)",
        "since:2026-10-05",
        "started > last week",
        "started between 2026-10-02 and 2026-10-03",
        "tag:project:morrow",
        "tag:project:morrow | tool_calls where action:shell | group by tool | count",
        "tool_calls where outcome:error",
        "tool_calls where outcome:error | group by session | count",
        "tool_calls where seq(tool:Bash -> [next] tool:Edit -> [within:5m] tool:Bash)",
        "tool_calls where session.tag:project:morrow | agg count",
        "tool_calls where tool:Bash | agg count",
        "tool_calls | group by action | count",
        "tool_calls | group by day(at), tool | agg count, sum:duration, p90:duration, count_distinct:session | sort by count desc | where count >= 3 | limit 5 | offset 10",
        "tree_cost > 2.90",
        "weekday_offset()",
    ] {
        parse(text, &c).unwrap_or_else(|e| panic!("{text}: {e}"));
    }
}
#[test]
fn sinex_predecessor_positive_corpus() {
    let c = support::sinex();
    for (original, shared) in [
        ("\"cargo test\" deploy*", "\"cargo test\" deploy*"),
        ("\"related\"", "\"related\""),
        ("\"sort\" algorithm", "\"sort\" algorithm"),
        ("commands count", "commands count"),
        (
            "commands during (intervals where state = focused)",
            "commands during (intervals where state = focused)",
        ),
        (
            "commands group by cwd count where count > 1 sort count desc limit 5",
            "commands group by cwd count where count > 1 sort count desc limit 5",
        ),
        (
            "commands group by day(when) sum(duration)",
            "commands group by day(when) sum(duration)",
        ),
        (
            "commands where exit_code != 0 and cwd ^ /src/tide sort when desc limit 20",
            "commands where exit_code != 0 AND cwd ^ /src/tide sort when desc limit 20",
        ),
        (
            "commands where exit_code != 0 sort when desc limit 5",
            "commands where exit_code != 0 sort when desc limit 5",
        ),
        (
            "commands where when > 2026-02-01 as of 2026-03-01",
            "commands where when > 2026-02-01 as of 2026-03-01",
        ),
        ("events as of seq 42", "events as of seq 42"),
        (
            "events near (commands) within 5m after (commands) within 1m fields near_2, offset_2",
            "events near (commands) within 5m after (commands) within 1m fields near_2, offset_2",
        ),
        (
            "events near (commands) within 5m during (commands) fields near, during_2",
            "events near (commands) within 5m during (commands) fields near, during_2",
        ),
        (
            "events near (commands) within 5m fields when, near, offset sort offset",
            "events near (commands) within 5m fields when, near, offset sort offset",
        ),
        ("events since 7d", "events since 7d"),
        (
            "events where event_type = [command.executed, command.imported] and not material exists",
            "events where event_type = [command.executed, command.imported] AND NOT material exists",
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
            "events where when >= 2026-09-14",
            "events where when >= 2026-09-14",
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
        ("recall", "recall"),
        (
            "recall 2026-03-02 14:05 ±20m",
            "recall 2026-03-02 14:05 ±20m",
        ),
        ("recall last", "recall last"),
        ("recall mark", "recall mark"),
        ("recall yesterday", "recall yesterday"),
        ("tide", "tide"),
        ("tide offset", "tide offset"),
    ] {
        if original != shared {
            let refusal = parse(original, &c).expect_err(original);
            let suggestion = refusal
                .suggestion
                .expect("shared spelling supplies a runnable correction");
            parse(&suggestion, &c)
                .unwrap_or_else(|e| panic!("{original} correction {suggestion}: {e}"));
        }
        parse(shared, &c).unwrap_or_else(|e| panic!("{original} => {shared}: {e}"));
    }
}
#[test]
fn predecessor_refusals_and_documented_changes() {
    for (catalog, text, reason) in [
        (
            support::polylogue(),
            "count words",
            "a declared field name is never treated as a text term",
        ),
        (
            support::sinex(),
            "archive where reason = unknown_reason",
            "predecessor refusal",
        ),
        (support::sinex(), "comands since 7d", "predecessor refusal"),
        (
            support::sinex(),
            "commands group by cwd fill",
            "predecessor refusal",
        ),
        (
            support::sinex(),
            "commands group by cwd where exit_code = 0",
            "predecessor refusal",
        ),
        (
            support::sinex(),
            "commands where exit_cod = 0",
            "predecessor refusal",
        ),
        (
            support::sinex(),
            "events near (commands) ",
            "predecessor refusal",
        ),
        (
            support::sinex(),
            "events where when > 7d",
            "predecessor refusal",
        ),
        (
            support::sinex(),
            "materials where payload.x = 1",
            "predecessor refusal",
        ),
        (
            support::sinex(),
            "related sinex:ev/0190",
            "predecessor refusal",
        ),
        (
            support::sinex(),
            "runs where status = hide",
            "predecessor refusal",
        ),
        (support::sinex(), "sort algorithm", "predecessor refusal"),
        (
            support::polylogue(),
            "kind:* where exists tool_call(outcome:error)",
            "predecessor refusal",
        ),
        (
            support::polylogue(),
            "kind:* NOT where",
            "predecessor refusal",
        ),
        (
            support::polylogue(),
            "frobs where x:1",
            "predecessor refusal",
        ),
        (
            support::polylogue(),
            "sessions where exists frob(x:1)",
            "predecessor refusal",
        ),
        (support::polylogue(), "repo:(a|", "predecessor refusal"),
        (
            support::polylogue(),
            "\"unclosed phrase",
            "predecessor refusal",
        ),
        (
            support::polylogue(),
            "messages where",
            "predecessor refusal",
        ),
        (
            support::polylogue(),
            "reop:morrow",
            "field typo is refused by the catalog",
        ),
        (
            support::polylogue(),
            "messages:>=10",
            "compact comparison has one corrected spelling",
        ),
    ] {
        assert!(parse(text, &catalog).is_err(), "{text}: {reason}");
    }
}
