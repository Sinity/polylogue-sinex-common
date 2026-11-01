use super::*;
use std::collections::BTreeSet;
impl NotificationState {
    pub fn attempts(&self) -> &BTreeMap<String, Attempt> {
        &self.attempts
    }
    pub fn attempt(&self, id: &str) -> Option<&Attempt> {
        self.attempts.get(id)
    }
    pub fn server_owner(&self) -> Option<&str> {
        self.server_owner.as_deref()
    }
    pub fn set_server_owner(&mut self, owner: Option<String>) {
        if self.server_owner != owner {
            self.invalidate_server();
            self.server_owner = owner;
        }
    }
    /// An already-shown group update is a new replace operation, whose unknown
    /// checkpoint must also be committed before touching the platform.
    pub fn prepare_refresh(&mut self, id: &str) -> Option<NotificationReceipt> {
        let a = self.attempts.get_mut(id)?;
        if !a.active || a.desktop != Some(DeliveryOutcome::Delivered) {
            return None;
        }
        a.desktop = Some(DeliveryOutcome::Unknown);
        Some(receipt(a, Outlet::Desktop))
    }
    pub fn presentation(&mut self, notice: &Notice, actions: bool) -> Presentation {
        let slot = self.slot(notice);
        let critical = notice.critical
            || (slot.starts_with("summary:")
                && self.attempts.values().any(|a| {
                    a.active
                        && a.notice.critical
                        && a.notice.product == notice.product
                        && a.notice.rule == notice.rule
                }));
        let summary = if slot.starts_with("summary:") {
            format!(
                "{}: {} open occurrences",
                notice.summary,
                self.open_occurrences(notice).len()
            )
        } else {
            notice.summary.clone()
        };
        let mut body = notice.body.clone();
        for line in &notice.lines {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("└ ");
            body.push_str(line);
        }
        if !actions && !notice.command_hint.is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&notice.command_hint);
        }
        Presentation {
            replaces: self.slots.get(&slot).copied().unwrap_or(0),
            slot,
            summary,
            body,
            critical,
        }
    }
    pub fn completed(
        &mut self,
        id: &str,
        outlet: Outlet,
        completion: Completion,
    ) -> NotificationReceipt {
        let Completion {
            outcome,
            notification_id,
            server_owner,
            slot,
        } = completion;
        let notice = self.attempts[id].notice.clone();
        if outcome == DeliveryOutcome::Delivered && outlet == Outlet::Desktop {
            self.replaced(&notice);
        }
        if let (Some(id), Some(slot)) = (notification_id, slot.as_ref()) {
            self.slots.insert(slot.clone(), id);
            self.shown_delivery.insert(id, notice.delivery_id.clone());
            for a in self
                .attempts
                .values_mut()
                .filter(|a| a.notification_id == Some(id))
            {
                a.shown = false;
            }
        }
        let attempt = self.attempts.get_mut(id).expect("existing attempt");
        if outlet == Outlet::Desktop {
            attempt.desktop = Some(outcome.clone());
            attempt.notification_id = notification_id;
            attempt.notification_slot = slot;
            attempt.shown = outcome == DeliveryOutcome::Delivered && notification_id.is_some();
            attempt.server_owner = server_owner;
        } else {
            attempt.command = Some(outcome);
        }
        receipt(attempt, outlet)
    }
    /// The condition ended or the operator acted: stop this occurrence and
    /// close its notification unless another active notice still shows there.
    pub fn deactivate(&mut self, id: &str) -> Cancellation {
        let Some(notice) = self.attempts.get(id).map(|a| a.notice.clone()) else {
            return Cancellation::default();
        };
        let mut result = Cancellation::default();
        let mut ids = BTreeSet::new();
        for a in self
            .attempts
            .values_mut()
            .filter(|a| same_occurrence(&a.notice, &notice))
        {
            a.active = false;
            result.deactivated.push(a.notice.delivery_id.clone());
            if let Some(id) = a.notification_id {
                ids.insert(id);
            }
            for outlet in [Outlet::Desktop, Outlet::Command] {
                let outcome = if outlet == Outlet::Desktop {
                    &mut a.desktop
                } else {
                    &mut a.command
                };
                if outcome.is_some() && *outcome != Some(DeliveryOutcome::Delivered) {
                    *outcome = Some(DeliveryOutcome::Cancelled);
                    result.receipts.push(receipt(a, outlet));
                }
            }
        }
        for id in ids {
            if !self
                .attempts
                .values()
                .any(|a| a.active && a.notification_id == Some(id))
            {
                self.slots.retain(|_, v| *v != id);
                self.shown_delivery.remove(&id);
                result.close.push(id);
            }
        }
        result
    }
    /// After an occurrence of a rule closes: promote the next open occurrence
    /// into the rule's first notification, refresh or close its summary.
    pub fn group_update(&mut self, notice: &Notice) -> GroupUpdate {
        let mut result = GroupUpdate::default();
        let group = Self::group(notice);
        let open = self.open_occurrences(notice);
        let first = self.first_occurrence.get(&group).cloned();
        let mut promoted = None;
        if first.as_ref().is_some_and(|f| !open.contains(f)) {
            self.first_occurrence.remove(&group);
            self.slots.remove(&format!("occurrence:{group}"));
            if let Some(next) = open.first() {
                self.first_occurrence.insert(group.clone(), next.clone());
                if let Some(id) = self.slots.remove(&format!("summary:{group}")) {
                    let promoted_slot = format!("occurrence:{group}");
                    self.slots.insert(promoted_slot.clone(), id);
                    for a in self
                        .attempts
                        .values_mut()
                        .filter(|a| a.active && a.notification_id == Some(id))
                    {
                        a.notification_slot = Some(promoted_slot.clone());
                    }
                    promoted = Some(next.clone());
                }
            }
        }
        let eligible = |a: &&Attempt| {
            a.active
                && a.desktop == Some(DeliveryOutcome::Delivered)
                && a.notice.product == notice.product
                && a.notice.rule == notice.rule
        };
        if let Some(promoted) = promoted
            && let Some(a) = self
                .attempts
                .values()
                .filter(eligible)
                .find(|a| a.notice.occurrence == promoted)
        {
            result.notices.push(a.notice.clone());
        }
        if open.len() <= 1 {
            if let Some(id) = self.slots.remove(&format!("summary:{group}")) {
                self.shown_delivery.remove(&id);
                result.close.push(id);
            }
        } else if let Some(a) = self
            .attempts
            .values()
            .filter(eligible)
            .find(|a| Some(&a.notice.occurrence) != self.first_occurrence.get(&group))
        {
            result.notices.push(a.notice.clone());
        }
        result
    }
    /// Records unknown for each outlet before any effect. A later showing of
    /// the same delivery (a snooze ending) replaces its attempt.
    pub fn begin(&mut self, notice: Notice) -> Vec<NotificationReceipt> {
        if self
            .attempts
            .get(&notice.delivery_id)
            .is_some_and(|existing| existing.notice.showing >= notice.showing)
            || notice.outlets == Outlets::default()
        {
            return Vec::new();
        }
        let mut attempt = Attempt {
            desktop: notice.outlets.desktop.then_some(DeliveryOutcome::Unknown),
            command: notice.outlets.command.then_some(DeliveryOutcome::Unknown),
            notice,
            notification_id: None,
            notification_slot: None,
            shown: false,
            retried: false,
            active: true,
            server_owner: None,
        };
        if let Some(previous) = self.attempts.get(&attempt.notice.delivery_id) {
            attempt.notification_id = previous.notification_id;
            attempt.notification_slot = previous.notification_slot.clone();
        }
        let mut receipts = Vec::new();
        if attempt.desktop.is_some() {
            receipts.push(receipt(&attempt, Outlet::Desktop));
        }
        if attempt.command.is_some() {
            receipts.push(receipt(&attempt, Outlet::Command));
        }
        self.attempts
            .insert(attempt.notice.delivery_id.clone(), attempt);
        receipts
    }
    /// Called when the notification service appears. Each explicit desktop
    /// failure is retried once under its identity; unknown never is.
    pub fn server_appeared(&mut self) -> Vec<NotificationReceipt> {
        self.invalidate_server();
        let mut receipts = Vec::new();
        for attempt in self.attempts.values_mut() {
            if attempt.active
                && !attempt.retried
                && matches!(&attempt.desktop, Some(DeliveryOutcome::NotSent(_)))
            {
                attempt.retried = true;
                attempt.desktop = Some(DeliveryOutcome::Unknown);
                receipts.push(receipt(attempt, Outlet::Desktop));
            }
        }
        receipts
    }
    pub fn invalidate_server(&mut self) {
        self.slots.clear();
        self.shown_delivery.clear();
        for attempt in self.attempts.values_mut() {
            attempt.notification_id = None;
            attempt.shown = false;
        }
    }
    pub(super) fn group(notice: &Notice) -> String {
        // JSON tuples keep product/rule separators unambiguous.
        serde_json::to_string(&(&notice.product, &notice.rule)).expect("string tuple")
    }
    pub(super) fn open_occurrences(&self, notice: &Notice) -> BTreeSet<String> {
        self.attempts
            .values()
            .filter(|a| {
                a.active && a.notice.product == notice.product && a.notice.rule == notice.rule
            })
            .map(|a| a.notice.occurrence.clone())
            .collect()
    }
    pub(super) fn slot(&mut self, notice: &Notice) -> String {
        let group = Self::group(notice);
        let first = self
            .first_occurrence
            .entry(group.clone())
            .or_insert_with(|| notice.occurrence.clone());
        if *first == notice.occurrence {
            format!("occurrence:{group}")
        } else {
            format!("summary:{group}")
        }
    }
    pub(super) fn replaced(&mut self, notice: &Notice) {
        for prior in self.attempts.values_mut().filter(|a| {
            a.notice.delivery_id != notice.delivery_id && same_occurrence(&a.notice, notice)
        }) {
            prior.active = false;
        }
    }
    pub fn desktop_event(
        &mut self,
        notification_id: u32,
        event: DesktopEvent,
    ) -> Vec<NotificationReceipt> {
        let mut receipts = Vec::new();
        for attempt in self
            .attempts
            .values_mut()
            .filter(|a| a.active && a.notification_id == Some(notification_id))
        {
            let event = match &event {
                DesktopEvent::OpenReference {
                    activation_token, ..
                } if self.shown_delivery.get(&notification_id)
                    == Some(&attempt.notice.delivery_id) =>
                {
                    DesktopEvent::OpenReference {
                        reference: attempt.notice.reference.clone(),
                        activation_token: activation_token.clone(),
                    }
                }
                DesktopEvent::OpenReference { .. } => DesktopEvent::Seen,
                event => event.clone(),
            };
            if matches!(
                event,
                DesktopEvent::Dismissed | DesktopEvent::Expired | DesktopEvent::Closed
            ) {
                attempt.active = false;
            }
            let mut receipt = receipt(attempt, Outlet::Desktop);
            receipt.event = Some(event);
            receipt.via = Some("desktop".into());
            receipts.push(receipt);
        }
        if matches!(
            event,
            DesktopEvent::Dismissed | DesktopEvent::Expired | DesktopEvent::Closed
        ) {
            self.slots.retain(|_, id| *id != notification_id);
            self.shown_delivery.remove(&notification_id);
            self.first_occurrence.retain(|group, occurrence| {
                self.attempts.values().any(|a| {
                    a.active
                        && Self::group(&a.notice) == *group
                        && a.notice.occurrence == *occurrence
                })
            });
        }
        receipts
    }
}
fn same_occurrence(a: &Notice, b: &Notice) -> bool {
    a.product == b.product && a.rule == b.rule && a.occurrence == b.occurrence
}

fn receipt(attempt: &Attempt, outlet: Outlet) -> NotificationReceipt {
    NotificationReceipt {
        delivery_id: attempt.notice.delivery_id.clone(),
        outlet,
        outcome: if outlet == Outlet::Desktop {
            attempt.desktop.clone()
        } else {
            attempt.command.clone()
        }
        .unwrap_or(DeliveryOutcome::Unknown),
        notification_id: attempt.notification_id,
        event: None,
        via: None,
    }
}

#[derive(Clone, Debug, Default)]
pub struct Cancellation {
    pub receipts: Vec<NotificationReceipt>,
    pub close: Vec<u32>,
    pub deactivated: Vec<String>,
}
#[derive(Clone, Debug, Default)]
pub struct GroupUpdate {
    pub notices: Vec<Notice>,
    pub close: Vec<u32>,
}
#[derive(Clone, Debug)]
pub struct Presentation {
    pub slot: String,
    pub replaces: u32,
    pub summary: String,
    pub body: String,
    pub critical: bool,
}

/// The platform result, applied without selecting a new effect.
pub struct Completion {
    pub outcome: DeliveryOutcome,
    pub notification_id: Option<u32>,
    pub server_owner: Option<String>,
    pub slot: Option<String>,
}
