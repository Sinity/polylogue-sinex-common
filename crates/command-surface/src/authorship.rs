//! Who is calling, and who a write is attributed to.
//!
//! The caller's facts are gathered at the transport boundary (the harness
//! variables of the calling process, its nearest harness ancestor, its
//! controlling terminal, its cgroup unit, and an MCP call's own identity),
//! and `author_of` turns them into an author the same way in both products.
//! No request field names an author.
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{borrow::Cow, fmt};

/// Stored and sent as its text form, parsed back with `parse`.
macro_rules! stored_as_text {
    ($type:ident, $description:literal) => {
        impl Serialize for $type {
            fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }
        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Self::parse(&text).ok_or_else(|| {
                    serde::de::Error::custom(format!("expected {}, got {text:?}", $description))
                })
            }
        }
        impl JsonSchema for $type {
            fn schema_name() -> Cow<'static, str> {
                stringify!($type).into()
            }
            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                schemars::json_schema!({"type": "string", "description": $description})
            }
        }
    };
}

mod gather;
pub use gather::*;

/// An agent harness by its stored name (`claude-code`, `codex`, `hermes`,
/// `gemini-cli`): lowercase letters, digits and `-`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, JsonSchema)]
#[serde(transparent)]
pub struct Harness(String);
impl Harness {
    pub fn new(name: impl Into<String>) -> Option<Self> {
        let name = name.into();
        (!name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
        .then_some(Self(name))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for Harness {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::new(name).ok_or_else(|| serde::de::Error::custom("not a harness name"))
    }
}
impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One agent session: its harness and the harness's own ID for it,
/// written `harness:native`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AgentSession {
    pub harness: Harness,
    pub native: String,
}
impl AgentSession {
    pub fn new(harness: Harness, native: impl Into<String>) -> Option<Self> {
        let native = native.into();
        (!native.is_empty()).then_some(Self { harness, native })
    }
    /// Parse `harness:native`.
    pub fn parse(text: &str) -> Option<Self> {
        let (harness, native) = text.split_once(':')?;
        Self::new(Harness::new(harness)?, native)
    }
}
impl fmt::Display for AgentSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.harness, self.native)
    }
}
stored_as_text!(AgentSession, "an agent session, `harness:native`");

/// The author an authored record keeps.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Author {
    Operator,
    /// An agent, by the session it wrote from.
    Agent(AgentSession),
    /// An agent whose harness is known and whose session is not.
    AgentOfHarness(Harness),
    /// An agent whose harness is not known either (an MCP call with no
    /// identity at all).
    UnidentifiedAgent,
    /// A program acting for the operator, by its cgroup unit.
    Script(String),
    /// Nothing identifies a person, an agent or a named program.
    Unattributed,
}
impl Author {
    pub fn is_agent(&self) -> bool {
        matches!(
            self,
            Self::Agent(_) | Self::AgentOfHarness(_) | Self::UnidentifiedAgent
        )
    }
    /// Parse a stored author: `operator`, `agent:<harness>:<native>`,
    /// `agent:<harness>`, `agent`, `script:<unit>`, `unattributed`.
    pub fn parse(text: &str) -> Option<Self> {
        match text.split_once(':') {
            None => match text {
                "operator" => Some(Self::Operator),
                "agent" => Some(Self::UnidentifiedAgent),
                "unattributed" => Some(Self::Unattributed),
                _ => None,
            },
            Some(("agent", rest)) if rest.contains(':') => {
                AgentSession::parse(rest).map(Self::Agent)
            }
            Some(("agent", harness)) => Harness::new(harness).map(Self::AgentOfHarness),
            Some(("script", unit)) if !unit.is_empty() => Some(Self::Script(unit.into())),
            _ => None,
        }
    }
}
impl fmt::Display for Author {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operator => f.write_str("operator"),
            Self::Agent(session) => write!(f, "agent:{session}"),
            Self::AgentOfHarness(harness) => write!(f, "agent:{harness}"),
            Self::UnidentifiedAgent => f.write_str("agent"),
            Self::Script(unit) => write!(f, "script:{unit}"),
            Self::Unattributed => f.write_str("unattributed"),
        }
    }
}
stored_as_text!(
    Author,
    "`operator`, `agent:<harness>:<native>`, `agent:<harness>`, `agent`, `script:<unit>` or `unattributed`"
);

