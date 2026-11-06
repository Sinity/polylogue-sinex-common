//! The one grammar: catalog-driven parsing, typing, and refusals that carry a
//! corrected query.
use crate::ast::*;
use crate::catalog::{Catalog, ExtensionPosition, ExtensionSlot, Field, FieldKind, OutputSchema};
use crate::scope::Origin;
use crate::{Error, ErrorKind};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
mod typing;

#[derive(Debug, Clone)]
struct Token {
    text: String,
    quoted: bool,
    span: Span,
}
fn tokens(text: &str, catalog: &Catalog) -> Result<Vec<Token>, Error> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < text.len() {
        let c = text[i..].chars().next().unwrap();
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let start = i;
        if c == '"' {
            i += 1;
            let mut value = String::new();
            let mut closed = false;
            while i < text.len() {
                let c = text[i..].chars().next().unwrap();
                i += c.len_utf8();
                if c == '"' {
                    closed = true;
                    break;
                }
                if c == '\\' {
                    let Some(e) = text[i..].chars().next() else {
                        break;
                    };
                    i += e.len_utf8();
                    value.push(match e {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    value.push(c);
                }
            }
            if !closed {
                return Err(
                    Error::new(ErrorKind::InvalidQuery, "unclosed quoted value", start)
                        .suggestion(format!("{text}\"")),
                );
            }
            out.push(Token {
                text: value,
                quoted: true,
                span: Span { start, end: i },
            });
            continue;
        }
        // A declared reference is one literal up to whitespace or a delimiter,
        // so its own punctuation (`#`, `=`, `:`) stays inside it.
        let literal_end = text[i..]
            .find(|c: char| c.is_whitespace() || matches!(c, '|' | ')' | ']' | ','))
            .map_or(text.len(), |offset| i + offset);
        if !text[i..literal_end].contains('"') && catalog.is_reference(&text[i..literal_end]) {
            i = literal_end;
            out.push(Token {
                text: text[start..i].into(),
                quoted: false,
                span: Span { start, end: i },
            });
            continue;
        }
        if text[i..].starts_with("->") {
            i += 2;
        } else if "()[]|,=<>!".contains(c) {
            i += c.len_utf8();
            if matches!(c, '<' | '>' | '!') && text[i..].starts_with('=') {
                i += 1;
            }
        } else {
            while i < text.len() {
                let c = text[i..].chars().next().unwrap();
                if c.is_whitespace() || "()[]|,\"=<>!".contains(c) || text[i..].starts_with("->") {
                    break;
                }
                i += c.len_utf8();
            }
        }
        out.push(Token {
            text: text[start..i].into(),
            quoted: false,
            span: Span { start, end: i },
        });
    }
    // Only the two source-reference namespaces permit whitespace before `:`.
    // Keep original byte spans even when their normalized token text is shorter.
    let mut merged: Vec<Token> = Vec::new();
    for token in out {
        if !token.quoted
            && token.text.starts_with(':')
            && let Some(previous) = merged.last_mut()
            && !previous.quoted
            && source_namespace(&previous.text).is_some()
        {
            previous.text.push_str(&token.text);
            previous.span.end = token.span.end;
        } else {
            merged.push(token);
        }
    }
    Ok(merged)
}
fn source_namespace(name: &str) -> Option<&'static str> {
    if name.eq_ignore_ascii_case("query") {
        Some("query")
    } else if name.eq_ignore_ascii_case("result") {
        Some("result")
    } else {
        None
    }
}
/// Source-name completion uses the parser's tokens and original byte offsets.
pub(crate) fn source_value(text: &str, catalog: &Catalog) -> Option<(&'static str, String, Span)> {
    let words = tokens(text, catalog).ok()?;
    let (index, token, field, raw) =
        words.iter().enumerate().rev().find_map(|(index, token)| {
            if token.quoted {
                return None;
            }
            let (field, raw) = token.text.split_once(':')?;
            Some((index, token, source_namespace(field)?, raw))
        })?;
    if raw.is_empty() {
        match &words[index + 1..] {
            [] => Some((
                field,
                String::new(),
                Span {
                    start: text.len(),
                    end: text.len(),
                },
            )),
            [value]
                if !value.quoted
                    && !text.ends_with(char::is_whitespace)
                    && !is_symbol(&value.text) =>
            {
                Some((field, value.text.clone(), value.span))
            }
            _ => None,
        }
    } else if index + 1 == words.len() && !text.ends_with(char::is_whitespace) {
        let colon = text[token.span.start..token.span.end].find(':')? + token.span.start;
        Some((
            field,
            raw.into(),
            Span {
                start: colon + 1,
                end: token.span.end,
            },
        ))
    } else {
        None
    }
}
pub(crate) fn starts_with_source(text: &str, catalog: &Catalog) -> bool {
    tokens(text, catalog)
        .ok()
        .and_then(|words| words.into_iter().next())
        .is_some_and(|token| {
            !token.quoted
                && token
                    .text
                    .split_once(':')
                    .is_some_and(|(field, _)| source_namespace(field).is_some())
        })
}
/// Parse a complete question using one immutable vocabulary snapshot.
pub fn parse(text: &str, catalog: &Catalog) -> Result<Query, Error> {
    match parse_once(text, catalog) {
        Ok(query) => Ok(query),
        Err(mut error) => {
            // A suggestion is a complete question that parses as given, also
            // when more than one mistake needs the same kind of correction.
            let mut candidate = error.suggestion.take();
            let mut seen = BTreeSet::new();
            while let Some(corrected) = candidate {
                if !seen.insert(corrected.clone()) {
                    break;
                }
                match parse_once(&corrected, catalog) {
                    Ok(query) => {
                        if crate::bind::check_parsed(&query, catalog).is_ok() {
                            error.suggestion = Some(corrected);
                        }
                        break;
                    }
                    Err(next) => candidate = next.suggestion,
                }
            }
            Err(error)
        }
    }
}
fn parse_once(text: &str, catalog: &Catalog) -> Result<Query, Error> {
    parse_internal(text, catalog, false).map(|parsed| parsed.query)
}
pub(crate) fn parse_typed(text: &str, catalog: &Catalog) -> Result<Parsed, Error> {
    parse_internal(text, catalog, true)
}
pub(crate) struct Parsed {
    pub query: Query,
    pub typing: crate::scope::Typing,
}
fn parse_internal(text: &str, catalog: &Catalog, capture_types: bool) -> Result<Parsed, Error> {
    let mut p = Parser {
        tokens: tokens(text, catalog)?,
        at: 0,
        text,
        catalog,
        captures: Vec::new(),
        query_stack: Vec::new(),
        capture_types,
        inner_scopes: BTreeMap::new(),
    };
    let (query, _) = p.relation(None)?;
    if p.peek().is_some() {
        return Err(p.error("expected `|` before a transform, or the end of the query"));
    }
    check_ranking(&query, text)?;
    let typing = if capture_types {
        typing::project(&p, &query)
    } else {
        crate::scope::Typing::default()
    };
    Ok(Parsed { query, typing })
}
#[derive(Clone, PartialEq, Eq)]
struct Scope {
    unit: String,
    // Native field ownership survives grouping; the resulting row domain does not.
    row_domain: Option<String>,
    /// The visible columns after a grouping, projection or aggregate; None
    /// means every field of the unit or output, plus `extra`.
    columns: Option<BTreeSet<String>>,
    /// Generated names visible beside the unit's own fields.
    extra: BTreeSet<String>,
    bucketed: bool,
    output: Option<Arc<OutputSchema>>,
    columns_unknown: bool,
    typed_extra: Arc<Vec<Field>>,
    origins: Arc<BTreeMap<String, Origin>>,
    group_input: Option<Arc<Scope>>,
    output_source: Option<crate::catalog::OutputSource>,
    /// The step units of the relation's top-level `exists` and `seq`
    /// patterns, which `matches` expands; None when a saved or kept source
    /// hides them.
    patterns: Option<Vec<String>>,
}
impl Scope {
    fn new(unit: String) -> Self {
        Self {
            row_domain: Some(unit.clone()),
            unit,
            columns: None,
            extra: BTreeSet::new(),
            bucketed: false,
            output: None,
            columns_unknown: false,
            typed_extra: Arc::new(Vec::new()),
            origins: Arc::new(BTreeMap::new()),
            group_input: None,
            output_source: None,
            patterns: Some(Vec::new()),
        }
    }
}
/// Where an expression stands: the left side of a condition holds fields and
/// functions; a column or aggregate argument may also hold literals; inside a
/// function call a word that is not a field is a value (`month_day(today)`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Condition,
    Column,
    Argument,
}
struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    text: &'a str,
    catalog: &'a Catalog,
    captures: Vec<Option<typing::Capture>>,
    query_stack: Vec<(usize, usize)>,
    capture_types: bool,
    /// The final scope of each order relation's inner query, by the byte
    /// offset where the relation starts.
    inner_scopes: BTreeMap<usize, Scope>,
}
impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }
    fn next_is(&self, n: usize, word: &str) -> bool {
        self.tokens
            .get(self.at + n)
            .is_some_and(|t| !t.quoted && t.text.eq_ignore_ascii_case(word))
    }
    fn is(&self, word: &str) -> bool {
        self.next_is(0, word)
    }
    fn take(&mut self, word: &str) -> bool {
        if self.is(word) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn next(&mut self) -> Result<Token, Error> {
        let t = self
            .peek()
            .cloned()
            .ok_or_else(|| self.error("unexpected end of query"))?;
        self.at += 1;
        Ok(t)
    }
    fn pos(&self) -> usize {
        self.peek().map_or(self.text.len(), |t| t.span.start)
    }
    fn end(&self) -> usize {
        self.tokens
            .get(self.at.saturating_sub(1))
            .map_or(0, |t| t.span.end)
    }
    fn error(&self, message: impl Into<String>) -> Error {
        Error::new(ErrorKind::InvalidQuery, message, self.pos())
    }
    fn expect(&mut self, word: &str) -> Result<(), Error> {
        if self.take(word) {
            Ok(())
        } else {
            let error = self.error(format!("expected `{word}`"));
            if word == ")" && self.peek().is_none() {
                let mut depth = 0usize;
                for token in &self.tokens {
                    if token.quoted {
                        continue;
                    }
                    match token.text.as_str() {
                        "(" => depth += 1,
                        ")" if depth > 0 => depth -= 1,
                        ")" => return Err(error),
                        _ => {}
                    }
                }
                if depth > 0 {
                    return Err(error.suggestion(format!("{}{}", self.text, ")".repeat(depth))));
                }
            }
            Err(error)
        }
    }
    fn name(&mut self) -> Result<Token, Error> {
        let t = self.next()?;
        if t.quoted || is_symbol(&t.text) {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "expected a name",
                t.span.start,
            ));
        }
        Ok(t)
    }
    fn unit_name(&mut self) -> Result<String, Error> {
        let t = self.name()?;
        self.catalog
            .unit(&t.text)
            .map(|u| u.name.clone())
            .ok_or_else(|| self.unknown_unit(&t))
    }
    fn unknown_unit(&self, t: &Token) -> Error {
        let names: Vec<&str> = self
            .catalog
            .units
            .iter()
            .flat_map(|unit| std::iter::once(unit.name.as_str()).chain(unit.singular.as_deref()))
            .collect();
        self.near_miss(
            t,
            format!("`{}` is not a unit", t.text),
            ErrorKind::InvalidQuery,
            names,
        )
    }
    /// A refusal naming the known names nearest to an unknown one; the nearest
    /// becomes the suggestion.
    fn near_miss(&self, t: &Token, message: String, kind: ErrorKind, names: Vec<&str>) -> Error {
        let nearest = closest(&t.text, names.into_iter());
        let mut e = Error::new(kind, message, t.span.start).length(t.span.end - t.span.start);
        if let Some(fix) = nearest.first() {
            e = e.suggestion(self.replace(t.span, fix));
        }
        e.candidates(nearest)
    }
    fn replace(&self, span: Span, value: &str) -> String {
        format!(
            "{}{}{}",
            &self.text[..span.start],
            value,
            &self.text[span.end..]
        )
    }
    fn query_in(&mut self, inherited: &Scope) -> Result<(Query, Scope), Error> {
        self.expect("(")?;
        let relation = self.relation(Some(inherited))?;
        self.expect(")")?;
        Ok(relation)
    }
    fn relation(&mut self, inherited: Option<&Scope>) -> Result<(Query, Scope), Error> {
        let capture = self.captures.len();
        if self.capture_types {
            self.captures.push(None);
        }
        self.query_stack.push((capture, 0));
        let mut scope = if let Some(parent) = inherited {
            // An omitted nested source inherits the row domain, rather than
            // the enclosing grouping/projection's visible output columns.
            let mut rows = Scope::new(parent.unit.clone());
            rows.columns_unknown = self.catalog.unit(&parent.unit).is_none();
            rows
        } else {
            Scope::new(self.catalog.default_unit.clone())
        };
        let mut source = Source::Unit {
            unit: scope.unit.clone(),
            implicit: true,
            condition: None,
        };
        let mut consumed_source = false;
        let source_start = self.at;
        if let Some(first) = self.peek().cloned()
            && !first.quoted
            && let Some((prefix, raw)) = first.text.split_once(':')
            && let Some(namespace) = source_namespace(prefix)
        {
            self.at += 1;
            source = if namespace == "query" {
                let name = self.saved_name(raw, first.span.start)?;
                Source::Saved {
                    name,
                    arguments: self.arguments()?,
                    condition: None,
                    output: SourceColumns::Unknown,
                }
            } else {
                let (mut id, quoted, end) = if raw.is_empty() {
                    let token = self.next()?;
                    if !token.quoted && is_symbol(&token.text) {
                        return Err(self.error("a kept result needs a literal identity"));
                    }
                    (token.text, token.quoted, token.span.end)
                } else {
                    (raw.to_string(), false, first.span.end)
                };
                let mut part = ResultPart::Rows;
                if !quoted {
                    if let Some(base) = id.strip_suffix(".matches") {
                        id = base.to_string();
                        part = ResultPart::Matches;
                    } else if let Some(base) = id.strip_suffix(".rows") {
                        id = base.to_string();
                    }
                } else if let Some(selector) = self.peek()
                    && selector.span.start == end
                    && matches!(selector.text.as_str(), ".rows" | ".matches")
                    && self.next()?.text == ".matches"
                {
                    part = ResultPart::Matches;
                }
                if id.is_empty() {
                    return Err(self.error("a kept result needs a literal identity"));
                }
                Source::Result {
                    reference: format!("result:{id}"),
                    part,
                    condition: None,
                    output: SourceColumns::Unknown,
                }
            };
            if matches!(source, Source::Saved { .. }) && self.bool_is("OR") {
                // Here the saved relation is the first boolean operand, not
                // the source whose output owns the following suffix fields.
                self.at = source_start;
                source = Source::Unit {
                    unit: scope.unit.clone(),
                    implicit: true,
                    condition: None,
                };
            } else {
                self.output_scope(&mut source, &mut scope);
                consumed_source = true;
            }
        }
        if !consumed_source
            && let Some(first) = self.peek().cloned()
            && !first.quoted
            && !self.field_syntax(&scope)
        {
            let lower = first.text.to_ascii_lowercase();
            if let Some(unit) = self.catalog.unit(&first.text) {
                self.at += 1;
                consumed_source = true;
                scope = Scope::new(unit.name.clone());
                let condition = if self.take("where") {
                    Some(self.condition(&scope)?)
                } else {
                    None
                };
                source = Source::Unit {
                    unit: scope.unit.clone(),
                    implicit: false,
                    condition,
                };
            } else if self.field_known(&scope, &first.text)
                && !matches!(lower.as_str(), "since" | "until")
            {
                return Err(self.bare_field(&first));
            } else if self.next_is(1, "where") {
                return Err(self.unknown_unit(&first));
            }
        }
        if consumed_source
            && matches!(
                source,
                Source::Unit {
                    condition: None,
                    ..
                }
            )
            && !self.query_end()
            && !self.is("|")
            && !self.is("with")
        {
            return Err(self
                .error("a unit's conditions start with where")
                .suggestion(format!(
                    "{}where {}",
                    &self.text[..self.pos()],
                    &self.text[self.pos()..]
                )));
        }
        // A source not consumed above starts as conditions on the default unit.
        if !self.query_end()
            && !self.is("|")
            && !self.is("with")
            && let Source::Unit { condition, .. } = &mut source
            && condition.is_none()
        {
            *condition = Some(self.condition(&scope)?);
        }
        source = match source {
            Source::Unit {
                implicit: true,
                condition:
                    Some(Condition {
                        kind: ConditionKind::Saved { name, arguments },
                        ..
                    }),
                ..
            } => Source::Saved {
                name,
                arguments,
                condition: None,
                output: SourceColumns::Unknown,
            },
            Source::Unit {
                implicit: true,
                condition:
                    Some(Condition {
                        kind: ConditionKind::Reference { reference },
                        ..
                    }),
                ..
            } if reference.starts_with("result:") => Source::Result {
                reference,
                part: ResultPart::Rows,
                condition: None,
                output: SourceColumns::Unknown,
            },
            other => other,
        };
        if matches!(source, Source::Saved { .. } | Source::Result { .. }) {
            self.output_scope(&mut source, &mut scope);
        }
        if matches!(source, Source::Saved { .. } | Source::Result { .. })
            && !self.query_end()
            && !self.is("|")
            && !self.is("with")
        {
            self.take("where");
            let inline = Some(self.condition(&scope)?);
            if let Source::Saved { condition, .. } | Source::Result { condition, .. } = &mut source
            {
                *condition = inline;
            }
        }
        let mut attach = Vec::new();
        if self.take("with") {
            attach = self.attachments()?;
        }
        scope.patterns = match &source {
            Source::Unit { condition, .. } => Some(patterns(condition.as_ref())),
            Source::Saved { condition, .. } => self
                .catalog
                .source_output(&source)
                .and_then(|output| output.match_units.clone())
                .map(|mut inherited| {
                    inherited.extend(patterns(condition.as_ref()));
                    inherited
                }),
            Source::Result { .. } => None,
        };
        match &source {
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
            } => self.expose_order_match(&mut scope, condition),
            _ => {}
        }
        let source_scope = self.capture_types.then(|| scope.clone());
        let mut transform_scopes = Vec::new();
        let mut transforms = Vec::new();
        while !self.query_end() {
            if !self.take("|") {
                return Err(self.error("expected `|` before a transform"));
            }
            self.query_stack.last_mut().unwrap().1 = transforms.len();
            let before = self.capture_types.then(|| scope.clone());
            transforms.push(self.transform(&mut scope)?);
            if let Some(before) = before {
                transform_scopes.push((before, scope.clone()));
            }
        }
        if let Some(source) = source_scope {
            self.captures[capture] = Some(typing::Capture {
                source,
                transforms: transform_scopes,
            });
        }
        self.query_stack.pop();
        Ok((
            Query {
                source,
                attach,
                transforms,
            },
            scope,
        ))
    }
    fn output_scope(&self, source: &mut Source, scope: &mut Scope) {
        scope.unit = match source {
            Source::Saved { name, .. } => format!("query:{name}"),
            Source::Result { reference, .. } => reference.clone(),
            _ => return,
        };
        scope.output_source = match source {
            Source::Saved {
                name, arguments, ..
            } => Some(crate::catalog::OutputSource::Saved {
                name: name.clone(),
                arguments: arguments.clone(),
            }),
            Source::Result {
                reference, part, ..
            } => Some(crate::catalog::OutputSource::Result {
                reference: reference.clone(),
                part: *part,
            }),
            _ => None,
        };
        scope.output = self.catalog.source_output(source).cloned().map(Arc::new);
        scope.row_domain = scope.output.as_ref().and_then(|output| output.unit.clone());
        if let Some(unit) = scope
            .output
            .as_ref()
            .and_then(|output| output.unit.as_ref())
        {
            scope.unit = unit.clone();
        }
        scope.columns_unknown = scope.output.is_none();
        scope.columns = scope
            .output
            .as_ref()
            .map(|output| output.fields.iter().map(|f| f.name.clone()).collect());
        if let Source::Saved { output, .. } | Source::Result { output, .. } = source {
            *output = if scope.columns_unknown {
                SourceColumns::Unknown
            } else {
                SourceColumns::Known
            };
        }
    }
    fn scope_field<'a>(&'a self, scope: &'a Scope, name: &str) -> Option<&'a Field> {
        scope
            .typed_extra
            .iter()
            .find(|field| field.name == name)
            .or_else(|| {
                if let Some(output) = &scope.output {
                    output.fields.iter().find(|field| field.name == name)
                } else if scope.columns_unknown {
                    None
                } else {
                    self.catalog.field(&scope.unit, name)
                }
            })
    }
    fn main_time(&self, scope: &Scope) -> Option<String> {
        if let Some(output) = &scope.output {
            output.main_time.clone()
        } else if scope.columns_unknown {
            Some("$unknown_main_time".into())
        } else if scope.row_domain.is_none() {
            None
        } else {
            self.catalog
                .unit(&scope.unit)
                .and_then(|unit| unit.main_time.clone())
        }
    }
    fn query_end(&self) -> bool {
        self.peek().is_none() || self.is(")")
    }
    /// The next tokens are a field condition rather than a text term.
    fn field_syntax(&self, scope: &Scope) -> bool {
        let Some(current) = self.peek() else {
            return false;
        };
        if current.quoted {
            return false;
        }
        current.text.contains(':')
            || self.tokens.get(self.at + 1).is_some_and(|t| {
                !t.quoted
                    && (Op::parse(&t.text).is_some()
                        || matches!(t.text.as_str(), "between" | "in" | "exists")
                        || (Arithmetic::parse(&t.text).is_some()
                            && self.field_known(scope, &current.text)))
            })
    }
    fn condition_end(&self) -> bool {
        self.peek().is_none()
            || ["|", ")", "]", ",", "->"].iter().any(|s| self.is(s))
            || (self.is("with") && self.next_is(2, "("))
    }
    fn bool_is(&self, word: &str) -> bool {
        self.peek().is_some_and(|t| !t.quoted && t.text == word)
    }
    fn take_bool(&mut self, word: &str) -> bool {
        if self.bool_is(word) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn condition(&mut self, scope: &Scope) -> Result<Condition, Error> {
        let mut conditions = vec![self.conjunction(scope)?];
        while self.take_bool("OR") {
            conditions.push(self.conjunction(scope)?);
        }
        Ok(join_conditions(conditions, false))
    }
    fn conjunction(&mut self, scope: &Scope) -> Result<Condition, Error> {
        let mut conditions = vec![self.atom(scope)?];
        loop {
            if self.bool_is("OR") || self.condition_end() {
                break;
            }
            self.refuse_lowercase_boolean()?;
            self.take_bool("AND");
            conditions.push(self.atom(scope)?);
        }
        Ok(join_conditions(conditions, true))
    }
    /// A lowercase `and`, `or` or `not` between two conditions would silently
    /// become a search word; the capitalised operator is the suggestion.
    fn refuse_lowercase_boolean(&self) -> Result<(), Error> {
        let Some(t) = self.peek() else {
            return Ok(());
        };
        if t.quoted || !matches!(t.text.as_str(), "and" | "or" | "not") {
            return Ok(());
        }
        let followed = self.tokens.get(self.at + 1).is_some_and(|next| {
            next.quoted || !["|", ")", "]", ",", "->"].contains(&next.text.as_str())
        });
        if !followed {
            return Ok(());
        }
        let error = Error::new(
            ErrorKind::InvalidQuery,
            format!(
                "`{}` between conditions reads as a search word; boolean operators are AND, OR and NOT in capitals",
                t.text
            ),
            t.span.start,
        )
        .length(t.text.len())
        .hint("quote the word to search for it");
        Err(error.suggestion(self.replace(t.span, &t.text.to_uppercase())))
    }
    fn atom(&mut self, scope: &Scope) -> Result<Condition, Error> {
        let start = self.pos();
        if self.peek().is_some_and(|t| !t.quoted && t.text == "not")
            && self
                .tokens
                .get(self.at + 1)
                .is_some_and(|t| self.field_known(scope, &t.text) || t.text.contains(':'))
        {
            let t = self.peek().unwrap();
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "write NOT in capitals",
                t.span.start,
            )
            .length(3)
            .suggestion(self.replace(t.span, "NOT")));
        }
        if self.take_bool("NOT") || self.take("-") {
            let c = self.atom(scope)?;
            return Ok(self.negation(start, c));
        }
        if self
            .peek()
            .is_some_and(|t| !t.quoted && t.text.starts_with('-') && t.text.len() > 1)
        {
            let mut token = self.next()?;
            token.text.remove(0);
            token.span.start += 1;
            self.tokens.insert(self.at, token);
            let c = self.atom(scope)?;
            return Ok(self.negation(start, c));
        }
        if self.take("(") {
            let mut c = self.condition(scope)?;
            self.expect(")")?;
            c.span = Span {
                start,
                end: self.end(),
            };
            return Ok(c);
        }
        if self.is("exists")
            && self.tokens.get(self.at + 1).is_some_and(|t| !t.quoted)
            && self.next_is(2, "(")
        {
            self.at += 1;
            let unit = self.unit_name()?;
            self.expect("(")?;
            let c = if self.is(")") {
                None
            } else {
                Some(Box::new(self.condition(&Scope::new(unit.clone()))?))
            };
            self.expect(")")?;
            return Ok(self.cond(start, ConditionKind::Exists { unit, condition: c }));
        }
        if self.is("count")
            && self.next_is(1, "(")
            && self
                .tokens
                .get(self.at + 2)
                .is_some_and(|t| self.catalog.unit(&t.text).is_some())
        {
            self.at += 2;
            let unit = self.unit_name()?;
            let condition = if self.take("where") {
                Some(Box::new(self.condition(&Scope::new(unit.clone()))?))
            } else {
                None
            };
            self.expect(")")?;
            let op = self.op()?;
            let count = self.number()?;
            return Ok(self.cond(
                start,
                ConditionKind::Count {
                    unit,
                    condition,
                    op,
                    count,
                },
            ));
        }
        if self.is("seq") && (self.next_is(1, "(") || self.next_is(1, "all")) {
            return self.sequence(scope);
        }
        if let Some(t) = self.peek()
            && !t.quoted
            && OrderRelation::parse(&t.text).is_some()
            && self.next_is(1, "(")
        {
            return self.order_condition(scope);
        }
        if let Some(extension) = self
            .catalog
            .extensions
            .iter()
            .find(|e| e.position == ExtensionPosition::Atom && self.is(&e.name))
            .cloned()
        {
            self.at += 1;
            let arguments = self.extension_arguments(scope, &extension.arguments)?;
            return Ok(self.cond(
                start,
                ConditionKind::Extension {
                    name: extension.name,
                    arguments,
                },
            ));
        }
        let token = self.next()?;
        if token.quoted {
            return Ok(self.cond(
                start,
                ConditionKind::Text {
                    text: token.text,
                    mode: TextMode::Phrase,
                },
            ));
        }
        if is_symbol(&token.text) {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "expected a condition",
                start,
            ));
        }
        if let Some((field, raw)) = token.text.split_once(':') {
            return self.colon_atom(scope, &token, field, raw, start);
        }
        // Function calls and field syntax take precedence over text and keyword positions.
        self.at -= 1;
        let function_call = self.next_is(1, "(") && self.catalog.function(&token.text)?.is_some();
        if self.field_syntax(scope) || function_call {
            return self.comparison(scope, start);
        }
        self.at += 1;
        // `since 7d` is the spelling `since:7d` written apart: refuse it with
        // the joined form, so the word does not silently become a search term.
        if token.text.eq_ignore_ascii_case("since") || token.text.eq_ignore_ascii_case("until") {
            let resume = self.at;
            if let Some(next) = self.peek().cloned()
                && (next.quoted || !is_symbol(&next.text))
                && (next.text.starts_with('$') || crate::time::is_time_expression(&next.text))
                && self.main_time(scope).is_some()
            {
                self.at += 1;
                let end = self.end();
                let joined = format!(
                    "{}:{}",
                    token.text.to_ascii_lowercase(),
                    &self.text[next.span.start..end]
                );
                let error = Error::new(
                    ErrorKind::InvalidQuery,
                    format!("a time bound is written `{joined}`"),
                    start,
                )
                .length(end - start);
                return Err(error.suggestion(self.replace(Span { start, end }, &joined)));
            }
            self.at = resume;
        }
        if token.text == "where" {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "where starts a unit filter or a transform; use `| where` after conditions",
                start,
            ));
        }
        if self.catalog.is_reference(&token.text) {
            return Ok(self.cond(
                start,
                ConditionKind::Reference {
                    reference: token.text,
                },
            ));
        }
        if self.field_known(scope, &token.text) {
            return Err(self.bare_field(&token));
        }
        self.refuse_clause(scope, &token)?;
        // Pasted calls remain text, including their parentheses, when no function is declared.
        let mut text = token.text;
        if self.is("(") && self.peek().is_some_and(|t| t.span.start == token.span.end) {
            let begin = self.at;
            self.at += 1;
            while !self.query_end() && !self.is(")") {
                self.at += 1;
            }
            if self.take(")") {
                text = self.text[start..self.end()].into();
            } else {
                self.at = begin;
            }
        }
        let mode = if text.ends_with('*') {
            text.pop();
            TextMode::Prefix
        } else if text.ends_with('~') {
            text.pop();
            TextMode::Approximate
        } else {
            TextMode::Word
        };
        // Construct only text nodes that retain their meaning when rendered.
        // This also excludes a prefix or approximate match with no word.
        if let Err(mut error) = crate::format::word(&text) {
            let end = self.end();
            error.position = start;
            error.length = end - start;
            return Err(error.suggestion(self.replace(
                Span { start, end },
                &crate::format::quote(&self.text[start..end]),
            )));
        }
        Ok(self.cond(start, ConditionKind::Text { text, mode }))
    }
    /// A field name standing alone is never a search word; quoting it searches.
    fn bare_field(&self, token: &Token) -> Error {
        Error::new(
            ErrorKind::InvalidQuery,
            format!(
                "`{}` is a field; give it an operator and value, or quote it to search for the word",
                token.text
            ),
            token.span.start,
        )
        .length(token.span.end - token.span.start)
        .suggestion(self.replace(token.span, &crate::format::quote(&token.text)))
    }
    fn negation(&self, start: usize, condition: Condition) -> Condition {
        Condition {
            span: Span {
                start,
                end: condition.span.end,
            },
            kind: ConditionKind::Not {
                condition: Box::new(condition),
            },
        }
    }
    /// A transform written as a clause without its `|` (`sort when desc`,
    /// `limit 5`) is refused with the piped spelling.
    fn refuse_clause(&self, scope: &Scope, token: &Token) -> Result<(), Error> {
        let lower = token.text.to_ascii_lowercase();
        let next = self.peek().filter(|t| !t.quoted);
        let clause = match lower.as_str() {
            "sort" | "group" => {
                next.is_some_and(|t| t.text == "by" || self.field_known(scope, &t.text))
            }
            "limit" | "offset" | "first" => {
                next.is_some_and(|t| t.text.chars().all(|c| c.is_ascii_digit()))
            }
            "agg" => next.is_some_and(|t| AggregateFunction::parse(&t.text).is_some()),
            "as" => next.is_some_and(|t| t.text == "of"),
            "fill" => self.peek().is_none_or(|t| !t.quoted && t.text == "|"),
            _ => false,
        };
        if !clause {
            return Ok(());
        }
        let needs_by = matches!(lower.as_str(), "sort" | "group") && !self.is("by");
        let replacement = format!("| {}{}", token.text, if needs_by { " by" } else { "" });
        Err(Error::new(
            ErrorKind::InvalidQuery,
            format!("`{}` is a transform; transforms follow `|`", token.text),
            token.span.start,
        )
        .length(token.text.len())
        .suggestion(self.replace(token.span, &replacement)))
    }
    fn colon_atom(
        &mut self,
        scope: &Scope,
        token: &Token,
        field: &str,
        raw: &str,
        start: usize,
    ) -> Result<Condition, Error> {
        if field.eq_ignore_ascii_case("query") {
            let name = self.saved_name(raw, start)?;
            let arguments = self.arguments()?;
            return Ok(self.cond(start, ConditionKind::Saved { name, arguments }));
        }
        if field.eq_ignore_ascii_case("result") {
            let id = if raw.is_empty() {
                let value = self.next()?;
                if !value.quoted && is_symbol(&value.text) {
                    return Err(self.error("a kept result needs a literal identity"));
                }
                value.text
            } else {
                raw.into()
            };
            if id.is_empty() {
                return Err(self.error("a kept result needs a literal identity"));
            }
            return Ok(self.cond(
                start,
                ConditionKind::Reference {
                    reference: format!("result:{id}"),
                },
            ));
        }
        let quoted_reference = raw
            .is_empty()
            .then(|| self.peek())
            .flatten()
            .filter(|value| value.quoted)
            .map(|value| format!("{field}:{}", value.text));
        let reference = quoted_reference.as_deref().unwrap_or(&token.text);
        if self.catalog.is_reference(reference)
            && (self.scope_field(scope, field).is_none()
                || self.catalog.reference_overrides_field(reference))
        {
            let reference = reference.to_owned();
            if quoted_reference.is_some() {
                self.at += 1;
            }
            return Ok(self.cond(start, ConditionKind::Reference { reference }));
        }
        let list = RankList::ALL
            .into_iter()
            .find(|list| field.eq_ignore_ascii_case(list.name()));
        let keyword = list.is_some()
            || ["in", "since", "until"]
                .iter()
                .any(|name| field.eq_ignore_ascii_case(name));
        if !keyword {
            self.check_field(scope, field, start)?;
        }
        let mut value = if raw.is_empty() {
            if self
                .peek()
                .is_some_and(|t| !t.quoted && Op::parse(&t.text).is_some())
            {
                let op = self.next()?;
                let value = self.value()?;
                let end = self.end();
                let replacement = format!("{field} {} {}", op.text, value_surface(&value));
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "write comparisons with a space, without `:`",
                    start + field.len(),
                )
                .suggestion(self.replace(Span { start, end }, &replacement)));
            }
            self.value()?
        } else {
            value_token(raw, false)
        };
        if let Some(list) = list {
            return Ok(self.cond(start, ConditionKind::Rank { list, value }));
        }
        if field.eq_ignore_ascii_case("in")
            && value.text().is_some_and(|v| v.starts_with("result:"))
        {
            return Ok(self.cond(
                start,
                ConditionKind::Reference {
                    reference: format!("in:{}", value.text().unwrap()),
                },
            ));
        }
        if field.eq_ignore_ascii_case("since") || field.eq_ignore_ascii_case("until") {
            let kind = self.time_bound(scope, field.eq_ignore_ascii_case("until"), value)?;
            return Ok(self.cond(start, kind));
        }
        self.check_field(scope, field, start)?;
        self.normalize_field_value(scope, field, &mut value);
        self.check_value(scope, field, &value, start + field.len() + 1)?;
        Ok(self.cond(
            start,
            ConditionKind::Match {
                field: field.into(),
                value,
            },
        ))
    }
    /// `EXPR OP value`, `EXPR between A and B`, `EXPR in RANGE`, `EXPR in (QUERY)`.
    fn comparison(&mut self, scope: &Scope, start: usize) -> Result<Condition, Error> {
        let left = self.expression(scope, Place::Condition)?;
        if self.is("exists") {
            let exists = self.next()?;
            let field = self.text[start..exists.span.start].trim_end().to_string();
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "a field's presence is written `field:known`",
                start,
            )
            .suggestion(self.replace(
                Span {
                    start,
                    end: exists.span.end,
                },
                &format!("{field}:known"),
            )));
        }
        if self.take("between") {
            let mut from = self.value()?;
            self.expect("and")?;
            let mut to = self.value()?;
            self.normalize_expression_value(scope, &left, &mut from);
            self.normalize_expression_value(scope, &left, &mut to);
            self.check_expression_value(scope, &left, &from, start)?;
            self.check_expression_value(scope, &left, &to, start)?;
            return Ok(self.cond(start, ConditionKind::Between { left, from, to }));
        }
        if self.take("in") {
            if self.is("(") {
                let (query, _) = self.query_in(scope)?;
                return Ok(self.cond(
                    start,
                    ConditionKind::In {
                        left,
                        query: Box::new(query),
                    },
                ));
            }
            let value = self.value()?;
            return Ok(self.cond(start, ConditionKind::TimeRange { left, value }));
        }
        let op = self.op()?;
        if op == Op::Eq
            && self.is("[")
            && let Expression::Field { name } = &left
            && !["since", "until", "in"]
                .iter()
                .any(|word| name.eq_ignore_ascii_case(word))
        {
            // `F = [a, b]` is the set match `F:(a|b)`.
            let (open, set) = self.bracket_list()?;
            let end = self.end();
            let error = Error::new(
                ErrorKind::InvalidQuery,
                "a set is written (a|b) and matched with `:`",
                open.span.start,
            );
            return Err(error.suggestion(self.replace(
                Span { start, end },
                &format!("{name}:{}", value_surface(&set)),
            )));
        }
        let right = if let Some(t) = self.peek()
            && !t.quoted
            && self.next_is(1, "(")
            && self.catalog.function(&t.text)?.is_some()
        {
            let right = self.primary(scope, Place::Argument)?;
            self.check_comparable(
                &self.expression_info(scope, &left).field.kind,
                &self.expression_info(scope, &right).field.kind,
                start,
            )?;
            right
        } else {
            let mut value = self.value()?;
            self.normalize_expression_value(scope, &left, &mut value);
            if let Expression::Field { name } = &left
                && self
                    .scope_field(scope, name)
                    .is_some_and(|f| f.kind == FieldKind::Time)
                && matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge)
                && matches!(&value, Value::Scalar { text, quoted: false } if is_duration(text))
            {
                let duration = value.text().unwrap();
                let end = self.end();
                return Err(Error::new(
                    ErrorKind::BadValue,
                    "a bare duration is not a time instant; use since or a relative range",
                    start,
                )
                .suggestion(
                    self.replace(Span { start, end }, &format!("{name} in last {duration}")),
                ));
            }
            self.check_expression_value(scope, &left, &value, start)?;
            Expression::Value { value }
        };
        Ok(self.cond(start, ConditionKind::Compare { left, op, right }))
    }
    /// `term { (+|-) term }`, with `*` and `/` binding tighter. There are no
    /// parentheses in arithmetic; a parenthesis starts a condition group.
    fn expression(&mut self, scope: &Scope, place: Place) -> Result<Expression, Error> {
        let mut left = self.term(scope, place)?;
        while let Some(operator) = self.arithmetic_operator(false) {
            let at = self.pos();
            self.at += 1;
            let right = self.term(scope, place)?;
            left = self.arithmetic(scope, operator, left, right, at)?;
        }
        Ok(left)
    }
    fn term(&mut self, scope: &Scope, place: Place) -> Result<Expression, Error> {
        let mut left = self.primary(scope, place)?;
        while let Some(operator) = self.arithmetic_operator(true) {
            let at = self.pos();
            self.at += 1;
            let right = self.primary(scope, place)?;
            left = self.arithmetic(scope, operator, left, right, at)?;
        }
        Ok(left)
    }
    fn arithmetic_operator(&self, tight: bool) -> Option<Arithmetic> {
        let t = self.peek().filter(|t| !t.quoted)?;
        Arithmetic::parse(&t.text).filter(|operator| operator.binds_tightly() == tight)
    }
    fn arithmetic(
        &self,
        scope: &Scope,
        operator: Arithmetic,
        left: Expression,
        right: Expression,
        at: usize,
    ) -> Result<Expression, Error> {
        let l = self.expression_info(scope, &left).field.kind;
        let r = self.expression_info(scope, &right).field.kind;
        let (l, r) = (
            typing::operand_kind_in(operator, &left, &l, &r),
            typing::operand_kind_in(operator, &right, &r, &l),
        );
        if crate::scope::arithmetic_kind(operator, &l, &r).is_none() {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                format!(
                    "`{}` cannot combine {} and {}",
                    operator.symbol(),
                    kind_name(&l),
                    kind_name(&r)
                ),
                at,
            )
            .length(1));
        }
        Ok(Expression::Arithmetic {
            operator,
            left: Box::new(left),
            right: Box::new(right),
        })
    }
    fn primary(&mut self, scope: &Scope, place: Place) -> Result<Expression, Error> {
        let t = self
            .peek()
            .cloned()
            .ok_or_else(|| self.error("expected a field"))?;
        if t.quoted {
            if place == Place::Condition {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "expected a field",
                    t.span.start,
                ));
            }
            self.at += 1;
            return Ok(Expression::value(Value::Scalar {
                text: t.text,
                quoted: true,
            }));
        }
        if is_symbol(&t.text) {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "expected a field",
                t.span.start,
            ));
        }
        if self.next_is(1, "(")
            && let Some(function) = self.catalog.function(&t.text)?
        {
            self.at += 2;
            let mut arguments = Vec::new();
            if !self.is(")") {
                loop {
                    arguments.push(self.expression(scope, Place::Argument)?);
                    if !self.take(",") {
                        break;
                    }
                }
            }
            self.expect(")")?;
            if arguments.len() != function.arguments {
                return Err(Error::new(
                    ErrorKind::BadValue,
                    format!(
                        "`{}` takes {} argument{}",
                        function.name,
                        function.arguments,
                        if function.arguments == 1 { "" } else { "s" }
                    ),
                    t.span.start,
                ));
            }
            if function.name == "abs"
                && !matches!(
                    self.expression_info(scope, &arguments[0]).field.kind,
                    FieldKind::Integer
                        | FieldKind::Number
                        | FieldKind::Money
                        | FieldKind::Duration
                        | FieldKind::Json
                )
                && !matches!(
                    &arguments[0],
                    Expression::Value {
                        value: Value::Unknown
                    }
                )
            {
                return Err(Error::new(
                    ErrorKind::BadValue,
                    "abs takes a linear number, money or duration",
                    t.span.start,
                ));
            }
            if function.name == "glob" {
                for argument in &arguments {
                    if !matches!(
                        self.expression_info(scope, argument).field.kind,
                        FieldKind::Text
                            | FieldKind::Exact
                            | FieldKind::Substring
                            | FieldKind::Words
                            | FieldKind::Subtree
                            | FieldKind::Reference
                            | FieldKind::Json
                    ) {
                        return Err(Error::new(
                            ErrorKind::BadValue,
                            "glob takes text values",
                            t.span.start,
                        ));
                    }
                }
            }
            if function.name == "case_eq" {
                let (first, rest) = arguments.split_at_mut(1);
                let (left, right) = (&mut first[0], &mut rest[0]);
                match (left, right) {
                    (left, Expression::Value { value }) => {
                        self.normalize_expression_value(scope, left, value);
                        self.check_expression_value(scope, left, value, t.span.start)?;
                    }
                    (Expression::Value { value }, right) => {
                        self.normalize_expression_value(scope, right, value);
                        self.check_expression_value(scope, right, value, t.span.start)?;
                    }
                    _ => {}
                }
                let left = &arguments[0];
                let right = &arguments[1];
                let left_kind = self.expression_info(scope, left).field.kind;
                let right_kind = self.expression_info(scope, right).field.kind;
                let contextual = |expression: &Expression, own: &FieldKind, other: &FieldKind| {
                    if let Expression::Value { value } = expression {
                        if matches!(value, Value::Unknown) {
                            FieldKind::Json
                        } else if matches!(
                            other,
                            FieldKind::Text
                                | FieldKind::Reference
                                | FieldKind::Bool
                                | FieldKind::Json
                                | FieldKind::List(_)
                        ) {
                            other.clone()
                        } else {
                            typing::operand_kind(expression, other)
                        }
                    } else {
                        own.clone()
                    }
                };
                self.check_comparable(
                    &contextual(left, &left_kind, &right_kind),
                    &contextual(right, &right_kind, &left_kind),
                    t.span.start,
                )?;
            }
            return Ok(Expression::Function {
                name: function.name,
                arguments,
            });
        }
        let known = self.field_known(scope, &t.text);
        if !known && (place == Place::Argument || is_literal(&t.text)) {
            self.at += 1;
            return Ok(Expression::value(value_token(&t.text, false)));
        }
        self.at += 1;
        self.check_field(scope, &t.text, t.span.start)?;
        Ok(Expression::field(t.text))
    }
    fn cond(&self, start: usize, kind: ConditionKind) -> Condition {
        Condition {
            span: Span {
                start,
                end: self.end(),
            },
            kind,
        }
    }
    /// The former list spelling `[a, b]`, read only to refuse it.
    fn bracket_list(&mut self) -> Result<(Token, Value), Error> {
        let open = self.next()?;
        let mut values = vec![self.value()?];
        while self.take(",") {
            values.push(self.value()?);
        }
        self.expect("]")?;
        Ok((open, Value::Set { values }))
    }
    fn op(&mut self) -> Result<Op, Error> {
        let t = self.next()?;
        Op::parse(&t.text).filter(|_| !t.quoted).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidQuery,
                "expected a comparison operator",
                t.span.start,
            )
        })
    }
    fn number(&mut self) -> Result<u64, Error> {
        let t = self.next()?;
        t.text.parse().map_err(|_| {
            Error::new(
                ErrorKind::BadValue,
                "expected a nonnegative integer",
                t.span.start,
            )
        })
    }
    fn time_bound(&self, scope: &Scope, until: bool, value: Value) -> Result<ConditionKind, Error> {
        let field = self
            .main_time(scope)
            .ok_or_else(|| self.error("this unit has no main time"))?;
        Ok(if until {
            ConditionKind::Until { field, value }
        } else {
            ConditionKind::Since { field, value }
        })
    }
    fn value(&mut self) -> Result<Value, Error> {
        if self.is("(") {
            let open = self.next()?;
            let mut values = vec![self.value()?];
            while self.take("|") {
                values.push(self.value()?);
            }
            if self.peek().is_some() && !self.is(")") {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "a set separates its values with `|`",
                    open.span.start,
                ));
            }
            self.expect(")")?;
            return Ok(Value::Set { values });
        }
        if self.is("[") {
            let (open, set) = self.bracket_list()?;
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "a set is written (a|b)",
                open.span.start,
            )
            .suggestion(self.replace(
                Span {
                    start: open.span.start,
                    end: self.end(),
                },
                &value_surface(&set),
            )));
        }
        let t = self.next()?;
        if !t.quoted && is_symbol(&t.text) {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "expected a value",
                t.span.start,
            ));
        }
        let mut value = value_token(&t.text, t.quoted);
        if !t.quoted {
            let mut phrase = t.text.clone();
            if matches!(t.text.as_str(), "last" | "this") {
                if let Some(next) = self.peek()
                    && !next.quoted
                    && !is_symbol(&next.text)
                {
                    phrase.push(' ');
                    phrase.push_str(&self.next()?.text);
                    if phrase
                        .split_whitespace()
                        .last()
                        .is_some_and(|s| s.chars().all(|c| c.is_ascii_digit()))
                    {
                        phrase.push(' ');
                        phrase.push_str(&self.next()?.text);
                    }
                }
            } else if t.text.chars().all(|c| c.is_ascii_digit())
                && self
                    .peek()
                    .is_some_and(|t| matches!(t.text.as_str(), "days" | "day" | "weeks" | "hours"))
            {
                phrase.push(' ');
                phrase.push_str(&self.next()?.text);
                if self.take("ago") {
                    phrase.push_str(" ago");
                }
            } else if t.text.len() == 10
                && t.text.as_bytes().get(4) == Some(&b'-')
                && self.peek().is_some_and(|t| is_clock(&t.text))
            {
                phrase.push(' ');
                phrase.push_str(&self.next()?.text);
            }
            if phrase != t.text {
                value = Value::scalar(phrase);
            }
        }
        Ok(value)
    }
    fn arguments(&mut self) -> Result<Vec<Argument>, Error> {
        let mut arguments = Vec::new();
        if !self.take("(") {
            return Ok(arguments);
        }
        if self.take(")") {
            return Ok(arguments);
        }
        loop {
            let t = self.name()?;
            if arguments.iter().any(|a: &Argument| a.parameter == t.text) {
                return Err(Error::new(
                    ErrorKind::BadValue,
                    format!("duplicate parameter `{}`", t.text),
                    t.span.start,
                ));
            }
            self.expect("=")?;
            let value = self.value()?;
            arguments.push(Argument {
                parameter: t.text,
                value,
            });
            if !self.take(",") {
                break;
            }
        }
        self.expect(")")?;
        Ok(arguments)
    }
    fn sequence(&mut self, scope: &Scope) -> Result<Condition, Error> {
        let unit = self
            .catalog
            .unit(&scope.unit)
            .and_then(|unit| unit.sequence_unit.clone())
            .unwrap_or_else(|| scope.unit.clone());
        let steps_scope = &Scope::new(unit.clone());
        let start = self.pos();
        self.expect("seq")?;
        let all = self.take("all");
        self.expect("(")?;
        let mut steps = vec![self.condition(steps_scope)?];
        let mut gaps = Vec::new();
        while self.take("->") {
            let gap = if self.take("[") {
                let gap = if self.take("next") {
                    Gap::Next
                } else if self.take("within") {
                    let distance = self.distance()?;
                    if matches!(distance, Distance::Steps { .. }) {
                        self.require_order_scope(&unit, start)?;
                    }
                    Gap::Within { distance }
                } else {
                    let t = self.next()?;
                    let mut error = Error::new(
                        ErrorKind::InvalidQuery,
                        "a gap is [next] or [within D]",
                        t.span.start,
                    );
                    if let Some(duration) = t.text.strip_prefix("within:") {
                        error =
                            error.suggestion(self.replace(t.span, &format!("within {duration}")));
                    }
                    return Err(error);
                };
                self.expect("]")?;
                gap
            } else {
                Gap::Later
            };
            gaps.push(gap);
            steps.push(self.condition(steps_scope)?);
        }
        self.expect(")")?;
        if steps.len() < 2 {
            return Err(self.error("a sequence needs at least two steps"));
        }
        Ok(self.cond(
            start,
            ConditionKind::Sequence {
                unit,
                all,
                steps,
                gaps,
            },
        ))
    }
    /// `5m`, `2 hours` or `N steps`.
    fn distance(&mut self) -> Result<Distance, Error> {
        if self.peek().is_some_and(|t| {
            !t.quoted && !t.text.is_empty() && t.text.chars().all(|c| c.is_ascii_digit())
        }) && (self.next_is(1, "steps") || self.next_is(1, "step"))
        {
            let count = self.number()?;
            self.at += 1;
            return Ok(Distance::Steps { count });
        }
        Ok(Distance::Time {
            duration: self.value()?,
        })
    }
    fn order_scope(&self, unit: &str) -> Option<String> {
        self.catalog
            .unit(unit)
            .and_then(|unit| unit.order_scope.clone())
    }
    fn require_order_scope(&self, unit: &str, at: usize) -> Result<String, Error> {
        if self.catalog.unit(unit).is_none() {
            return Ok(String::new());
        }
        self.order_scope(unit).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidQuery,
                format!("`{unit}` has no order scope; steps count positions in one, so use a time distance"),
                at,
            )
        })
    }
    fn order_condition(&mut self, scope: &Scope) -> Result<Condition, Error> {
        let start = self.pos();
        let word = self.next()?;
        let relation = OrderRelation::parse(&word.text).expect("checked by the caller");
        let (query, inner) = self.query_in(scope)?;
        let (within, at, order_scope) = self.order_options(relation, scope, &inner, start)?;
        let pick = if self.take("pick") {
            self.expect("first")?;
            self.expect("by")?;
            let candidate = self.witness_scope(&inner, within.as_ref());
            let mut keys = Vec::new();
            loop {
                let expression = self.expression(&candidate, Place::Condition)?;
                let descending = self.take("desc");
                if !descending {
                    self.take("asc");
                }
                keys.push(SortKey {
                    expression,
                    descending,
                });
                if !self.take(",") {
                    break;
                }
            }
            WitnessChoice::FirstBy { keys }
        } else {
            WitnessChoice::Nearest
        };
        self.inner_scopes.insert(start, inner);
        Ok(self.cond(
            start,
            ConditionKind::Order {
                relation,
                query: Box::new(query),
                within,
                at,
                scope: order_scope,
                pick,
            },
        ))
    }
    fn witness_scope(&self, inner: &Scope, within: Option<&Distance>) -> Scope {
        let mut candidate = inner.clone();
        for (name, kind) in [
            ("match.id", FieldKind::Reference),
            ("match.coarse", FieldKind::Bool),
            (
                crate::catalog::MATCH_DISTANCE,
                if matches!(within, Some(Distance::Steps { .. })) {
                    FieldKind::Integer
                } else {
                    FieldKind::Duration
                },
            ),
        ] {
            self.add_generated(
                &mut candidate,
                Field {
                    name: name.into(),
                    kind,
                    ..Field::default()
                },
                Origin::Generated,
            );
        }
        candidate
    }
    /// `[within DISTANCE] [at when|start|end] [in SCOPE]` after an order relation.
    fn order_options(
        &mut self,
        relation: OrderRelation,
        outer: &Scope,
        inner: &Scope,
        start: usize,
    ) -> Result<(Option<Distance>, TimePoint, Option<String>), Error> {
        let within = if self.take("within") {
            Some(self.distance()?)
        } else {
            None
        };
        // `at` is a time point only when one follows; otherwise it may be a
        // field named `at` in the next condition.
        let point_follows = [
            "when",
            "start",
            "end",
            "match.when",
            "match.start",
            "match.end",
        ]
        .iter()
        .any(|point| self.next_is(1, point));
        let at = if self.is("at") && point_follows {
            self.at += 1;
            let point = self.next()?;
            match point.text.to_ascii_lowercase().as_str() {
                "when" => TimePoint::When,
                "start" => TimePoint::Start,
                "end" => TimePoint::End,
                "match.when" => TimePoint::MatchWhen,
                "match.start" => TimePoint::MatchStart,
                "match.end" => TimePoint::MatchEnd,
                _ => {
                    return Err(Error::new(
                        ErrorKind::BadValue,
                        "at takes when, start, end, match.when, match.start or match.end",
                        point.span.start,
                    ));
                }
            }
        } else {
            TimePoint::When
        };
        let order_scope = if self.take("in") {
            Some(self.name()?.text)
        } else {
            None
        };
        if relation == OrderRelation::Near && within.is_none() {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "near needs a distance: `within 5m` or `within 3 steps`",
                start,
            ));
        }
        if matches!(within, Some(Distance::Steps { .. })) {
            if !relation.measures_steps() {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!(
                        "{} compares times; steps measure positions",
                        relation.name()
                    ),
                    start,
                ));
            }
            if at != TimePoint::When {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "at chooses a time point; steps count positions",
                    start,
                ));
            }
            let declared = self.require_order_scope(&outer.unit, start)?;
            let inner_scope = self.order_scope(&inner.unit);
            if !declared.is_empty()
                && self.catalog.unit(&inner.unit).is_some()
                && inner_scope.as_deref() != Some(declared.as_str())
            {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!(
                        "`{}` steps in `{declared}` and `{}` in {}; steps never cross order scopes",
                        outer.unit,
                        inner.unit,
                        inner_scope.map_or("none".to_string(), |scope| format!("`{scope}`"))
                    ),
                    start,
                ));
            }
            if let Some(named) = &order_scope
                && !declared.is_empty()
                && *named != declared
            {
                return Err(Error::new(
                    ErrorKind::BadValue,
                    format!("`{}` steps in `{declared}`, not `{named}`", outer.unit),
                    start,
                ));
            }
        }
        Ok((within, at, order_scope))
    }
    /// One top-level order relation in a relation's condition exposes its
    /// nearest inner row as `match.F` and the signed distance as `match.distance`.
    fn expose_order_match(&mut self, scope: &mut Scope, condition: &Condition) {
        if let Some(order) = condition.exposed_order()
            && let ConditionKind::Order { within, .. } = &order.kind
            && let Some(inner) = self.inner_scopes.get(&order.span.start).cloned()
        {
            self.expose_inner(scope, &inner, within.as_ref());
        }
    }
    fn expose_inner(&self, scope: &mut Scope, inner: &Scope, within: Option<&Distance>) {
        for info in self.visible(inner) {
            let mut field = info.field.clone();
            field.name = format!("match.{}", info.field.name);
            self.add_generated(
                scope,
                field,
                Origin::Matched {
                    input: Box::new(info),
                },
            );
        }
        if let Some(within) = within {
            self.add_generated(
                scope,
                Field {
                    name: crate::catalog::MATCH_DISTANCE.into(),
                    kind: match within {
                        Distance::Steps { .. } => FieldKind::Integer,
                        Distance::Time { .. } => FieldKind::Duration,
                    },
                    ..Field::default()
                },
                Origin::Generated,
            );
        }
    }
    fn add_generated(&self, scope: &mut Scope, field: Field, origin: Origin) {
        let name = field.name.clone();
        Arc::make_mut(&mut scope.typed_extra).retain(|f| f.name != name);
        Arc::make_mut(&mut scope.typed_extra).push(field);
        Arc::make_mut(&mut scope.origins).insert(name.clone(), origin);
        if let Some(columns) = &mut scope.columns {
            columns.insert(name);
        } else {
            scope.extra.insert(name);
        }
    }
    fn field_known(&self, scope: &Scope, field: &str) -> bool {
        if (scope.columns_unknown && scope.columns.is_none()) || scope.extra.contains(field) {
            return true;
        }
        scope.columns.as_ref().map_or_else(
            || self.scope_field(scope, field).is_some(),
            |c| c.contains(field),
        )
    }
    fn check_field(&self, scope: &Scope, field: &str, position: usize) -> Result<(), Error> {
        if self.field_known(scope, field) {
            return Ok(());
        }
        let names: Vec<String> = if let Some(columns) = &scope.columns {
            columns.iter().cloned().collect()
        } else {
            self.catalog
                .unit(&scope.unit)
                .into_iter()
                .flat_map(|u| u.fields.iter().map(|f| f.name.clone()))
                .chain(scope.extra.iter().cloned())
                .collect()
        };
        let message = format!(
            "unknown {}field `{field}` on `{}`; fields: {}",
            if scope.columns.is_some() {
                "grouped "
            } else {
                ""
            },
            scope.unit,
            names.join(", ")
        );
        let token = Token {
            text: field.into(),
            quoted: false,
            span: Span {
                start: position,
                end: position + field.len(),
            },
        };
        Err(self.near_miss(
            &token,
            message,
            ErrorKind::UnknownField,
            names.iter().map(String::as_str).collect(),
        ))
    }
    fn normalize_expression_value(&self, scope: &Scope, left: &Expression, value: &mut Value) {
        if let Expression::Field { name } = left {
            self.normalize_field_value(scope, name, value);
        }
    }
    fn normalize_field_value(&self, scope: &Scope, field: &str, value: &mut Value) {
        if let Value::Set { values } = value {
            for value in values {
                self.normalize_field_value(scope, field, value);
            }
            return;
        }
        if let Some(spec) = self.scope_field(scope, field)
            && let crate::catalog::UnknownRule::Word(word) = &spec.unknown
            && matches!(value,Value::Scalar{text,quoted:false} if text.eq_ignore_ascii_case(word))
        {
            *value = Value::Unknown;
        }
        if let Some(spec) = self.scope_field(scope, field)
            && let Value::Scalar { text, .. } = value
            && let Some(canonical) = spec
                .values
                .iter()
                .find(|candidate| candidate.eq_ignore_ascii_case(text))
        {
            *text = canonical.clone();
        }
    }
    fn check_value(
        &self,
        scope: &Scope,
        field: &str,
        value: &Value,
        position: usize,
    ) -> Result<(), Error> {
        if let Value::Set { values } = value {
            for v in values {
                self.check_value(scope, field, v, position)?;
            }
            return Ok(());
        }
        let Value::Scalar { text, .. } = value else {
            return Ok(());
        };
        let Some(spec) = self.scope_field(scope, field) else {
            return Ok(());
        };
        if !spec.values.is_empty() && !spec.values.iter().any(|v| v.eq_ignore_ascii_case(text)) {
            let elsewhere = self.catalog.units_accepting(text);
            let mut message = format!(
                "`{field}` on `{}` is one of: {}",
                scope.unit,
                spec.values.join(", ")
            );
            if !elsewhere.is_empty() {
                message.push_str(&format!(
                    "; `{text}` is a value of {}",
                    elsewhere.join(", ")
                ));
            }
            return Err(
                Error::new(ErrorKind::BadValue, message, position).candidates(spec.values.clone())
            );
        }
        let valid = match spec.kind {
            FieldKind::Integer => crate::number::is_integer(text),
            FieldKind::Number => crate::number::is_decimal(text),
            FieldKind::Bool => matches!(text.as_str(), "true" | "false"),
            _ => true,
        };
        if valid {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::BadValue,
                format!("`{text}` is not a {:?} value for `{field}`", spec.kind),
                position,
            ))
        }
    }
    fn check_expression_value(
        &self,
        scope: &Scope,
        left: &Expression,
        value: &Value,
        position: usize,
    ) -> Result<(), Error> {
        if let Expression::Field { name } = left {
            return self.check_value(scope, name, value, position);
        }
        let left_kind = self.expression_info(scope, left).field.kind;
        let values = match value {
            Value::Set { values } => values.as_slice(),
            other => std::slice::from_ref(other),
        };
        for value in values {
            if matches!(value, Value::Scalar { .. }) {
                let right_kind =
                    typing::operand_kind(&Expression::value(value.clone()), &left_kind);
                self.check_comparable(&left_kind, &right_kind, position)?;
            }
        }
        Ok(())
    }
    /// Refuse a comparison whose two sides have different dimensions:
    /// `cost * 2 > 1h` compares money with a duration.
    fn check_comparable(
        &self,
        left: &FieldKind,
        right: &FieldKind,
        position: usize,
    ) -> Result<(), Error> {
        if typing::comparable(left, right) {
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::InvalidQuery,
                format!(
                    "cannot compare {} with {}",
                    kind_name(left),
                    kind_name(right)
                ),
                position,
            ))
        }
    }
    fn saved_name(&mut self, raw: &str, position: usize) -> Result<String, Error> {
        let (name, quoted) = if raw.is_empty() {
            let token = self.next()?;
            (token.text, token.quoted)
        } else {
            (raw.to_string(), false)
        };
        if name.is_empty() || (!quoted && (name.starts_with('$') || is_symbol(&name))) {
            return Err(Error::new(
                ErrorKind::BadValue,
                "a saved query needs a literal name; parameters stand only for values",
                position,
            ));
        }
        Ok(name)
    }
    fn attachments(&mut self) -> Result<Vec<Attachment>, Error> {
        let mut attachments = Vec::new();
        loop {
            let start = self.pos();
            let unit = self.unit_name()?;
            let scope = Scope::new(unit.clone());
            self.expect("(")?;
            let mut fields = Vec::new();
            if !self.is(")") {
                loop {
                    let f = self.name()?;
                    self.check_field(&scope, &f.text, f.span.start)?;
                    fields.push(f.text);
                    if !self.take(",") {
                        break;
                    }
                }
            }
            self.expect(")")?;
            let mut windows = Vec::new();
            if self.take("[") {
                loop {
                    if self.peek().is_some_and(|t| {
                        t.text.starts_with("first:") || t.text.starts_with("last:")
                    }) {
                        let t = self.next()?;
                        let (kind, n) = t.text.split_once(':').unwrap();
                        let n = if n.is_empty() {
                            self.number()?
                        } else {
                            n.parse().map_err(|_| {
                                Error::new(
                                    ErrorKind::BadValue,
                                    "expected a nonnegative integer",
                                    t.span.start,
                                )
                            })?
                        };
                        windows.push(if kind == "first" {
                            Window::First(n)
                        } else {
                            Window::Last(n)
                        });
                    } else {
                        windows.push(Window::Where(self.condition(&scope)?));
                    }
                    if !self.take(",") {
                        break;
                    }
                }
                self.expect("]")?;
            }
            attachments.push(Attachment {
                span: Span {
                    start,
                    end: self.end(),
                },
                unit,
                fields,
                windows,
            });
            if !self.take(",") {
                break;
            }
        }
        Ok(attachments)
    }
    fn transform(&mut self, scope: &mut Scope) -> Result<Transform, Error> {
        let start = self.pos();
        let t = self.name()?;
        let word = t.text.to_ascii_lowercase();
        let kind = match word.as_str() {
            "where" => {
                let condition = self.condition(scope)?;
                if let Some(known) = &mut scope.patterns {
                    known.extend(patterns(Some(&condition)));
                }
                self.expose_order_match(scope, &condition);
                TransformKind::Where { condition }
            }
            "since" | "until" => {
                let error = Error::new(
                    ErrorKind::InvalidQuery,
                    "time bounds are conditions; write them after where",
                    start,
                );
                // Suggest the joined bound only for a value that reads as a time.
                return Err(
                    match self.peek().filter(|next| {
                        (next.quoted || !is_symbol(&next.text))
                            && (next.text.starts_with('$')
                                || crate::time::is_time_expression(&next.text))
                    }) {
                        Some(next) => {
                            let bound = Span {
                                start: t.span.start,
                                end: next.span.end,
                            };
                            let value = &self.text[next.span.start..next.span.end];
                            error.suggestion(self.replace(bound, &format!("where {word}:{value}")))
                        }
                        None => error,
                    },
                );
            }
            "sort" => {
                if !self.take("by") {
                    return Err(Error::new(
                        ErrorKind::InvalidQuery,
                        "sort is written `sort by KEY`",
                        start,
                    )
                    .suggestion(self.replace(t.span, &format!("{} by", t.text))));
                }
                let mut keys = Vec::new();
                loop {
                    let expression = if self.is("random") && !self.field_known(scope, "random") {
                        self.at += 1;
                        let mut arguments = Vec::new();
                        if self.take("(") && !self.take(")") {
                            let n = self.number()?;
                            arguments.push(Expression::value(Value::scalar(n.to_string())));
                            self.expect(")")?;
                        }
                        Expression::Function {
                            name: "random".into(),
                            arguments,
                        }
                    } else if self.is("rank") && !self.field_known(scope, "rank") {
                        self.at += 1;
                        Expression::field("rank")
                    } else {
                        self.expression(scope, Place::Condition)?
                    };
                    let descending = self.take("desc");
                    if !descending {
                        self.take("asc");
                    }
                    keys.push(SortKey {
                        expression,
                        descending,
                    });
                    if !self.take(",") {
                        break;
                    }
                }
                TransformKind::Sort { keys }
            }
            "group" => {
                self.expect("by")?;
                let mut keys = Vec::new();
                loop {
                    let expression = self.expression(scope, Place::Condition)?;
                    let name = if self.take("as") {
                        Some(self.name()?.text)
                    } else {
                        None
                    };
                    keys.push(Column { expression, name });
                    if !self.take(",") {
                        break;
                    }
                }
                let inputs: Vec<_> = keys
                    .iter()
                    .map(|key| self.expression_info(scope, &key.expression))
                    .collect();
                let rows = scope.clone();
                Arc::make_mut(&mut scope.typed_extra).clear();
                Arc::make_mut(&mut scope.origins).clear();
                scope.extra.clear();
                scope.columns = Some(BTreeSet::new());
                for (index, (key, mut info)) in keys.iter().zip(inputs).enumerate() {
                    let input = info.clone();
                    info.field.name = key.column();
                    self.add_generated(
                        scope,
                        info.field,
                        Origin::Group {
                            operand: self.transform_path().field("keys").index(index),
                            input: Box::new(input),
                        },
                    );
                }
                self.add_generated(
                    scope,
                    Field {
                        name: "count".into(),
                        kind: FieldKind::Integer,
                        ..Field::default()
                    },
                    Origin::Aggregate {
                        at: self.transform_path(),
                        function: AggregateFunction::Count,
                        inputs: Vec::new(),
                        rule: crate::scope::TypeRule::Fixed,
                    },
                );
                scope.group_input = Some(Arc::new(rows));
                scope.bucketed = keys.iter().any(|key| {
                    matches!(&key.expression, Expression::Function { name, .. }
                        if crate::catalog::TIME_BUCKETS.contains(&name.as_str()))
                });
                scope.patterns = Some(Vec::new());
                TransformKind::Group { keys }
            }
            "agg" => TransformKind::Aggregate {
                aggregates: self.aggregates(scope, false)?,
            },
            // `| count` is exactly `| agg count`.
            "count" => {
                self.at -= 1;
                TransformKind::Aggregate {
                    aggregates: self.aggregates(scope, true)?,
                }
            }
            "limit" => TransformKind::Limit {
                count: self.number()?,
            },
            "offset" => {
                if self.query_end() && self.field_known(scope, &t.text) {
                    return Err(Error::new(
                        ErrorKind::InvalidQuery,
                        "`offset` is a field and a transform; supply a count or quote it to search",
                        t.span.start,
                    )
                    .suggestion(self.replace(t.span, "\"offset\"")));
                }
                TransformKind::Offset {
                    count: self.number()?,
                }
            }
            "first" => {
                let count = self.number()?;
                if let Some(input) = scope.group_input.take() {
                    *scope = (*input).clone();
                }
                TransformKind::First { count }
            }
            "distinct" => TransformKind::Distinct,
            "matches" => {
                self.matches(scope, start)?;
                TransformKind::Matches
            }
            "fill" => {
                if !scope.bucketed {
                    return Err(self.error("fill follows a group by a time bucket"));
                }
                self.add_generated(
                    scope,
                    Field {
                        name: crate::coverage::STATE_FIELD.into(),
                        kind: FieldKind::Exact,
                        values: crate::coverage::CoverageState::STATES
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                        ..Field::default()
                    },
                    Origin::Generated,
                );
                self.add_generated(
                    scope,
                    Field {
                        name: crate::coverage::DETAIL_FIELD.into(),
                        kind: FieldKind::Text,
                        ..Field::default()
                    },
                    Origin::Generated,
                );
                TransformKind::Fill
            }
            "fields" => {
                let mut columns = Vec::new();
                loop {
                    let expression = self.expression(scope, Place::Column)?;
                    let name = if self.take("as") {
                        Some(self.name()?.text)
                    } else {
                        None
                    };
                    columns.push(Column { expression, name });
                    if !self.take(",") {
                        break;
                    }
                }
                let infos: Vec<_> = columns
                    .iter()
                    .map(|column| {
                        let mut info = self.expression_info(scope, &column.expression);
                        info.field.name = column.column();
                        info
                    })
                    .collect();
                let mut names = BTreeSet::new();
                for info in &infos {
                    if !names.insert(info.field.name.clone()) {
                        return Err(Error::new(
                            ErrorKind::InvalidQuery,
                            format!("two columns are named `{}`", info.field.name),
                            start,
                        ));
                    }
                }
                *Arc::make_mut(&mut scope.typed_extra) =
                    infos.iter().map(|info| info.field.clone()).collect();
                *Arc::make_mut(&mut scope.origins) = infos
                    .into_iter()
                    .map(|info| (info.field.name, info.origin))
                    .collect();
                scope.extra.clear();
                scope.columns = Some(names);
                TransformKind::Fields { columns }
            }
            "union" | "intersect" | "except" | "fuse" => {
                let (query, inner) = self.query_in(scope)?;
                let how = match word.as_str() {
                    "union" => Combination::Union,
                    "intersect" => Combination::Intersect,
                    "except" => Combination::Except,
                    _ => Combination::Fuse,
                };
                if how != Combination::Fuse {
                    self.check_same_rows(scope, &inner, &word, start)?;
                }
                TransformKind::Combine {
                    how,
                    query: Box::new(query),
                }
            }
            "join" => self.join(scope, start)?,
            "diff" => {
                let (query, inner) = self.query_in(scope)?;
                self.check_same_rows(scope, &inner, "diff", start)?;
                self.add_generated(
                    scope,
                    Field {
                        name: "change".into(),
                        kind: FieldKind::Exact,
                        values: crate::diff::ChangeKind::NAMES
                            .iter()
                            .map(|s| s.to_string())
                            .collect(),
                        ..Field::default()
                    },
                    Origin::Generated,
                );
                for name in ["old_rank", "new_rank"] {
                    self.add_generated(
                        scope,
                        Field {
                            name: name.into(),
                            kind: FieldKind::Integer,
                            ..Field::default()
                        },
                        Origin::Generated,
                    );
                }
                TransformKind::Diff {
                    query: Box::new(query),
                }
            }
            "as" => {
                self.expect("of")?;
                let sequence = self.take("seq");
                TransformKind::AsOf {
                    value: self.value()?,
                    sequence,
                }
            }
            _ => {
                if let Some(extension) = self
                    .catalog
                    .extensions
                    .iter()
                    .find(|e| {
                        e.position == ExtensionPosition::Transform
                            && e.name.eq_ignore_ascii_case(&t.text)
                    })
                    .cloned()
                {
                    let arguments = self.extension_arguments(scope, &extension.arguments)?;
                    TransformKind::Extension {
                        name: extension.name,
                        arguments,
                    }
                } else if let Some(unit) = self.catalog.unit(&t.text) {
                    *scope = Scope::new(unit.name.clone());
                    let condition = if self.take("where") {
                        Some(self.condition(scope)?)
                    } else {
                        None
                    };
                    scope.patterns = Some(patterns(condition.as_ref()));
                    if let Some(condition) = &condition {
                        self.expose_order_match(scope, condition);
                    }
                    TransformKind::Scope {
                        unit: scope.unit.clone(),
                        condition,
                    }
                } else if AggregateFunction::parse(t.text.split(':').next().unwrap_or("")).is_some()
                {
                    return Err(Error::new(
                        ErrorKind::InvalidQuery,
                        "aggregates follow agg",
                        start,
                    )
                    .suggestion(self.replace(t.span, &format!("agg {}", t.text))));
                } else {
                    let names: Vec<&str> = crate::catalog::TRANSFORMS
                        .iter()
                        .map(|transform| transform.name)
                        .chain(self.catalog.units.iter().map(|u| u.name.as_str()))
                        .chain(
                            self.catalog
                                .extensions
                                .iter()
                                .filter(|e| e.position == ExtensionPosition::Transform)
                                .map(|e| e.name.as_str()),
                        )
                        .collect();
                    return Err(self.near_miss(
                        &t,
                        format!("`{}` is not a transform or a unit", t.text),
                        ErrorKind::InvalidQuery,
                        names,
                    ));
                }
            }
        };
        if matches!(
            kind,
            TransformKind::Group { .. } | TransformKind::Aggregate { .. }
        ) {
            scope.row_domain = None;
        }
        if matches!(
            kind,
            TransformKind::Group { .. }
                | TransformKind::Aggregate { .. }
                | TransformKind::Fields { .. }
                | TransformKind::Join { .. }
                | TransformKind::Diff { .. }
        ) {
            scope.patterns = Some(Vec::new());
        }
        if !matches!(
            kind,
            TransformKind::Group { .. } | TransformKind::Aggregate { .. }
        ) {
            scope.group_input = None;
        }
        Ok(Transform {
            span: Span {
                start,
                end: self.end(),
            },
            kind,
        })
    }
    /// Set algebra and diff need rows of one unit, or identical projected columns.
    fn check_same_rows(
        &self,
        outer: &Scope,
        inner: &Scope,
        word: &str,
        at: usize,
    ) -> Result<(), Error> {
        if outer.columns_unknown || inner.columns_unknown {
            return Ok(());
        }
        let same = match (&outer.columns, &inner.columns) {
            (Some(a), Some(b)) => a == b,
            (None, None) => {
                self.catalog.unit(&outer.unit).map(|u| &u.name)
                    == self.catalog.unit(&inner.unit).map(|u| &u.name)
            }
            _ => false,
        };
        if same {
            return Ok(());
        }
        Err(Error::new(
            ErrorKind::InvalidQuery,
            format!(
                "{word} needs rows of one unit or the same projected fields; project both sides with `| fields …`"
            ),
            at,
        ))
    }
    fn join(&mut self, scope: &mut Scope, start: usize) -> Result<TransformKind, Error> {
        if self.is("(") {
            let (query, inner) = self.query_in(scope)?;
            self.expect("on")?;
            let left = self.name()?;
            self.check_field(scope, &left.text, left.span.start)?;
            self.expect("=")?;
            let right = self.name()?;
            self.check_field(&inner, &right.text, right.span.start)?;
            self.expose_inner(scope, &inner, None);
            return Ok(TransformKind::Join {
                query: Box::new(query),
                on: JoinOn::Key {
                    left: left.text,
                    right: right.text,
                },
            });
        }
        let word = self.name()?;
        let Some(relation) = OrderRelation::parse(&word.text) else {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "join takes an order relation (near, before, after, during, containing) or `(QUERY) on F = G`",
                word.span.start,
            ));
        };
        let (query, inner) = self.query_in(scope)?;
        let (within, at, order_scope) = self.order_options(relation, scope, &inner, start)?;
        self.expose_inner(scope, &inner, within.as_ref());
        Ok(TransformKind::Join {
            query: Box::new(query),
            on: JoinOn::Order {
                relation,
                within,
                at,
                scope: order_scope,
            },
        })
    }
    /// One row per step of each match: `parent`, `match`, `position`, `step`,
    /// and the step row's fields as `step.F`.
    fn matches(&mut self, scope: &mut Scope, start: usize) -> Result<(), Error> {
        let Some(patterns) = scope.patterns.clone() else {
            // Unknown saved outputs are syntax until the caller supplies their
            // immutable schema. Binding reparses and validates the actual steps.
            if scope.columns_unknown {
                *scope = Scope::new("$matches".into());
                scope.columns_unknown = true;
                scope.patterns = None;
                return Ok(());
            }
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "matches expands an exists or seq written in this query; a saved or kept source hides its own",
                start,
            ));
        };
        let unit = match patterns.as_slice() {
            [unit] => unit.clone(),
            [] => {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "matches needs one top-level exists or seq",
                    start,
                ));
            }
            _ => {
                return Err(Error::new(
                    ErrorKind::Ambiguous,
                    format!(
                        "matches needs exactly one top-level exists or seq; this relation has {}",
                        patterns.len()
                    ),
                    start,
                ));
            }
        };
        let parent = scope.unit.clone();
        let parent_fields = self.visible(scope);
        let step_fields: Vec<Field> = self
            .catalog
            .unit(&unit)
            .map(|u| u.fields.clone())
            .unwrap_or_default();
        *scope = Scope::new("$matches".into());
        scope.columns = Some(BTreeSet::new());
        for (name, kind, unit) in [
            ("parent", FieldKind::Reference, Some(parent)),
            ("match", FieldKind::Integer, None),
            ("position", FieldKind::Integer, None),
            ("step", FieldKind::Reference, Some(unit.clone())),
        ] {
            self.add_generated(
                scope,
                Field {
                    name: name.into(),
                    kind,
                    unit,
                    ..Field::default()
                },
                Origin::Generated,
            );
        }
        for mut info in parent_fields {
            info.field.name = format!("parent.{}", info.field.name);
            self.add_generated(scope, info.field, info.origin);
        }
        for mut field in step_fields {
            let origin = Origin::Catalog {
                unit: unit.clone(),
                field: field.name.clone(),
            };
            field.name = format!("step.{}", field.name);
            self.add_generated(scope, field, origin);
        }
        Ok(())
    }
    /// `agg A, …`; `single` reads the one aggregate of `| count`.
    fn aggregates(&mut self, scope: &mut Scope, single: bool) -> Result<Vec<Aggregate>, Error> {
        let mut aggregates = Vec::new();
        let grouped = scope.group_input.is_some();
        let rows = scope
            .group_input
            .take()
            .map(|rows| (*rows).clone())
            .unwrap_or_else(|| scope.clone());
        if !grouped {
            scope.columns = Some(BTreeSet::new());
            scope.extra.clear();
            Arc::make_mut(&mut scope.typed_extra).clear();
            Arc::make_mut(&mut scope.origins).clear();
        }
        loop {
            let t = self.name()?;
            if let Some((name, field)) = t.text.split_once(':')
                && AggregateFunction::parse(name).is_some()
            {
                let mut error = Error::new(
                    ErrorKind::InvalidQuery,
                    "aggregates are written as functions: sum(cost)",
                    t.span.start,
                );
                if !field.is_empty() {
                    error = error.suggestion(self.replace(t.span, &format!("{name}({field})")));
                }
                return Err(error);
            }
            let function = AggregateFunction::parse(&t.text).ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidQuery,
                    format!("`{}` is not an aggregate", t.text),
                    t.span.start,
                )
                .candidates(
                    crate::catalog::AGGREGATES
                        .iter()
                        .map(|a| a.to_string())
                        .collect(),
                )
            })?;
            let mut arguments = Vec::new();
            if self.take("(") {
                if !self.is(")") {
                    loop {
                        arguments.push(self.expression(&rows, Place::Column)?);
                        if !self.take(",") {
                            break;
                        }
                    }
                }
                self.expect(")")?;
            }
            if !function.arguments().contains(&arguments.len()) {
                let range = function.arguments();
                return Err(Error::new(
                    ErrorKind::BadValue,
                    format!(
                        "`{}` takes {} argument{}",
                        function.name(),
                        range.end(),
                        if *range.end() == 1 { "" } else { "s" }
                    ),
                    t.span.start,
                ));
            }
            if function == AggregateFunction::Covered {
                let kinds: Vec<_> = arguments
                    .iter()
                    .map(|a| self.expression_info(&rows, a).field.kind)
                    .collect();
                let interval = |k: &FieldKind| {
                    matches!(
                        k,
                        FieldKind::Time
                            | FieldKind::Integer
                            | FieldKind::Number
                            | FieldKind::Duration
                            | FieldKind::Json
                    )
                };
                if kinds[0] != kinds[1] || !interval(&kinds[0]) {
                    return Err(Error::new(
                        ErrorKind::InvalidQuery,
                        "covered takes a start and an end of one time or number kind",
                        t.span.start,
                    ));
                }
            }
            let name = if self.take("as") {
                Some(self.name()?.text)
            } else {
                None
            };
            let aggregate = Aggregate {
                function,
                arguments,
                name,
            };
            let inputs = aggregate
                .arguments
                .iter()
                .map(|argument| self.expression_info(&rows, argument))
                .collect();
            let info = self.aggregate_info(
                &aggregate,
                inputs,
                self.transform_path()
                    .field("aggregates")
                    .index(aggregates.len()),
            );
            self.add_generated(scope, info.field, info.origin);
            aggregates.push(aggregate);
            if single || !self.take(",") {
                break;
            }
        }
        scope.patterns = Some(Vec::new());
        Ok(aggregates)
    }
    fn extension_arguments(
        &mut self,
        scope: &Scope,
        slots: &[ExtensionSlot],
    ) -> Result<Vec<ExtensionArgument>, Error> {
        let mut arguments = Vec::new();
        for slot in slots {
            arguments.push(self.extension_argument(scope, slot)?);
        }
        Ok(arguments)
    }
    fn extension_argument(
        &mut self,
        scope: &Scope,
        slot: &ExtensionSlot,
    ) -> Result<ExtensionArgument, Error> {
        Ok(match slot {
            ExtensionSlot::Keyword(word) => {
                self.expect(word)?;
                ExtensionArgument::Keyword(word.clone())
            }
            ExtensionSlot::Value => ExtensionArgument::Value(self.value()?),
            ExtensionSlot::Name => ExtensionArgument::Name(self.name()?.text),
            ExtensionSlot::Optional(slots) => {
                // A failed optional part leaves no trace: position, captured
                // scopes and open queries return to the checkpoint.
                let checkpoint = (
                    self.at,
                    self.captures.len(),
                    self.query_stack.len(),
                    self.inner_scopes.clone(),
                );
                match self.extension_arguments(scope, slots) {
                    Ok(arguments) => ExtensionArgument::Optional(arguments),
                    Err(_) => {
                        self.at = checkpoint.0;
                        self.captures.truncate(checkpoint.1);
                        self.query_stack.truncate(checkpoint.2);
                        self.inner_scopes = checkpoint.3;
                        ExtensionArgument::Optional(Vec::new())
                    }
                }
            }
            ExtensionSlot::Field => {
                let t = self.name()?;
                self.check_field(scope, &t.text, t.span.start)?;
                ExtensionArgument::Field(t.text)
            }
            ExtensionSlot::Unit => ExtensionArgument::Unit(self.unit_name()?),
            ExtensionSlot::Condition => {
                self.expect("(")?;
                let condition = self.condition(scope)?;
                self.expect(")")?;
                ExtensionArgument::Condition(Box::new(condition))
            }
            ExtensionSlot::Query => {
                let (query, _) = self.query_in(scope)?;
                ExtensionArgument::Query(Box::new(query))
            }
            ExtensionSlot::Expression => {
                ExtensionArgument::Expression(self.expression(scope, Place::Column)?)
            }
            ExtensionSlot::List(slot) => {
                let mut items = vec![self.extension_argument(scope, slot)?];
                while self.take(",") {
                    items.push(self.extension_argument(scope, slot)?);
                }
                ExtensionArgument::List(items)
            }
        })
    }
}
/// Visit the operands of a condition's top-level AND.
fn top_level<'a>(condition: &'a Condition, visit: &mut impl FnMut(&'a Condition)) {
    match &condition.kind {
        ConditionKind::And { conditions } => {
            for c in conditions {
                top_level(c, visit);
            }
        }
        _ => visit(condition),
    }
}
/// The step units of a condition's top-level `exists` and `seq` patterns.
fn patterns(condition: Option<&Condition>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(condition) = condition {
        top_level(condition, &mut |c| match &c.kind {
            ConditionKind::Exists { unit, .. } | ConditionKind::Sequence { unit, .. } => {
                out.push(unit.clone())
            }
            _ => {}
        });
    }
    out
}
/// A ranking atom orders the whole relation, so it stands only in a top-level
/// AND of a query's conditions; under OR or NOT, or inside `exists`, `count`
/// or `seq`, it is refused.
fn check_ranking(query: &Query, text: &str) -> Result<(), Error> {
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
    fn condition(c: &Condition, place: Place<'_>, text: &str) -> Result<(), Error> {
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
            ConditionKind::Not { condition: inner } => condition(inner, nested("under NOT"), text)?,
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
    }
    fn arguments_check(arguments: &[ExtensionArgument], text: &str) -> Result<(), Error> {
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
fn join_conditions(mut conditions: Vec<Condition>, and: bool) -> Condition {
    if conditions.len() == 1 {
        return conditions.pop().unwrap();
    }
    let span = Span {
        start: conditions[0].span.start,
        end: conditions.last().unwrap().span.end,
    };
    Condition {
        span,
        kind: if and {
            ConditionKind::And { conditions }
        } else {
            ConditionKind::Or { conditions }
        },
    }
}
fn value_token(text: &str, quoted: bool) -> Value {
    if !quoted {
        match text {
            "*" => return Value::Any,
            "unknown" => return Value::Unknown,
            "known" => return Value::Known,
            _ => {}
        }
        if let Some(name) = text.strip_prefix('$') {
            return Value::Parameter { name: name.into() };
        }
    }
    Value::Scalar {
        text: text.into(),
        quoted,
    }
}
fn value_surface(value: &Value) -> String {
    match value {
        Value::Scalar { text, quoted: true } => {
            format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
        }
        Value::Scalar { text, .. } => text.clone(),
        Value::Unknown => "unknown".into(),
        Value::Known => "known".into(),
        Value::Any => "*".into(),
        Value::Parameter { name } => format!("${name}"),
        Value::Set { values } => format!(
            "({})",
            values
                .iter()
                .map(value_surface)
                .collect::<Vec<_>>()
                .join("|")
        ),
    }
}
fn is_symbol(text: &str) -> bool {
    matches!(
        text,
        "(" | ")" | "[" | "]" | "|" | "," | "->" | "=" | "!=" | ">" | ">=" | "<" | "<="
    )
}
fn is_clock(text: &str) -> bool {
    text.len() >= 5
        && text.as_bytes().get(2) == Some(&b':')
        && text[..2].chars().all(|c| c.is_ascii_digit())
}
/// A literal a column or aggregate argument may hold unquoted: a number, a
/// duration or an amount of money.
pub(crate) fn is_literal(text: &str) -> bool {
    crate::number::is_decimal(text) || is_duration(text) || is_money(text)
}
pub(crate) fn is_money(text: &str) -> bool {
    let digits = text
        .strip_suffix("usd")
        .or_else(|| text.strip_prefix('$'))
        .unwrap_or("");
    !digits.is_empty() && crate::number::is_decimal(digits)
}
fn kind_name(kind: &FieldKind) -> String {
    match kind {
        FieldKind::List(inner) => format!("a list of {}", kind_name(inner)),
        FieldKind::Integer | FieldKind::Number => "a number".into(),
        FieldKind::Money => "money".into(),
        FieldKind::Time => "a time".into(),
        FieldKind::Duration => "a duration".into(),
        FieldKind::Bool => "a boolean".into(),
        FieldKind::Reference => "a reference".into(),
        FieldKind::Json => "a document".into(),
        _ => "text".into(),
    }
}
/// Known names within two typing mistakes of an unknown one, nearest first.
/// Transposition counts as one mistake; words under four letters never match.
pub(crate) fn closest<'a>(word: &str, candidates: impl Iterator<Item = &'a str>) -> Vec<String> {
    let length = word.chars().count();
    let bound = if length < 4 {
        return Vec::new();
    } else if length <= 6 {
        1
    } else {
        2
    };
    let mut found: Vec<(usize, &str)> = candidates
        .filter_map(|candidate| {
            let distance = distance(&word.to_lowercase(), &candidate.to_lowercase());
            (distance <= bound).then_some((distance, candidate))
        })
        .collect();
    found.sort();
    found.dedup();
    found
        .into_iter()
        .map(|(_, word)| word.to_string())
        .collect()
}
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut before = previous.clone();
    for (i, ac) in a.iter().enumerate() {
        let mut row = vec![i + 1; b.len() + 1];
        for (j, bc) in b.iter().enumerate() {
            row[j + 1] = (previous[j + 1] + 1)
                .min(row[j] + 1)
                .min(previous[j] + usize::from(ac != bc));
            if i > 0 && j > 0 && *ac == b[j - 1] && a[i - 1] == *bc {
                row[j + 1] = row[j + 1].min(before[j - 1] + 1);
            }
        }
        before = previous;
        previous = row;
    }
    previous[b.len()]
}

pub(crate) fn is_duration(text: &str) -> bool {
    let at = text.find(|c: char| !c.is_ascii_digit() && c != '.');
    at.is_some_and(|at| {
        at > 0
            && matches!(&text[at..], "ms" | "s" | "m" | "h" | "d" | "w")
            && text[..at].parse::<f64>().is_ok()
    })
}
#[cfg(test)]
mod tests;
