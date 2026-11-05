//! Sections of a composed answer (a pack, a recall, a status page) and the
//! budget fitter that chooses which of their entries fit a token budget.
//!
//! The fitter keeps whole entries, never part of one; it fills pending
//! sections before past ones; and it says, per section, how many entries it
//! left out and how to read them. Its text, left-out lines included, stays
//! within any budget that can hold what every section is sure to need.
use crate::{LeftOut, RunnableRequest, TokenBudget};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How complete a section's data is, independent of what the budget dropped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SectionStatus {
    Complete,
    /// Some of the section's data is missing, for the stated reason.
    Partial {
        reason: String,
    },
    /// The section's question cannot be answered from what is held.
    NotEstimable {
        reason: String,
    },
    /// None of the named sources observed the section's interval.
    NotObserved {
        sources: Vec<String>,
    },
    /// The section's question was answered and matched nothing.
    Empty,
}
impl SectionStatus {
    fn text(&self) -> String {
        match self {
            Self::Complete => String::new(),
            Self::Partial { reason } => format!(" (partial: {reason})"),
            Self::NotEstimable { reason } => format!(" (not estimable: {reason})"),
            Self::NotObserved { sources } => format!(" (not observed: {})", sources.join(", ")),
            Self::Empty => " (none)".into(),
        }
    }
}

/// One whole item of a section: a line, or several lines that belong
/// together (a row and its detail). Kept or left out as one. Every entry has
/// at least one line and cites at least one reference it was drawn from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "EntryWire")]
pub struct Entry {
    lines: Vec<String>,
    cites: Vec<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "Entry")]
