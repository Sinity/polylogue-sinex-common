use super::*;
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
    /// Import an immutable handoff without creating a new attempt. Repeating the
    /// same snapshot after local progress preserves that progress.
    pub fn import_attempt(&mut self, mut attempt: Attempt) -> Result<ImportStatus, ImportError> {
        attempt.handoff = None;
        let identity = blake3::hash(&serde_json::to_vec(&attempt).expect("attempt JSON"))
            .to_hex()
            .to_string();
        if let Some(existing) = self.attempts.get(&attempt.notice.delivery_id) {
            return if existing.handoff.as_ref() == Some(&identity) {
                Ok(ImportStatus::Known)
            } else {
                Err(ImportError::Conflict)
            };
        }
        attempt.handoff = Some(identity);
        if attempt.server_owner.as_deref() != self.server_owner() {
            attempt.notification_id = None;
            attempt.shown = false;
        }
        if attempt.active
            && let (Some(id), Some(slot)) =
                (attempt.notification_id, attempt.notification_slot.clone())
        {
            if slot.starts_with("occurrence:") {
                self.first_occurrence.insert(
                    Self::group(&attempt.notice),
                    attempt.notice.occurrence.clone(),
                );
            }
            self.slots.insert(slot, id);
            if attempt.shown {
                self.shown_delivery
                    .insert(id, attempt.notice.delivery_id.clone());
            }
        }
        self.attempts
            .insert(attempt.notice.delivery_id.clone(), attempt);
        Ok(ImportStatus::Imported)
    }
    /// Stops this owner without rewriting the frozen handoff outcomes.
    pub fn release(&mut self, id: &str) -> Option<Attempt> {
        let attempt = self.attempts.get_mut(id)?;
        if !attempt.active {
            return None;
        }
        let original = attempt.clone();
        attempt.active = false;
        Some(original)
    }
    pub fn prepare_retry(&mut self, id: &str) -> Vec<NotificationReceipt> {
        let Some(attempt) = self.attempts.get_mut(id) else {
            return Vec::new();
        };
        if !attempt.active
            || attempt.retried
            || !matches!(attempt.dbus, Some(DeliveryOutcome::NotSent(_)))
        {
            return Vec::new();
        }
        attempt.retried = true;
        attempt.dbus = Some(DeliveryOutcome::Unknown);
        vec![receipt(attempt, Route::Dbus)]
    }
    pub fn group_members(&self, notice: &Notice) -> Vec<Notice> {
        self.attempts
            .values()
            .filter(|a| {
                a.active
                    && a.notice.product == notice.product
                    && a.notice.rule == notice.rule
                    && a.notice.budget_passive_count.is_some()
                        == notice.budget_passive_count.is_some()
            })
            .map(|a| a.notice.clone())
            .collect()
    }
    pub fn presentation(&mut self, notice: &Notice, actions: bool) -> Presentation {
        let slot = self.slot(notice);
        let data_loss = notice.data_loss
            || (slot.starts_with("summary:")
                && self.attempts.values().any(|a| {
                    a.active
                        && a.notice.budget_passive_count.is_none()
                        && a.notice.data_loss
                        && a.notice.product == notice.product
                        && a.notice.rule == notice.rule
                }));
        let summary = if let Some(count) = notice.budget_passive_count {
            format!("{count} notices held passive (interrupt budget spent)")
        } else if slot.starts_with("summary:") {
            format!(
                "{}: {} open occurrences",
                notice.summary,
                self.open_occurrences(notice).len()
            )
        } else {
            notice.summary.clone()
        };
        let mut body = notice.body.clone();
        if !actions && !notice.command_hint.is_empty() {
            body.push('\n');
            body.push_str(&notice.command_hint);
        }
        Presentation {
            replaces: self.slots.get(&slot).copied().unwrap_or(0),
            slot,
            summary,
            body,
            data_loss,
        }
    }
    pub fn completed(
        &mut self,
        id: &str,
        route: Route,
        completion: Completion,
    ) -> NotificationReceipt {
        let Completion {
            outcome,
            notification_id,
            server_owner,
            slot,
        } = completion;
        let notice = self.attempts[id].notice.clone();
        if outcome == DeliveryOutcome::Delivered && route == Route::Dbus {
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
        if route == Route::Dbus {
            attempt.dbus = Some(outcome.clone());
            attempt.notification_id = notification_id;
            attempt.notification_slot = slot;
            attempt.shown = outcome == DeliveryOutcome::Delivered && notification_id.is_some();
            attempt.server_owner = server_owner;
        } else {
            attempt.command = Some(outcome);
        }
        receipt(attempt, route)
    }
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
            for route in [Route::Dbus, Route::Command] {
                let outcome = if route == Route::Dbus {
                    &mut a.dbus
                } else {
                    &mut a.command
                };
                if outcome.is_some() && *outcome != Some(DeliveryOutcome::Delivered) {
                    *outcome = Some(DeliveryOutcome::Cancelled);
                    result.receipts.push(receipt(a, route));
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
    pub fn group_update(&mut self, notice: &Notice) -> GroupUpdate {
        let mut result = GroupUpdate::default();
        if notice.budget_passive_count.is_some() {
            return result;
        }
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
                && a.dbus == Some(DeliveryOutcome::Delivered)
                && a.notice.budget_passive_count.is_none()
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
    pub fn begin(&mut self, notice: Notice, route: Route) -> Vec<NotificationReceipt> {
        if self.attempts.contains_key(&notice.delivery_id)
            || (!notice.interrupt && notice.budget_passive_count.is_none())
        {
            return Vec::new();
        }
        let mut attempt = Attempt {
            notice,
            dbus: None,
            command: None,
            notification_id: None,
            notification_slot: None,
            shown: false,
            retried: false,
            active: true,
            server_owner: None,
            handoff: None,
        };
        let mut receipts = Vec::new();
        if matches!(route, Route::Dbus | Route::Both) {
            attempt.dbus = Some(DeliveryOutcome::Unknown);
            receipts.push(receipt(&attempt, Route::Dbus));
        }
        if matches!(route, Route::Command | Route::Both) {
            attempt.command = Some(DeliveryOutcome::Unknown);
            receipts.push(receipt(&attempt, Route::Command));
        }
        self.attempts
            .insert(attempt.notice.delivery_id.clone(), attempt);
        receipts
    }
    pub fn server_appeared(&mut self) -> Vec<NotificationReceipt> {
        self.invalidate_server();
        let mut receipts = Vec::new();
        for attempt in self.attempts.values_mut() {
            if attempt.active
                && !attempt.retried
                && matches!(&attempt.dbus, Some(DeliveryOutcome::NotSent(_)))
            {
                attempt.retried = true;
                attempt.dbus = Some(DeliveryOutcome::Unknown);
                receipts.push(receipt(attempt, Route::Dbus));
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
        // JSON tuples keep product/rule/occurrence separators unambiguous.
        serde_json::to_string(&(&notice.product, &notice.rule)).expect("string tuple")
    }
    pub(super) fn open_occurrences(&self, notice: &Notice) -> BTreeSet<String> {
        self.attempts
            .values()
            .filter(|a| {
                a.active
                    && a.notice.budget_passive_count.is_none()
                    && a.notice.product == notice.product
                    && a.notice.rule == notice.rule
            })
            .map(|a| a.notice.occurrence.clone())
            .collect()
    }
    pub(super) fn slot(&mut self, notice: &Notice) -> String {
        if notice.budget_passive_count.is_some() {
            return "budget".into();
        }
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
                DesktopEvent::OpenReference { .. }
                    if self.shown_delivery.get(&notification_id)
                        == Some(&attempt.notice.delivery_id) =>
                {
                    DesktopEvent::OpenReference {
                        reference: attempt.notice.reference.clone(),
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
            let mut receipt = receipt(attempt, Route::Dbus);
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
    a.product == b.product
        && a.rule == b.rule
        && a.occurrence == b.occurrence
        && a.budget_passive_count.is_some() == b.budget_passive_count.is_some()
}

fn receipt(attempt: &Attempt, route: Route) -> NotificationReceipt {
    NotificationReceipt {
        delivery_id: attempt.notice.delivery_id.clone(),
        route,
        outcome: if route == Route::Dbus {
            attempt.dbus.clone()
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
    pub data_loss: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportStatus {
    Imported,
    Known,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportError {
    Conflict,
}

/// The platform result, applied without selecting a new effect.
pub struct Completion {
    pub outcome: DeliveryOutcome,
    pub notification_id: Option<u32>,
    pub server_owner: Option<String>,
    pub slot: Option<String>,
}
