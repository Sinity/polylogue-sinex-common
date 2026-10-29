//! The final host handoff, shared by local and forwarded fired intents.
use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// A firing is an intent until the host's quiet hours and attention budget have
/// been applied. Authorship comes from the authenticated product write path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FiredIntent {
    pub delivery: Delivery,
    pub author: Author,
    pub quiet_hours: Option<QuietHours>,
    pub rate_limit: Option<RateLimit>,
    pub current: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    Matching,
    Ended,
    Incomplete,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Held {
    intent: FiredIntent,
    due: Instant,
}

/// One value belongs to the host deliverer, including all local and forwarded
/// product intents. Persist it atomically with emitted delivery records.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HostPolicy {
    pub quiet_hours: Option<QuietHours>,
    pub budget: InterruptBudget,
    pending: BTreeMap<String, Held>,
    consumed: BTreeSet<String>,
    sent: BTreeMap<String, Vec<Instant>>,
}
impl HostPolicy {
    pub fn new(budget: InterruptBudget, quiet_hours: Option<QuietHours>) -> Self {
        Self {
            budget,
            quiet_hours,
            ..Self::default()
        }
    }
    /// Duplicate transport submissions preserve the original match-time basis.
    pub fn submit(&mut self, intent: FiredIntent, at: Instant) {
        if !self.consumed.contains(&intent.delivery.id) {
            self.pending
                .entry(intent.delivery.id.clone())
                .or_insert(Held { intent, due: at });
        }
    }
    pub fn next_due(&self) -> Option<Instant> {
        self.pending.values().map(|held| held.due).min()
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    /// Fold policy boundaries during downtime without effects or budget use.
    pub fn fold(&mut self, check: impl Fn(&FiredIntent) -> Condition) {
        self.pending.retain(|id, held| {
            if check(&held.intent) == Condition::Ended {
                self.consumed.insert(id.clone());
                false
            } else {
                true
            }
        });
    }
    /// Recheck the active revision, current condition and source freshness via
    /// the product adapter before the one final host policy decision.
    pub fn advance(
        &mut self,
        at: Instant,
        calendar: &impl Calendar,
        check: impl Fn(&FiredIntent) -> Condition,
    ) -> Vec<Delivery> {
        self.fold(&check);
        let mut intents: Vec<_> = self.pending.values().cloned().collect();
        intents.sort_by(|a, b| {
            let size = |held: &Held| {
                held.intent
                    .delivery
                    .change
                    .as_ref()
                    .map_or(0, |c| c.joined.len() + c.left.len())
            };
            size(b)
                .cmp(&size(a))
                .then_with(|| a.intent.delivery.trigger.cmp(&b.intent.delivery.trigger))
        });
        let mut deliveries = Vec::new();
        for held in intents {
            let intent = held.intent;
            if check(&intent) != Condition::Matching {
                continue;
            }
            let id = intent.delivery.id.clone();
            if intent.delivery.channel == Channel::Interrupt
                && let Some(hours) = intent.quiet_hours.or(self.quiet_hours)
                && let Some(until) = calendar.quiet_until(at, hours)
            {
                self.pending.get_mut(&id).expect("pending host intent").due = until;
                continue;
            }
            self.pending.remove(&id);
            if !self.consumed.insert(id) {
                continue;
            }
            let mut delivery = intent.delivery;
            delivery.at = at;
            let key =
                serde_json::to_string(&(&delivery.trigger.rule.product, &delivery.trigger.rule.id))
                    .expect("rule key");
            let sent = self.sent.entry(key).or_default();
            if let Some(limit) = intent.rate_limit.as_ref() {
                sent.retain(|sent| at.saturating_sub(*sent) < limit.window.0);
            }
            if intent
                .rate_limit
                .as_ref()
                .is_some_and(|limit| sent.len() >= limit.count)
            {
                delivery.channel = Channel::Brief;
                delivery.downgrade = Some(Downgrade::RateLimit);
            } else {
                if delivery.channel == Channel::Interrupt && !self.budget.admit(at, &intent.author)
                {
                    delivery.channel = Channel::Notice;
                    delivery.downgrade = Some(Downgrade::InterruptBudgetSpent);
                }
                sent.push(at);
            }
            deliveries.push(delivery);
        }
        deliveries
    }
}

/// Selected by trusted daemon configuration, never deserialized from a request.
/// Primary-mode Polylogue forwards intents; Sinex owns the Local host policy.
pub enum EvaluationMode<'a> {
    Local {
        host: &'a mut HostPolicy,
        forwarded: &'a dyn Fn(&FiredIntent) -> Condition,
    },
    Forward,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EvaluationOutput {
    pub deliveries: Vec<Delivery>,
    pub intents: Vec<FiredIntent>,
}
