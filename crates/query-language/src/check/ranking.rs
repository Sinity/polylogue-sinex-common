//! A ranking atom orders the whole relation; where it may stand.
use crate::ast::*;
use crate::{Error, ErrorKind};

/// A ranking atom orders the whole relation, so it stands only in a top-level
/// AND of a query's conditions; under OR or NOT, or inside `exists`, `count`
/// or `seq`, it is refused.
pub(crate) fn check_ranking(query: &Query, text: Option<&str>) -> Result<(), Error> {
    /// Where a condition stands: `None` is a top-level AND position; otherwise
    /// why a ranking atom cannot stand there, and the OR it is a direct
    /// operand of when that OR is itself in a top-level position.
    #[derive(Clone, Copy)]
    struct Place<'a> {
        refused: Option<&'static str>,
        top_or: Option<&'a Condition>,
    }
    const TOP: Place<'static> = Place {
        refused: None,
        top_or: None,
    };
    fn nested(why: &'static str) -> Place<'static> {
        Place {
            refused: Some(why),
            top_or: None,
        }
    }
    fn condition(c: &Condition, place: Place<'_>, text: Option<&str>) -> Result<(), Error> {
        crate::depth::deeper(|| {
            match &c.kind {
                ConditionKind::And { conditions } => {
                    let inner = Place {
                        top_or: None,
                        ..place
                    };
                    for child in conditions {
                        condition(child, inner, text)?;
                    }
                }
                ConditionKind::Or { conditions } => {
                    let inner = Place {
                        refused: Some(place.refused.unwrap_or("under OR")),
                        top_or: place.refused.is_none().then_some(c),
                    };
                    for child in conditions {
                        condition(child, inner, text)?;
                    }
                }
                ConditionKind::Not { condition: inner } => {
                    condition(inner, nested("under NOT"), text)?
                }
                ConditionKind::Rank { list, .. } if place.refused.is_some() => {
                    let mut error = Error::new(
                        ErrorKind::InvalidQuery,
                        format!(
                            "{}: ranks the whole query and cannot sit {}",
                            list.name(),
                            place.refused.unwrap_or_default()
                        ),
                        c.span.start,
                    )
                    .length(c.span.end - c.span.start);
                    if let Some(or) = place.top_or
                        && let Some(text) = text
                        && let ConditionKind::Or { conditions } = &or.kind
                    {
                        let rest: Vec<&str> = conditions
                            .iter()
                            .filter(|other| other.span != c.span)
                            .map(|other| &text[other.span.start..other.span.end])
                            .collect();
                        let rest = if rest.len() == 1 {
                            rest[0].to_string()
                        } else {
                            format!("({})", rest.join(" OR "))
                        };
                        error = error.suggestion(format!(
                            "{}{rest} {}{}",
                            &text[..or.span.start],
                            &text[c.span.start..c.span.end],
                            &text[or.span.end..]
                        ));
                    }
                    return Err(error);
                }
                ConditionKind::Exists {
                    condition: Some(inner),
                    ..
                }
                | ConditionKind::Count {
                    condition: Some(inner),
                    ..
                } => condition(inner, nested("inside a related-row condition"), text)?,
                ConditionKind::Sequence { steps, .. } => {
                    for step in steps {
                        condition(step, nested("inside a sequence"), text)?;
                    }
                }
                ConditionKind::In { query, .. } | ConditionKind::Order { query, .. } => {
                    check_ranking(query, text)?
                }
                ConditionKind::Extension { arguments, .. } => arguments_check(arguments, text)?,
                _ => {}
            }
            Ok(())
        })
    }
    fn arguments_check(arguments: &[ExtensionArgument], text: Option<&str>) -> Result<(), Error> {
        for argument in arguments {
            match argument {
                ExtensionArgument::Condition(c) => {
                    condition(c, nested("inside an extension"), text)?
                }
                ExtensionArgument::Query(q) => check_ranking(q, text)?,
                ExtensionArgument::Optional(a) | ExtensionArgument::List(a) => {
                    arguments_check(a, text)?
                }
                _ => {}
            }
        }
        Ok(())
    }
    match &query.source {
        Source::Unit {
            condition: Some(c), ..
        }
        | Source::Saved {
            condition: Some(c), ..
        }
        | Source::Result {
            condition: Some(c), ..
        } => condition(c, TOP, text)?,
        _ => {}
    }
    for attachment in &query.attach {
        for window in &attachment.windows {
            if let Window::Where(c) = window {
                condition(c, nested("inside an attachment window"), text)?;
            }
        }
    }
    for transform in &query.transforms {
        match &transform.kind {
            TransformKind::Where { condition: c }
            | TransformKind::Scope {
                condition: Some(c), ..
            } => condition(c, TOP, text)?,
            TransformKind::Combine { query, .. }
            | TransformKind::Join { query, .. }
            | TransformKind::Diff { query } => check_ranking(query, text)?,
            TransformKind::Extension { arguments, .. } => arguments_check(arguments, text)?,
            _ => {}
        }
    }
    Ok(())
}
