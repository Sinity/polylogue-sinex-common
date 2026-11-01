//! The Inbox, Why and Signals views, as maud functions over this crate's
//! types, so both products show the same view for the same data. The
//! products' frames supply links, times and the page around them.
use crate::*;
use maud::{Markup, html};

/// Renders an instant in the product's logical day.
pub type TimeText<'a> = &'a dyn Fn(Instant) -> String;

pub struct InboxEntry<'a> {
    pub delivery: &'a Delivery,
    pub state: &'a DeliveryState,
    /// The product's one-line rendering of the notice.
    pub title: &'a str,
    pub href: &'a str,
    pub why_href: &'a str,
    /// Consequences folded into this root cause.
    pub lines: &'a [String],
}
pub struct HeldEntry<'a> {
    pub held: &'a Held,
    pub title: &'a str,
    pub why_href: &'a str,
}
pub struct Inbox<'a> {
    pub attention: &'a Attention,
    pub quiet_hours: Option<QuietHours>,
    pub entries: &'a [InboxEntry<'a>],
    pub held: &'a [HeldEntry<'a>],
    pub time: TimeText<'a>,
}

fn author(author: &Author) -> String {
    match author {
        Author::Operator => "you".into(),
        Author::Shipped => "shipped".into(),
        Author::Agent(agent) => agent.clone(),
        Author::Producer(producer) => format!("producer:{producer}"),
    }
}
fn clock(minute: u16) -> String {
    format!("{:02}:{:02}", minute / 60, minute % 60)
}

/// `present · deep work · budget 2 of 4 · quiet 22:00–08:00`
pub fn attention_line(attention: &Attention, quiet_hours: Option<QuietHours>) -> String {
    let mut parts = vec![
        match attention.presence {
            Presence::Present => "present",
            Presence::Away => "away",
            Presence::Unknown => "presence unknown",
        }
        .to_string(),
    ];
    if attention.deep_work {
        parts.push("deep work".into());
    }
    parts.push(format!(
        "budget {} of {}",
        attention.budget_used, attention.budget_per_hour
    ));
    if let Some(hours) = quiet_hours {
        parts.push(format!("quiet {}–{}", clock(hours.from), clock(hours.to)));
    }
    parts.join(" · ")
}

fn entry(entry: &InboxEntry, time: TimeText) -> Markup {
    let delivery = entry.delivery;
    html! {
        li.notice data-seen=(entry.state.seen) {
            span.time { (time(delivery.at)) }
            span.product { (delivery.fired.trigger.rule.product) }
            a href=(entry.href) { (entry.title) }
            span.decision { (delivery.decision) }
            a.why href=(entry.why_href) { "why" }
            @if !entry.lines.is_empty() {
                ul.caused {
                    @for line in entry.lines { li { (line) } }
                }
            }
        }
    }
}

/// `/notices`: interrupted, held and passive notices, unseen first.
pub fn inbox(page: &Inbox) -> Markup {
    let unseen = page.entries.iter().filter(|e| !e.state.seen).count();
    let mut entries: Vec<&InboxEntry> = page.entries.iter().collect();
    entries.sort_by_key(|e| (e.state.seen, std::cmp::Reverse(e.delivery.at)));
    let (interrupted, passive): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|e| e.delivery.decision.interrupts());
    html! {
        section.notices {
            header {
                h1 { "Notices" }
                span.count { (unseen) " unseen · " (page.held.len()) " held" }
                p.attention { "Attention: " (attention_line(page.attention, page.quiet_hours)) }
            }
            @if !interrupted.is_empty() {
                h2 { "Interrupted" }
                ul { @for e in interrupted { (entry(e, page.time)) } }
            }
            @if !page.held.is_empty() {
                h2 { "Held" }
                ul {
                    @for held in page.held {
                        li.notice {
                            span.time { ((page.time)(held.held.fired.at)) }
                            span.product { (author(&held.held.fired.author)) }
                            span { (held.title) }
                            span.decision {
                                @match held.held.holds.last() {
                                    Some(hold) => {
                                        "held: " (hold.reason)
                                        @if let Some(until) = hold.until { " (≤ " ((page.time)(until)) ")" }
                                    }
                                    None => { "due" }
                                }
                            }
                            a.why href=(held.why_href) { "why" }
                        }
                    }
                }
            }
            @if !passive.is_empty() {
                h2 { "Passive" }
                ul { @for e in passive { (entry(e, page.time)) } }
            }
        }
    }
}

/// The inspector panel for one delivery: the rule, the matched rows with their
/// observation, the route decision and its inputs, and the outcomes so far.
pub fn why(
    delivery: &Delivery,
    read_as: &str,
    outcomes: &[OutcomeRecord],
    time: TimeText,
) -> Markup {
    let fired = &delivery.fired;
    let attention = &delivery.attention;
    html! {
        section.why {
            h2 { (fired.rule_name) " · revision " (fired.trigger.rule.revision) }
            p { "by " (author(&fired.author)) " · " (read_as) }
            h3 { "Matched" }
            ul {
                @for row in &fired.basis {
                    li { code { (row.reference) } " observed " (time(row.observed)) }
                }
            }
            h3 { "Route" }
            dl {
                dt { "quiet" } dd { @if attention.quiet_until.is_some_and(|until| until > attention.at) { "yes" } @else { "no" } }
                dt { "presence" } dd { (attention_line(attention, None)) }
                dt { "focus" } dd { @if attention.focus.is_empty() { "unknown" } @else { (attention.focus.join(", ")) } }
                dt { "decision" } dd { (delivery.why()) }
            }
            h3 { "Outcomes" }
            ol {
                @for record in outcomes {
                    li { (time(record.at)) " " (serde_json::to_string(&record.outcome).unwrap_or_default()) }
                }
            }
        }
    }
}

pub struct RuleRow<'a> {
    pub rule: &'a ActiveRule,
    pub read_as: &'a str,
    pub last_fired: Option<Instant>,
    /// Deliveries in the last seven days.
    pub deliveries: usize,
    pub usefulness: Usefulness,
    pub href: &'a str,
}

/// `/signals`: shipped and authored rules together, with author, channel,
/// last firing, recent deliveries and usefulness.
pub fn signals(rows: &[RuleRow], time: TimeText) -> Markup {
    html! {
        table.signals {
            thead { tr { th { "Rule" } th { "Author" } th { "Channel" } th { "Last fired" } th { "7 days" } th { "Usefulness" } } }
            tbody {
                @for row in rows {
                    tr data-enabled=(row.rule.rule.enabled) {
                        td { a href=(row.href) { (row.rule.rule.name) } br; small { (row.read_as) } }
                        td { (author(&row.rule.author)) }
                        td { @if row.rule.rule.interrupt { "interrupt" } @else { "passive" } }
                        td {
                            @match row.last_fired {
                                Some(at) => { (time(at)) }
                                None => { "never" }
                            }
                        }
                        td { (row.deliveries) }
                        td { "opened " (row.usefulness.opened) " · dismissed at once " (row.usefulness.dismissed_fast) " · expired " (row.usefulness.expired) }
                    }
                }
            }
        }
    }
}
