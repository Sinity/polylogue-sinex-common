use crate::*;
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
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Occurrence {
    pub id: String,
    pub episode: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    Source,
    SignalDelivery,
    InjectedContext,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evaluation {
    pub rule: RuleVersion,
    pub at: Instant,
    pub complete: bool,
    pub rows: Vec<Row>,
    pub value: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Interval {
    pub start: Instant,
    pub end: Instant,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Policy {
    pub private: Vec<Interval>,
}
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Trigger {
    pub rule: RuleVersion,
    pub occurrence: Occurrence,
}
fn identity(value: &impl Serialize) -> String {
    blake3::hash(&serde_json::to_vec(value).expect("signal identity contains no non-finite values"))
        .to_hex()
        .to_string()
}
impl Trigger {
    pub fn delivery_id(&self) -> String {
        identity(self)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Notice,
    Interrupt,
    Brief,
    Value,
    Action,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Downgrade {
    RateLimit,
    InterruptBudgetSpent,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub what: ChangeKind,
    pub joined: Vec<Occurrence>,
    pub left: Vec<Occurrence>,
    pub before: Vec<Occurrence>,
    pub after: Vec<Occurrence>,
}
impl Change {
    fn size(&self) -> usize {
        self.joined.len() + self.left.len()
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub trigger: Trigger,
    pub matched: Instant,
    pub at: Instant,
    pub basis: Vec<Row>,
    pub channel: Channel,
    pub downgrade: Option<Downgrade>,
    pub change: Option<Change>,
    pub value: Option<f64>,
    pub scope: Option<String>,
}
/// A durable attempt begins unknown before effects run. Persist this record in
/// the same owner as deliveries; only a returned receipt changes its outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", content = "reason", rename_all = "snake_case")]
pub enum DeliveryOutcome {
    Unknown,
    Delivered,
    NotSent(String),
    Cancelled,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeliveryReceipt {
    pub delivery_id: String,
    pub at: Instant,
    pub route: String,
    pub outcome: DeliveryOutcome,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
    /// One instance belongs to the host deliverer, covering both products.
    pub fn admit(&mut self, at: Instant, author: &Author) -> bool {
        self.handoffs
            .retain(|previous| at.saturating_sub(*previous) < HOUR);
        if author.budget_exempt() {
            return true;
        }
        if self.handoffs.len() >= self.per_hour {
            return false;
        }
        self.handoffs.push(at);
        true
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Pending {
    trigger: Trigger,
    due: Instant,
    matched: Instant,
    basis: Vec<Row>,
    change: Option<Change>,
    value: Option<f64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Snapshot {
    at: Instant,
    order: Vec<Occurrence>,
    rows: Vec<Row>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Evaluator {
    now: Instant,
    policy: Policy,
    rules: BTreeMap<String, RuleState>,
    products: BTreeSet<String>,
    inputs: Vec<Evaluation>,
    consumed: BTreeSet<Trigger>,
    pending: BTreeMap<String, Pending>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvaluationError(pub String);
impl std::fmt::Display for EvaluationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for EvaluationError {}
fn rule_id(version: &RuleVersion) -> String {
    identity(&(&version.product, &version.id))
}
fn occurrence_id(occurrence: &Occurrence) -> String {
    identity(occurrence)
}

impl Evaluator {
    pub fn new(
        start: Instant,
        rules: Vec<ActiveRule>,
        policy: Policy,
        calendar: &impl Calendar,
    ) -> Result<Self, EvaluationError> {
        if policy.private.iter().any(|i| i.end <= i.start) {
            return Err(EvaluationError("invalid private interval".into()));
        }
        let mut evaluator = Self {
            now: start,
            policy,
            rules: BTreeMap::new(),
            products: BTreeSet::new(),
            inputs: Vec::new(),
            consumed: BTreeSet::new(),
            pending: BTreeMap::new(),
        };
        for active in rules {
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
            evaluator
                .rules
                .insert(key, Self::state(active, start, calendar));
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
    pub fn set_policy(&mut self, policy: Policy) -> Result<(), EvaluationError> {
        if policy.private.iter().any(|i| i.end <= i.start) {
            return Err(EvaluationError("invalid private interval".into()));
        }
        self.policy = policy;
        Ok(())
    }
    /// Queue observations, never effective timestamps. Already observed rows
    /// may schedule future starts/ends; unseen metadata contributes no wakeup.
    pub fn observe(&mut self, evaluation: Evaluation) -> Result<(), EvaluationError> {
        if evaluation.at < self.now {
            return Err(EvaluationError("observation precedes checkpoint".into()));
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
    /// Rule versions are insert-only. The current observed matches become the
    /// new baseline and pending work is cancelled, including quiet-held work.
    pub fn replace_rule(
        &mut self,
        active: ActiveRule,
        calendar: &impl Calendar,
    ) -> Result<(), EvaluationError> {
        let key = rule_id(&active.version);
        if self
            .rules
            .get(&key)
            .is_some_and(|old| active.version.revision <= old.active.version.revision)
        {
            return Err(EvaluationError("rule edits require a new revision".into()));
        }
        self.products.insert(active.version.product.clone());
        let mut state = Self::state(active, self.now, calendar);
        // A revised query can read a different relation. Its first complete
        // evaluation, rather than the previous query's rows, is its baseline.
        state.revision_baseline = true;
        state.incomplete = true;
        self.pending
            .retain(|_, pending| rule_id(&pending.trigger.rule) != key);
        self.inputs
            .retain(|input| rule_id(&input.rule) != key || input.rule == state.active.version);
        self.rules.insert(key, state);
        Ok(())
    }
    pub fn withdraw(&mut self, version: &RuleVersion) {
        let key = rule_id(version);
        self.rules.remove(&key);
        self.pending
            .retain(|_, pending| rule_id(&pending.trigger.rule) != key);
        self.inputs.retain(|input| rule_id(&input.rule) != key);
    }
    pub fn advance(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        host: &mut HostPolicy,
    ) -> Result<Vec<Delivery>, EvaluationError> {
        Ok(self
            .advance_in_mode(
                stop,
                calendar,
                EvaluationMode::Local {
                    host,
                    forwarded: &|_| Condition::Incomplete,
                },
            )?
            .deliveries)
    }
    pub fn resume(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        host: &mut HostPolicy,
    ) -> Result<Vec<Delivery>, EvaluationError> {
        Ok(self
            .run(
                stop,
                calendar,
                EvaluationMode::Local {
                    host,
                    forwarded: &|_| Condition::Incomplete,
                },
                true,
            )?
            .deliveries)
    }
    pub fn advance_in_mode(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        mode: EvaluationMode<'_>,
    ) -> Result<EvaluationOutput, EvaluationError> {
        self.run(stop, calendar, mode, false)
    }
    pub fn resume_in_mode(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        mode: EvaluationMode<'_>,
    ) -> Result<EvaluationOutput, EvaluationError> {
        self.run(stop, calendar, mode, true)
    }
    pub fn condition(&self, intent: &FiredIntent, at: Instant) -> Condition {
        if self.private(at) {
            return Condition::Ended;
        }
        let Some(state) = self.rules.get(&rule_id(&intent.delivery.trigger.rule)) else {
            return Condition::Ended;
        };
        if state.active.version != intent.delivery.trigger.rule || !state.active.rule.enabled {
            return Condition::Ended;
        }
        if state.incomplete {
            return Condition::Incomplete;
        }
        if current_match(
            &state.active,
            &state.rows,
            state.threshold,
            state.latest.as_ref(),
            &intent.delivery.basis,
            intent.delivery.change.as_ref(),
            at,
        ) {
            Condition::Matching
        } else {
            Condition::Ended
        }
    }
    fn run(
        &mut self,
        stop: Instant,
        calendar: &impl Calendar,
        mut mode: EvaluationMode<'_>,
        resume: bool,
    ) -> Result<EvaluationOutput, EvaluationError> {
        if stop < self.now {
            return Err(EvaluationError("clock must be monotone".into()));
        }
        let mut output = EvaluationOutput::default();
        loop {
            self.read(self.now, calendar);
            if !resume || self.now == stop {
                let intents = self.fire(self.now);
                match &mut mode {
                    EvaluationMode::Local { host, forwarded } => {
                        for intent in intents {
                            host.submit(intent, self.now);
                        }
                        output
                            .deliveries
                            .extend(host.advance(self.now, calendar, |intent| {
                                if self
                                    .products
                                    .contains(&intent.delivery.trigger.rule.product)
                                {
                                    self.condition(intent, self.now)
                                } else {
                                    forwarded(intent)
                                }
                            }));
                    }
                    EvaluationMode::Forward => output.intents.extend(intents),
                }
            } else if let EvaluationMode::Local { host, forwarded } = &mut mode {
                host.fold(|intent| {
                    if self
                        .products
                        .contains(&intent.delivery.trigger.rule.product)
                    {
                        self.condition(intent, self.now)
                    } else {
                        forwarded(intent)
                    }
                });
            }
            if self.now == stop {
                break;
            }
            let next = self.boundary(stop);
            self.now = match &mode {
                EvaluationMode::Local { host, .. } => host
                    .next_due()
                    .filter(|due| *due > self.now)
                    .map_or(next, |due| due.min(next)),
                EvaluationMode::Forward => next,
            };
        }
        Ok(output)
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
        for pending in self.pending.values() {
            consider(pending.due);
        }
        for interval in &self.policy.private {
            consider(interval.start);
            consider(interval.end);
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
    /// complete-query changes, policy edits and rule edits.
    pub fn next_due(&self) -> Option<Instant> {
        let next = self.boundary(i64::MAX);
        (next != i64::MAX).then_some(next)
    }
    fn private(&self, at: Instant) -> bool {
        self.policy
            .private
            .iter()
            .any(|interval| interval.start <= at && at < interval.end)
    }
    fn read(&mut self, at: Instant, calendar: &impl Calendar) {
        let private = self.private(at);
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
                    if state.baseline.is_none() || private || !state.active.rule.enabled {
                        state.baseline = Some(snapshot.clone());
                    }
                    state.latest = Some(snapshot);
                }
                Fires::OnThreshold { value, compare } => {
                    if let Some(sample) = input.value {
                        let condition = compare.matches(sample, value);
                        if state.threshold == Some(false)
                            && condition
                            && !private
                            && state.active.rule.enabled
                        {
                            let trigger = Trigger {
                                rule: state.active.version.clone(),
                                occurrence: Occurrence {
                                    id: "threshold".into(),
                                    episode: input.at.to_string(),
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
        if private {
            for pending in self.pending.values() {
                self.consumed.insert(pending.trigger.clone());
            }
            self.pending.clear();
        }
        for state in self.rules.values_mut() {
            let active = &state.active;
            if private || !active.rule.enabled {
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
                self.pending.retain(|_, pending| {
                    if pending.trigger.rule != active.version {
                        return true;
                    }
                    let matching = current_match(
                        active,
                        &state.rows,
                        state.threshold,
                        state.latest.as_ref(),
                        &pending.basis,
                        pending.change.as_ref(),
                        at,
                    );
                    if !matching {
                        self.consumed.insert(pending.trigger.clone());
                    }
                    matching
                });
            }
            match active.rule.fires {
                Fires::OnNew => {
                    for row in state.rows.values() {
                        let trigger = Trigger {
                            rule: active.version.clone(),
                            occurrence: row.occurrence.clone(),
                        };
                        if private
                            || !active.rule.enabled
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
                Fires::OnChange { what, k } if !private && active.rule.enabled => {
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
                                        episode: after.at.to_string(),
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
                Fires::At { minute } => {
                    if state.next_at.is_some_and(|due| due <= at) {
                        state.next_at = Some(calendar.next_at(at, minute));
                        if !private && active.rule.enabled {
                            let trigger = Trigger {
                                rule: active.version.clone(),
                                occurrence: Occurrence {
                                    id: "logical_day".into(),
                                    episode: calendar.logical_day(at),
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
                }
                _ => {}
            }
        }
    }
    fn fire(&mut self, at: Instant) -> Vec<FiredIntent> {
        if self.private(at) {
            return Vec::new();
        }
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
        let mut intents = Vec::new();
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
            intents.push(FiredIntent {
                delivery: Delivery {
                    id,
                    trigger: pending.trigger,
                    matched: pending.matched,
                    at,
                    basis: pending.basis,
                    channel,
                    downgrade: None,
                    change: pending.change,
                    value: pending.value,
                    scope: active.rule.scope.clone(),
                },
                author: active.author.clone(),
                quiet_hours: active.rule.quiet_hours,
                rate_limit: active.rule.rate_limit.clone(),
                current: active.current(),
            });
        }
        intents
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
