//! The host deliverer's policy: holds, rechecks, the interrupt budget and the
//! route function, over reports from every product's evaluator and notices
//! from other producers.
use crate::evaluator::{identity, rule_id};
use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Interrupting handoffs charged in the last hour. One per host, covering
/// every product and producer; the operator's and shipped rules are never
/// charged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InterruptBudget {
    pub per_hour: usize,
    pub handoffs: Vec<Instant>,
}
impl Default for InterruptBudget {
    fn default() -> Self {
        Self {
            per_hour: 4,
            handoffs: Vec::new(),
        }
    }
}
impl InterruptBudget {
    pub fn used(&self, at: Instant) -> usize {
        self.handoffs
            .iter()
            .filter(|handoff| at.saturating_sub(**handoff) < HOUR)
            .count()
    }
    fn charge(&mut self, at: Instant) {
        self.handoffs
            .retain(|handoff| at.saturating_sub(*handoff) < HOUR);
        self.handoffs.push(at);
    }
}

/// The deliverer's settings. Quiet hours and the budget are changeable
/// settings of the deliverer's product; the away command comes from
/// deployment configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostSettings {
    /// The product whose deliverer this is; it owns the summary notifications.
    pub product: String,
    pub quiet_hours: Option<QuietHours>,
    pub away_command: bool,
    pub deep_work_deferral: DeepWorkDeferral,
}
impl Default for HostSettings {
    fn default() -> Self {
        Self {
            product: "host".into(),
            quiet_hours: None,
            away_command: false,
            deep_work_deferral: DeepWorkDeferral::default(),
        }
    }
}

