//! Pure grammar-position completion against a product catalog.
use crate::ast::Span;
use crate::catalog::{Catalog, Field, FieldKind};
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
    // Within a quoted value, only product-declared values can complete.
    if let Some(open) = open_quote(prefix) {
        return quoted_values(text, end, open, catalog);
    }
    let scope = context(before, catalog);
    let fields: Vec<_> = scope
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.help.as_str()))
        .collect();
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
        if let Some(spec) = scope.field(field) {
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
        if let Some(spec) = scope.field(field) {
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
            if let Some(spec) = scope.field(field) {
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
        } else if scope.field(last).is_some() {
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
fn open_quote(text: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' && quote.is_some() {
            escaped = true;
        } else if c == '"' {
            quote = if quote.is_some() { None } else { Some(at) };
        }
    }
    quote
}

fn quoted_values(text: &str, cursor: usize, open: usize, catalog: &Catalog) -> Vec<Completion> {
    let before = &text[..open];
    let words: Vec<_> = before.split_whitespace().collect();
    let last = words.last().copied().unwrap_or("");
    let field = set_field(before)
        .or_else(|| last.strip_suffix(':'))
        .or_else(|| {
            ["=", "!=", ">", ">=", "<", "<=", "~", "^", "between"]
                .contains(&last)
                .then(|| words.iter().rev().nth(1).copied())
                .flatten()
        });
    let scope = context(before, catalog);
    let Some(spec) = field.and_then(|field| scope.field(field)) else {
        return Vec::new();
    };
    let mut partial = String::new();
    let mut escaped = false;
    for c in text[open + 1..cursor].chars() {
        if escaped {
            partial.push(match c {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else {
            partial.push(c);
        }
    }
    if escaped {
        partial.push('\\');
    }
    let mut end = cursor;
    let mut escaped = false;
    for (offset, c) in text[open + 1..].char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            end = open + 1 + offset + 1;
            break;
        }
    }
    let mut values: Vec<_> = spec
        .values
        .iter()
        .filter(|value| value.starts_with(&partial))
        .map(|value| Completion {
            value: crate::format::quote(value),
            replace: Span { start: open, end },
            kind: CompletionKind::Value,
            help: spec.help.clone(),
        })
        .collect();
    values.sort_by(|a, b| a.value.cmp(&b.value));
    values.dedup_by(|a, b| a.value == b.value);
    values
}
#[derive(Default)]
struct CompletionScope {
    fields: Vec<Field>,
}
impl CompletionScope {
    fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }
    fn unit(name: &str, catalog: &Catalog) -> Self {
        Self {
            fields: catalog
                .unit(name)
                .map(|unit| unit.fields.clone())
                .unwrap_or_default(),
        }
    }
}
fn context(text: &str, catalog: &Catalog) -> CompletionScope {
    use crate::ast::{Source, StageKind};
    // Pick the innermost query/exists scope from delimiter tokens, while quoted data
    // contributes no grammar. Function and set parentheses retain the outer scope.
    let mut stack: Vec<(usize, String, bool)> = Vec::new();
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
                stack.push((start, String::new(), false));
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
                    let unit = row_unit(&text[start..at], catalog)
                        .unwrap_or_else(|| "$deferred_completion".into());
                    let entry = stack.last_mut().unwrap();
                    entry.1 = unit;
                    entry.2 = true;
                    start = at + 1;
                }
            } else if c == ')'
                && let Some((outer, _, _)) = stack.pop()
            {
                start = outer;
            }
        } else {
            word.push(c);
        }
    }
    let inner = &text[start..];
    let explicit = stack.iter().rev().find(|(_, unit, _)| !unit.is_empty());
    let inner_first = inner.split_whitespace().next().unwrap_or("");
    let inner_source = catalog.unit(inner_first).is_some() || source_prefix(inner_first);
    let inherited_deferred = explicit.is_some_and(|(_, unit, inherited)| {
        *inherited && unit == "$deferred_completion" && !inner_source
    });
    let current = if let Some((_, unit, inherited)) = explicit {
        if *inherited && inner_source {
            inner.to_string()
        } else {
            format!("{unit} where {inner}")
        }
    } else {
        inner.to_string()
    };
    let first = current.split_whitespace().next().unwrap_or("");
    let mut unit = catalog
        .unit(first)
        .map_or_else(|| catalog.default_unit.clone(), |u| u.name.clone());
    let parsed = parse_prefix(&current, catalog);
    // A source-like prefix cannot acquire the default unit's fields merely
    // because an unfinished suffix forced the parser to shorten its input.
    let source_like = source_prefix(first) || inherited_deferred;
    let mut scope = if source_like {
        CompletionScope::default()
    } else {
        CompletionScope::unit(&unit, catalog)
    };
    let mut group_input = None;
    if let Some(q) = parsed {
        match &q.source {
            Source::Unit { unit: source, .. } if !source_like => {
                unit = source.clone();
                scope = CompletionScope::unit(&unit, catalog);
            }
            Source::Saved { .. } | Source::Result { .. } => {
                scope.fields = catalog
                    .source_output(&q.source)
                    .map(|output| output.fields.clone())
                    .unwrap_or_default();
            }
            _ => {}
        }
        for stage in q.stages {
            match stage.kind {
                StageKind::Scope { unit: source, .. } => {
                    unit = source;
                    scope = CompletionScope::unit(&unit, catalog);
                    group_input = None;
                }
                StageKind::Group { keys } => {
                    let fields = keys
                        .iter()
                        .map(|key| {
                            let mut field = match key {
                                crate::ast::Operand::Field { name } => {
                                    scope.field(name).cloned().unwrap_or_default()
                                }
                                _ => Field::default(),
                            };
                            field.name = key.column();
                            field
                        })
                        .chain(std::iter::once(Field {
                            name: "count".into(),
                            kind: FieldKind::Integer,
                            ..Field::default()
                        }))
                        .collect();
                    group_input = Some(std::mem::replace(&mut scope.fields, fields));
                }
                StageKind::Aggregate { aggregates } => {
                    let grouped = group_input.is_some();
                    let input = group_input.take().unwrap_or_else(|| scope.fields.clone());
                    if !grouped {
                        scope.fields.clear();
                    }
                    for aggregate in aggregates {
                        let kind = match aggregate.function {
                            crate::ast::AggregateFunction::Count
                            | crate::ast::AggregateFunction::CountDistinct => FieldKind::Integer,
                            _ => aggregate
                                .fields
                                .first()
                                .and_then(|name| input.iter().find(|field| &field.name == name))
                                .map(|field| field.kind.clone())
                                .unwrap_or(FieldKind::Number),
                        };
                        let name = aggregate.column();
                        scope.fields.retain(|field| field.name != name);
                        scope.fields.push(Field {
                            name,
                            kind,
                            ..Field::default()
                        });
                    }
                }
                StageKind::Fill => scope.fields.push(Field {
                    name: "coverage".into(),
                    ..Field::default()
                }),
                StageKind::Fields { fields } => {
                    scope.fields.retain(|field| fields.contains(&field.name))
                }
                _ => {}
            }
        }
    }
    scope
}

fn source_prefix(first: &str) -> bool {
    first.split_once(':').is_some_and(|(prefix, _)| {
        prefix.eq_ignore_ascii_case("query") || prefix.eq_ignore_ascii_case("result")
    })
}
fn parse_prefix(text: &str, catalog: &Catalog) -> Option<crate::ast::Query> {
    let mut candidate = text.trim_end();
    loop {
        if let Ok(query) = crate::parse::parse(candidate, catalog) {
            return Some(query);
        }
        let (at, _) = candidate
            .char_indices()
            .rev()
            .find(|(_, c)| c.is_whitespace() || *c == '|')?;
        candidate = candidate[..at].trim_end();
    }
}
fn row_unit(text: &str, catalog: &Catalog) -> Option<String> {
    use crate::ast::{Source, StageKind};
    let query = parse_prefix(text, catalog)?;
    let mut unit = match &query.source {
        Source::Unit { unit, .. } => Some(unit.clone()),
        Source::Saved { .. } | Source::Result { .. } => catalog
            .source_output(&query.source)
            .and_then(|output| output.unit.clone()),
        _ => None,
    };
    for stage in query.stages {
        if let StageKind::Scope { unit: next, .. } = stage.kind {
            unit = Some(next);
        }
    }
    unit
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
