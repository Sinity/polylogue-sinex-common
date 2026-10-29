//! Qualifiers and product-declared Matches outputs share one syntax collector.
use crate::{Error, ErrorKind, ast::*, catalog::*};

/// Collect only top-level AND qualifiers of the actual relation. OR, NOT and
/// nested child conditions do not nominate projections of their enclosing rows.
pub fn patterns(condition: Option<&Condition>) -> Vec<MatchPattern> {
    fn collect(condition: &Condition, out: &mut Vec<MatchPattern>) {
        match &condition.kind {
            ConditionKind::And { conditions } => conditions.iter().for_each(|c| collect(c, out)),
            ConditionKind::Exists { unit, .. } => {
                out.push(MatchPattern::Exists { unit: unit.clone() })
            }
            ConditionKind::Sequence { unit, steps, .. } => out.push(MatchPattern::Sequence {
                unit: unit.clone(),
                steps: steps.len(),
            }),
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(condition) = condition {
        collect(condition, &mut out);
    }
    out
}

pub(crate) fn source_context(
    source: &Source,
    catalog: &Catalog,
) -> (Option<String>, Option<Vec<MatchPattern>>) {
    let (unit, mut available, condition) = match source {
        Source::Unit {
            unit, condition, ..
        }
        | Source::RecallLast { unit, condition } => {
            (Some(unit.clone()), Some(Vec::new()), condition.as_ref())
        }
        Source::Saved { condition, .. } | Source::Result { condition, .. } => {
            let output = catalog.source_output(source);
            (
                output.and_then(|output| output.unit.clone()),
                output.map(|output| output.match_patterns.clone()),
                condition.as_ref(),
            )
        }
        _ => (None, Some(Vec::new()), None),
    };
    if let Some(available) = &mut available {
        available.extend(patterns(condition));
    }
    (
        unit.as_deref()
            .and_then(|unit| catalog.unit(unit))
            .map(|unit| unit.name.clone()),
        available,
    )
}

pub(crate) fn request(
    unit: Option<&str>,
    available: Option<&[MatchPattern]>,
    at: usize,
) -> Result<Option<MatchSource>, Error> {
    let Some(available) = available else {
        return Ok(None);
    };
    let [pattern] = available else {
        return Err(Error::new(
            if available.len() > 1 {
                ErrorKind::Ambiguous
            } else {
                ErrorKind::InvalidQuery
            },
            format!(
                "matches needs exactly one top-level exists or sequence qualifier; this relation has {}",
                available.len()
            ),
            at,
        ));
    };
    let unit = unit.ok_or_else(|| {
        Error::new(
            ErrorKind::BadValue,
            "matches input has no known row domain",
            at,
        )
    })?;
    Ok(Some(MatchSource {
        parent_unit: unit.into(),
        pattern: pattern.clone(),
    }))
}

/// A projected row shape cannot reuse its old relation qualifier. Selection
/// and distinct preserve the actual row domain and its per-row facts.
pub(crate) fn invalidates(stage: &StageKind) -> bool {
    matches!(
        stage,
        StageKind::Group { .. } | StageKind::Aggregate { .. } | StageKind::Fields { .. }
    )
}
