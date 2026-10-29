//! Binding syntax to immutable, caller-derived saved/result output schemas.
//! Products expand saved definitions or read kept column declarations first,
//! supply their typed outputs in the catalog, then bind before execution.
use crate::{Error, ErrorKind, ast::*, catalog::Catalog};
use std::collections::BTreeSet;

/// Constructed only by shared binding. Deserialization cannot forge a binding.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundQuery {
    query: Query,
    typing: crate::scope::Typing,
}
impl BoundQuery {
    pub fn query(&self) -> &Query {
        &self.query
    }
    /// Semantic scopes and structural operand addresses in `query()`.
    pub fn typing(&self) -> &crate::scope::Typing {
        &self.typing
    }
    /// Move the exact bound tree and its independently retainable type snapshot.
    pub fn into_parts(self) -> (Query, crate::scope::Typing) {
        (self.query, self.typing)
    }
    pub fn into_query(self) -> Query {
        self.query
    }
}
/// Validate every source and suffix against the supplied immutable output
/// snapshot. Rendering and reparsing reuse the parser's one field/type checker;
/// output markers supplied by an incoming syntax tree are never trusted.
pub fn bind(query: &Query, catalog: &Catalog) -> Result<BoundQuery, Error> {
    for (index, output) in catalog.source_outputs.iter().enumerate() {
        if catalog.source_outputs[..index]
            .iter()
            .any(|other| crate::catalog::same_output_source(&other.source, &output.source))
        {
            return Err(Error::new(
                ErrorKind::Ambiguous,
                "duplicate source output schema",
                0,
            ));
        }
        if let Some(unit) = &output.schema.unit
            && catalog.unit(unit).is_none()
        {
            return Err(unbound(format!("output row domain {unit:?}")));
        }
        let mut names = BTreeSet::new();
        for field in &output.schema.fields {
            if field.name.is_empty() || !names.insert(field.name.as_str()) {
                return Err(Error::new(
                    ErrorKind::BadValue,
                    "output fields need distinct nonempty names",
                    0,
                ));
            }
        }
        if output
            .schema
            .main_time
            .as_ref()
            .is_some_and(|name| !names.contains(name.as_str()))
        {
            return Err(Error::new(
                ErrorKind::BadValue,
                "output main time is not an output field",
                0,
            ));
        }
    }
    let text = crate::format::render_query(query, catalog)?;
    let (query, typing) = crate::parse::parse_typed(&text, catalog)
        .map_err(|error| crate::parse::parse(&text, catalog).err().unwrap_or(error))?;
    check_query(&query, catalog)?;
    Ok(BoundQuery { query, typing })
}
fn check_query(query: &Query, catalog: &Catalog) -> Result<(), Error> {
    if let Source::Unit { unit, .. } = &query.source
        && catalog.unit(unit).is_none()
    {
        return Err(unbound(format!("implicit row domain {unit:?}")));
    }
    match &query.source {
        Source::Saved {
            output: OutputBinding::Deferred,
            name,
            ..
        } => return Err(unbound(format!("saved query {name:?}"))),
        Source::Result {
            output: OutputBinding::Deferred,
            reference,
            ..
        } => return Err(unbound(format!("kept result {reference:?}"))),
        Source::Unit {
            condition: Some(condition),
            ..
        }
        | Source::RecallLast {
            condition: Some(condition),
            ..
        }
        | Source::Saved {
            condition: Some(condition),
            ..
        }
        | Source::Result {
            condition: Some(condition),
            ..
        } => check_condition(condition, catalog)?,
        _ => {}
    }
    for attachment in &query.attach {
        for window in &attachment.windows {
            if let Window::Where(condition) = window {
                check_condition(condition, catalog)?;
            }
        }
    }
    for stage in &query.stages {
        match &stage.kind {
            StageKind::Where { condition }
            | StageKind::Scope {
                condition: Some(condition),
                ..
            } => check_condition(condition, catalog)?,
            StageKind::Combine { query, .. } | StageKind::Temporal { query, .. } => {
                check_query(query, catalog)?
            }
            StageKind::Extension { arguments, .. } => check_arguments(arguments, catalog)?,
            _ => {}
        }
    }
    Ok(())
}
fn unbound(source: String) -> Error {
    Error::new(
        ErrorKind::BadValue,
        format!("{source} has no bound output schema"),
        0,
    )
    .hint("Supply its expanded or kept typed output before execution")
}
fn check_condition(condition: &Condition, catalog: &Catalog) -> Result<(), Error> {
    match &condition.kind {
        ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
            for condition in conditions {
                check_condition(condition, catalog)?;
            }
        }
        ConditionKind::Not { condition } => check_condition(condition, catalog)?,
        ConditionKind::Exists {
            condition: Some(condition),
            ..
        }
        | ConditionKind::Count {
            condition: Some(condition),
            ..
        } => check_condition(condition, catalog)?,
        ConditionKind::In { query, .. } => check_query(query, catalog)?,
        ConditionKind::Sequence { steps, .. } => {
            for condition in steps {
                check_condition(condition, catalog)?;
            }
        }
        ConditionKind::Extension { arguments, .. } => check_arguments(arguments, catalog)?,
        _ => {}
    }
    Ok(())
}
fn check_arguments(arguments: &[ExtensionArgument], catalog: &Catalog) -> Result<(), Error> {
    for argument in arguments {
        match argument {
            ExtensionArgument::Query(query) => check_query(query, catalog)?,
            ExtensionArgument::Condition(condition) => check_condition(condition, catalog)?,
            ExtensionArgument::Optional(arguments) => check_arguments(arguments, catalog)?,
            _ => {}
        }
    }
    Ok(())
}
