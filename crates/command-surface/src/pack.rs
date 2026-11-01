//! Sections of a composed answer (a pack, a recall, a status page) and the
//! budget fitter that chooses which of their entries fit a token budget.
//!
//! The fitter keeps whole entries, never part of one; it fills pending
//! sections before past ones; and it says, per section, how many entries it
//! left out and how to read them. Its text, left-out lines included, stays
//! within any budget that can hold one left-out line per section.
use crate::{LeftOut, RunnableRequest};
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

/// The token budget of one answer: what was asked, clamped to what the
/// caller's harness accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub tokens: u32,
    /// The budget asked for, when the harness's cap lowered it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clamped_from: Option<u32>,
}
impl Budget {
    /// The requested budget, else the operation's default, lowered to the
    /// caller's cap when it has one.
    pub fn new(requested: Option<u32>, default: u32, cap: Option<u32>) -> Self {
        let asked = requested.unwrap_or(default);
        match cap {
            Some(cap) if asked > cap => Self {
                tokens: cap,
                clamped_from: Some(asked),
            },
            _ => Self {
                tokens: asked,
                clamped_from: None,
            },
        }
    }
    /// The note an answer carries when the cap lowered its budget.
    pub fn note(&self) -> Option<String> {
        self.clamped_from.map(|asked| {
            format!(
                "budget lowered from {asked} to {} tokens, the caller's output cap",
                self.tokens
            )
        })
    }
}

/// An estimate of tokens from text: one per four bytes, rounded up. Products
/// with a tokenizer for the caller's model pass their own counter.
pub fn approximate_tokens(text: &str) -> u32 {
    u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
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
/// is left out whole. Room for one left-out line per section not yet placed
/// is held back, so no section can take the room a later one needs to say
/// it was left out. A budget that cannot hold one left-out line per section
/// leaves every section out whole; the answer is then those lines alone,
/// and `used_tokens` says it is over the budget.
pub fn fit(
    sections: Vec<Section>,
    budget: Budget,
    tokens: &dyn Fn(&str) -> u32,
    left_out_line: &dyn Fn(&LeftOut) -> String,
) -> Fitted {
    let left_out = |section: &Section, count: usize| LeftOut {
        what: section.title.clone(),
        count: count as u64,
        expand: section.expand.clone(),
    };
    let reserve = |section: &Section| {
        u64::from(tokens(&left_out_line(&left_out(
            section,
            section.entries.len(),
        ))))
    };
    let mut reserved: u64 = sections.iter().map(reserve).sum();
    let mut remaining = u64::from(budget.tokens);
    // For each section, how many entries it keeps, or `None` when it is left
    // out whole.
    let mut kept: Vec<Option<usize>> = vec![None; sections.len()];
    if reserved <= remaining {
        let mut order: Vec<usize> = (0..sections.len()).collect();
        order.sort_by_key(|&index| !sections[index].pending);
        for index in order {
            let section = &sections[index];
            let own = reserve(section);
            reserved -= own;
            let free = remaining - reserved;
            let heading_cost = u64::from(tokens(&heading(section)));
            let costs: Vec<u64> = section
                .entries
                .iter()
                .map(|entry| u64::from(tokens(&entry.text())))
                .collect();
            let whole = heading_cost + costs.iter().sum::<u64>();
            if whole <= free {
                // Everything fits: no left-out line is needed.
                kept[index] = Some(section.entries.len());
                remaining -= whole;
                continue;
            }
            // Some entry is left out, so the section ends with its left-out line.
            let mut used = heading_cost + own;
            let mut count = 0;
            for cost in costs {
                if used + cost > free {
                    break;
                }
                used += cost;
                count += 1;
            }
            if used <= free {
                kept[index] = Some(count);
                remaining -= used;
            } else {
                remaining -= own;
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
            let mut part = heading(&section);
            for entry in &section.entries[..count] {
                part.push_str(&entry.text());
            }
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
