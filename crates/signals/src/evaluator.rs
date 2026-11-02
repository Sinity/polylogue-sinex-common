use crate::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Products implement this with the shared query language's DaySetting. Local
/// gaps resolve to the first valid instant, folds to the earlier instant.
/// `next_at` is strictly after its argument; quiet_end is strictly after now.
pub trait Calendar {
    fn logical_day(&self, at: Instant) -> String;
    fn minute(&self, at: Instant) -> u16;
    fn next_at(&self, after: Instant, minute: u16) -> Instant;
    fn quiet_end(&self, at: Instant, hours: QuietHours) -> Instant {
        self.next_at(at, hours.to)
    }
    /// Named-zone adapters resolve the absolute interval here so a repeated
    /// civil minute cannot re-enter a quiet interval that already ended.
    fn quiet_until(&self, at: Instant, hours: QuietHours) -> Option<Instant> {
        hours
            .contains(self.minute(at))
            .then(|| self.quiet_end(at, hours))
    }
}
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct Occurrence {
    pub id: String,
    pub period: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Source,
    SignalDelivery,
    InjectedContext,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Row {
    pub occurrence: Occurrence,
    pub reference: String,
    pub observed: Instant,
    pub start: Instant,
    pub end: Option<Instant>,
    #[serde(default)]
    pub origin: Origin,
    /// Evidence is preserved at match time. Scores never define occurrence or
    /// change identity; the adapter supplies the complete result order.
    pub evidence: serde_json::Value,
}
impl Row {
    pub fn current_at(&self, at: Instant) -> bool {
        self.start <= at && self.end.is_none_or(|end| at < end)
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Evaluation {
    pub rule: RuleVersion,
    pub at: Instant,
    pub complete: bool,
    pub rows: Vec<Row>,
    pub value: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct Trigger {
    pub rule: RuleVersion,
    pub occurrence: Occurrence,
}
pub(crate) fn identity(value: &impl Serialize) -> String {
    blake3::hash(&serde_json::to_vec(value).expect("signal identity contains no non-finite values"))
        .to_hex()
        .to_string()
}
impl Trigger {
    pub fn delivery_id(&self) -> String {
        identity(self)
    }
}
/// The channel a rule asks for. Routing decides what happens to a notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Notice,
    Interrupt,
    Value,
    Action,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Change {
    pub what: ChangeKind,
    pub joined: Vec<Occurrence>,
    pub left: Vec<Occurrence>,
    pub before: Vec<Occurrence>,
    pub after: Vec<Occurrence>,
}
impl Change {
    pub fn size(&self) -> usize {
        self.joined.len() + self.left.len()
    }
}
/// One firing of a rule, before routing. Its basis (rows and their
/// observation) is fixed when it matched, never when it is handed off.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Fired {
    /// Deterministic: product, rule ID and revision, occurrence and period.
    pub id: String,
    pub trigger: Trigger,
    pub rule_name: String,
    pub matched: Instant,
    /// When the evaluator fired it.
    pub at: Instant,
    pub basis: Vec<Row>,
    pub channel: Channel,
    pub change: Option<Change>,
    pub value: Option<f64>,
    pub scope: Option<String>,
    pub to: Vec<Recipient>,
    pub author: Author,
    pub quiet_hours: Option<QuietHours>,
    pub rate_limit: Option<RateLimit>,
    /// A current-state notice: it rechecks its condition until handoff and is
    /// closed by a `Cleared` report when the condition ends.
    pub current: bool,
    pub shipped: Shipped,
}
/// What an evaluator tells the deliverer. A standalone product hands these to
/// its own host policy; in primary mode Polylogue sends them to Sinex as facts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "report", rename_all = "snake_case")]
pub enum Report {
    Fired(Box<Fired>),
    /// A current-state condition ended, or its rule changed or was withdrawn.
    Cleared {
        delivery: String,
        at: Instant,
    },
    /// A delivery the product's writer refused because its cited basis
    /// changed. Every record of it is dropped, so the trigger can fire afresh.
    Retracted {
        delivery: String,
        at: Instant,
    },
    /// The active revision of a rule, and whether it runs.
    Rule {
        version: RuleVersion,
        enabled: bool,
    },
    Withdrawn {
        product: String,
        id: String,
    },
    /// The product's evaluation is up to date as of `at`, except the named
    /// rules, whose data is behind.
    Progress {
        product: String,
        at: Instant,
        incomplete: BTreeSet<String>,
    },
    /// A rebuilt threshold rule found its condition already true, so a
    /// crossing during the lost checkpoint cannot be told apart from the baseline.
    RearmedLate {
        rule: RuleVersion,
        at: Instant,
    },
}
/// A durable value or action effect starts with an unknown receipt. Persist it
/// with the delivery; only a returned receipt changes its outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "outcome", content = "reason", rename_all = "snake_case")]
pub enum DeliveryOutcome {
    Unknown,
    Delivered,
    NotSent(String),
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeliveryReceipt {
    pub delivery_id: String,
    pub at: Instant,
    pub outlet: String,
    pub outcome: DeliveryOutcome,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Pending {
    trigger: Trigger,
    due: Instant,
    matched: Instant,
    basis: Vec<Row>,
    change: Option<Change>,
    value: Option<f64>,
}
/// A fired current-state delivery whose condition is still checked.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Open {
    trigger: Trigger,
    basis: Vec<Row>,
    change: Option<Change>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct Snapshot {
    at: Instant,
    order: Vec<Occurrence>,
    rows: Vec<Row>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
struct RuleState {
    active: ActiveRule,
    started: Instant,
    revision_baseline: bool,
    rows: BTreeMap<String, Row>,
    order: Vec<Occurrence>,
    incomplete: bool,
    baseline: Option<Snapshot>,
    latest: Option<Snapshot>,
    reported: bool,
    change_due: Option<Instant>,
    threshold: Option<bool>,
    last_value: Option<f64>,
    observed_at: Option<Instant>,
    next_at: Option<Instant>,
    /// Built after its checkpoint was lost, from stored deliveries.
    rebuilt: bool,
}
fn current_match(
    active: &ActiveRule,
    rows: &BTreeMap<String, Row>,
    threshold: Option<bool>,
    latest: Option<&Snapshot>,
    basis: &[Row],
    change: Option<&Change>,
    at: Instant,
) -> bool {
    if !active.current() {
        return true;
    }
    if let Fires::OnChange { what, k } = active.rule.fires {
        return change.is_some_and(|change| {
            latest.is_some_and(|latest| difference(&change.after, &latest.order, what, k).is_none())
        });
    }
    (!matches!(active.rule.fires, Fires::OnThreshold { .. }) || threshold == Some(true))
        && basis.iter().all(|basis| {
            rows.get(&occurrence_id(&basis.occurrence))
                .is_some_and(|row| row.current_at(at))
        })
}
fn change_basis(before: &Snapshot, after: &Snapshot) -> Vec<Row> {
    before
        .rows
        .iter()
        .chain(&after.rows)
        .map(|row| (row.occurrence.clone(), row.clone()))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect()
}
/// The serializable evaluator checkpoint. Products persist it with the
/// deliveries it produced.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Evaluator {
    now: Instant,
    rules: BTreeMap<String, RuleState>,
    products: BTreeSet<String>,
    inputs: Vec<Evaluation>,
    consumed: BTreeSet<Trigger>,
    pending: BTreeMap<String, Pending>,
    open: BTreeMap<String, Open>,
    /// Refused deliveries, each dropped at its instant, in time order. An
    /// absent field means none are queued.
    #[serde(default)]
    retractions: Vec<(Instant, String)>,
    /// Reports not yet handed to a host policy or returned to the caller.
    reports: Vec<Report>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationError(pub String);
impl std::fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for EvaluationError {}
pub(crate) fn rule_id(version: &RuleVersion) -> String {
    identity(&(&version.product, &version.id))
}
fn occurrence_id(occurrence: &Occurrence) -> String {
    identity(occurrence)
}

impl Evaluator {
    pub fn new(
        start: Instant,
        rules: Vec<ActiveRule>,
        calendar: &impl Calendar,
    ) -> Result<Self, EvaluationError> {
        let mut evaluator = Self {
            now: start,
            rules: BTreeMap::new(),
            products: BTreeSet::new(),
            inputs: Vec::new(),
            consumed: BTreeSet::new(),
            pending: BTreeMap::new(),
            open: BTreeMap::new(),
            retractions: Vec::new(),
            reports: Vec::new(),
        };
        for active in rules {
            active
                .validate_authorship()
                .map_err(|error| EvaluationError(error.0))?;
            let key = rule_id(&active.version);
            if evaluator.rules.values().any(|state| {
                state.active.version.product == active.version.product
                    && state.active.rule.name == active.rule.name
            }) {
                return Err(EvaluationError("duplicate rule name".into()));
            }
            if evaluator.rules.contains_key(&key) {
                return Err(EvaluationError("duplicate rule identity".into()));
            }
            evaluator.products.insert(active.version.product.clone());
            evaluator.reports.push(Report::Rule {
                version: active.version.clone(),
                enabled: active.rule.enabled,
            });
            evaluator
                .rules
                .insert(key, Self::state(active, start, calendar));
        }
        Ok(evaluator)
    }
    /// Rebuild after the checkpoint was lost: from the rules, the triggers of
    /// the deliveries already stored, and stored observations queued afterwards
    /// with `observe`. No stored delivery identity is produced again.
    pub fn rebuild(
        start: Instant,
        rules: Vec<ActiveRule>,
        delivered: impl IntoIterator<Item = Trigger>,
        calendar: &impl Calendar,
    ) -> Result<Self, EvaluationError> {
        let mut evaluator = Self::new(start, rules, calendar)?;
        evaluator.consumed.extend(delivered);
        for state in evaluator.rules.values_mut() {
            state.rebuilt = true;
        }
        Ok(evaluator)
    }
    fn state(active: ActiveRule, start: Instant, calendar: &impl Calendar) -> RuleState {
        let next_at = match active.rule.fires {
            Fires::At { minute } => Some(calendar.next_at(start.saturating_sub(1), minute)),
            _ => None,
        };
        RuleState {
            active,
            started: start,
            revision_baseline: false,
            rows: BTreeMap::new(),
            order: Vec::new(),
            incomplete: true,
            baseline: None,
            latest: None,
            reported: false,
            change_due: None,
            threshold: None,
            last_value: None,
            observed_at: None,
            next_at,
            rebuilt: false,
        }
    }
    /// A follow has no baseline: rows already matching are its first message.
    pub(crate) fn without_baseline(&mut self) {
        for state in self.rules.values_mut() {
            state.started = Instant::MIN;
        }
    }
    pub fn now(&self) -> Instant {
        self.now
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn incomplete(&self) -> Vec<RuleVersion> {
        self.rules
            .values()
            .filter(|state| state.incomplete)
            .map(|state| state.active.version.clone())
            .collect()
    }
    /// Queue observations, never effective timestamps. Already observed rows
    /// may schedule future starts/ends; unseen metadata contributes no wakeup.
    pub fn observe(&mut self, evaluation: Evaluation) -> Result<(), EvaluationError> {
        if evaluation.at < self.now {
            return Err(EvaluationError("observation precedes checkpoint".into()));
        }
        if self
            .rules
            .get(&rule_id(&evaluation.rule))
            .is_some_and(|state| {
                state.active.version == evaluation.rule && evaluation.at < state.started
            })
        {
            return Err(EvaluationError("observation precedes rule adoption".into()));
        }
        if evaluation.value.is_some_and(|v| !v.is_finite()) {
            return Err(EvaluationError("sample must be finite".into()));
        }
        if evaluation
            .rows
            .iter()
            .any(|row| row.observed > evaluation.at)
        {
            return Err(EvaluationError("unobserved row in evaluation".into()));
        }
        let mut occurrences = BTreeSet::new();
        if evaluation
            .rows
            .iter()
            .any(|row| !occurrences.insert(row.occurrence.clone()))
        {
            return Err(EvaluationError("ambiguous occurrence in evaluation".into()));
        }
        if self
            .rules
            .get(&rule_id(&evaluation.rule))
            .is_some_and(|state| {
                state.active.version == evaluation.rule
                    && state.observed_at.is_some_and(|at| evaluation.at <= at)
            })
        {
            return Err(EvaluationError(
                "evaluation is not newer than checkpoint".into(),
            ));
        }
        if self
            .inputs
            .iter()
            .any(|old| old.rule == evaluation.rule && old.at == evaluation.at)
        {
            return Err(EvaluationError("ambiguous simultaneous evaluations".into()));
        }
        self.inputs.push(evaluation);
        self.inputs
            .sort_by(|a, b| (a.at, &a.rule).cmp(&(b.at, &b.rule)));
        Ok(())
    }
    /// Adopt a revision at the owner's captured observation time, without
    /// advancing old rules first. Its first complete result is the new baseline.
    /// Retrying an identical active revision preserves its original baseline and
    /// held occurrences. A changed revision cancels the previous pending work
    /// and clears its open current-state deliveries.
    pub fn replace_rule(
        &mut self,
        active: ActiveRule,
        observed_at: Instant,
        calendar: &impl Calendar,
    ) -> Result<(), EvaluationError> {
        active
            .validate_authorship()
            .map_err(|error| EvaluationError(error.0))?;
        let key = rule_id(&active.version);
        if let Some(old) = self.rules.get(&key) {
            if old.active == active {
                return Ok(());
            }
            if active.version.revision <= old.active.version.revision {
                return Err(EvaluationError("rule edits require a new revision".into()));
            }
        }
        if observed_at < self.now {
            return Err(EvaluationError("rule adoption precedes checkpoint".into()));
        }
        if self
            .inputs
            .iter()
            .any(|input| input.rule == active.version && input.at < observed_at)
        {
            return Err(EvaluationError(
                "queued observation precedes rule adoption".into(),
            ));
        }
        self.products.insert(active.version.product.clone());
        self.reports.push(Report::Rule {
            version: active.version.clone(),
            enabled: active.rule.enabled,
        });
        let mut state = Self::state(active, observed_at, calendar);
        // A revised query can read a different relation. Its first complete
        // evaluation, rather than the previous query's rows, is its baseline.
        state.revision_baseline = true;
        state.incomplete = true;
        self.forget(&key, observed_at);
        self.inputs
            .retain(|input| rule_id(&input.rule) != key || input.rule == state.active.version);
        self.rules.insert(key, state);
        Ok(())
    }
    pub fn withdraw(&mut self, version: &RuleVersion) {
        let key = rule_id(version);
        if self.rules.remove(&key).is_some() {
            self.reports.push(Report::Withdrawn {
                product: version.product.clone(),
                id: version.id.clone(),
            });
        }
        self.forget(&key, self.now);
        self.inputs.retain(|input| rule_id(&input.rule) != key);
    }
    /// Queue the retraction of a delivery the writer refused because its
    /// cited basis changed. At `at`, after the observations up to `at`, its
    /// pending work, open condition and consumed trigger are dropped and a
    /// `Retracted` report tells the host policy to drop its held or routed
    /// copy. The same evaluation fires the trigger afresh, under the same
    /// identity and from the current rows, if the rule still finds it: an
    /// occurrence still in a new-row rule's result fires again; a threshold,
    /// change or daily firing is keyed by its crossing, change or day and
    /// fires again only on a new one. The rule's other deliveries are
    /// untouched.
    pub fn retract(&mut self, delivery: &str, at: Instant) -> Result<(), EvaluationError> {
        if at < self.now {
            return Err(EvaluationError("retraction precedes checkpoint".into()));
        }
        let index = self
            .retractions
            .partition_point(|(queued, _)| *queued <= at);
        self.retractions.insert(index, (at, delivery.to_string()));
        Ok(())
    }
    /// Cancel a rule's pending work and clear its open deliveries.
    fn forget(&mut self, key: &str, at: Instant) {
        self.pending
            .retain(|_, pending| rule_id(&pending.trigger.rule) != key);
        let cleared: Vec<_> = self
            .open
            .iter()
            .filter(|(_, open)| rule_id(&open.trigger.rule) == key)
            .map(|(id, _)| id.clone())
            .collect();
        for delivery in cleared {
            self.open.remove(&delivery);
            self.reports.push(Report::Cleared { delivery, at });
        }
    }
    /// Advance a standalone product: evaluation and its own host policy at
    /// every causal boundary, so any partition gives the same handoffs.
    pub fn advance(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        host: &mut HostPolicy,
    ) -> Result<Handoffs, EvaluationError> {
        self.run(stop, calendar, Some(host), false)
    }
    /// After downtime: take in what was observed without handing anything off
    /// for past instants, then hand off overdue work once, at `stop`.
    pub fn resume(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        host: &mut HostPolicy,
    ) -> Result<Handoffs, EvaluationError> {
        self.run(stop, calendar, Some(host), true)
    }
    /// Advance without a host: the reports a product sends to the host
    /// deliverer of another product (Polylogue in primary mode).
    pub fn reports(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
    ) -> Result<Vec<Report>, EvaluationError> {
        self.run(stop, calendar, None, false)?;
        Ok(std::mem::take(&mut self.reports))
    }
    pub fn resume_reports(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
    ) -> Result<Vec<Report>, EvaluationError> {
        self.run(stop, calendar, None, true)?;
        Ok(std::mem::take(&mut self.reports))
    }
    fn run(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        mut host: Option<&mut HostPolicy>,
        resume: bool,
    ) -> Result<Handoffs, EvaluationError> {
        if stop < self.now || host.as_ref().is_some_and(|host| stop < host.now()) {
            return Err(EvaluationError("clock must be monotone".into()));
        }
        let mut output = Handoffs::default();
        loop {
            self.read(self.now, calendar);
            let handing_off = !resume || self.now == stop;
            if handing_off {
                self.fire(self.now);
            }
            self.progress(self.now);
            if let Some(host) = host.as_deref_mut() {
                host.absorb(std::mem::take(&mut self.reports));
                if handing_off {
                    output.extend(host.advance(self.now, calendar));
                } else {
                    host.pass(self.now);
                }
            }
            if self.now == stop {
                break;
            }
            let next = self.boundary(stop);
            self.now = host
                .as_ref()
                .and_then(|host| host.next_due())
                .filter(|due| *due > self.now)
                .map_or(next, |due| due.min(next));
        }
        Ok(output)
    }
    fn progress(&mut self, at: Instant) {
        for product in &self.products {
            let incomplete = self
                .rules
                .values()
                .filter(|state| state.active.version.product == *product && state.incomplete)
                .map(|state| state.active.version.id.clone())
                .collect();
            self.reports.push(Report::Progress {
                product: product.clone(),
                at,
                incomplete,
            });
        }
    }
    fn boundary(&self, stop: Instant) -> Instant {
        let mut next = stop;
        let mut consider = |at: Instant| {
            if self.now < at && at < next {
                next = at;
            }
        };
        for input in &self.inputs {
            consider(input.at);
        }
        for (at, _) in &self.retractions {
            consider(*at);
        }
        for pending in self.pending.values() {
            consider(pending.due);
        }
        for state in self.rules.values() {
            for row in state.rows.values() {
                consider(row.start);
                if let Some(end) = row.end {
                    consider(end);
                }
            }
            if let Some(at) = state.change_due {
                consider(at);
            }
            if let Some(at) = state.next_at {
                consider(at);
            }
        }
        next
    }
    /// Earliest causal wakeup known at this checkpoint. The host also wakes on
    /// complete-query changes, attention facts and rule edits.
    pub fn next_due(&self) -> Option<Instant> {
        let next = self.boundary(i64::MAX);
        (next != i64::MAX).then_some(next)
    }
    fn read(&mut self, at: Instant, calendar: &impl Calendar) {
        let count = self.inputs.partition_point(|input| input.at <= at);
        let inputs: Vec<_> = self.inputs.drain(..count).collect();
        for input in inputs {
            let Some(state) = self
                .rules
                .get_mut(&rule_id(&input.rule))
                .filter(|s| s.active.version == input.rule)
            else {
                continue;
            };
            state.observed_at = Some(input.at);
            state.incomplete = !input.complete;
            if !input.complete {
                continue;
            }
            let rows: Vec<_> = input
                .rows
                .into_iter()
                .filter(|r| r.origin == Origin::Source)
                .collect();
            state.order = rows.iter().map(|r| r.occurrence.clone()).collect();
            let order = rows
                .iter()
                .filter(|row| !state.active.current() || row.current_at(at))
                .map(|r| r.occurrence.clone())
                .collect();
            state.rows = rows
                .into_iter()
                .map(|r| (occurrence_id(&r.occurrence), r))
                .collect();
            state.last_value = input.value;
            let snapshot = Snapshot {
                at: input.at,
                order,
                rows: state
                    .rows
                    .values()
                    .filter(|row| !state.active.current() || row.current_at(at))
                    .cloned()
                    .collect(),
            };
            match state.active.rule.fires {
                Fires::OnChange { .. } => {
                    if state.baseline.is_none() || !state.active.rule.enabled {
                        state.baseline = Some(snapshot.clone());
                    }
                    state.latest = Some(snapshot);
                }
                Fires::OnThreshold { value, compare } => {
                    if let Some(sample) = input.value {
                        let condition = compare.matches(sample, value);
                        if state.threshold.is_none() && condition && state.rebuilt {
                            self.reports.push(Report::RearmedLate {
                                rule: state.active.version.clone(),
                                at: input.at,
                            });
                        }
                        if state.threshold == Some(false) && condition && state.active.rule.enabled
                        {
                            let trigger = Trigger {
                                rule: state.active.version.clone(),
                                occurrence: Occurrence {
                                    id: "threshold".into(),
                                    period: input.at.to_string(),
                                },
                            };
                            let pending = Pending {
                                trigger: trigger.clone(),
                                due: at,
                                matched: input.at,
                                basis: state.rows.values().cloned().collect(),
                                change: None,
                                value: Some(sample),
                            };
                            if !self.consumed.contains(&trigger) {
                                self.pending.entry(trigger.delivery_id()).or_insert(pending);
                            }
                        }
                        state.threshold = Some(condition);
                    }
                }
                _ => {}
            }
            for row in state.rows.values() {
                let revision_baseline = state.revision_baseline
                    && row.start <= at
                    && (!state.active.current() || row.current_at(at));
                let initial_baseline = row.observed < state.started
                    && row.start <= state.started
                    && (!state.active.current() || row.current_at(state.started));
                if revision_baseline || initial_baseline {
                    self.consumed.insert(Trigger {
                        rule: state.active.version.clone(),
                        occurrence: row.occurrence.clone(),
                    });
                }
            }
            state.revision_baseline = false;
        }
        let count = self
            .retractions
            .partition_point(|(queued, _)| *queued <= at);
        for (_, delivery) in self.retractions.drain(..count).collect::<Vec<_>>() {
            self.pending.remove(&delivery);
            self.open.remove(&delivery);
            self.consumed
                .retain(|trigger| trigger.delivery_id() != delivery);
            self.reports.push(Report::Retracted { delivery, at });
        }
        for state in self.rules.values_mut() {
            let active = &state.active;
            if !active.rule.enabled {
                for row in state.rows.values() {
                    self.consumed.insert(Trigger {
                        rule: active.version.clone(),
                        occurrence: row.occurrence.clone(),
                    });
                }
                state.change_due = None;
                if let Some(latest) = &state.latest {
                    state.baseline = Some(latest.clone());
                }
                self.pending.retain(|_, p| p.trigger.rule != active.version);
                let cleared: Vec<_> = self
                    .open
                    .iter()
                    .filter(|(_, open)| open.trigger.rule == active.version)
                    .map(|(id, _)| id.clone())
                    .collect();
                for delivery in cleared {
                    self.open.remove(&delivery);
                    self.reports.push(Report::Cleared { delivery, at });
                }
            }
            if state.incomplete {
                continue;
            }
            // Effective boundaries from observed rows can change a live result
            // without another arrival. Keep the complete query's original order.
            if active.current() && matches!(active.rule.fires, Fires::OnChange { .. }) {
                let order: Vec<_> = state
                    .order
                    .iter()
                    .filter(|occurrence| {
                        state
                            .rows
                            .get(&occurrence_id(occurrence))
                            .is_some_and(|row| row.current_at(at))
                    })
                    .cloned()
                    .collect();
                if state
                    .latest
                    .as_ref()
                    .is_some_and(|latest| latest.order != order)
                {
                    state.latest = Some(Snapshot {
                        at,
                        order,
                        rows: state
                            .rows
                            .values()
                            .filter(|row| row.current_at(at))
                            .cloned()
                            .collect(),
                    });
                }
            }
            if active.current() {
                let matching = |basis: &[Row], change: Option<&Change>| {
                    current_match(
                        active,
                        &state.rows,
                        state.threshold,
                        state.latest.as_ref(),
                        basis,
                        change,
                        at,
                    )
                };
                self.pending.retain(|_, pending| {
                    if pending.trigger.rule != active.version {
                        return true;
                    }
                    let still = matching(&pending.basis, pending.change.as_ref());
                    if !still {
                        self.consumed.insert(pending.trigger.clone());
                    }
                    still
                });
                let ended: Vec<_> = self
                    .open
                    .iter()
                    .filter(|(_, open)| {
                        open.trigger.rule == active.version
                            && !matching(&open.basis, open.change.as_ref())
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                for delivery in ended {
                    self.open.remove(&delivery);
                    self.reports.push(Report::Cleared { delivery, at });
                }
            }
            match active.rule.fires {
                Fires::OnNew => {
                    for row in state.rows.values() {
                        let trigger = Trigger {
                            rule: active.version.clone(),
                            occurrence: row.occurrence.clone(),
                        };
                        if !active.rule.enabled
                            || self.consumed.contains(&trigger)
                            || row.start > at
                        {
                            continue;
                        }
                        if active.current() && !row.current_at(at) {
                            continue;
                        }
                        let due = row
                            .start
                            .max(row.observed)
                            .saturating_add(active.rule.duration.0);
                        self.pending
                            .entry(trigger.delivery_id())
                            .or_insert_with(|| Pending {
                                trigger,
                                due,
                                matched: at,
                                basis: vec![row.clone()],
                                change: None,
                                value: state.last_value,
                            });
                    }
                }
                Fires::OnChange { what, k } if active.rule.enabled => {
                    if let (Some(before), Some(after)) = (&state.baseline, &state.latest) {
                        let change = difference(&before.order, &after.order, what, k);
                        if let Some(change) = change.filter(|change| {
                            change.size() >= active.rule.min_change
                                || (what != ChangeKind::Membership
                                    && change.before != change.after
                                    && active.rule.min_change == 1)
                        }) {
                            let due = if state.reported {
                                before.at.saturating_add(HOUR)
                            } else {
                                at
                            };
                            state.change_due = Some(due);
                            if due <= at {
                                let trigger = Trigger {
                                    rule: active.version.clone(),
                                    occurrence: Occurrence {
                                        id: "change".into(),
                                        period: after.at.to_string(),
                                    },
                                };
                                self.pending
                                    .entry(trigger.delivery_id())
                                    .or_insert_with(|| Pending {
                                        trigger,
                                        due: at,
                                        matched: after.at,
                                        basis: change_basis(before, after),
                                        change: Some(change),
                                        value: state.last_value,
                                    });
                                state.baseline = Some(Snapshot {
                                    at,
                                    order: after.order.clone(),
                                    rows: after.rows.clone(),
                                });
                                state.reported = true;
                                state.change_due = None;
                            }
                        } else {
                            state.change_due = None;
                        }
                    }
                }
                Fires::At { minute } if state.next_at.is_some_and(|due| due <= at) => {
                    state.next_at = Some(calendar.next_at(at, minute));
                    if active.rule.enabled {
                        let trigger = Trigger {
                            rule: active.version.clone(),
                            occurrence: Occurrence {
                                id: "logical_day".into(),
                                period: calendar.logical_day(at),
                            },
                        };
                        if !self.consumed.contains(&trigger) {
                            self.pending
                                .entry(trigger.delivery_id())
                                .or_insert_with(|| Pending {
                                    trigger,
                                    due: at,
                                    matched: at,
                                    basis: state.rows.values().cloned().collect(),
                                    change: None,
                                    value: state.last_value,
                                });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    fn fire(&mut self, at: Instant) {
        let mut pending: Vec<_> = self
            .pending
            .values()
            .filter(|p| p.due <= at)
            .cloned()
            .collect();
        pending.sort_by(|a, b| {
            b.change
                .as_ref()
                .map_or(0, Change::size)
                .cmp(&a.change.as_ref().map_or(0, Change::size))
                .then_with(|| a.trigger.cmp(&b.trigger))
        });
        for pending in pending {
            let id = pending.trigger.delivery_id();
            let Some(state) = self.rules.get(&rule_id(&pending.trigger.rule)) else {
                self.pending.remove(&id);
                continue;
            };
            let active = &state.active;
            if state.incomplete {
                continue;
            }
            if active.version != pending.trigger.rule
                || !active.rule.enabled
                || !current_match(
                    active,
                    &state.rows,
                    state.threshold,
                    state.latest.as_ref(),
                    &pending.basis,
                    pending.change.as_ref(),
                    at,
                )
            {
                self.pending.remove(&id);
                self.consumed.insert(pending.trigger);
                continue;
            }
            self.pending.remove(&id);
            if !self.consumed.insert(pending.trigger.clone()) {
                continue;
            }
            let channel = match active.rule.deliver {
                Deliver::Notice if active.rule.interrupt => Channel::Interrupt,
                Deliver::Notice => Channel::Notice,
                Deliver::Value => Channel::Value,
                Deliver::Action => Channel::Action,
            };
            if active.current() {
                self.open.insert(
                    id.clone(),
                    Open {
                        trigger: pending.trigger.clone(),
                        basis: pending.basis.clone(),
                        change: pending.change.clone(),
                    },
                );
            }
            self.reports.push(Report::Fired(Box::new(Fired {
                id,
                trigger: pending.trigger,
                rule_name: active.rule.name.clone(),
                matched: pending.matched,
                at,
                basis: pending.basis,
                channel,
                change: pending.change,
                value: pending.value,
                scope: active.rule.scope.clone(),
                to: active.rule.to.clone(),
                author: active.author.clone(),
                quiet_hours: active.rule.quiet_hours,
                rate_limit: active.rule.rate_limit.clone(),
                current: active.current(),
                shipped: active.shipped.clone(),
            })));
        }
    }
}
/// Compare whole result sets. Scores and page boundaries do not enter here.
pub fn difference(
    before: &[Occurrence],
    after: &[Occurrence],
    what: ChangeKind,
    k: Option<usize>,
) -> Option<Change> {
    let selected_before = if what == ChangeKind::TopK {
        &before[..before.len().min(k.unwrap_or(0))]
    } else {
        before
    };
    let selected_after = if what == ChangeKind::TopK {
        &after[..after.len().min(k.unwrap_or(0))]
    } else {
        after
    };
    let old: BTreeSet<_> = selected_before.iter().cloned().collect();
    let new: BTreeSet<_> = selected_after.iter().cloned().collect();
    let changed = if what == ChangeKind::Ranking {
        selected_before != selected_after
    } else {
        old != new
    };
    changed.then(|| Change {
        what,
        joined: new.difference(&old).cloned().collect(),
        left: old.difference(&new).cloned().collect(),
        before: selected_before.to_vec(),
        after: selected_after.to_vec(),
    })
}
