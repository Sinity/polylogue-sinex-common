//! Deterministic shared surface text from typed syntax. Values never become syntax.
use crate::ast::*;
use crate::catalog::{Catalog, ExtensionPosition, ExtensionSlot};
use crate::{Error, ErrorKind};

/// Render capital boolean operators and piped transforms while preserving
/// whether a source unit was explicit or inherited.
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
        for transform in &query.transforms {
            text.push_str(" | ");
            text.push_str(&self.transform(&transform.kind)?);
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
            ConditionKind::Compare { left, op, right } => format!(
                "{} {} {}",
                self.expression(left)?,
                op.symbol(),
                match right {
                    Expression::Value { value: literal } => value(literal)?,
                    other => self.expression(other)?,
                }
            ),
            ConditionKind::Since { value: literal, .. } => format!("since:{}", value(literal)?),
            ConditionKind::Until { value: literal, .. } => format!("until:{}", value(literal)?),
            ConditionKind::Between { left, from, to } => format!(
                "{} between {} and {}",
                self.expression(left)?,
                value(from)?,
                value(to)?
            ),
            ConditionKind::TimeRange {
                left,
                value: literal,
            } => format!("{} in {}", self.expression(left)?, value(literal)?),
            ConditionKind::In { left, query } => {
                format!("{} in ({})", self.expression(left)?, self.query(query)?)
            }
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
            ConditionKind::Rank {
                list,
                value: literal,
            } => format!("{}:{}", list.name(), value(literal)?),
            ConditionKind::Exists { unit, condition } => format!(
                "exists {}({})",
                name(unit)?,
                condition
                    .as_ref()
                    .map(|c| self.condition(c))
                    .transpose()?
                    .unwrap_or_default()
            ),
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
                        Gap::Within { distance: d } => {
                            text.push_str(&format!("[within {}] ", distance(d)?))
                        }
                    }
                    text.push_str(&self.condition(step)?);
                }
                text.push(')');
                text
            }
            ConditionKind::Order {
                relation,
                query,
                within,
                at,
                scope,
            } => format!(
                "{}({}){}",
                relation.name(),
                self.query(query)?,
                order_options(within.as_ref(), *at, scope.as_deref())?
            ),
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
    /// Arithmetic renders without parentheses, so a tree whose shape differs
    /// from operator precedence and left association has no spelling.
    fn expression(&self, expression: &Expression) -> Result<String, Error> {
        Ok(match expression {
            Expression::Field { name: field } => name(field)?,
            Expression::Value { value: literal } => match literal {
                Value::Scalar {
                    text,
                    quoted: false,
                } if crate::parse::is_literal(text) => text.clone(),
                other => value(other)?,
            },
            Expression::Function {
                name: function,
                arguments,
            } => format!(
                "{}({})",
                name(function)?,
                arguments
                    .iter()
                    .map(|argument| self.expression(argument))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            Expression::Arithmetic {
                operator,
                left,
                right,
            } => {
                let looser = |child: &Expression, or_equal: bool| {
                    matches!(child, Expression::Arithmetic { operator: inner, .. }
                        if (!inner.binds_tightly() && operator.binds_tightly())
                            || (or_equal && inner.binds_tightly() == operator.binds_tightly()))
                };
                if looser(left, false) || looser(right, true) {
                    return Err(invalid(
                        "arithmetic needs parentheses, which the language does not have",
                    ));
                }
                format!(
                    "{} {} {}",
                    self.expression(left)?,
                    operator.symbol(),
                    self.expression(right)?
                )
            }
        })
    }
    fn column(&self, column: &Column) -> Result<String, Error> {
        let expression = self.expression(&column.expression)?;
        Ok(match &column.name {
            Some(alias) => format!("{expression} as {}", name(alias)?),
            None => expression,
        })
    }
    fn transform(&self, transform: &TransformKind) -> Result<String, Error> {
        Ok(match transform {
            TransformKind::Where { condition } => format!("where {}", self.condition(condition)?),
            TransformKind::Sort { keys } => {
                if keys.is_empty() {
                    return Err(invalid("sort needs a key"));
                }
                format!(
                    "sort by {}",
                    keys.iter()
                        .map(|key| Ok(format!(
                            "{} {}",
                            self.expression(&key.expression)?,
                            if key.descending { "desc" } else { "asc" }
                        )))
                        .collect::<Result<Vec<_>, Error>>()?
                        .join(", ")
                )
            }
            TransformKind::Group { keys } => format!(
                "group by {}",
                keys.iter()
                    .map(|key| self.column(key))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            TransformKind::Aggregate { aggregates } => format!(
                "agg {}",
                aggregates
                    .iter()
                    .map(|a| self.aggregate(a))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            TransformKind::Limit { count } => format!("limit {count}"),
            TransformKind::Offset { count } => format!("offset {count}"),
            TransformKind::First { count } => format!("first {count}"),
            TransformKind::Distinct => "distinct".into(),
            TransformKind::Matches => "matches".into(),
            TransformKind::Fill => "fill".into(),
            TransformKind::Fields { columns } => format!(
                "fields {}",
                columns
                    .iter()
                    .map(|column| self.column(column))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
            TransformKind::Scope { unit, condition } => format!(
                "{}{}",
                name(unit)?,
                condition
                    .as_ref()
                    .map(|c| self.condition(c).map(|text| format!(" where {text}")))
                    .transpose()?
                    .unwrap_or_default()
            ),
            TransformKind::Combine { how, query } => format!(
                "{} ({})",
                match how {
                    Combination::Union => "union",
                    Combination::Intersect => "intersect",
                    Combination::Except => "except",
                    Combination::Fuse => "fuse",
                },
                self.query(query)?
            ),
            TransformKind::Join { query, on } => match on {
                JoinOn::Order {
                    relation,
                    within,
                    at,
                    scope,
                } => format!(
                    "join {} ({}){}",
                    relation.name(),
                    self.query(query)?,
                    order_options(within.as_ref(), *at, scope.as_deref())?
                ),
                JoinOn::Key { left, right } => format!(
                    "join ({}) on {} = {}",
                    self.query(query)?,
                    name(left)?,
                    name(right)?
                ),
            },
            TransformKind::Diff { query } => format!("diff ({})", self.query(query)?),
            TransformKind::AsOf {
                value: literal,
                sequence,
            } => format!(
                "as of {}{}",
                if *sequence { "seq " } else { "" },
                value(literal)?
            ),
            TransformKind::Extension { name, arguments } => {
                self.extension(name, arguments, ExtensionPosition::Transform)?
            }
        })
    }
    fn aggregate(&self, aggregate: &Aggregate) -> Result<String, Error> {
        if !aggregate
            .function
            .arguments()
            .contains(&aggregate.arguments.len())
        {
            return Err(invalid(format!(
                "{} has the wrong number of arguments",
                aggregate.function.name()
            )));
        }
        let mut text = aggregate.function.name();
        if !aggregate.arguments.is_empty() {
            text = format!(
                "{text}({})",
                aggregate
                    .arguments
                    .iter()
                    .map(|argument| self.expression(argument))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            );
        }
        if let Some(alias) = &aggregate.name {
            text.push_str(&format!(" as {}", name(alias)?));
        }
        Ok(text)
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
            .map(|(slot, argument)| self.extension_argument(slot, argument))
            .collect()
    }
    fn extension_argument(
        &self,
        slot: &ExtensionSlot,
        argument: &ExtensionArgument,
    ) -> Result<String, Error> {
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
            (ExtensionSlot::Expression, ExtensionArgument::Expression(expression)) => {
                self.expression(expression)?
            }
            (ExtensionSlot::List(slot), ExtensionArgument::List(items)) => {
                if items.is_empty() {
                    return Err(invalid("an extension list needs at least one item"));
                }
                items
                    .iter()
                    .map(|item| self.extension_argument(slot, item))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
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
    }
}
fn distance(distance: &Distance) -> Result<String, Error> {
    Ok(match distance {
        Distance::Time { duration } => value(duration)?,
        Distance::Steps { count } => {
            format!("{count} {}", if *count == 1 { "step" } else { "steps" })
        }
    })
}
fn order_options(
    within: Option<&Distance>,
    at: TimePoint,
    scope: Option<&str>,
) -> Result<String, Error> {
    let mut text = String::new();
    if let Some(within) = within {
        text.push_str(&format!(" within {}", distance(within)?));
    }
    match at {
        TimePoint::When => {}
        TimePoint::Start => text.push_str(" at start"),
        TimePoint::End => text.push_str(" at end"),
    }
    if let Some(scope) = scope {
        text.push_str(&format!(" in {}", name(scope)?));
    }
    Ok(text)
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
        Value::Known => "known".into(),
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
