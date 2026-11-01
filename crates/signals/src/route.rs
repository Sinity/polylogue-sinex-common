//! The route function: one pure decision per notice, with its reason.
use crate::{AuthorClass, Instant, Urgency};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A breakpoint hold in deep work ends at the first breakpoint or after this
/// long, whichever comes first, so no notice waits indefinitely.
pub const BREAKPOINT_BOUND: i64 = 25 * 60_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Present,
    Away,
    /// Never counts as away: a standalone product, or a gap in capture.
    #[default]
    Unknown,
}

/// Which non-critical interrupting notices wait for a breakpoint in deep work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeepWorkDeferral {
    Never,
    /// Notices from agents' rules and producers wait; the operator's own and
    /// shipped rules show at once.
    #[default]
    AgentsAndProducers,
    All,
}

/// The deliverer's snapshot of the operator's attention at one instant, kept
/// current from facts it already has. Every field has a value meaning
/// "unknown", and unknown inputs only ever leave routing as quiet hours and the
/// budget decide it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Attention {
    pub at: Instant,
    /// The end of an explicit `quiet until T` act.
    pub quiet_until: Option<Instant>,
    pub presence: Presence,
    /// References shown in the focused window: an agent session joined to the
    /// window, a repository, a web route. Empty when unknown.
    pub focus: Vec<String>,
    pub deep_work: bool,
    /// A breakpoint happens at `at`: an app or work-episode switch, the end of an
    /// input bout, an unlock.
    pub breakpoint: bool,
    /// Interrupting handoffs charged to the host budget in the last hour.
    pub budget_used: usize,
    pub budget_per_hour: usize,
    /// The deployment configures a command that sends notices away.
    pub away_command: bool,
    pub deep_work_deferral: DeepWorkDeferral,
}

/// What routing knows about one notice at one instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Candidate {
    pub interrupt: bool,
    pub urgency: Urgency,
    pub author: AuthorClass,
    /// The references the notice is about; the subject is on screen when the
    /// focus shows one of them.
    pub subjects: Vec<String>,
    /// The end of the quiet hours (the rule's own, else the host's) containing
    /// this instant.
    pub quiet_hours_until: Option<Instant>,
    /// An open root-cause delivery that folds this notice.
    pub inhibited_by: Option<String>,
    /// When this notice first waited for a breakpoint.
    pub deferred_since: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    /// Shown as a desktop notification; critical notices are also sent away
    /// while the operator is away.
    Show {
        send_away: bool,
    },
    /// Sent to the deployment's away command.
    SendAway,
    /// Waits until `until`, or until its releasing input changes when `None`.
    Hold {
        until: Option<Instant>,
        reason: HoldReason,
    },
    Passive {
        reason: PassiveReason,
    },
    /// A value or action delivery, run through its configured target.
    Run,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "delivery", rename_all = "snake_case")]
pub enum PassiveReason {
    RuleIsPassive,
    CausedBy(String),
    BudgetSpent,
    OnScreen,
    RateLimit,
    /// Shown as a line of one summary notification (the away summary, or the
    /// consequences of a cleared root cause) instead of on its own.
    Summarised(String),
    /// Acknowledged while it was held.
    Acknowledged,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    Quiet,
    Away,
    DeepWork,
    /// The producer has not confirmed that its data is up to date.
    Incomplete,
    Snoozed,
}

impl Decision {
    pub fn interrupts(&self) -> bool {
        matches!(self, Self::Show { .. } | Self::SendAway)
    }
    pub fn sends_away(&self) -> bool {
        matches!(self, Self::Show { send_away: true } | Self::SendAway)
    }
}

/// Rules apply in order, and the first that applies decides.
pub fn route(notice: &Candidate, attention: &Attention) -> Decision {
    let passive = |reason| Decision::Passive { reason };
    let hold = |until, reason| Decision::Hold { until, reason };
    let at = attention.at;
    if !notice.interrupt {
        return passive(PassiveReason::RuleIsPassive);
    }
    if let Some(root) = &notice.inhibited_by {
        return passive(PassiveReason::CausedBy(root.clone()));
    }
    let away = attention.presence == Presence::Away;
    if notice.urgency == Urgency::Critical {
        return Decision::Show {
            send_away: away && attention.away_command,
        };
    }
    let author = notice.author;
    if author != AuthorClass::Operator && attention.budget_used >= attention.budget_per_hour {
        return passive(PassiveReason::BudgetSpent);
    }
    let quiet = [notice.quiet_hours_until, attention.quiet_until]
        .into_iter()
        .flatten()
        .filter(|until| *until > at)
        .max();
    if let Some(until) = quiet {
        return hold(Some(until), HoldReason::Quiet);
    }
    if notice
        .subjects
        .iter()
        .any(|subject| attention.focus.contains(subject))
    {
        return passive(PassiveReason::OnScreen);
    }
    if away {
        return if attention.away_command {
            Decision::SendAway
        } else {
            hold(None, HoldReason::Away)
        };
    }
    let defers = match attention.deep_work_deferral {
        DeepWorkDeferral::Never => false,
        DeepWorkDeferral::AgentsAndProducers => author != AuthorClass::Operator,
        DeepWorkDeferral::All => true,
    };
    if attention.deep_work && defers && !attention.breakpoint {
        let bound = notice
            .deferred_since
            .unwrap_or(at)
            .saturating_add(BREAKPOINT_BOUND);
        if at < bound {
            return hold(Some(bound), HoldReason::DeepWork);
        }
    }
    Decision::Show { send_away: false }
}

impl std::fmt::Display for Decision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Show { send_away: false } => f.write_str("shown"),
            Self::Show { send_away: true } => f.write_str("shown and sent away"),
            Self::SendAway => f.write_str("sent away"),
            Self::Hold { reason, .. } => write!(f, "held: {reason}"),
            Self::Passive { reason } => write!(f, "passive: {reason}"),
            Self::Run => f.write_str("run"),
        }
    }
}
impl std::fmt::Display for PassiveReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RuleIsPassive => f.write_str("rule is passive"),
            Self::CausedBy(root) => write!(f, "caused by delivery:{root}"),
            Self::BudgetSpent => f.write_str("interrupt budget spent"),
            Self::OnScreen => f.write_str("on screen"),
            Self::RateLimit => f.write_str("rate limit"),
            Self::Summarised(summary) => write!(f, "shown in summary {summary}"),
            Self::Acknowledged => f.write_str("acknowledged while held"),
        }
    }
}
impl std::fmt::Display for HoldReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Quiet => "quiet",
            Self::Away => "away",
            Self::DeepWork => "deep work",
            Self::Incomplete => "incomplete",
            Self::Snoozed => "snoozed",
        })
    }
}
