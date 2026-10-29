//! Canonical question identity. Paging and resolved clocks never enter this form.
use crate::{Error, ErrorKind, ast::Query, catalog::Catalog};
mod scalars;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QuestionId(pub [u8; 32]);
impl std::fmt::Display for QuestionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Resolve product references and saved names to stored identities before calling.
/// Relative time expressions deliberately remain expressions.
pub fn normal_form(query: &Query, catalog: &Catalog) -> Result<Value, Error> {
    let mut query = query.clone();
    scalars::canonicalize(&mut query, catalog);
    let mut value = serde_json::to_value(query).map_err(serialization)?;
    normalize(&mut value);
    Ok(value)
}

/// Parameter declarations are part of a saved question's identity, names are not.
pub fn question_id(
    query: &Query,
    catalog: &Catalog,
    parameters: &BTreeMap<String, String>,
) -> Result<QuestionId, Error> {
    let canonical =
        serde_json::to_vec(&(normal_form(query, catalog)?, parameters)).map_err(serialization)?;
    Ok(QuestionId(*blake3::hash(&canonical).as_bytes()))
}

fn serialization(error: serde_json::Error) -> Error {
    Error::new(
        ErrorKind::InvalidQuery,
        format!("question cannot be serialized: {error}"),
        0,
    )
}

fn normalize(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize(value);
            }
        }
        Value::Object(fields) => {
            fields.remove("span");
            if fields.get("implicit") == Some(&Value::Bool(false)) {
                fields.remove("implicit");
            }
            if matches!(
                fields.get("kind").and_then(Value::as_str),
                Some("saved" | "result")
            ) {
                fields.remove("output");
            }
            if fields.get("kind").and_then(Value::as_str) == Some("scalar") {
                fields.remove("quoted");
            }
            for value in fields.values_mut() {
                normalize(value);
            }
            let unordered = matches!(
                fields.get("kind").and_then(Value::as_str),
                Some("and" | "or" | "set")
            );
            if unordered {
                let kind = fields
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if let Some(Value::Array(conditions)) = fields.get_mut("conditions") {
                    let mut flat = Vec::new();
                    for condition in std::mem::take(conditions) {
                        let nested = condition
                            .get("kind")
                            .filter(|node| {
                                node.get("kind").and_then(Value::as_str) == Some(kind.as_str())
                            })
                            .and_then(|node| node.get("conditions"))
                            .and_then(Value::as_array);
                        if let Some(nested) = nested {
                            flat.extend(nested.iter().cloned());
                        } else {
                            flat.push(condition);
                        }
                    }
                    *conditions = flat;
                }
                for key in ["conditions", "values"] {
                    if let Some(Value::Array(values)) = fields.get_mut(key) {
                        // Child serialization is infallible for a JSON value.
                        values.sort_by_cached_key(|value| value.to_string());
                        values.dedup();
                    }
                }
                if matches!(kind.as_str(), "and" | "or")
                    && let Some(Value::Array(conditions)) = fields.get("conditions")
                    && conditions.len() == 1
                    && let Some(inner) = conditions[0].get("kind")
                {
                    *value = inner.clone();
                }
            }
        }
        _ => {}
    }
}
