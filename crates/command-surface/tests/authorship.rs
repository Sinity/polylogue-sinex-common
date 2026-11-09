use command_surface::*;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

fn harness(name: &str) -> Harness {
    Harness::new(name).unwrap()
}
fn environment(values: &[(&str, &str)]) -> BTreeMap<String, OsString> {
    values
        .iter()
        .map(|(key, value)| (key.to_string(), OsString::from(value)))
        .collect()
}

#[test]
fn id_one() {
    checks::id_one(author_of).unwrap();
}

#[test]
fn id_one_detects_a_product_with_rules_of_its_own() {
    // A product that let a terminal make every CLI caller the operator would
    // differ from the shared rule on a harness caller.
    let own_rule = |evidence: &CallerEvidence, via: Via, hook: Option<&HookIdentity>| {
        if via == Via::Cli && evidence.controlling_terminal {
            Authorship {
                author: Author::Operator,
                via,
            }
        } else {
            author_of(evidence, via, hook)
        }
    };
    assert!(checks::id_one(own_rule).is_err());
}

#[test]
fn id_shared() {
    let server = CallerEvidence {
        harness: Some(harness("codex")),
        session: Some(AgentSession::new(harness("codex"), "first-thread").unwrap()),
        harness_pid: Some(7),
        shared_server: true,
        ..CallerEvidence::default()
    };
    let unidentified = author_of(&server, Via::Mcp, None);
    assert_eq!(unidentified.author.to_string(), "agent:codex");
    let identified = CallerEvidence {
        call_session: Some(AgentSession::new(harness("codex"), "second-thread").unwrap()),
        ..server.clone()
    };
    assert_eq!(
        author_of(&identified, Via::Mcp, None).author.to_string(),
        "agent:codex:second-thread"
    );
    // The same server's start-up session still names a CLI caller started
    // from it: only the MCP server is shared.
    assert_eq!(
        author_of(&server, Via::Cli, None).author.to_string(),
        "agent:codex:first-thread"
    );
}

#[test]
fn author_text_forms_round_trip() {
    for text in [
        "operator",
        "agent",
        "agent:gemini-cli",
        "agent:codex:thread",
        "agent:claude-code:agent-worker",
        "script:worker.service",
        "unattributed",
    ] {
        let author = Author::parse(text).unwrap();
        assert_eq!(author.to_string(), text);
        let json = serde_json::to_string(&author).unwrap();
        assert_eq!(serde_json::from_str::<Author>(&json).unwrap(), author);
    }
    for malformed in [
        "",
        "agent:",
        "script:",
        "person:synthetic",
        "operator:extra",
        "agent:Codex",
    ] {
        assert!(Author::parse(malformed).is_none(), "{malformed}");
        assert!(serde_json::from_value::<Author>(malformed.into()).is_err());
    }
}

#[test]
fn nearest_harness_ancestor_selects_the_identity_that_launched_the_caller() {
    let env = environment(&[
        ("CLAUDE_CODE_SESSION_ID", "outer"),
        ("CODEX_THREAD_ID", "inner"),
        ("CODEX_TOOL_CALL_ID", "call-one"),
    ]);
    let ancestors = [(17, harness("codex")), (12, harness("claude-code"))];
    let caller = select(&env, &ancestors, true, Some("shell.scope".into()));
    assert_eq!(caller.session.unwrap().to_string(), "codex:inner");
    assert_eq!(caller.harness_pid, Some(17));
    assert_eq!(caller.tool_call_id.as_deref(), Some("call-one"));
    assert!(!caller.ambiguous_harness);
}

#[test]
fn known_harness_without_session_remains_an_agent() {
    let caller = select(&BTreeMap::new(), &[(17, harness("gemini-cli"))], true, None);
    assert_eq!(
        author_of(&caller, Via::Cli, None).author.to_string(),
        "agent:gemini-cli"
    );
}

#[test]
fn variables_of_one_harness_name_it_without_an_ancestor() {
    let caller = select(
        &environment(&[("CLAUDE_CODE_SESSION_ID", "one"), ("CLAUDE_PID", "99")]),
        &[],
        false,
        None,
    );
    assert_eq!(caller.session.unwrap().to_string(), "claude-code:one");
    assert_eq!(caller.harness_pid, Some(99));
    let gemini = select(&environment(&[("GEMINI_CLI", "0")]), &[], false, None);
    assert_eq!(gemini.harness, None);
}

#[test]
fn conflicting_variables_without_an_ancestor_stay_unattributed() {
    let caller = select(
        &environment(&[
            ("CLAUDE_CODE_SESSION_ID", "one"),
            ("CODEX_THREAD_ID", "two"),
        ]),
        &[],
        true,
        Some("shell.scope".into()),
    );
    assert!(caller.controlling_terminal);
    assert!(caller.ambiguous_harness);
    assert_eq!(
        author_of(&caller, Via::Cli, None).author,
        Author::Unattributed
    );
}

