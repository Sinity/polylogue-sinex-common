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
    check_catalog(catalog)?;
    let text = crate::format::render_query(query, catalog)?;
    let parsed = crate::parse::parse_typed(&text, catalog)
        .map_err(|error| crate::parse::parse(&text, catalog).err().unwrap_or(error))?;
    check_query(&parsed.query, catalog)?;
    Ok(BoundQuery {
        query: parsed.query,
        typing: parsed.typing,
    })
}
/// Check a tree the parser just constructed without formatting or reparsing it.
/// Correction chaining owns its intermediate text; only its final tree is checked.
pub(crate) fn check_parsed(query: &Query, catalog: &Catalog) -> Result<(), Error> {
    check_catalog(catalog)?;
    check_query(query, catalog)
}
fn check_catalog(catalog: &Catalog) -> Result<(), Error> {
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
        check_schema(&output.schema, catalog)?;
    }
    Ok(())
}
fn check_schema(output: &crate::catalog::OutputSchema, catalog: &Catalog) -> Result<(), Error> {
    if let Some(unit) = &output.unit
        && catalog.unit(unit).is_none()
    {
        return Err(unbound(format!("output row domain {unit:?}")));
    }
    for unit in output.match_units.iter().flatten() {
        if catalog.unit(unit).is_none() {
            return Err(unbound(format!("match row domain {unit:?}")));
        }
    }
    let mut names = BTreeSet::new();
    for field in &output.fields {
        if field.name.is_empty() || !names.insert(field.name.as_str()) {
            return Err(Error::new(
                ErrorKind::BadValue,
                "output fields need distinct nonempty names",
                0,
            ));
        }
    }
    if output
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
    Ok(())
}

fn check_query(query: &Query, catalog: &Catalog) -> Result<(), Error> {
    crate::depth::deeper(|| {
        if let Source::Unit { unit, .. } = &query.source
            && catalog.unit(unit).is_none()
        {
            return Err(unbound(format!("implicit row domain {unit:?}")));
        }
        match &query.source {
            Source::Saved {
                output: SourceColumns::Unknown,
                name,
                ..
            } => return Err(unbound(format!("saved query {name:?}"))),
            Source::Result {
                output: SourceColumns::Unknown,
                reference,
                ..
            } => return Err(unbound(format!("kept result {reference:?}"))),
            Source::Unit {
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
        for transform in &query.transforms {
            match &transform.kind {
                TransformKind::Where { condition }
                | TransformKind::Scope {
                    condition: Some(condition),
                    ..
                } => check_condition(condition, catalog)?,
                TransformKind::Combine { query, .. }
                | TransformKind::Join { query, .. }
                | TransformKind::Diff { query } => check_query(query, catalog)?,
                TransformKind::Extension { arguments, .. } => check_arguments(arguments, catalog)?,
                _ => {}
            }
        }
        Ok(())
    })
}
fn unbound(source: String) -> Error {
    Error::new(
        ErrorKind::BadValue,
        format!("{source} has no bound output schema"),
        0,
    )
    .hint("Supply its actual typed output snapshot before execution")
}
fn check_condition(condition: &Condition, catalog: &Catalog) -> Result<(), Error> {
    crate::depth::deeper(|| {
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
            ConditionKind::In { query, .. } | ConditionKind::Order { query, .. } => {
                check_query(query, catalog)?
            }
            ConditionKind::Sequence { steps, .. } => {
                for condition in steps {
                    check_condition(condition, catalog)?;
                }
            }
            ConditionKind::Extension { arguments, .. } => check_arguments(arguments, catalog)?,
            _ => {}
        }
        Ok(())
    })
}
fn check_arguments(arguments: &[ExtensionArgument], catalog: &Catalog) -> Result<(), Error> {
    crate::depth::deeper(|| {
        for argument in arguments {
            match argument {
                ExtensionArgument::Query(query) => check_query(query, catalog)?,
                ExtensionArgument::Condition(condition) => check_condition(condition, catalog)?,
                ExtensionArgument::Optional(arguments) | ExtensionArgument::List(arguments) => {
                    check_arguments(arguments, catalog)?
                }
                _ => {}
            }
        }
        Ok(())
    })
}