struct EntryWire {
    lines: Vec<String>,
    cites: Vec<String>,
}
impl TryFrom<EntryWire> for Entry {
    type Error = &'static str;
    fn try_from(wire: EntryWire) -> Result<Self, Self::Error> {
        Self::with_lines(wire.lines, wire.cites).ok_or("an entry has a line and a citation")
    }
}
impl Entry {
    pub fn new(line: impl Into<String>, cite: impl Into<String>) -> Self {
        Self {
            lines: vec![line.into()],
            cites: vec![cite.into()],
        }
    }
    /// `None` when there is no line or no citation.
    pub fn with_lines(lines: Vec<String>, cites: Vec<String>) -> Option<Self> {
        (!lines.is_empty() && !cites.is_empty()).then_some(Self { lines, cites })
    }
    pub fn cite(mut self, reference: impl Into<String>) -> Self {
        self.cites.push(reference.into());
        self
    }
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
    pub fn cites(&self) -> &[String] {
        &self.cites
    }
    fn text(&self) -> String {
        let mut text = String::new();
        for line in &self.lines {
            text.push_str(line);
            text.push('\n');
        }
        text
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Section {
    pub title: String,
    #[serde(flatten)]
    pub status: SectionStatus,
    /// Pending work (waiting, running, open) is filled before what is past.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
    pub entries: Vec<Entry>,
    /// Reads the section's entries in full, at the answer's read context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expand: Option<RunnableRequest>,
}

/// An applied token budget, after resolving the operation's default and the
/// caller's output cap. Only this constructor creates its states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    tokens: Option<u32>,
    clamped_from: Option<TokenBudget>,
}
impl Budget {
    pub fn new(requested: TokenBudget, default: u32, cap: Option<u32>) -> Self {
        let asked = requested.resolve(Some(default));
        match cap {
            Some(cap) if asked.is_none_or(|asked| asked > cap) => Self {
                tokens: Some(cap),
                clamped_from: Some(asked.map_or(TokenBudget::Unlimited, TokenBudget::Tokens)),
            },
            _ => Self {
                tokens: asked,
                clamped_from: None,
            },
        }
    }
    pub(crate) fn reserve(self, held: u32) -> Self {
        Self {
            tokens: self.tokens.map(|tokens| tokens.saturating_sub(held)),
            ..self
        }
    }
    pub fn note(&self) -> Option<String> {
        self.clamped_from.and_then(|asked| {
            self.tokens.map(|tokens| {
                let from = match asked {
                    TokenBudget::Tokens(n) => n.to_string(),
                    TokenBudget::Unlimited => "unlimited".into(),
                    TokenBudget::Default => unreachable!("the constructor resolves defaults"),
                };
                format!("budget lowered from {from} to {tokens} tokens, the caller's output cap")
            })
        })
    }
}

/// An estimate of tokens from text: one per four bytes, rounded up. Products
/// with a tokenizer for the caller's model pass their own counter.
pub fn approximate_tokens(text: &str) -> u32 {
    u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Render the logical sections retained by the fitter without fitting them again.
pub fn render_sections(sections: &[Section]) -> String {
    sections
        .iter()
        .map(|section| render_section(section, &section.entries))
        .collect()
}
fn render_section(section: &Section, entries: &[Entry]) -> String {
    let mut text = heading(section);
    for entry in entries {
        text.push_str(&entry.text());
    }
    text
}

/// The fitted sections in their given order, the text of what was kept, and
/// what was left out. `used_tokens` counts the text and one line per
/// left-out entry, as the answer's renderer prints them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fitted {
    pub sections: Vec<Section>,
    pub text: String,
    pub left_out: Vec<LeftOut>,
    pub used_tokens: u32,
}

fn heading(section: &Section) -> String {
    format!("## {}{}\n", section.title, section.status.text())
}

/// Choose the entries of `sections` that fit `budget`.
///
/// `tokens` counts a text's tokens; `left_out_line` is the line the answer's
/// renderer prints for one left-out item, so its cost is counted exactly.
/// Pending sections are filled first, then the rest in their given order;
/// the text keeps the given order. Entries are taken in order and never
/// split; a section that fits whole is kept whole, and otherwise the first
/// entry that does not fit beside the section's left-out line ends it, the
/// remaining entries becoming one left-out item. A section with no room for its heading
/// is left out whole. Each section not yet placed holds back what it is sure
/// to need (being kept whole or named as left out, whichever is cheaper), so
/// no section can take the room a later one needs, and everything is kept
/// whenever everything fits. A budget that cannot hold what every section is
/// sure to need leaves every section out whole; the answer is then their
/// left-out lines alone, and `used_tokens` says it is over the budget.
pub fn fit(
    sections: Vec<Section>,
    budget: Budget,
    tokens: &dyn Fn(&str) -> u32,
    left_out_line: &dyn Fn(&LeftOut) -> String,
) -> Fitted {
    let left_out = |section: &Section, count: usize| LeftOut {
        what: section.title.clone(),
        count: count as u64,
        expand: section.expand.clone().map(|mut request| {
            request.budget_tokens = TokenBudget::Unlimited;
            request
        }),
    };
    let line_cost = |section: &Section| {
        u64::from(tokens(&left_out_line(&left_out(
            section,
            section.entries.len(),
        ))))
    };
    let whole_cost = |section: &Section| {
        u64::from(tokens(&heading(section)))
            + section
                .entries
                .iter()
                .map(|entry| u64::from(tokens(&entry.text())))
                .sum::<u64>()
    };
    // What a section not yet placed is sure to need: being kept whole or
    // being named as left out, whichever is cheaper.
    let reserve = |section: &Section| line_cost(section).min(whole_cost(section));
    let mut reserved: u64 = sections.iter().map(reserve).sum();
    let mut remaining = budget.tokens.map(u64::from);
    // For each section, how many entries it keeps, or `None` when it is left
    // out whole.
    let mut kept: Vec<Option<usize>> = vec![None; sections.len()];
    if remaining.is_none_or(|remaining| reserved <= remaining) {
        let mut order: Vec<usize> = (0..sections.len()).collect();
        order.sort_by_key(|&index| !sections[index].pending);
        for index in order {
            let section = &sections[index];
            reserved -= reserve(section);
            // Room for this section with every later one's need held back;
            // never less than this section's own need.
            let free = remaining.map(|remaining| remaining - reserved);
            let whole = whole_cost(section);
            if free.is_none_or(|free| whole <= free) {
                kept[index] = Some(section.entries.len());
                remaining = remaining.map(|remaining| remaining - whole);
                continue;
            }
            let free = free.expect("an unlimited budget kept the whole section");
            // Some entry is left out, so the section ends with its left-out line.
            let line = line_cost(section);
            let mut used = u64::from(tokens(&heading(section))) + line;
            let mut count = 0;
            for entry in &section.entries {
                let cost = u64::from(tokens(&entry.text()));
                if used + cost > free {
                    break;
                }
                used += cost;
                count += 1;
            }
            if used <= free {
                kept[index] = Some(count);
                remaining = remaining.map(|remaining| remaining - used);
            } else {
                // Its own need was the left-out line, which `free` holds.
                remaining = remaining.map(|remaining| remaining - line);
            }
        }
    }
    let mut text = String::new();
    let mut fitted = Vec::with_capacity(sections.len());
    let mut left = Vec::new();
    let mut used_tokens = 0u64;
    for (mut section, kept) in sections.into_iter().zip(kept) {
        let count = kept.unwrap_or(0);
        if kept.is_some() {
            let part = render_section(&section, &section.entries[..count]);
            used_tokens += u64::from(tokens(&part));
            text.push_str(&part);
        }
        if kept.is_none() || count < section.entries.len() {
            let item = left_out(&section, section.entries.len() - count);
            used_tokens += u64::from(tokens(&left_out_line(&item)));
            left.push(item);
        }
        if kept.is_some() {
            section.entries.truncate(count);
            fitted.push(section);
        }
    }
    Fitted {
        sections: fitted,
        text,
        left_out: left,
        used_tokens: u32::try_from(used_tokens).unwrap_or(u32::MAX),
    }
}