/// A fact about the operator's attention, applied at its instant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "input", rename_all = "snake_case")]
pub enum AttentionInput {
    Presence {
        presence: Presence,
    },
    Focus {
        references: Vec<String>,
    },
    DeepWork {
        active: bool,
    },
    Breakpoint,
    /// The operator's `quiet until T` act, or `quiet off` with `None`.
    QuietUntil {
        until: Option<Instant>,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
struct AttentionFacts {
    presence: Presence,
    focus: Vec<String>,
    deep_work: bool,
    quiet_until: Option<Instant>,
    breakpoint_at: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HoldRecord {
    pub at: Instant,
    pub reason: HoldReason,
    pub until: Option<Instant>,
}

/// Why a held notice will be routed again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "release", content = "root", rename_all = "snake_case")]
pub enum Release {
    /// Never handed off yet.
    First,
    /// Handed off before and snoozed.
    Snoozed,
    /// Folded into a root cause that has now cleared.
    AfterRoot(String),
}

/// A notice waiting in the host policy: the inbox's Held section.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Held {
    pub fired: Fired,
    pub due: Option<Instant>,
    pub holds: Vec<HoldRecord>,
    pub deferred_since: Option<Instant>,
    pub release: Release,
}

/// The record of one handoff. It never changes; later news is an outcome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Delivery {
    pub fired: Fired,
    pub at: Instant,
    pub decision: Decision,
    pub held: Vec<HoldRecord>,
    /// The attention the decision was taken under.
    pub attention: Attention,
}
impl Delivery {
    pub fn id(&self) -> &str {
        &self.fired.id
    }
    /// The why line: how long it waited and why, the budget, the decision.
    pub fn why(&self) -> String {
        let mut parts = Vec::new();
        for (index, hold) in self.held.iter().enumerate() {
            let end = self.held.get(index + 1).map_or(self.at, |next| next.at);
            let minutes = end.saturating_sub(hold.at) / 60_000;
            parts.push(format!("held {minutes}m ({})", hold.reason));
        }
        if self.fired.author.class() != AuthorClass::Operator
            && self.fired.channel == Channel::Interrupt
        {
            parts.push(format!(
                "budget {} of {} this hour",
                self.attention.budget_used, self.attention.budget_per_hour
            ));
        }
        parts.push(self.decision.to_string());
        parts.join(" · ")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AwayState {
    Held,
    SentAway,
    Cleared,
    Acknowledged,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AwayEntry {
    pub delivery: String,
    pub state: AwayState,
}
/// One notification standing for several deliveries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "summary", rename_all = "snake_case")]
pub enum SummaryKind {
    /// Notices made passive by the spent budget in the last hour.
    Budget { held_passive: usize },
    /// Everything that happened while the operator was away, each notice once.
    Away { entries: Vec<AwayEntry> },
    /// Consequences still current when their root cause cleared.
    Consequences {
        root: String,
        deliveries: Vec<String>,
    },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Summary {
    pub id: String,
    pub at: Instant,
    pub kind: SummaryKind,
}
impl Summary {
    /// The host policy makes summaries; its identity is the instant and the
    /// occurrence it replaces.
    pub fn new(at: Instant, kind: SummaryKind) -> Self {
        let mut summary = Self {
            id: String::new(),
            at,
            kind,
        };
        summary.id = identity(&(at, summary.occurrence()));
        summary
    }
    /// A later summary of the same kind replaces this one in place.
    pub fn occurrence(&self) -> String {
        match &self.kind {
            SummaryKind::Budget { .. } => "budget".into(),
            SummaryKind::Away { .. } => "away".into(),
            SummaryKind::Consequences { root, .. } => format!("consequences:{root}"),
        }
    }
    pub fn title(&self) -> String {
        match &self.kind {
            SummaryKind::Budget { held_passive } => {
                format!("{held_passive} notices held passive (interrupt budget spent)")
            }
            SummaryKind::Away { entries } => {
                let open = entries
                    .iter()
                    .filter(|entry| matches!(entry.state, AwayState::Held | AwayState::SentAway))
                    .count();
                format!("{} while you were away · {open} still open", entries.len())
            }
            SummaryKind::Consequences { root, deliveries } => format!(
                "{} still current after delivery:{root} cleared",
                deliveries.len()
            ),
        }
    }
}

/// What one advance hands off: new deliveries, outcomes of earlier ones, and
/// summary notifications. A product shows a desktop notification for every
/// delivery and `Released` outcome whose decision interrupts, and for every
/// summary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Handoffs {
    pub deliveries: Vec<Delivery>,
    pub outcomes: Vec<OutcomeRecord>,
    pub summaries: Vec<Summary>,
    pub rearmed_late: Vec<RuleVersion>,
}
impl Handoffs {
    pub fn extend(&mut self, other: Handoffs) {
        self.deliveries.extend(other.deliveries);
        self.outcomes.extend(other.outcomes);
        self.summaries.extend(other.summaries);
        self.rearmed_late.extend(other.rearmed_late);
    }
    pub fn is_empty(&self) -> bool {
        self.deliveries.is_empty()
            && self.outcomes.is_empty()
            && self.summaries.is_empty()
            && self.rearmed_late.is_empty()
    }
}

/// A notice from any credentialed program (`sinexctl notify`). It is a
/// delivery of the implicit rule `producer:<name>` with author class producer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProducerNotice {
    pub producer: String,
    /// The submitted fact's identity.
    pub id: String,
    /// Groups and replaces: a notice with the same key replaces the earlier
    /// notification in place.
    pub key: String,
    pub title: String,
    pub body: String,
    pub reference: Option<String>,
    pub interrupt: bool,
    /// Clears the open notice with this key.
    pub clears: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
struct RuleRevision {
    revision: u64,
    enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
struct Progress {
    at: Instant,
    incomplete: BTreeSet<String>,
}
/// A current-state delivery that has fired and not cleared, held or handed off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
struct OpenNotice {
    product: String,
    rule_name: String,
    scope: Option<String>,
    inhibitor: bool,
    shipped: Shipped,
    handed_off: bool,
}

/// One value belongs to the host deliverer. Persist it atomically with the
/// deliveries, outcomes and summaries each advance returns.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HostPolicy {
    pub settings: HostSettings,
    pub budget: InterruptBudget,
    now: Instant,
    attention: AttentionFacts,
    inputs: Vec<(Instant, AttentionInput)>,
    pending: BTreeMap<String, Held>,
    consumed: BTreeSet<String>,
    /// Actual handoffs per rule, for rate limits.
    sent: BTreeMap<String, Vec<Instant>>,
    rules: BTreeMap<String, RuleRevision>,
    progress: BTreeMap<String, Progress>,
    open: BTreeMap<String, OpenNotice>,
    /// Current consequences handed off as passive under each root.
    caused: BTreeMap<String, BTreeMap<String, Fired>>,
    away: Vec<AwayEntry>,
    budget_passive: Vec<Instant>,
    /// The latest delivery for each producer key.
    keys: BTreeMap<String, String>,
    news: Vec<OutcomeRecord>,
    rearmed_late: Vec<RuleVersion>,
}

fn rule_key(fired: &Fired) -> String {
    rule_id(&fired.trigger.rule)
}

impl HostPolicy {
    pub fn new(settings: HostSettings, budget: InterruptBudget) -> Self {
        Self {
            settings,
            budget,
            ..Self::default()
        }
    }
    pub fn now(&self) -> Instant {
        self.now
    }
    /// Notices waiting: the inbox's Held section.
    pub fn held(&self) -> impl Iterator<Item = &Held> {
        self.pending.values()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    /// The attention routing would use at the current instant.
    pub fn attention(&self) -> Attention {
        self.snapshot(self.now)
    }
    /// Queue an attention fact. It applies at its instant, which is a boundary
    /// of every advance, so live runs and backtests see it alike.
    pub fn observe(&mut self, at: Instant, input: AttentionInput) -> Result<(), EvaluationError> {
        if at < self.now {
            return Err(EvaluationError("attention fact precedes checkpoint".into()));
        }
        let index = self.inputs.partition_point(|(queued, _)| *queued <= at);
        self.inputs.insert(index, (at, input));
        Ok(())
    }
    pub fn next_due(&self) -> Option<Instant> {
        self.pending
            .values()
            .filter_map(|held| held.due)
            .chain(self.inputs.first().map(|(at, _)| *at))
            .min()
    }
    /// Take in an evaluator's reports. Duplicate firings keep the original
    /// basis and never consume the budget twice.
    pub fn absorb(&mut self, reports: impl IntoIterator<Item = Report>) {
        for report in reports {
            match report {
                Report::Fired(fired) => self.submit_fired(*fired),
                Report::Cleared { delivery, at } => self.clear(&delivery, at),
                Report::Rule { version, enabled } => {
                    self.rules.insert(
                        rule_id(&version),
                        RuleRevision {
                            revision: version.revision,
                            enabled,
                        },
                    );
                    self.cancel_stale(&rule_id(&version));
                }
                Report::Withdrawn { product, id } => {
                    let key = identity(&(&product, &id));
                    self.rules.insert(
                        key.clone(),
                        RuleRevision {
                            revision: u64::MAX,
                            enabled: false,
                        },
                    );
                    self.cancel_stale(&key);
                }
                Report::Progress {
                    product,
                    at,
                    incomplete,
                } => {
                    self.progress.insert(product, Progress { at, incomplete });
                }
                Report::RearmedLate { rule, .. } => self.rearmed_late.push(rule),
            }
        }
    }
    fn submit_fired(&mut self, fired: Fired) {
        if self.consumed.contains(&fired.id) || self.pending.contains_key(&fired.id) {
            return;
        }
        if fired.current {
            self.open.insert(
                fired.id.clone(),
                OpenNotice {
                    product: fired.trigger.rule.product.clone(),
                    rule_name: fired.rule_name.clone(),
                    scope: fired.scope.clone(),
                    inhibitor: fired.author == Author::Shipped,
                    shipped: fired.shipped.clone(),
                    handed_off: false,
                },
            );
        }
        self.pending.insert(
            fired.id.clone(),
            Held {
                due: Some(fired.at),
                fired,
                holds: Vec::new(),
                deferred_since: None,
                release: Release::First,
            },
        );
    }
    fn cancel_stale(&mut self, key: &str) {
        let Some(rule) = self.rules.get(key).cloned() else {
            return;
        };
        let stale: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, held)| {
                rule_key(&held.fired) == key
                    && (held.fired.trigger.rule.revision != rule.revision || !rule.enabled)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.pending.remove(&id);
            self.consumed.insert(id);
        }
    }
    /// A current-state condition ended. A held notice is dropped; a handed-off
    /// one gains a `cleared` outcome; consequences folded under it are routed
    /// again at once.
    fn clear(&mut self, delivery: &str, at: Instant) {
        let open = self.open.remove(delivery);
        if let Some(held) = self.pending.remove(delivery) {
            if held.release == Release::First {
                self.consumed.insert(delivery.to_string());
            }
            if held.holds.last().map(|hold| hold.reason) == Some(HoldReason::Away) {
                self.away_state(delivery, AwayState::Cleared, true);
            }
        }
        if open.is_some_and(|open| open.handed_off) {
            self.news.push(OutcomeRecord {
                delivery: delivery.to_string(),
                at,
                outcome: Outcome::Cleared,
            });
        }
        self.away_state(delivery, AwayState::Cleared, false);
        for consequences in self.caused.values_mut() {
            consequences.remove(delivery);
        }
        if let Some(consequences) = self.caused.remove(delivery) {
            for (id, fired) in consequences {
                if self.open.contains_key(&id) {
                    self.pending.insert(
                        id,
                        Held {
                            fired,
                            due: Some(at),
                            holds: Vec::new(),
                            deferred_since: None,
                            release: Release::AfterRoot(delivery.to_string()),
                        },
                    );
                }
            }
        }
    }
    fn away_state(&mut self, delivery: &str, state: AwayState, insert: bool) {
        if let Some(entry) = self
            .away
            .iter_mut()
            .find(|entry| entry.delivery == delivery)
        {
            entry.state = state;
        } else if insert {
            self.away.push(AwayEntry {
                delivery: delivery.to_string(),
                state,
            });
        }
    }
    fn apply_inputs(&mut self, at: Instant) {
        let count = self.inputs.partition_point(|(queued, _)| *queued <= at);
        for (when, input) in self.inputs.drain(..count).collect::<Vec<_>>() {
            match input {
                AttentionInput::Presence { presence } => self.attention.presence = presence,
                AttentionInput::Focus { references } => self.attention.focus = references,
                AttentionInput::DeepWork { active } => self.attention.deep_work = active,
                AttentionInput::QuietUntil { until } => self.attention.quiet_until = until,
                AttentionInput::Breakpoint => {
                    self.attention.breakpoint_at = Some(when);
                    for held in self.pending.values_mut() {
                        if held.holds.last().map(|hold| hold.reason) == Some(HoldReason::DeepWork) {
                            held.due = Some(when);
                        }
                    }
                }
            }
        }
    }
    fn snapshot(&self, at: Instant) -> Attention {
        Attention {
            at,
            quiet_until: self.attention.quiet_until,
            presence: self.attention.presence,
            focus: self.attention.focus.clone(),
            deep_work: self.attention.deep_work,
            breakpoint: self.attention.breakpoint_at == Some(at),
            budget_used: self.budget.used(at),
            budget_per_hour: self.budget.per_hour,
            away_command: self.settings.away_command,
            deep_work_deferral: self.settings.deep_work_deferral,
        }
    }
    /// The root cause folding this notice: an open current-state occurrence of
    /// a shipped rule of the same product that declares it, in scope.
    fn inhibitor(&self, fired: &Fired) -> Option<String> {
        self.open
            .iter()
            .find(|(id, root)| {
                **id != fired.id
                    && root.inhibitor
                    && root.product == fired.trigger.rule.product
                    && root.shipped.inhibits.contains(&fired.rule_name)
                    && (root.shipped.inhibition_scope == InhibitionScope::Host
                        || root.scope == fired.scope)
            })
            .map(|(id, _)| id.clone())
    }
    fn candidate(&self, held: &Held, at: Instant, calendar: &impl Calendar) -> Candidate {
        let fired = &held.fired;
        Candidate {
            interrupt: fired.channel == Channel::Interrupt,
            urgency: fired.shipped.urgency,
            author: fired.author.class(),
            subjects: fired
                .basis
                .iter()
                .map(|row| row.reference.clone())
                .collect(),
            quiet_hours_until: fired
                .quiet_hours
                .or(self.settings.quiet_hours)
                .and_then(|hours| calendar.quiet_until(at, hours)),
            inhibited_by: self.inhibitor(fired),
            deferred_since: held.deferred_since,
        }
    }
    /// Pass time without handing anything off (downtime): attention facts
    /// apply, nothing is routed.
    pub fn pass(&mut self, at: Instant) {
        self.apply_inputs(at);
        self.now = self.now.max(at);
    }
    /// Route every due notice at `at`. Holds and rechecks happen here, and the
    /// budget is charged at handoff, never at match.
    pub fn advance(&mut self, at: Instant, calendar: &impl Calendar) -> Handoffs {
        self.apply_inputs(at);
        self.now = self.now.max(at);
        let mut output = Handoffs {
            outcomes: std::mem::take(&mut self.news),
            rearmed_late: std::mem::take(&mut self.rearmed_late),
            ..Handoffs::default()
        };
        let mut due: Vec<Held> = self
            .pending
            .values()
            .filter(|held| held.due.is_none_or(|due| due <= at))
            .cloned()
            .collect();
        due.sort_by(|a, b| {
            let size = |held: &Held| held.fired.change.as_ref().map_or(0, Change::size);
            size(b)
                .cmp(&size(a))
                .then_with(|| a.fired.trigger.cmp(&b.fired.trigger))
        });
        let away_summary = Summary::new(
            at,
            SummaryKind::Away {
                entries: Vec::new(),
            },
        )
        .id;
        let mut consequences: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut budget_passive = false;
        for held in due {
            let id = held.fired.id.clone();
            if self.rules.get(&rule_key(&held.fired)).is_some_and(|rule| {
                rule.revision != held.fired.trigger.rule.revision || !rule.enabled
            }) {
                self.pending.remove(&id);
                self.open.remove(&id);
                self.consumed.insert(id);
                continue;
            }
            if held.release == Release::Snoozed
                && held.fired.current
                && !self.open.contains_key(&id)
            {
                self.pending.remove(&id);
                continue;
            }
            let behind = match self.progress.get(&held.fired.trigger.rule.product) {
                Some(progress) => {
                    progress.incomplete.contains(&held.fired.trigger.rule.id)
                        || (held.fired.current && progress.at < at)
                }
                // A forwarded current-state notice waits for its producer.
                None => held.fired.current,
            };
            let mut decision = if behind {
                Decision::Hold {
                    until: None,
                    reason: HoldReason::Incomplete,
                }
            } else if matches!(held.fired.channel, Channel::Value | Channel::Action) {
                Decision::Run
            } else {
                route(&self.candidate(&held, at, calendar), &self.snapshot(at))
            };
            if let Decision::Hold { until, reason } = decision {
                let entry = self.pending.get_mut(&id).expect("due notice is pending");
                entry.due = until;
                if reason == HoldReason::DeepWork && entry.deferred_since.is_none() {
                    entry.deferred_since = Some(at);
                }
                if entry.holds.last().map(|hold| hold.reason) != Some(reason) {
                    entry.holds.push(HoldRecord { at, reason, until });
                }
                continue;
            }
            self.pending.remove(&id);
            if held.release == Release::First && !self.consumed.insert(id.clone()) {
                continue;
            }
            if held.release == Release::First {
                let sent = self.sent.entry(rule_key(&held.fired)).or_default();
                if held.fired.rate_limit.as_ref().is_some_and(|limit| {
                    sent.iter()
                        .filter(|sent| at.saturating_sub(**sent) < limit.window.0)
                        .count()
                        >= limit.count
                }) {
                    decision = Decision::Passive {
                        reason: PassiveReason::RateLimit,
                    };
                } else {
                    sent.push(at);
                }
            }
            let attention = self.snapshot(at);
            let was_away = held.holds.last().map(|hold| hold.reason) == Some(HoldReason::Away);
            if was_away && decision.interrupts() {
                decision = Decision::Passive {
                    reason: PassiveReason::Summarised(away_summary.clone()),
                };
                self.away_state(&id, AwayState::Held, true);
            }
            if let Release::AfterRoot(root) = &held.release
                && decision.interrupts()
            {
                let summary = Summary::new(
                    at,
                    SummaryKind::Consequences {
                        root: root.clone(),
                        deliveries: Vec::new(),
                    },
                );
                decision = Decision::Passive {
                    reason: PassiveReason::Summarised(summary.id),
                };
                consequences
                    .entry(root.clone())
                    .or_default()
                    .push(id.clone());
            }
            if decision.interrupts() && held.fired.author.class() != AuthorClass::Operator {
                self.budget.charge(at);
            }
            if decision.sends_away() && self.attention.presence == Presence::Away {
                self.away_state(&id, AwayState::SentAway, true);
            }
            match &decision {
                Decision::Passive {
                    reason: PassiveReason::BudgetSpent,
                } => {
                    self.budget_passive.push(at);
                    budget_passive = true;
                }
                Decision::Passive {
                    reason: PassiveReason::CausedBy(root),
                } if held.fired.current => {
                    self.caused
                        .entry(root.clone())
                        .or_default()
                        .insert(id.clone(), held.fired.clone());
                }
                _ => {}
            }
            if let Some(open) = self.open.get_mut(&id) {
                open.handed_off = true;
            }
            match held.release {
                Release::First => output.deliveries.push(Delivery {
                    fired: held.fired,
                    at,
                    decision,
                    held: held.holds,
                    attention,
                }),
                Release::Snoozed | Release::AfterRoot(_) => output.outcomes.push(OutcomeRecord {
                    delivery: id,
                    at,
                    outcome: Outcome::Released { decision },
                }),
            }
        }
        for (root, deliveries) in consequences {
            output.summaries.push(Summary::new(
                at,
                SummaryKind::Consequences { root, deliveries },
            ));
        }
        if budget_passive {
            self.budget_passive
                .retain(|passive| at.saturating_sub(*passive) < HOUR);
            output.summaries.push(Summary::new(
                at,
                SummaryKind::Budget {
                    held_passive: self.budget_passive.len(),
                },
            ));
        }
        if self.attention.presence != Presence::Away && !self.away.is_empty() {
            output.summaries.push(Summary::new(
                at,
                SummaryKind::Away {
                    entries: std::mem::take(&mut self.away),
                },
            ));
        }
        output
    }
    /// The operator's act on a delivery, held or handed off. Acknowledging a
    /// held notice hands it off as passive.
    pub fn acknowledge(&mut self, delivery: &str, at: Instant) -> Handoffs {
        let mut output = Handoffs::default();
        if let Some(held) = self.pending.remove(delivery) {
            if held.holds.last().map(|hold| hold.reason) == Some(HoldReason::Away) {
                self.away_state(delivery, AwayState::Acknowledged, true);
            }
            if held.release == Release::First && self.consumed.insert(delivery.to_string()) {
                if let Some(open) = self.open.get_mut(delivery) {
                    open.handed_off = true;
                }
                output.deliveries.push(Delivery {
                    attention: self.snapshot(at),
                    fired: held.fired,
                    at,
                    decision: Decision::Passive {
                        reason: PassiveReason::Acknowledged,
                    },
                    held: held.holds,
                });
            }
        }
        self.away_state(delivery, AwayState::Acknowledged, false);
        output.outcomes.push(OutcomeRecord {
            delivery: delivery.to_string(),
            at,
            outcome: Outcome::Acknowledged,
        });
        output
    }
    /// Hold a handed-off delivery again. At `until` a current-state notice is
    /// shown again only if its condition still holds; a historical one as is.
    pub fn snooze(&mut self, delivery: &Delivery, until: Instant, at: Instant) -> OutcomeRecord {
        self.pending.insert(
            delivery.fired.id.clone(),
            Held {
                fired: delivery.fired.clone(),
                due: Some(until),
                holds: vec![HoldRecord {
                    at,
                    reason: HoldReason::Snoozed,
                    until: Some(until),
                }],
                deferred_since: None,
                release: Release::Snoozed,
            },
        );
        OutcomeRecord {
            delivery: delivery.fired.id.clone(),
            at,
            outcome: Outcome::Snoozed { until },
        }
    }
    /// Take in a producer's notice. It is routed like any delivery at the next
    /// advance; `clears` closes the open notice with that key now.
    pub fn submit(&mut self, notice: ProducerNotice, at: Instant) {
        let product = format!("producer:{}", notice.producer);
        if let Some(key) = &notice.clears
            && let Some(delivery) = self.keys.remove(&identity(&(&notice.producer, key)))
        {
            // Producer notices are historical; clearing one is the only way it
            // becomes open, so its outcome is recorded once.
            self.open.insert(
                delivery.clone(),
                OpenNotice {
                    product: product.clone(),
                    rule_name: product.clone(),
                    scope: None,
                    inhibitor: false,
                    shipped: Shipped::default(),
                    handed_off: !self.pending.contains_key(&delivery),
                },
            );
            self.clear(&delivery, at);
        }
        if notice.title.is_empty() {
            return;
        }
        let trigger = Trigger {
            rule: RuleVersion {
                product: product.clone(),
                id: "notice".into(),
                revision: 0,
            },
            occurrence: Occurrence {
                id: notice.key.clone(),
                period: notice.id.clone(),
            },
        };
        let row = Row {
            occurrence: trigger.occurrence.clone(),
            reference: notice.reference.clone().unwrap_or_default(),
            observed: at,
            start: at,
            end: None,
            origin: Origin::Source,
            evidence: serde_json::json!({"title": notice.title, "body": notice.body}),
        };
        let id = trigger.delivery_id();
        self.keys
            .insert(identity(&(&notice.producer, &notice.key)), id.clone());
        self.submit_fired(Fired {
            id,
            trigger,
            rule_name: product,
            matched: at,
            at,
            basis: vec![row],
            channel: if notice.interrupt {
                Channel::Interrupt
            } else {
                Channel::Notice
            },
            change: None,
            value: None,
            scope: None,
            to: Vec::new(),
            author: Author::Producer(notice.producer),
            quiet_hours: None,
            rate_limit: None,
            current: false,
            shipped: Shipped::default(),
        });
    }
}