/// How a request reached the product.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Via {
    Cli,
    Mcp,
    Web,
    Extension,
    Desktop,
    Hook,
    Markup,
    Import,
    Daemon,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Authorship {
    pub author: Author,
    pub via: Via,
}

/// What the transport observed about the caller. Gathered by `gather`, or
/// set by a product's transport for what only it sees (an MCP call's own
/// identity, whether its MCP server serves several sessions).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CallerEvidence {
    /// The harness the caller runs under: its nearest harness ancestor, or
    /// the one harness its variables name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<Harness>,
    /// The session the harness's variables name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<AgentSession>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_pid: Option<u32>,
    #[serde(default)]
    pub controlling_terminal: bool,
    /// The calling process's cgroup unit (`backup.service`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// Variables of several harnesses and no harness ancestor to choose one.
    #[serde(default)]
    pub ambiguous_harness: bool,
    /// The MCP server process serves several sessions, so its start-up
    /// variables name none of them.
    #[serde(default)]
    pub shared_server: bool,
    /// The session an MCP call names for itself, in its own metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call_session: Option<AgentSession>,
}

/// One identity read from a harness's hook spool: the session (and subagent)
/// a harness process or one of its tool calls belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HookIdentity {
    pub harness_pid: u32,
    pub tool_call_id: Option<String>,
    pub session: AgentSession,
    pub agent_id: Option<String>,
}

/// The author of a request, from what the transport observed, how the
/// request came, and the matching hook identity if the product's spool has
/// one. Both products call this; neither has rules of its own.
///
/// - The operator's own surfaces (web, extension, desktop) are the operator.
/// - A hook identity refines the caller only when it names the caller's
///   harness process and, if it is per call, the caller's tool call; a
///   subagent's hook identity names the subagent.
/// - An MCP call that names its own session is that session. A shared MCP
///   server never lends its start-up session to a call: without the call's
///   own identity the author is the harness (`agent:<harness>`).
/// - A CLI caller with several harnesses' variables and no harness ancestor
///   is unattributed: it is neither the operator nor any one of them.
/// - Otherwise the nearest harness wins; a terminal without one is the
///   operator; a cgroup unit names a script; anything else is unattributed.
pub fn author_of(caller: &CallerEvidence, via: Via, hook: Option<&HookIdentity>) -> Authorship {
    if matches!(via, Via::Web | Via::Extension | Via::Desktop) {
        return Authorship {
            author: Author::Operator,
            via,
        };
    }
    let hook = hook.filter(|hook| {
        caller.harness_pid == Some(hook.harness_pid)
            && hook
                .tool_call_id
                .as_ref()
                .is_none_or(|id| caller.tool_call_id.as_ref() == Some(id))
    });
    let hook_session = hook.map(|hook| match hook.agent_id.as_deref() {
        Some(agent) if !agent.is_empty() => AgentSession {
            harness: hook.session.harness.clone(),
            native: format!("agent-{agent}"),
        },
        _ => hook.session.clone(),
    });
    let started_session = if via == Via::Mcp && caller.shared_server {
        None
    } else {
        caller.session.clone()
    };
    let session = hook_session.or_else(|| {
        if via == Via::Mcp {
            caller.call_session.clone().or(started_session)
        } else {
            started_session
        }
    });
    let author = if via == Via::Cli && caller.ambiguous_harness && hook.is_none() {
        Author::Unattributed
    } else if let Some(session) = session {
        Author::Agent(session)
    } else if let Some(harness) = &caller.harness {
        Author::AgentOfHarness(harness.clone())
    } else if via == Via::Mcp {
        Author::UnidentifiedAgent
    } else if via == Via::Cli && caller.controlling_terminal {
        Author::Operator
    } else if let Some(unit) = caller.unit.as_ref().filter(|unit| !unit.is_empty()) {
        Author::Script(unit.clone())
    } else {
        Author::Unattributed
    };
    Authorship { author, via }
}
