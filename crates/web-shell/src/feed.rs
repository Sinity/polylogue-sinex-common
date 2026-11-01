//! Live subscriptions over one event connection per browser.
//!
//! Every tab's `<shell-feed>` elements subscribe through the shell's feed hub,
//! which runs in a SharedWorker and holds one event stream per product route
//! for the whole browser profile. The stream carries the union of the tabs'
//! subscriptions; each subscription has a hub-assigned ID and resumes from its
//! own position, and every event names the subscription it belongs to.
use crate::FragmentUpdate;
use command_surface::{
    CommittedEvent, EventLog, Failure, FailureKind, RELOAD_EVENT, Resume, SseEvent,
};
use maud::{Markup, html};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use ts_rs::TS;

/// The query parameter of the union stream route holding the JSON subscriptions.
pub const SUBSCRIPTIONS_PARAMETER: &str = "subscriptions";
/// The SSE event name of a fragment for one subscription.
pub const FRAGMENT_EVENT: &str = "fragment";

/// One followed reference, resumed strictly after `since` (an event ID).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct Subscription {
    pub id: String,
    /// What is followed: a reference, a saved query or a product stream name
    /// such as `waiting`.
    pub reference: String,
    pub since: String,
}

fn bad_request(message: impl Into<String>) -> Failure {
    Failure::new(FailureKind::BadRequest, message)
}

/// Parse and check the `subscriptions` parameter of a union stream request.
pub fn subscriptions(parameter: &str) -> Result<Vec<Subscription>, Failure> {
    let list: Vec<Subscription> = serde_json::from_str(parameter)
        .map_err(|error| bad_request(format!("invalid subscriptions: {error}")))?;
    let mut seen = BTreeSet::new();
    for subscription in &list {
        if subscription.id.is_empty() || subscription.reference.is_empty() {
            return Err(bad_request("a subscription needs an ID and a reference"));
        }
        if !seen.insert(subscription.id.as_str()) {
            return Err(bad_request(format!(
                "subscription {} appears twice",
                subscription.id
            )));
        }
    }
    Ok(list)
}

/// A fragment for one subscription.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FeedEvent {
    pub subscription: String,
    pub update: FragmentUpdate,
}

/// The shared `reload` event: one subscription, or the whole stream when
/// `subscription` is absent. The tabs concerned stop following and say why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
pub struct FeedReload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription: Option<String>,
    pub reason: String,
}
impl FeedReload {
    /// Also the product's answer to a subscription it cannot evaluate.
    pub fn refuse(subscription: &Subscription, reason: impl Into<String>) -> Self {
        Self {
            subscription: Some(subscription.id.clone()),
            reason: reason.into(),
        }
    }
    pub fn encode(&self) -> Result<String, Failure> {
        SseEvent {
            id: None,
            event: RELOAD_EVENT.into(),
            data: serde_json::to_string(self).expect("reload strings serialize"),
            retry: None,
        }
        .encode()
    }
}

/// The opening of a union stream: for each subscription, the retained events
/// after its own position that concern it, in commit order; a subscription
/// whose position is no longer retained gets a `reload` instead. `updates`
/// is the product's filter: the fragments one committed value gives one
/// subscription (none when the value does not concern it).
pub fn catch_up<T>(
    log: &EventLog<T>,
    subscriptions: &[Subscription],
    updates: impl Fn(&Subscription, &T) -> Vec<FragmentUpdate>,
) -> Result<String, Failure> {
    let mut stream = String::new();
    let mut following = Vec::new();
    for subscription in subscriptions {
        match log.resume(Some(subscription.since.as_str())) {
            Resume::Events(events) => following.push((subscription, events)),
            Resume::Reload { reason } => {
                stream.push_str(&FeedReload::refuse(subscription, reason).encode()?)
            }
        }
    }
    // Every resumed suffix ends at the log's newest event, so the longest one
    // orders them all; a subscription receives the last `len` events of it.
    let longest = following
        .iter()
        .map(|(_, events)| events.as_slice())
        .max_by_key(|events| events.len())
        .unwrap_or(&[]);
    for (index, event) in longest.iter().enumerate() {
        let remaining = longest.len() - index;
        for (subscription, events) in &following {
            if events.len() >= remaining {
                stream.push_str(&encode(subscription, event, &updates)?);
            }
        }
    }
    Ok(stream)
}

/// One newly committed event for every subscription of an open stream.
pub fn live<T>(
    event: &CommittedEvent<T>,
    subscriptions: &[Subscription],
    updates: impl Fn(&Subscription, &T) -> Vec<FragmentUpdate>,
) -> Result<String, Failure> {
    subscriptions
        .iter()
        .map(|subscription| encode(subscription, event, &updates))
        .collect()
}

fn encode<T>(
    subscription: &Subscription,
    event: &CommittedEvent<T>,
    updates: &impl Fn(&Subscription, &T) -> Vec<FragmentUpdate>,
) -> Result<String, Failure> {
    updates(subscription, &event.value)
        .into_iter()
        .map(|update| {
            SseEvent::json(
                event.position.event_id(),
                FRAGMENT_EVENT,
                &FeedEvent {
                    subscription: subscription.id.clone(),
                    update,
                },
            )?
            .encode()
        })
        .collect()
}

/// A `<shell-feed>` element: follow `reference` through the product's union
/// stream `route`, strictly after `since`, the position the page was rendered
/// at. `refresh` is an existing operation route returning a `Refresh` snapshot
/// of this subscription's fragments, read when a hidden tab is shown again.
pub struct Feed<'a> {
    pub route: &'a str,
    pub reference: &'a str,
    pub since: &'a str,
    pub refresh: Option<&'a str>,
}
impl Feed<'_> {
    pub fn render(&self) -> Markup {
        html! {
            shell-feed data-route=(self.route) data-reference=(self.reference)
                data-since=(self.since) data-refresh=[self.refresh] {}
        }
    }
}
