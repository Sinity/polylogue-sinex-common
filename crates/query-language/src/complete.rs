//! Pure grammar-position completion against a product catalog.
use crate::ast::Span;
use crate::catalog::Catalog;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completion {
    pub value: String,
    pub replace: Span,
    pub kind: CompletionKind,
    pub help: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    Unit,
    Field,
    Value,
    Stage,
    Operator,
    Keyword,
    Command,
}
/// Complete at a UTF-8 byte position. A replacement covers the unfinished token.
pub fn complete(text: &str, position: usize, catalog: &Catalog) -> Vec<Completion> {
    let mut end = position.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &text[..end];
    let start = prefix
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_whitespace() || "|(),[]".contains(*c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let fragment = &prefix[start..];
    let before = &prefix[..start];
    // Quotes contain data; syntax completion does not invent words within them.
    if unclosed_quote(prefix) {
        return Vec::new();
    }
    let (unit, fields) = context(before, catalog);
    let mut out = Vec::new();
    let mut add = |value: &str, kind, help: &str, replace: Span, fragment: &str| {
        if value.starts_with(fragment) {
            out.push(Completion {
                value: value.into(),
                replace,
                kind,
                help: help.into(),
            });
        }
    };
    let span = Span { start, end };
    if let Some(field) = set_field(before) {
        if let Some(spec) = catalog.field(&unit, field) {
            for value in &spec.values {
                if value.starts_with(fragment) {
                    add(
                        &literal_spelling(value, &spec.unknown),
                        CompletionKind::Value,
                        &spec.help,
                        span,
                        "",
                    );
                }
            }
        }
    } else if let Some((field, partial)) = fragment.split_once(':') {
        if let Some(spec) = catalog.field(&unit, field) {
            for value in &spec.values {
                if value.starts_with(partial) {
                    add(
                        &literal_spelling(value, &spec.unknown),
                        CompletionKind::Value,
                        &spec.help,
                        Span {
                            start: start + field.len() + 1,
                            end,
                        },
                        "",
                    );
                }
            }
            for value in ["unknown", "*"] {
                add(
                    value,
                    CompletionKind::Value,
                    "",
                    Span {
                        start: start + field.len() + 1,
                        end,
                    },
                    partial,
                );
            }
        }
    } else {
        let words: Vec<&str> = before.split_whitespace().collect();
        let last = words.last().copied().unwrap_or("");
        let field_before = words.iter().rev().nth(1).copied();
        if ["=", "!=", ">", ">=", "<", "<=", "~", "^", "between"].contains(&last) {
            let field = field_before.unwrap_or("");
            if let Some(spec) = catalog.field(&unit, field) {
                for value in &spec.values {
                    if value.starts_with(fragment) {
                        add(
                            &literal_spelling(value, &spec.unknown),
                            CompletionKind::Value,
                            &spec.help,
                            span,
                            "",
                        );
                    }
                }
                for value in ["unknown", "*"] {
                    add(value, CompletionKind::Value, "", span, fragment);
                }
            }
        } else if fields.iter().any(|(name, _)| name == last) {
            for op in [
                ":", "=", "!=", ">", ">=", "<", "<=", "~", "^", "between", "in", "exists",
            ] {
                add(op, CompletionKind::Operator, "", span, fragment);
            }
        } else if before.trim_end().ends_with('|') {
            for stage in crate::catalog::STAGES {
                add(stage.completion, CompletionKind::Stage, "", span, fragment);
            }
            for ext in &catalog.extensions {
                if ext.position == crate::catalog::ExtensionPosition::Stage {
                    add(&ext.name, CompletionKind::Stage, &ext.help, span, fragment);
                }
            }
            for u in &catalog.units {
                add(&u.name, CompletionKind::Unit, &u.help, span, fragment);
            }
        } else if matches!(last, "exists") || before.trim().is_empty() {
            for u in &catalog.units {
                add(&u.name, CompletionKind::Unit, &u.help, span, fragment);
            }
            if before.trim().is_empty() {
                for command in &catalog.commands {
                    add(command, CompletionKind::Command, "", span, fragment);
                }
                for (name, help) in &fields {
                    add(name, CompletionKind::Field, help, span, fragment);
                }
            }
        } else if matches!(last, "sort" | "group") {
            add("by", CompletionKind::Keyword, "", span, fragment);
            if last == "sort" && catalog.unpiped_stages {
                for (name, help) in &fields {
                    add(name, CompletionKind::Field, help, span, fragment);
                }
            }
        } else if last == "as" {
            add("of", CompletionKind::Keyword, "", span, fragment);
        } else {
            for (name, help) in &fields {
                add(name, CompletionKind::Field, help, span, fragment);
            }
            if catalog.unpiped_stages {
                for stage in crate::catalog::STAGES {
                    add(stage.completion, CompletionKind::Stage, "", span, fragment);
                }
            }
            if matches!(last, "asc" | "desc") || before.contains("sort ") {
                for direction in ["asc", "desc"] {
                    add(direction, CompletionKind::Keyword, "", span, fragment);
                }
            }
            for keyword in ["AND", "OR", "NOT", "exists", "similar:", "query:"] {
                add(keyword, CompletionKind::Keyword, "", span, fragment);
            }
        }
    }
    out.sort_by(|a, b| a.value.cmp(&b.value));
    out.dedup_by(|a, b| a.value == b.value && a.replace == b.replace);
    out
}
fn unclosed_quote(text: &str) -> bool {
    let mut quote = false;
    let mut escaped = false;
    for c in text.chars() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            quote = !quote;
        }
    }
    quote
}
fn context(text: &str, catalog: &Catalog) -> (String, Vec<(String, String)>) {
    use crate::ast::{Source, StageKind};
    // Pick the innermost query/exists scope from delimiter tokens, while quoted data
    // contributes no grammar. Function and set parentheses retain the outer scope.
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut quote = false;
    let mut escape = false;
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut start = 0;
    for (at, c) in text.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        if c == '\\' && quote {
            escape = true;
            continue;
        }
        if c == '"' {
            quote = !quote;
            continue;
        }
        if quote {
            continue;
        }
        if c.is_whitespace() || "()|,".contains(c) {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            if c == '(' {
                let preceding = words.last().cloned().unwrap_or_default();
                let relation = words.iter().rev().nth(1).is_some_and(|s| s == "exists");
                stack.push((start, String::new()));
                if relation && catalog.unit(&preceding).is_some() {
                    stack.last_mut().unwrap().1 = preceding;
                    start = at + 1;
                } else if matches!(
                    preceding.as_str(),
                    "near"
                        | "before"
                        | "after"
                        | "during"
                        | "containing"
                        | "union"
                        | "intersect"
                        | "except"
                        | "fuse"
                        | "in"
                ) {
                    start = at + 1;
                }
            } else if c == ')'
                && let Some((outer, _)) = stack.pop()
            {
                start = outer;
            }
        } else {
            word.push(c);
        }
    }
    let inner = &text[start..];
    let explicit = stack
        .last()
        .map(|(_, u)| u.as_str())
        .filter(|u| !u.is_empty());
    let current = if let Some(unit) = explicit {
        format!("{unit} where {inner}")
    } else {
        inner.to_string()
    };
    let first = current.split_whitespace().next().unwrap_or("");
    let mut unit = catalog
        .unit(first)
        .map_or_else(|| catalog.default_unit.clone(), |u| u.name.clone());
    let mut candidate = current.trim_end();
    let mut parsed = None;
    loop {
        if let Ok(q) = crate::parse::parse(candidate, catalog) {
            parsed = Some(q);
            break;
        }
        let Some((at, _)) = candidate
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace() || *c == '|')
        else {
            break;
        };
        candidate = candidate[..at].trim_end();
    }
    let mut columns: Option<Vec<String>> = None;
    if let Some(q) = parsed {
        if let Source::Unit { unit: source, .. } = q.source {
            unit = source;
        }
        for stage in q.stages {
            match stage.kind {
                StageKind::Scope { unit: source, .. } => {
                    unit = source;
                    columns = None;
                }
                StageKind::Group { keys } => {
                    columns = Some(keys.iter().map(|key| key.column()).collect())
                }
                StageKind::Aggregate { aggregates } => {
                    let c = columns.get_or_insert_with(Vec::new);
                    c.extend(aggregates.iter().map(|a| a.column()));
                }
                StageKind::Fill => {
                    if let Some(c) = &mut columns {
                        c.push("coverage".into());
                    }
                }
                _ => {}
            }
        }
    }
    let fields = if let Some(columns) = columns {
        columns
            .into_iter()
            .map(|name| (name, String::new()))
            .collect()
    } else {
        catalog.unit(&unit).map_or_else(Vec::new, |u| {
            u.fields
                .iter()
                .map(|f| (f.name.clone(), f.help.clone()))
                .collect()
        })
    };
    (unit, fields)
}

fn set_field(before: &str) -> Option<&str> {
    let open = before.rfind('(')?;
    if before[open..].contains(')') {
        return None;
    }
    let head = before[..open].split_whitespace().last()?;
    head.strip_suffix(':')
}

fn literal_spelling(value: &str, unknown: &crate::catalog::UnknownRule) -> String {
    if matches!(value, "unknown" | "*" | "last" | "this")
        || matches!(unknown,crate::catalog::UnknownRule::Word(word) if word.eq_ignore_ascii_case(value))
        || value.starts_with('$')
        || value.is_empty()
        || value
            .chars()
            .any(|c| c.is_whitespace() || "()[]|,\"=<>!~^".contains(c))
    {
        crate::format::quote(value)
    } else {
        value.to_string()
    }
}
