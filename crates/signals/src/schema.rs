use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub type Instant = i64;
pub const HOUR: i64 = 3_600_000;

/// Milliseconds on the wire, or a duration such as `20m` on input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Duration(pub i64);
impl JsonSchema for Duration {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "Duration".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"oneOf":[{"type":"integer","minimum":0},{"type":"string","pattern":"^[0-9]+(ms|s|m|h|d)$"}]})
    }
}
impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Input {
            Number(i64),
            Text(String),
        }
        let value = match Input::deserialize(d)? {
            Input::Number(n) => n,
            Input::Text(text) => {
                let split = text
                    .find(|c: char| !c.is_ascii_digit())
                    .ok_or_else(|| serde::de::Error::custom("duration needs a unit"))?;
                let (number, unit) = text.split_at(split);
                let multiplier = match unit {
                    "ms" => 1,
                    "s" => 1000,
                    "m" => 60_000,
                    "h" => HOUR,
                    "d" => 24 * HOUR,
                    _ => return Err(serde::de::Error::custom("unknown duration unit")),
                };
                number
                    .parse::<i64>()
                    .ok()
                    .and_then(|n| n.checked_mul(multiplier))
                    .ok_or_else(|| serde::de::Error::custom("duration out of range"))?
            }
        };
        if value < 0 {
            return Err(serde::de::Error::custom("negative duration"));
        }
        Ok(Self(value))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    #[default]
    Membership,
    Ranking,
    TopK,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Compare {
    #[serde(rename = "<")]
    Less,
    #[serde(rename = ">")]
    Greater,
}
impl Compare {
    pub fn matches(self, sample: f64, value: f64) -> bool {
        match self {
            Self::Less => sample < value,
            Self::Greater => sample > value,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Fires {
    #[default]
    OnNew,
    OnChange {
        #[serde(default)]
        what: ChangeKind,
        k: Option<usize>,
    },
    OnThreshold {
        value: f64,
        compare: Compare,
    },
    /// Minute of the local civil day. Calendar resolution belongs to DaySetting.
    At {
        minute: u16,
    },
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Deliver {
    #[default]
    Notice,
    Value,
    Action,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct QuietHours {
    pub from: u16,
    pub to: u16,
}
impl QuietHours {
    pub fn contains(self, minute: u16) -> bool {
        if self.from > self.to {
            minute >= self.from || minute < self.to
        } else {
            self.from <= minute && minute < self.to
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RateLimit {
    pub count: usize,
    pub window: Duration,
}
fn enabled() -> bool {
    true
}
fn minimum() -> usize {
    1
}

/// Authored fields only. Identity and authorship are supplied by the product's
/// write path; `deny_unknown_fields` prevents a client forging daemon metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    pub query: String,
    #[serde(default)]
    pub fires: Fires,
    #[serde(default, rename = "for")]
    pub duration: Duration,
    #[serde(default = "minimum")]
    pub min_change: usize,
    #[serde(default)]
    pub deliver: Deliver,
    pub target: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub interrupt: bool,
    pub quiet_hours: Option<QuietHours>,
    pub rate_limit: Option<RateLimit>,
    pub scope: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Configuration {
    pub value_targets: BTreeSet<String>,
    pub action_commands: BTreeSet<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidRule(pub String);
impl std::fmt::Display for InvalidRule {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for InvalidRule {}
impl Rule {
    pub fn validate(&self, configuration: &Configuration) -> Result<(), InvalidRule> {
        let refuse = |reason: &str| Err(InvalidRule(reason.into()));
        if self.name.is_empty() || self.query.is_empty() {
            return refuse("name and query must be nonempty");
        }
        if self.duration.0 < 0 || self.min_change == 0 {
            return refuse("duration must be nonnegative and min_change positive");
        }
        if self
            .quiet_hours
            .is_some_and(|q| q.from >= 1440 || q.to >= 1440)
        {
            return refuse("quiet hours must be civil minutes");
        }
        if self
            .rate_limit
            .as_ref()
            .is_some_and(|r| r.count == 0 || r.window.0 <= 0)
        {
            return refuse("rate limit count and window must be positive");
        }
        match self.fires {
            Fires::OnChange {
                what: ChangeKind::TopK,
                k,
            } if k.is_none_or(|k| k == 0) => return refuse("top_k requires positive k"),
            Fires::OnChange { what, k: Some(_) } if what != ChangeKind::TopK => {
                return refuse("k belongs to top_k");
            }
            Fires::OnThreshold { value, .. } if !value.is_finite() => {
                return refuse("threshold must be finite");
            }
            Fires::At { minute } if minute >= 1440 => return refuse("at requires a civil minute"),
            _ => {}
        }
        match self.deliver {
            Deliver::Value
                if !self
                    .target
                    .as_ref()
                    .is_some_and(|name| configuration.value_targets.contains(name)) =>
            {
                return refuse("value target is not configured");
            }
            Deliver::Action
                if !self
                    .command
                    .as_ref()
                    .is_some_and(|name| configuration.action_commands.contains(name)) =>
            {
                return refuse("action command is not configured");
            }
            _ => {}
        }
        if (self.deliver != Deliver::Value && self.target.is_some())
            || (self.deliver != Deliver::Action && self.command.is_some())
        {
            return refuse("target and command belong to their delivery mode");
        }
        if self.interrupt && self.deliver != Deliver::Notice {
            return refuse("only notices interrupt");
        }
        if self.duration.0 > 0 && self.fires != Fires::OnNew {
            return refuse("for applies to on_new rows");
        }
        Ok(())
    }
}
#[derive(
    Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub struct RuleVersion {
    pub product: String,
    pub id: String,
    pub revision: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Author {
    Operator,
    Shipped,
    Agent(String),
}
impl Author {
    pub fn budget_exempt(&self) -> bool {
        !matches!(self, Self::Agent(_))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActiveRule {
    pub version: RuleVersion,
    pub rule: Rule,
    pub author: Author,
    pub via: String,
    /// The adapter marks queries over live state, independently of `for`.
    pub live: bool,
}
impl ActiveRule {
    pub fn current(&self) -> bool {
        self.live || self.rule.duration.0 > 0
    }
}

/// The complete authored field table, used by product help and schema renderers.
pub const FIELDS: &[(&str, &str)] = &[
    ("name", "display label, unique per product"),
    ("query", "watched query or saved name"),
    ("fires", "on_new, on_change, on_threshold or at"),
    ("for", "continuous matching duration"),
    (
        "min_change",
        "minimum joins and leaves before a change report",
    ),
    ("deliver", "notice, value or action"),
    ("target", "configured value target"),
    ("command", "configured action command"),
    (
        "interrupt",
        "desktop interruption, false by default for every author",
    ),
    ("quiet_hours", "local interval holding interrupts"),
    (
        "rate_limit",
        "handoffs per window; excess goes to next brief",
    ),
    ("scope", "brief routing scope"),
    ("enabled", "whether this revision runs"),
];

/// Authored revisions replace shipped defaults by name, including disabled
/// revisions; withdrawing an authored revision restores its shipped default.
pub fn effective_rules(
    shipped: &[ActiveRule],
    authored: &[ActiveRule],
) -> Result<Vec<ActiveRule>, InvalidRule> {
    let mut rules = std::collections::BTreeMap::new();
    for group in [shipped, authored] {
        let mut seen = BTreeSet::new();
        for rule in group {
            let key = (rule.version.product.clone(), rule.rule.name.clone());
            if !seen.insert(key.clone()) {
                return Err(InvalidRule("ambiguous rule name".into()));
            }
            rules.insert(key, rule.clone());
        }
    }
    Ok(rules.into_values().collect())
}
