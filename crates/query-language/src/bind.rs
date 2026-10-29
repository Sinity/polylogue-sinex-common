//! Binding syntax to immutable, caller-derived saved/result output schemas.
//! Products expand saved definitions or read kept column declarations first,
//! supply their typed outputs in the catalog, then bind before execution.
use crate::{Error, ErrorKind, ast::*, catalog::Catalog};
use std::collections::BTreeSet;

/// Constructed only by shared binding. Deserialization cannot forge a binding.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundQuery {
    query: Query,
}
impl BoundQuery {
    pub fn query(&self) -> &Query {
        &self.query
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
            .any(|other| other.source == output.source)
        {
            return Err(Error::new(
                ErrorKind::Ambiguous,
                "duplicate source output schema",
                0,
            ));
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
    let query = crate::parse::parse(&text, catalog)?;
    check_query(&query)?;
    Ok(BoundQuery { query })
}
fn check_query(query: &Query) -> Result<(), Error> {
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
        } => check_condition(condition)?,
        _ => {}
    }
    for attachment in &query.attach {
        for window in &attachment.windows {
            if let Window::Where(condition) = window {
                check_condition(condition)?;
            }
        }
    }
    for stage in &query.stages {
        match &stage.kind {
            StageKind::Where { condition }
            | StageKind::Scope {
                condition: Some(condition),
                ..
            } => check_condition(condition)?,
            StageKind::Combine { query, .. } | StageKind::Temporal { query, .. } => {
                check_query(query)?
            }
            StageKind::Extension { arguments, .. } => check_arguments(arguments)?,
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
fn check_condition(condition: &Condition) -> Result<(), Error> {
    match &condition.kind {
        ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
            for condition in conditions {
                check_condition(condition)?;
            }
        }
        ConditionKind::Not { condition } => check_condition(condition)?,
        ConditionKind::Exists {
            condition: Some(condition),
            ..
        }
        | ConditionKind::Count {
            condition: Some(condition),
            ..
        } => check_condition(condition)?,
        ConditionKind::In { query, .. } => check_query(query)?,
        ConditionKind::Sequence { steps, .. } => {
            for condition in steps {
                check_condition(condition)?;
            }
        }
        ConditionKind::Extension { arguments, .. } => check_arguments(arguments)?,
        _ => {}
    }
    Ok(())
}
fn check_arguments(arguments: &[ExtensionArgument]) -> Result<(), Error> {
    for argument in arguments {
        match argument {
            ExtensionArgument::Query(query) => check_query(query)?,
            ExtensionArgument::Condition(condition) => check_condition(condition)?,
            ExtensionArgument::Optional(arguments) => check_arguments(arguments)?,
            _ => {}
        }
    }
    Ok(())
}
