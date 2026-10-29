//! Pure grammar-position completion against a product catalog.
use crate::ast::Span;
use crate::catalog::{Catalog, Field, FieldKind, MatchSource, OutputSource};
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Completion {
    pub value: String,
    pub replace: Span,
    pub kind: CompletionKind,
    pub help: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
/// Pure catalog sections needed at one completion cursor. Products load the
/// active source schemas first, repeat this call with that snapshot, then load
/// only the requested value or reference section. This function performs no I/O.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Requirements {
    pub sources: Vec<OutputSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub match_outputs: Vec<MatchSource>,
    pub value: Option<ValueRequirement>,
    pub reference: Option<ReferenceRequirement>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValueRequirement {
    pub unit: Option<String>,
    pub field: String,
    pub prefix: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceRequirement {
    pub prefix: String,
    pub replace: Span,
}
/// Inspect the same cursor, value position and active typed scope as `complete`.
pub fn requirements(text: &str, position: usize, catalog: &Catalog) -> Requirements {
    let cursor = Cursor::new(text, position);
    if let Some(open) = open_quote(cursor.prefix) {
        let before = &text[..open];
        if let Some((field, prefix, _)) = crate::parse::source_value(before, catalog)
            && prefix.is_empty()
        {
            return Requirements {
                value: Some(ValueRequirement {
                    unit: Some(catalog.default_unit.clone()),
                    field: field.into(),
                    prefix: decoded_quoted_prefix(&text[open + 1..cursor.end]),
                }),
                ..Requirements::default()
            };
        }
        let scope = context(before, catalog);
        let value = value_field(before)
            .and_then(|field| scope.field(field))
            .map(|field| ValueRequirement {
                unit: scope.unit.clone(),
                field: field.name.clone(),
                prefix: decoded_quoted_prefix(&text[open + 1..cursor.end]),
            });
        return Requirements {
            sources: scope.sources,
            match_outputs: scope.match_outputs,
            value,
            reference: None,
        };
    }
    if let Some((field, prefix, _)) = crate::parse::source_value(cursor.prefix, catalog) {
        return Requirements {
            value: Some(ValueRequirement {
                unit: Some(catalog.default_unit.clone()),
                field: field.into(),
                prefix,
            }),
            ..Requirements::default()
        };
    }
    let scope = context(cursor.before, catalog);
    let target = unquoted_value(&cursor);
    let value = target.as_ref().and_then(|target| {
        scope.field(target.field).map(|field| ValueRequirement {
            unit: scope.unit.clone(),
            field: field.name.clone(),
            prefix: target.prefix.to_string(),
        })
    });
    let reference = (value.is_none()
        && scope.reference_allowed
        && !source_prefix(cursor.fragment)
        && (target.is_none() || cursor.fragment.contains(':')))
    .then(|| ReferenceRequirement {
        prefix: cursor.fragment.to_string(),
        replace: Span {
            start: cursor.start,
            end: cursor.token_end(text),
        },
    });
    Requirements {
        sources: scope.sources,
        match_outputs: scope.match_outputs,
        value,
        reference,
    }
}
/// Complete at a UTF-8 byte position. A replacement covers the unfinished token.
pub fn complete(text: &str, position: usize, catalog: &Catalog) -> Vec<Completion> {
    let cursor = Cursor::new(text, position);
    let Cursor {
        end,
        start,
        prefix,
        fragment,
        before,
    } = cursor;
    // Within a quoted value, only product-declared values can complete.
    if let Some(open) = open_quote(prefix) {
        return quoted_values(text, end, open, catalog);
    }
    if let Some((field, partial, mut replace)) = crate::parse::source_value(prefix, catalog) {
        replace.end = cursor.token_end(text);
        return source_names(field, catalog)
            .into_iter()
            .filter(|value| value.starts_with(&partial))
            .map(|value| Completion {
                value: literal_spelling(&value, &crate::catalog::UnknownRule::default()),
                replace,
                kind: CompletionKind::Value,
                help: String::new(),
            })
            .collect();
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
    if let Some(target) = unquoted_value(&cursor) {
        if let Some(spec) = scope.field(target.field) {
            for value in &spec.values {
                if value.starts_with(target.prefix) {
                    add(
                        &literal_spelling(value, &spec.unknown),
                        CompletionKind::Value,
                        &spec.help,
                        target.replace,
                        "",
                    );
                }
            }
            if target.unknown {
                for value in ["unknown", "*"] {
                    add(
                        value,
                        CompletionKind::Value,
                        "",
                        target.replace,
                        target.prefix,
                    );
                }
            }
        }
    } else {
        let words: Vec<&str> = before.split_whitespace().collect();
        let last = words.last().copied().unwrap_or("");
        if scope.field(last).is_some() {
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
#[derive(Clone, Copy)]
struct Cursor<'a> {
    end: usize,
    start: usize,
    prefix: &'a str,
    fragment: &'a str,
    before: &'a str,
}
impl<'a> Cursor<'a> {
    fn token_end(self, text: &str) -> usize {
        text[self.end..]
            .find(|c: char| c.is_whitespace() || "|(),[]\"=<>!".contains(c))
            .map_or(text.len(), |offset| self.end + offset)
    }
    fn new(text: &'a str, position: usize) -> Self {
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
        Self {
            end,
            start,
            prefix,
            fragment: &prefix[start..],
            before: &prefix[..start],
        }
    }
}
struct ValueTarget<'a> {
    field: &'a str,
    prefix: &'a str,
    replace: Span,
    unknown: bool,
}
fn unquoted_value<'a>(cursor: &Cursor<'a>) -> Option<ValueTarget<'a>> {
    if let Some(field) = set_field(cursor.before) {
        return Some(ValueTarget {
            field,
            prefix: cursor.fragment,
            replace: Span {
                start: cursor.start,
                end: cursor.end,
            },
            unknown: false,
        });
    }
    if let Some((field, prefix)) = cursor.fragment.split_once(':') {
        return Some(ValueTarget {
            field,
            prefix,
            replace: Span {
                start: cursor.start + field.len() + 1,
                end: cursor.end,
            },
            unknown: true,
        });
    }
    value_field(cursor.before).map(|field| ValueTarget {
        field,
        prefix: cursor.fragment,
        replace: Span {
            start: cursor.start,
            end: cursor.end,
        },
        unknown: true,
    })
}
fn value_field(before: &str) -> Option<&str> {
    let words: Vec<_> = before.split_whitespace().collect();
    let last = words.last().copied().unwrap_or("");
    set_field(before)
        .or_else(|| last.strip_suffix(':'))
        .or_else(|| {
            ["=", "!=", ">", ">=", "<", "<=", "~", "^", "between"]
                .contains(&last)
                .then(|| words.iter().rev().nth(1).copied())
                .flatten()
        })
}
fn decoded_quoted_prefix(text: &str) -> String {
    let mut partial = String::new();
    let mut escaped = false;
    for c in text.chars() {
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
    partial
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
    let field = value_field(before);
    let scope = context(before, catalog);
    let source =
        crate::parse::source_value(before, catalog).filter(|(_, prefix, _)| prefix.is_empty());
    let spec = field.and_then(|field| scope.field(field));
    let values = if let Some((field, _, _)) = source {
        source_names(field, catalog)
    } else if let Some(spec) = spec {
        spec.values.clone()
    } else {
        return Vec::new();
    };
    let partial = decoded_quoted_prefix(&text[open + 1..cursor]);
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
    let mut values: Vec<_> = values
        .iter()
        .filter(|value| value.starts_with(&partial))
        .map(|value| Completion {
            value: crate::format::quote(value),
            replace: Span { start: open, end },
            kind: CompletionKind::Value,
            help: spec.map(|spec| spec.help.clone()).unwrap_or_default(),
        })
        .collect();
    values.sort_by(|a, b| a.value.cmp(&b.value));
    values.dedup_by(|a, b| a.value == b.value);
    values
}
fn source_names(field: &str, catalog: &Catalog) -> Vec<String> {
    let mut names: Vec<String> = catalog
        .field(&catalog.default_unit, field)
        .map(|field| field.values.clone())
        .unwrap_or_default();
    for output in &catalog.source_outputs {
        match (&output.source, field) {
            (OutputSource::Saved { name, .. }, "query") => names.push(name.clone()),
            (OutputSource::Result { reference, .. }, "result") => names.push(
                reference
                    .strip_prefix("result:")
                    .unwrap_or(reference)
                    .to_string(),
            ),
            _ => {}
        }
    }
    names.sort();
    names.dedup();
    names
}
#[derive(Default)]
struct CompletionScope {
    fields: Vec<Field>,
    unit: Option<String>,
    sources: Vec<OutputSource>,
    match_outputs: Vec<MatchSource>,
    reference_allowed: bool,
}
impl CompletionScope {
    fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.name == name)
    }
    fn unit(name: &str, catalog: &Catalog) -> Self {
        Self {
            unit: catalog.unit(name).map(|unit| unit.name.clone()),
            sources: Vec::new(),
            match_outputs: Vec::new(),
            reference_allowed: true,
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
    let mut stack: Vec<(usize, String, bool, Vec<OutputSource>, Vec<MatchSource>)> = Vec::new();
    let mut quote = false;
    let mut escape = false;
    let mut words: Vec<String> = Vec::new();
    let mut pipes = Vec::new();
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
            if c == '|' {
                words.push("|".into());
                pipes.push((at, words.len() - 1));
            }
            if c == '(' {
                let preceding = words.last().cloned().unwrap_or_default();
                let relation = words.iter().rev().nth(1).is_some_and(|s| s == "exists");
                stack.push((start, String::new(), false, Vec::new(), Vec::new()));
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
                    let (unit, sources, matches) = row_scope(&text[start..at], catalog);
                    let unit = unit.unwrap_or_else(|| "$deferred_completion".into());
                    let entry = stack.last_mut().unwrap();
                    entry.1 = unit;
                    entry.2 = true;
                    entry.3 = sources;
                    entry.4 = matches;
                    start = at + 1;
                }
            } else if c == ')'
                && let Some((outer, _, _, _, _)) = stack.pop()
            {
                start = outer;
            }
        } else {
            word.push(c);
        }
    }
    let inner = &text[start..];
    let explicit = stack
        .iter()
        .rev()
        .find(|(_, unit, _, _, _)| !unit.is_empty());
    let inner_first = inner.split_whitespace().next().unwrap_or("");
    let inner_source =
        catalog.unit(inner_first).is_some() || crate::parse::starts_with_source(inner, catalog);
    let inherited_deferred = explicit.is_some_and(|(_, unit, inherited, _, _)| {
        *inherited && unit == "$deferred_completion" && !inner_source
    });
    let current = if let Some((_, unit, inherited, _, _)) = explicit {
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
    let source_like = crate::parse::starts_with_source(&current, catalog) || inherited_deferred;
    let mut scope = if source_like {
        CompletionScope::default()
    } else {
        CompletionScope::unit(&unit, catalog)
    };
    let mut group_input = None;
    let mut condition_scope = true;
    if let Some(q) = parsed {
        let (mut match_unit, mut match_patterns) =
            crate::matches::source_context(&q.source, catalog);
        match &q.source {
            Source::Unit {
                unit: source,
                condition,
                ..
            } if !source_like
                || condition.as_ref().is_some_and(|condition| {
                    matches!(
                        condition.kind,
                        crate::ast::ConditionKind::Or { .. }
                            | crate::ast::ConditionKind::Saved { .. }
                    )
                }) =>
            {
                unit = source.clone();
                scope = CompletionScope::unit(&unit, catalog);
            }
            Source::Saved { .. } | Source::Result { .. } => {
                scope.fields = catalog
                    .source_output(&q.source)
                    .map(|output| output.fields.clone())
                    .unwrap_or_default();
                scope.unit = catalog
                    .source_output(&q.source)
                    .and_then(|output| output.unit.as_deref())
                    .and_then(|unit| catalog.unit(unit))
                    .map(|unit| unit.name.clone());
                if let Some(source) = output_source(&q.source) {
                    scope.sources.push(source);
                }
            }
            _ => {}
        }
        for stage in q.stages {
            if crate::matches::invalidates(&stage.kind) {
                match_patterns = Some(Vec::new());
            }
            condition_scope = matches!(
                &stage.kind,
                StageKind::Where { .. }
                    | StageKind::Scope {
                        condition: Some(_),
                        ..
                    }
            );
            match stage.kind {
                StageKind::Scope {
                    unit: source,
                    condition,
                } => {
                    match_patterns = Some(crate::matches::patterns(condition.as_ref()));
                    match_unit = Some(source.clone());
                    unit = source;
                    scope = CompletionScope::unit(&unit, catalog);
                    group_input = None;
                }
                StageKind::Matches => {
                    let source = crate::matches::request(
                        match_unit.as_deref(),
                        match_patterns.as_deref(),
                        stage.span.start,
                    )
                    .ok()
                    .flatten();
                    let output = source
                        .as_ref()
                        .and_then(|source| catalog.match_output(source));
                    if let Some(source) = source
                        && !scope.match_outputs.contains(&source)
                    {
                        scope.match_outputs.push(source);
                    }
                    scope.fields = output
                        .map(|output| output.fields.clone())
                        .unwrap_or_default();
                    scope.unit = output
                        .and_then(|output| output.unit.as_deref())
                        .and_then(|unit| catalog.unit(unit))
                        .map(|unit| unit.name.clone());
                    match_unit = scope.unit.clone();
                    match_patterns = output.map(|_| Vec::new());
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
    if let Some((_, _, inherited, sources, matches)) = explicit
        && *inherited
        && !inner_source
    {
        for source in sources {
            if !scope.sources.contains(source) {
                scope.sources.push(source.clone());
            }
        }
        for source in matches {
            if !scope.match_outputs.contains(source) {
                scope.match_outputs.push(source.clone());
            }
        }
    }
    let stage = pipes
        .iter()
        .rev()
        .find(|(at, _)| *at >= start)
        .and_then(|(_, index)| words.get(index + 1));
    let last = words.last().map(String::as_str).unwrap_or("");
    let non_condition_stage =
        stage.is_some_and(|stage| stage != "where" && catalog.unit(stage).is_none());
    let operator_expected = scope.field(last).is_some();
    let explicit_unit_only = words.len() == 1 && catalog.unit(last).is_some();
    scope.reference_allowed =
        (condition_scope || stage.is_some_and(|stage| stage == "where") || last == "where")
            && !non_condition_stage
            && !operator_expected
            && !explicit_unit_only
            && !matches!(
                last,
                "|" | "sort" | "group" | "fields" | "by" | "exists" | "as" | "of"
            )
            && !["=", "!=", ">", ">=", "<", "<=", "~", "^", "between"].contains(&last);
    scope
}

fn source_prefix(first: &str) -> bool {
    first.split_once(':').is_some_and(|(prefix, _)| {
        prefix.eq_ignore_ascii_case("query") || prefix.eq_ignore_ascii_case("result")
    })
}
fn parse_prefix(text: &str, catalog: &Catalog) -> Option<crate::ast::Query> {
    use crate::ast::Source;
    let mut candidate = text.trim_end();
    loop {
        if let Some(left) = candidate.strip_suffix(" OR")
            && let Ok(mut query) = crate::parse::parse(left, catalog)
            && query.stages.is_empty()
            && query.attach.is_empty()
            && let Source::Saved {
                name,
                arguments,
                condition: None,
                ..
            } = query.source
        {
            query.source = Source::Unit {
                unit: catalog.default_unit.clone(),
                implicit: true,
                condition: Some(crate::ast::Condition {
                    span: Span::default(),
                    kind: crate::ast::ConditionKind::Saved { name, arguments },
                }),
            };
            return Some(query);
        }
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
fn row_scope(
    text: &str,
    catalog: &Catalog,
) -> (Option<String>, Vec<OutputSource>, Vec<MatchSource>) {
    use crate::ast::{Source, StageKind};
    let Some(query) = parse_prefix(text, catalog) else {
        return (None, Vec::new(), Vec::new());
    };
    let mut matches = Vec::new();
    let mut sources = output_source(&query.source).into_iter().collect::<Vec<_>>();
    let mut unit = match &query.source {
        Source::Unit { unit, .. } => Some(unit.clone()),
        Source::Saved { .. } | Source::Result { .. } => catalog
            .source_output(&query.source)
            .and_then(|output| output.unit.clone()),
        _ => None,
    };
    let (_, mut available) = crate::matches::source_context(&query.source, catalog);
    for stage in query.stages {
        if crate::matches::invalidates(&stage.kind) {
            available = Some(Vec::new());
        }
        match stage.kind {
            StageKind::Scope {
                unit: next,
                condition,
            } => {
                unit = Some(next);
                available = Some(crate::matches::patterns(condition.as_ref()));
                sources.clear();
                matches.clear();
            }
            StageKind::Matches => {
                let request = crate::matches::request(
                    unit.as_deref(),
                    available.as_deref(),
                    stage.span.start,
                )
                .ok()
                .flatten();
                let output = request
                    .as_ref()
                    .and_then(|request| catalog.match_output(request));
                if let Some(request) = request
                    && !matches.contains(&request)
                {
                    matches.push(request);
                }
                unit = output
                    .and_then(|output| output.unit.as_deref())
                    .and_then(|unit| catalog.unit(unit))
                    .map(|unit| unit.name.clone());
                available = output.map(|_| Vec::new());
            }
            _ => {}
        }
    }
    (unit, sources, matches)
}
fn output_source(source: &crate::ast::Source) -> Option<OutputSource> {
    match source {
        crate::ast::Source::Saved {
            name, arguments, ..
        } => Some(OutputSource::Saved {
            name: name.clone(),
            arguments: arguments.clone(),
        }),
        crate::ast::Source::Result {
            reference, part, ..
        } => Some(OutputSource::Result {
            reference: reference.clone(),
            part: *part,
        }),
        _ => None,
    }
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
