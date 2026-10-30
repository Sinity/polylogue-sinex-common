//! Deterministic shared surface text from typed syntax. Values never become syntax.
use crate::ast::*;
use crate::catalog::{Catalog, ExtensionPosition, ExtensionSlot};
use crate::{Error, ErrorKind};

/// Render capital boolean operators and piped stages while preserving whether
/// a source unit was explicit or inherited.
/// A round trip through this catalog must preserve the canonical question. This
/// also refuses manually constructed trees the language cannot express.
pub fn format_query(query: &Query, catalog: &Catalog) -> Result<String, Error> {
    let text = render_query(query, catalog)?;
    let parsed = crate::parse::parse(&text, catalog)
        .map_err(|error| invalid(format!("syntax tree cannot be rendered: {}", error.message)))?;
    if crate::identity::normal_form(query, catalog)?
        != crate::identity::normal_form(&parsed, catalog)?
    {
        return Err(invalid(
            "syntax tree has no equivalent shared surface spelling",
        ));
    }
    Ok(text)
}
pub(crate) fn render_query(query: &Query, catalog: &Catalog) -> Result<String, Error> {
    Renderer { catalog }.query(query)
}
fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::InvalidQuery, message, 0)
}
struct Renderer<'a> {
    catalog: &'a Catalog,
}
impl Renderer<'_> {
    fn query(&self, query: &Query) -> Result<String, Error> {
        let mut text = match &query.source {
            Source::Unit {
                unit,
                implicit,
                condition,
            } => {
                let mut text = if *implicit {
                    String::new()
                } else {
                    name(unit)?
                };
                if let Some(condition) = condition {
                    if !*implicit {
                        text.push_str(" where ");
                    }
                    text.push_str(&self.condition(condition)?);
                }
                text
            }
            Source::Saved {
                name,
                arguments,
                condition,
                ..
            } => {
                let mut text = self.saved(name, arguments)?;
                if let Some(condition) = condition {
                    text.push(' ');
                    text.push_str(&self.condition(condition)?);
                }
                text
            }
            Source::Result {
                reference,
                part,
                condition,
                ..
            } => {
                let id = reference
                    .strip_prefix("result:")
                    .ok_or_else(|| invalid("a result source needs the result: prefix"))?;
                let mut text = format!(
                    "result:{}{}",
                    quote(id),
                    if *part == ResultPart::Matches {
                        ".matches"
                    } else {
                        ""
                    }
                );
                if let Some(condition) = condition {
                    text.push(' ');
                    text.push_str(&self.condition(condition)?);
                }
                text
            }
            Source::Walk {
                direction,
                reference,
            } => format!(
                "{} {}",
                match direction {
                    WalkDirection::Trace => "trace",
                    WalkDirection::Descendants => "descendants",
                    WalkDirection::Parents => "parents",
                },
                literal(reference)?
            ),
            Source::Recall { time, window } => {
                let mut text = "recall".to_string();
                if let Some(time) = time {
                    text.push(' ');
                    text.push_str(&value(time)?);
                }
                if let Some(window) = window {
                    text.push_str(" ±");
                    text.push_str(&window_value(window)?);
                }
                text
            }
            Source::RecallLast { condition, .. } => {
                let mut text = "recall last".to_string();
                if let Some(condition) = condition {
                    text.push_str(" where ");
                    text.push_str(&self.condition(condition)?);
                }
                text
            }
            Source::RecallMark { number, window } => {
                let mut text = format!("recall mark {number}");
                if let Some(window) = window {
                    text.push_str(" ±");
                    text.push_str(&window_value(window)?);
                }
                text
            }
        };
        if !query.attach.is_empty() {
            text.push_str(" with ");
            text.push_str(
                &query
                    .attach
                    .iter()
                    .map(|attachment| self.attachment(attachment))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", "),
            );
        }
        for stage in &query.stages {
            text.push_str(" | ");
            text.push_str(&self.stage(&stage.kind)?);
        }
        Ok(text)
    }
    fn saved(&self, saved: &str, arguments: &[Argument]) -> Result<String, Error> {
        if saved.is_empty() {
            return Err(invalid("a saved query needs a literal name"));
        }
        let spelling = name(saved)
            .ok()
            .filter(|_| !saved.starts_with('$'))
            .unwrap_or_else(|| quote(saved));
        let mut text = format!("query:{spelling}");
        if !arguments.is_empty() {
            text.push('(');
            text.push_str(
                &arguments
                    .iter()
                    .map(|argument| {
                        Ok(format!(
                            "{}={}",
                            name(&argument.parameter)?,
                            value(&argument.value)?
                        ))
                    })
                    .collect::<Result<Vec<_>, Error>>()?
                    .join(", "),
            );
            text.push(')');
        }
        Ok(text)
    }
    fn condition(&self, condition: &Condition) -> Result<String, Error> {
        Ok(match &condition.kind {
            ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
                if conditions.is_empty() {
                    return Err(invalid("a boolean condition needs at least one operand"));
                }
                let join = if matches!(condition.kind, ConditionKind::And { .. }) {
                    " AND "
                } else {
                    " OR "
                };
                let mut terms = conditions
                    .iter()
                    .map(|c| self.condition(c))
                    .collect::<Result<Vec<_>, _>>()?;
                terms.sort();
                terms.dedup();
                format!("({})", terms.join(join))
            }
            ConditionKind::Not { condition } => format!("NOT ({})", self.condition(condition)?),
            ConditionKind::Match {
                field,
                value: literal,
            } => format!("{}:{}", name(field)?, value(literal)?),
            ConditionKind::Compare {
                operand: field,
                op,
                value: literal,
            } => format!("{} {} {}", operand(field)?, op.symbol(), value(literal)?),
            ConditionKind::Since { value: literal, .. } => format!("since {}", value(literal)?),
            ConditionKind::Until { value: literal, .. } => format!("until {}", value(literal)?),
            ConditionKind::Between {
                operand: field,
                from,
                to,
            } => format!(
                "{} between {} and {}",
                operand(field)?,
                value(from)?,
                value(to)?
            ),
            ConditionKind::Text { text, mode } => match mode {
                TextMode::Phrase => quote(text),
                TextMode::Word => word(text)?,
                TextMode::Prefix => format!("{}*", word(text)?),
                TextMode::Approximate => format!("{}~", word(text)?),
            },
            ConditionKind::Reference { reference } => {
                if let Some(id) = reference.strip_prefix("result:") {
                    format!("result:{}", quote(id))
                } else {
                    literal(reference)?
                }
            }
            ConditionKind::Similar { value: literal } => format!("similar:{}", value(literal)?),
            ConditionKind::Exists { unit, condition } => format!(
                "exists {}({})",
                name(unit)?,
                condition
                    .as_ref()
                    .map(|c| self.condition(c))
                    .transpose()?
                    .unwrap_or_default()
            ),
            ConditionKind::FieldExists { operand: field } => format!("{} exists", operand(field)?),
            ConditionKind::Count {
                unit,
                condition,
                op,
                count,
            } => format!(
                "count({}{}) {} {}",
                name(unit)?,
                condition
                    .as_ref()
                    .map(|c| self.condition(c).map(|text| format!(" where {text}")))
                    .transpose()?
                    .unwrap_or_default(),
                op.symbol(),
                count
            ),
            ConditionKind::In {
                operand: field,
                query,
            } => format!("{} in ({})", operand(field)?, self.query(query)?),
            ConditionKind::TimeRange {
                operand: field,
                value: literal,
            } => format!("{} in {}", operand(field)?, value(literal)?),
            ConditionKind::Overlaps { from, to } => format!(
                "overlaps {}{}",
                value(from)?,
                to.as_ref()
                    .map(|to| value(to).map(|text| format!(" .. {text}")))
                    .transpose()?
                    .unwrap_or_default()
            ),
            ConditionKind::TimeDifference {
                left,
                right,
                op,
                value: literal,
            } => format!(
                "{} - {} {} {}",
                operand(left)?,
                operand(right)?,
                op.symbol(),
                value(literal)?
            ),
            ConditionKind::Saved { name, arguments } => self.saved(name, arguments)?,
            ConditionKind::Sequence {
                all, steps, gaps, ..
            } => {
                if steps.len() < 2 || gaps.len() + 1 != steps.len() {
                    return Err(invalid(
                        "a sequence needs one gap between each pair of steps",
                    ));
                }
                let mut text = format!(
                    "seq{}({}",
                    if *all { " all" } else { "" },
                    self.condition(&steps[0])?
                );
                for (gap, step) in gaps.iter().zip(&steps[1..]) {
                    text.push_str(" -> ");
                    match gap {
                        Gap::Later => {}
                        Gap::Next => text.push_str("[next] "),
                        Gap::Within { duration } => {
                            text.push_str(&format!("[within:{}] ", value(duration)?))
                        }
                    }
                    text.push_str(&self.condition(step)?);
                }
                text.push(')');
                text
            }
            ConditionKind::Extension { name, arguments } => {
                self.extension(name, arguments, ExtensionPosition::Atom)?
            }
        })
    }
    fn attachment(&self, attachment: &Attachment) -> Result<String, Error> {
        let fields = attachment
            .fields
            .iter()
            .map(|f| name(f))
            .collect::<Result<Vec<_>, _>>()?;
        let mut text = format!("{}({})", name(&attachment.unit)?, fields.join(", "));
        if !attachment.windows.is_empty() {
            let windows = attachment
                .windows
                .iter()
                .map(|window| {
                    Ok(match window {
                        Window::First(n) => format!("first:{n}"),
                        Window::Last(n) => format!("last:{n}"),
                        Window::Where(c) => self.condition(c)?,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            text.push('[');
            text.push_str(&windows.join(", "));
            text.push(']');
        }
        Ok(text)
    }
    fn stage(&self, stage: &StageKind) -> Result<String, Error> {
        Ok(match stage {
            StageKind::Where { condition } => format!("where {}", self.condition(condition)?),
            StageKind::Sort { keys } => {
                if keys.is_empty() {
                    return Err(invalid("sort needs a key"));
                }
                format!(
                    "sort by {}",
                    keys.iter()
                        .map(|key| Ok(format!(
                            "{} {}",
                            operand(&key.operand)?,
                            if key.descending { "desc" } else { "asc" }
                        )))
                        .collect::<Result<Vec<_>, Error>>()?
                        .join(", ")
                )
            }
            StageKind::Group { keys } => format!(
                "group by {}",
                keys.iter()
                    .map(operand)
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            StageKind::Aggregate { aggregates } => format!(
                "agg {}",
                aggregates
                    .iter()
                    .map(aggregate)
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            StageKind::Limit { count } => format!("limit {count}"),
            StageKind::Offset { count } => format!("offset {count}"),
            StageKind::First { count } => format!("first {count}"),
            StageKind::Distinct => "distinct".into(),
            StageKind::Matches => "matches".into(),
            StageKind::Fill => "fill".into(),
            StageKind::Fields { fields } => format!(
                "fields {}",
                fields
                    .iter()
                    .map(|f| name(f))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            StageKind::Scope { unit, condition } => format!(
                "{}{}",
                name(unit)?,
                condition
                    .as_ref()
                    .map(|c| self.condition(c).map(|text| format!(" where {text}")))
                    .transpose()?
                    .unwrap_or_default()
            ),
            StageKind::Combine { how, query } => format!(
                "{} ({})",
                match how {
                    Combination::Union => "union",
                    Combination::Intersect => "intersect",
                    Combination::Except => "except",
                    Combination::Fuse => "fuse",
                },
                self.query(query)?
            ),
            StageKind::Temporal {
                relation,
                query,
                at,
                within,
                all,
            } => format!(
                "{}{} ({}) at {}{}",
                match relation {
                    TemporalRelation::Near => "near",
                    TemporalRelation::Before => "before",
                    TemporalRelation::After => "after",
                    TemporalRelation::During => "during",
                    TemporalRelation::Containing => "containing",
                },
                if *all { " all" } else { "" },
                self.query(query)?,
                match at {
                    TimePoint::When => "when",
                    TimePoint::Start => "start",
                    TimePoint::End => "end",
                },
                within
                    .as_ref()
                    .map(|duration| value(duration).map(|text| format!(" within {text}")))
                    .transpose()?
                    .unwrap_or_default()
            ),
            StageKind::AsOf {
                value: literal,
                sequence,
            } => format!(
                "as of {}{}",
                if *sequence { "seq " } else { "" },
                value(literal)?
            ),
            StageKind::Measure { name: measure, by } => format!(
                "measure {}{}",
                quote(measure),
                if by.is_empty() {
                    String::new()
                } else {
                    format!(
                        " by {}",
                        by.iter()
                            .map(operand)
                            .collect::<Result<Vec<_>, _>>()?
                            .join(", ")
                    )
                }
            ),
            StageKind::Extension { name, arguments } => {
                self.extension(name, arguments, ExtensionPosition::Stage)?
            }
        })
    }
    fn extension(
        &self,
        name: &str,
        arguments: &[ExtensionArgument],
        position: ExtensionPosition,
    ) -> Result<String, Error> {
        let extension = self
            .catalog
            .extensions
            .iter()
            .find(|extension| extension.name == name && extension.position == position)
            .ok_or_else(|| invalid(format!("undeclared extension `{name}`")))?;
        let rendered = self
            .extension_arguments(&extension.arguments, arguments)?
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        Ok(if rendered.is_empty() {
            name.to_string()
        } else {
            format!("{name} {}", rendered.join(" "))
        })
    }
    fn extension_arguments(
        &self,
        slots: &[ExtensionSlot],
        arguments: &[ExtensionArgument],
    ) -> Result<Vec<String>, Error> {
        if slots.len() != arguments.len() {
            return Err(invalid(
                "extension argument count differs from its declaration",
            ));
        }
        slots
            .iter()
            .zip(arguments)
            .map(|(slot, argument)| {
                Ok(match (slot, argument) {
                    (ExtensionSlot::Keyword(expected), ExtensionArgument::Keyword(actual))
                        if expected == actual =>
                    {
                        expected.clone()
                    }
                    (ExtensionSlot::Value, ExtensionArgument::Value(literal)) => value(literal)?,
                    (ExtensionSlot::Name, ExtensionArgument::Name(identifier)) => name(identifier)?,
                    (ExtensionSlot::Field, ExtensionArgument::Field(field)) => name(field)?,
                    (ExtensionSlot::Unit, ExtensionArgument::Unit(unit)) => name(unit)?,
                    (ExtensionSlot::Condition, ExtensionArgument::Condition(condition)) => {
                        format!("({})", self.condition(condition)?)
                    }
                    (ExtensionSlot::Query, ExtensionArgument::Query(query)) => {
                        format!("({})", self.query(query)?)
                    }
                    (ExtensionSlot::Optional(_), ExtensionArgument::Optional(arguments))
                        if arguments.is_empty() =>
                    {
                        String::new()
                    }
                    (ExtensionSlot::Optional(slots), ExtensionArgument::Optional(arguments)) => {
                        self.extension_arguments(slots, arguments)?.join(" ")
                    }
                    _ => {
                        return Err(invalid(
                            "extension argument type differs from its declaration",
                        ));
                    }
                })
            })
            .collect()
    }
}
pub(crate) fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn value(value: &Value) -> Result<String, Error> {
    Ok(match value {
        Value::Scalar { text, .. } => quote(text),
        Value::Set { values } => {
            if values.is_empty() {
                return Err(invalid("a set needs at least one value"));
            }
            let mut values = values
                .iter()
                .map(self::value)
                .collect::<Result<Vec<_>, _>>()?;
            values.sort();
            values.dedup();
            format!("({})", values.join("|"))
        }
        Value::Any => "*".into(),
        Value::Unknown => "unknown".into(),
        Value::Parameter { name: parameter } => format!("${}", name(parameter)?),
    })
}
fn name(text: &str) -> Result<String, Error> {
    if text.is_empty()
        || text
            .chars()
            .any(|c| c.is_whitespace() || "()[]|,\"=<>!".contains(c))
    {
        return Err(invalid(
            "name cannot contain query punctuation or whitespace",
        ));
    }
    Ok(text.to_string())
}
fn literal(text: &str) -> Result<String, Error> {
    if text.is_empty()
        || text
            .chars()
            .any(|c| c.is_whitespace() || "()[]|,\"".contains(c))
    {
        return Err(invalid(
            "reference cannot contain whitespace or query delimiters",
        ));
    }
    Ok(text.to_string())
}
fn word(text: &str) -> Result<String, Error> {
    if text.is_empty()
        || text
            .chars()
            .any(|c| c.is_whitespace() || "[]|,\"=<>!".contains(c))
    {
        return Err(invalid(
            "text word cannot contain whitespace or query delimiters",
        ));
    }
    let word = text.to_string();
    if text.contains(':')
        || text.ends_with(['*', '~'])
        || text.starts_with('-')
        || matches!(text, "AND" | "OR" | "NOT")
    {
        return Err(invalid(
            "text word needs a different syntax node to preserve its meaning",
        ));
    }
    Ok(word)
}
fn window_value(value: &Value) -> Result<String, Error> {
    match value {
        Value::Scalar { text, .. } => literal(text),
        _ => Err(invalid("recall windows are literal durations")),
    }
}
fn operand(operand: &Operand) -> Result<String, Error> {
    match operand {
        Operand::Field { name: field } => name(field),
        Operand::Function {
            name: function,
            arguments,
        } => Ok(format!(
            "{}({})",
            name(function)?,
            arguments
                .iter()
                .map(self::operand)
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )),
    }
}
fn aggregate(aggregate: &Aggregate) -> Result<String, Error> {
    let function = match aggregate.function {
        AggregateFunction::Count => "count".into(),
        AggregateFunction::Sum => "sum".into(),
        AggregateFunction::Avg => "avg".into(),
        AggregateFunction::Min => "min".into(),
        AggregateFunction::Max => "max".into(),
        AggregateFunction::Percentile(p) => format!("p{p}"),
        AggregateFunction::CountDistinct => "count_distinct".into(),
        AggregateFunction::Histogram => "histogram".into(),
        AggregateFunction::Correlation => "corr".into(),
    };
    if aggregate.fields.is_empty() {
        if aggregate.function != AggregateFunction::Count {
            return Err(invalid("aggregate needs a field"));
        }
        Ok(function)
    } else {
        Ok(format!(
            "{function}:{}",
            aggregate
                .fields
                .iter()
                .map(|field| name(field))
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        ))
    }
}