#[test]
fn process_files_parse_without_losing_native_identity() {
    assert_eq!(
        parent_pid("17 (a command (with) parentheses) S 12 1 2 3"),
        Some(12)
    );
    assert_eq!(
        cgroup_unit("0::/user.slice/user-1000.slice/user@1000.service/app.slice/task.scope\n")
            .as_deref(),
        Some("task.scope")
    );
    assert_eq!(
        harness_of(
            Some(Path::new("/tools/node")),
            Some(b"node\0/tools/@anthropic-ai/claude-code/cli.js\0")
        )
        .unwrap()
        .as_str(),
        "claude-code"
    );
    assert_eq!(
        harness_of(
            Some(Path::new("/usr/bin/python3")),
            Some(b"python3\0/opt/bin/hermes\0")
        )
        .unwrap()
        .as_str(),
        "hermes"
    );
    assert_eq!(harness_of(Some(Path::new("/usr/bin/bash")), None), None);
}

#[test]
fn gathering_the_current_process_keeps_only_harness_variables() {
    let evidence = gather_current(|name| {
        (name == "CODEX_THREAD_ID").then(|| OsString::from("synthetic-thread"))
    });
    // The process tree of the test run decides the harness; when none of its
    // ancestors is a harness, the one variable given names codex.
    if evidence.harness_pid.is_none() {
        assert_eq!(
            evidence.session.map(|s| s.to_string()).as_deref(),
            Some("codex:synthetic-thread")
        );
    }
    assert!(!evidence.shared_server);
    assert_eq!(evidence.call_session, None);
}

#[test]
fn caller_evidence_round_trips_and_refuses_unknown_fields() {
    let evidence = CallerEvidence {
        harness: Some(harness("codex")),
        session: Some(AgentSession::parse("codex:thread").unwrap()),
        shared_server: true,
        ..CallerEvidence::default()
    };
    let value = serde_json::to_value(&evidence).unwrap();
    assert_eq!(
        serde_json::from_value::<CallerEvidence>(value.clone()).unwrap(),
        evidence
    );
    let mut forged = value;
    forged["author"] = "operator".into();
    assert!(serde_json::from_value::<CallerEvidence>(forged).is_err());
}

/// A process table for the walk up the tree: pid, executable, the harness
/// variables of its environment, and its parent.
fn table(rows: &[(u32, &str, &[(&str, &str)], u32)]) -> impl Fn(u32) -> Option<Process> + use<> {
    let rows: BTreeMap<u32, Process> = rows
        .iter()
        .map(|(pid, executable, variables, parent)| {
            (
                *pid,
                Process {
                    executable: Some(executable.into()),
                    arguments: None,
                    environment: Some(environment(variables)),
                    parent: Some(*parent),
                },
            )
        })
        .collect();
    move |pid| rows.get(&pid).cloned()
}

#[test]
fn claude_exe_is_claude_code() {
    assert_eq!(
        harness_of(Some(Path::new("/opt/claude/bin/claude.exe")), None)
            .unwrap()
            .as_str(),
        "claude-code"
    );
}

#[test]
fn mcp_child_without_claude_pid_gets_harness_pid_from_ancestor() {
    // The MCP server's parent is the harness under a name no list knows; it
    // set CLAUDECODE in its child but has none of its own.
    let caller = environment(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "s1")]);
    let processes = table(&[
        (40, "/opt/harness/bin/agent", &[], 30),
        (30, "/usr/bin/zsh", &[], 1),
    ]);
    let ancestors = harness_chain(40, &caller, processes);
    assert_eq!(ancestors, [(40, harness("claude-code"))]);
    let evidence = select(&caller, &ancestors, false, None);
    assert_eq!(evidence.session.unwrap().to_string(), "claude-code:s1");
    assert_eq!(evidence.harness_pid, Some(40));
}

#[test]
fn nearest_harness_wins_when_claude_runs_inside_codex() {
    // codex (10) -> shell (11) -> claude (20) -> shell (21) -> caller.
    let codex = [("CODEX_THREAD_ID", "t1")];
    let both = [
        ("CODEX_THREAD_ID", "t1"),
        ("CLAUDECODE", "1"),
        ("CLAUDE_CODE_SESSION_ID", "s1"),
    ];
    let processes = table(&[
        (21, "/usr/bin/bash", &both, 20),
        (20, "/opt/agent/bin/agent", &codex, 11),
        (11, "/usr/bin/bash", &codex, 10),
        (10, "/opt/other/bin/agent", &[], 1),
    ]);
    let caller = environment(&both);
    let ancestors = harness_chain(21, &caller, processes);
    assert_eq!(
        ancestors,
        [(20, harness("claude-code")), (10, harness("codex"))]
    );
    let evidence = select(&caller, &ancestors, false, None);
    assert_eq!(evidence.session.unwrap().to_string(), "claude-code:s1");
    assert_eq!(evidence.harness_pid, Some(20));
    assert!(!evidence.ambiguous_harness);
}

#[test]
fn an_unreadable_environment_names_no_harness_by_itself() {
    let caller = environment(&[("CLAUDECODE", "1")]);
    let processes = |pid: u32| {
        (pid == 50).then(|| Process {
            executable: Some("/usr/bin/sudo".into()),
            arguments: None,
            environment: None,
            parent: Some(1),
        })
    };
    assert_eq!(harness_chain(50, &caller, processes), []);
}
