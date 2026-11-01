//! Laws a product checks against its own operation table, in its own tests.
use crate::{
    AgentSession, Author, Authorship, CallerEvidence, Harness, HookIdentity, OperationKind,
    OperationTable, Via,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// `tools-generated`: `tools/list` is the grouping of the table. Every
/// request row is reachable from exactly one tool, the one it names; no
/// stream or local row is a tool; each tool's hints hold for every row it
/// holds; and every declared tool holds a row.
pub fn tools_generated(table: &OperationTable) -> Result<(), String> {
    let tools = table.mcp_tools();
    let mut reached: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for tool in &tools {
        let name = tool["name"]
            .as_str()
            .ok_or("a tool has no name")?
            .to_owned();
        let input = &tool["inputSchema"];
        let rows: Vec<String> = match input["properties"]["operation"]["enum"].as_array() {
            Some(rows) => rows
                .iter()
                .map(|row| {
                    row.as_str()
                        .map(str::to_owned)
                        .ok_or("a row name is not text")
                })
                .collect::<Result<_, _>>()?,
            None => vec![
                input["properties"]["arguments"]["title"]
                    .as_str()
                    .ok_or_else(|| format!("{name} names no row"))?
                    .to_owned(),
            ],
        };
        if rows.is_empty() {
            return Err(format!("{name} holds no row"));
        }
        for row in rows {
            let spec = table
                .spec_named(&row)
                .ok_or_else(|| format!("{name} offers {row}, which the table does not have"))?;
            let OperationKind::Request { tool: own } = &spec.kind else {
                return Err(format!("{name} offers {row}, which is not a request row"));
            };
            if name != format!("{}_{own}", table.product()) {
                return Err(format!("{name} offers {row}, which belongs to {own}"));
            }
            let read_only = tool["annotations"]["readOnlyHint"] == Value::Bool(true);
            let destructive = tool["annotations"]["destructiveHint"] == Value::Bool(true);
            if read_only != (spec.access == crate::Access::Read)
                || destructive != (spec.access == crate::Access::Permanent)
            {
                return Err(format!("{name}'s hints do not hold for {row}"));
            }
            reached.entry(row).or_default().push(name.clone());
        }
    }
    for description in table.descriptions() {
        let spec = &description.spec;
        let tools = reached.get(&spec.name).map_or(0, Vec::len);
        match (&spec.kind, tools) {
            (OperationKind::Request { .. }, 1)
            | (OperationKind::Stream | OperationKind::Local, 0) => {}
            (OperationKind::Request { .. }, count) => {
                return Err(format!("{} is reachable from {count} tools", spec.name));
            }
            (_, _) => {
                return Err(format!(
                    "{} is not a request row, yet a tool offers it",
                    spec.name
                ));
            }
        }
    }
    for declared in table.tools() {
        if !table
            .descriptions()
            .iter()
            .any(|d| d.spec.tool() == Some(declared.name.as_str()))
        {
            return Err(format!("tool {} holds no row", declared.name));
        }
    }
    Ok(())
}

/// One caller fixture of `id-one`: what the transport observed, how the
/// request came, the hook identity the product's spool holds, and the
/// author both products must compute.
#[derive(Clone, Debug)]
pub struct AuthorCase {
    pub name: &'static str,
    pub evidence: CallerEvidence,
    pub via: Via,
    pub hook: Option<HookIdentity>,
    pub author: Author,
}

fn harness(name: &str) -> Harness {
    Harness::new(name).expect("fixture harness names are valid")
}
fn session(harness_name: &str, native: &str) -> AgentSession {
    AgentSession::new(harness(harness_name), native).expect("fixture sessions are valid")
}

/// The shared caller fixtures, synthetic, covering every rule of `author_of`.
pub fn author_cases() -> Vec<AuthorCase> {
    let blank = CallerEvidence::default();
    let terminal = CallerEvidence {
        controlling_terminal: true,
        ..blank.clone()
    };
    let in_harness = CallerEvidence {
        harness: Some(harness("claude-code")),
        session: Some(session("claude-code", "main")),
        harness_pid: Some(42),
        tool_call_id: Some("call-1".into()),
        controlling_terminal: true,
        ..blank.clone()
    };
    let hook = HookIdentity {
        harness_pid: 42,
        tool_call_id: Some("call-1".into()),
        session: session("claude-code", "resumed"),
        agent_id: Some("worker".into()),
    };
    let shared = CallerEvidence {
        harness: Some(harness("codex")),
        session: Some(session("codex", "server-start")),
        shared_server: true,
        ..blank.clone()
    };
    let case =
        |name, evidence: &CallerEvidence, via, hook: Option<&HookIdentity>, author| AuthorCase {
            name,
            evidence: evidence.clone(),
            via,
            hook: hook.cloned(),
            author,
        };
    vec![
        case(
            "plain cli is unattributed",
            &blank,
            Via::Cli,
            None,
            Author::Unattributed,
        ),
        case(
            "terminal without a harness is the operator",
            &terminal,
            Via::Cli,
            None,
            Author::Operator,
        ),
        case(
            "mcp without identity is an unidentified agent",
            &blank,
            Via::Mcp,
            None,
            Author::UnidentifiedAgent,
        ),
        case(
            "mcp from a terminal is still an agent",
            &terminal,
            Via::Mcp,
            None,
            Author::UnidentifiedAgent,
        ),
        case(
            "a cgroup unit names a script",
            &CallerEvidence {
                unit: Some("worker.service".into()),
                ..blank.clone()
            },
            Via::Cli,
            None,
            Author::Script("worker.service".into()),
        ),
        case(
            "a harness without a session is that harness",
            &CallerEvidence {
                harness: Some(harness("gemini-cli")),
                controlling_terminal: true,
                ..blank.clone()
            },
            Via::Cli,
            None,
            Author::AgentOfHarness(harness("gemini-cli")),
        ),
        case(
            "the web is the operator",
            &in_harness,
            Via::Web,
            None,
            Author::Operator,
        ),
        case(
            "the extension is the operator",
            &in_harness,
            Via::Extension,
            None,
            Author::Operator,
        ),
        case(
            "the desktop is the operator",
            &in_harness,
            Via::Desktop,
            None,
            Author::Operator,
        ),
        case(
            "a harness session is that agent",
            &in_harness,
            Via::Cli,
            None,
            Author::Agent(session("claude-code", "main")),
        ),
        case(
            "a matching hook names its subagent",
            &in_harness,
            Via::Mcp,
            Some(&hook),
            Author::Agent(session("claude-code", "agent-worker")),
        ),
        case(
            "a hook for another tool call is ignored",
            &in_harness,
            Via::Mcp,
            Some(&HookIdentity {
                tool_call_id: Some("other".into()),
                ..hook.clone()
            }),
            Author::Agent(session("claude-code", "main")),
        ),
        case(
            "a session-wide hook names its session",
            &in_harness,
            Via::Mcp,
            Some(&HookIdentity {
                tool_call_id: None,
                agent_id: None,
                ..hook.clone()
            }),
            Author::Agent(session("claude-code", "resumed")),
        ),
        case(
            "a hook of another harness process is ignored",
            &in_harness,
            Via::Mcp,
            Some(&HookIdentity {
                harness_pid: 43,
                ..hook.clone()
            }),
            Author::Agent(session("claude-code", "main")),
        ),
        case(
            "several harnesses and no ancestor are unattributed",
            &CallerEvidence {
                ambiguous_harness: true,
                controlling_terminal: true,
                unit: Some("shell.scope".into()),
                ..blank.clone()
            },
            Via::Cli,
            None,
            Author::Unattributed,
        ),
        case(
            "a shared server never lends its start-up session",
            &shared,
            Via::Mcp,
            None,
            Author::AgentOfHarness(harness("codex")),
        ),
        case(
            "a shared server's call names its own session",
            &CallerEvidence {
                call_session: Some(session("codex", "thread-7")),
                ..shared.clone()
            },
            Via::Mcp,
            None,
            Author::Agent(session("codex", "thread-7")),
        ),
        case(
            "a dedicated server's call identity wins over its start-up session",
            &CallerEvidence {
                call_session: Some(session("claude-code", "other")),
                ..in_harness.clone()
            },
            Via::Mcp,
            None,
            Author::Agent(session("claude-code", "other")),
        ),
    ]
}

/// `id-one`: `compute` gives every shared caller fixture its author. Each
/// product runs this with the function its transports call, which is
/// `author_of` itself once it has adopted it.
pub fn id_one(
    compute: impl Fn(&CallerEvidence, Via, Option<&HookIdentity>) -> Authorship,
) -> Result<(), String> {
    for case in author_cases() {
        let computed = compute(&case.evidence, case.via, case.hook.as_ref());
        if computed.author != case.author || computed.via != case.via {
            return Err(format!(
                "{}: computed {} via {:?}, expected {}",
                case.name, computed.author, computed.via, case.author
            ));
        }
    }
    Ok(())
}
