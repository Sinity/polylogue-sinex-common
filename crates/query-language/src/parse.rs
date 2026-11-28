//! The one grammar: catalog-driven parsing and refusals that carry a
//! corrected query. The type rules are the checker's (`crate::check`); the
//! parser applies them to each node as it builds it.
use crate::ast::*;
use crate::catalog::{Catalog, ExtensionPosition, ExtensionSlot, FieldKind};
use crate::check::{Checker, Place, Scope};
use crate::{Error, ErrorKind};
use std::collections::BTreeSet;

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
        check: Checker::for_text(catalog, text, capture_types),
    };
    let (query, _) = p.relation(None)?;
    if p.peek().is_some() {
        return Err(p.error("expected `|` before a transform, or the end of the query"));
    }
    crate::check::ranking::check_ranking(&query, Some(text))?;
    let typing = if capture_types {
        crate::check::typing::project(&p.check, &query)
    } else {
        crate::scope::Typing::default()
    };
    Ok(Parsed { query, typing })
}
struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    text: &'a str,
    catalog: &'a Catalog,
    /// The type rules, applied to each node as it is built.
    check: Checker<'a>,
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
        self.check.unknown_unit(&t.text, t.span)
    }
    fn source_unit_correction(&self, token: &Token) -> Option<Error> {
        // Functions, references and grammar punctuation already own these
        // positions. Only an unquoted source word can be a misspelled unit.
        if is_symbol(&token.text)
            || token.text.contains(':')
            || self.next_is(1, "(")
            || self.catalog.is_reference(&token.text)
        {
            return None;
        }
        let mut error = self.unknown_unit(token);
        let nearest = error.candidates.first()?;
        let unit = self.catalog.unit(nearest)?;
        let has_condition = self.tokens.get(self.at + 1).is_some_and(|next| {
            next.quoted || !matches!(next.text.as_str(), "where" | "with" | "|" | ")")
        });
        let replacement = if has_condition {
            format!("{} where", unit.name)
        } else {
            unit.name.clone()
        };
        error.kind = ErrorKind::NotAQuery;
        error.suggestion = Some(self.replace(token.span, &replacement));
        Some(error)
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
        crate::depth::deeper(|| {
            let capture = self.check.captures.len();
            if self.check.capture_types {
                self.check.captures.push(None);
            }
            self.check.query_stack.push((capture, 0));
            let mut scope = self.check.relation_scope(inherited);
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
                    self.check.output_scope(&mut source, &mut scope);
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
                } else if self.check.field_known(&scope, &first.text)
                    && !matches!(lower.as_str(), "since" | "until")
                {
                    return Err(self.bare_field(&first));
                } else if let Some(error) = self.source_unit_correction(&first) {
                    return Err(error);
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
                    condition: Some(mut condition),
                    unit,
                } => {
                    let named = match &mut condition.kind {
                        ConditionKind::Saved { name, arguments } => Some(Source::Saved {
                            name: std::mem::take(name),
                            arguments: std::mem::take(arguments),
                            condition: None,
                            output: SourceColumns::Unknown,
                        }),
                        ConditionKind::Reference { reference }
                            if reference.starts_with("result:") =>
                        {
                            Some(Source::Result {
                                reference: std::mem::take(reference),
                                part: ResultPart::Rows,
                                condition: None,
                                output: SourceColumns::Unknown,
                            })
                        }
                        _ => None,
                    };
                    named.unwrap_or(Source::Unit {
                        implicit: true,
                        condition: Some(condition),
                        unit,
                    })
                }
                other => other,
            };
            if matches!(source, Source::Saved { .. } | Source::Result { .. }) {
                self.check.output_scope(&mut source, &mut scope);
            }
            if matches!(source, Source::Saved { .. } | Source::Result { .. })
                && !self.query_end()
                && !self.is("|")
                && !self.is("with")
            {
                self.take("where");
                let inline = Some(self.condition(&scope)?);
                if let Source::Saved { condition, .. } | Source::Result { condition, .. } =
                    &mut source
                {
                    *condition = inline;
                }
            }
            let mut attach = Vec::new();
            if self.take("with") {
                attach = self.attachments()?;
            }
            scope.patterns = self.check.source_patterns(&source);
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
                } => self.check.expose_order_match(&mut scope, condition),
                _ => {}
            }
            let source_scope = self.check.capture_types.then(|| scope.clone());
            let mut transform_scopes = Vec::new();
            let mut transforms = Vec::new();
            while !self.query_end() {
                if !self.take("|") {
                    return Err(self.error("expected `|` before a transform"));
                }
                self.check.query_stack.last_mut().unwrap().1 = transforms.len();
                let before = self.check.capture_types.then(|| scope.clone());
                transforms.push(self.transform(&mut scope)?);
                if let Some(before) = before {
                    transform_scopes.push((before, scope.clone()));
                }
            }
            if let Some(source) = source_scope {
                self.check.captures[capture] = Some(crate::check::typing::Capture {
                    source,
                    transforms: transform_scopes,
                });
            }
            self.check.query_stack.pop();
            Ok((
                Query {
                    source,
                    attach,
                    transforms,
                },
                scope,
            ))
        })
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
                            && self.check.field_known(scope, &current.text)))
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
        crate::depth::deeper(|| {
            let start = self.pos();
            if self.peek().is_some_and(|t| !t.quoted && t.text == "not")
                && self
                    .tokens
                    .get(self.at + 1)
                    .is_some_and(|t| self.check.field_known(scope, &t.text) || t.text.contains(':'))
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
            let function_call =
                self.next_is(1, "(") && self.catalog.function(&token.text)?.is_some();
            if self.field_syntax(scope) || function_call {
                return self.comparison(scope, start);
            }
            self.at += 1;
            // `since 7d` is the spelling `since:7d` written apart: refuse it with
            // the joined form, so the word does not silently become a search term.
            if token.text.eq_ignore_ascii_case("since") || token.text.eq_ignore_ascii_case("until")
            {
                let resume = self.at;
                if let Some(next) = self.peek().cloned()
                    && (next.quoted || !is_symbol(&next.text))
                    && (next.text.starts_with('$') || crate::time::is_time_expression(&next.text))
                    && self.check.main_time(scope).is_some()
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
            if self.check.reads_as_reference(scope, &token.text) {
                return Ok(self.cond(
                    start,
                    ConditionKind::Reference {
                        reference: token.text,
                    },
                ));
            }
            if self.check.field_known(scope, &token.text) {
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
        })
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
                next.is_some_and(|t| t.text == "by" || self.check.field_known(scope, &t.text))
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
        if self.check.reads_as_reference(scope, reference) {
            let reference = reference.to_owned();
            if quoted_reference.is_some() {
                self.at += 1;
            }
            return Ok(self.cond(start, ConditionKind::Reference { reference }));
        }
        let list = RankList::ALL
            .into_iter()
            .find(|list| field.eq_ignore_ascii_case(list.name()));
        let until = field.eq_ignore_ascii_case("until");
        let time_word = until || field.eq_ignore_ascii_case("since");
        let keyword = list.is_some() || time_word || field.eq_ignore_ascii_case("in");
        if !keyword {
            self.check.check_field(scope, field, start)?;
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
        if time_word
            && (self.check.main_time(scope).is_some() || !self.check.field_known(scope, field))
        {
            let kind = self.check.time_bound(scope, until, value, self.pos())?;
            return Ok(self.cond(start, kind));
        }
        self.check.check_match(scope, field, &mut value, start)?;
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
            self.check
                .check_between(scope, &left, &mut from, &mut to, start)?;
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
            self.check
                .check_compare_expressions(scope, &left, &right, start)?;
            right
        } else {
            let mut value = self.value()?;
            let span = Span {
                start,
                end: self.end(),
            };
            self.check
                .check_compare_value(scope, &left, op, &mut value, span)?;
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
        self.check
            .check_arithmetic(scope, operator, &left, &right, at)?;
        Ok(Expression::Arithmetic {
            operator,
            left: Box::new(left),
            right: Box::new(right),
        })
    }
    fn primary(&mut self, scope: &Scope, place: Place) -> Result<Expression, Error> {
        crate::depth::deeper(|| {
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
                self.check
                    .check_function(scope, &function, &mut arguments, t.span.start)?;
                return Ok(Expression::Function {
                    name: function.name,
                    arguments,
                });
            }
            if self.check.reads_as_value(scope, &t.text, place) {
                self.at += 1;
                return Ok(Expression::value(value_token(&t.text, false)));
            }
            self.at += 1;
            self.check.check_field(scope, &t.text, t.span.start)?;
            Ok(Expression::field(t.text))
        })
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
    fn value(&mut self) -> Result<Value, Error> {
        crate::depth::deeper(|| {
            if self.is("(") {
                let open = self.next()?;
                // A set inside a set adds its values: `(a|(b|c))` is `(a|b|c)`,
                // so a parsed set never holds another set.
                let mut values = Vec::new();
                loop {
                    match self.value()? {
                        Value::Set { values: inner } => values.extend(inner),
                        value => values.push(value),
                    }
                    if !self.take("|") {
                        break;
                    }
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
                    && self.peek().is_some_and(|t| {
                        matches!(t.text.as_str(), "days" | "day" | "weeks" | "hours")
                    })
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
        })
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
        let unit = self.check.sequence_unit(scope);
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
                        self.check.require_order_scope(&unit, start)?;
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
    fn order_condition(&mut self, scope: &Scope) -> Result<Condition, Error> {
        let start = self.pos();
        let word = self.next()?;
        let relation = OrderRelation::parse(&word.text).expect("checked by the caller");
        let (query, inner) = self.query_in(scope)?;
        let (within, at, order_scope) = self.order_options(relation, scope, &inner, start)?;
        let pick = if self.take("pick") {
            self.expect("first")?;
            self.expect("by")?;
            let candidate = self.check.match_scope(&inner, within.as_ref());
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
            MatchChoice::FirstBy { keys }
        } else {
            MatchChoice::Nearest
        };
        self.check.inner_scopes.insert(start, inner);
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
        self.check.check_order_options(
            relation,
            outer,
            inner,
            within.as_ref(),
            at,
            order_scope.as_deref(),
            start,
        )?;
        Ok((within, at, order_scope))
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
                    self.check.check_field(&scope, &f.text, f.span.start)?;
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
                self.check.where_scope(scope, &condition);
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
                    let expression =
                        if self.is("random") && !self.check.field_known(scope, "random") {
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
                        } else if self.is("rank") && !self.check.field_known(scope, "rank") {
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
                self.check.group_scope(scope, &keys);
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
                if self.query_end() && self.check.field_known(scope, &t.text) {
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
                self.check.first_scope(scope);
                TransformKind::First { count }
            }
            "distinct" => TransformKind::Distinct,
            "matches" => {
                self.check.matches_scope(scope, start)?;
                TransformKind::Matches
            }
            "fill" => {
                self.check.fill_scope(scope, self.pos())?;
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
                self.check.fields_scope(scope, &columns, start)?;
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
                self.check.check_combination(scope, &inner, how, start)?;
                TransformKind::Combine {
                    how,
                    query: Box::new(query),
                }
            }
            "join" => self.join(scope, start)?,
            "diff" => {
                let (query, inner) = self.query_in(scope)?;
                self.check.diff_scope(scope, &inner, start)?;
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
                    self.check.unit_condition_scope(scope, condition.as_ref());
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
                    return Err(self.check.near_miss(
                        &t.text,
                        t.span,
                        format!("`{}` is not a transform or a unit", t.text),
                        ErrorKind::InvalidQuery,
                        names,
                    ));
                }
            }
        };
        self.check.finish_transform(scope, &kind);
        Ok(Transform {
            span: Span {
                start,
                end: self.end(),
            },
            kind,
        })
    }
    fn join(&mut self, scope: &mut Scope, start: usize) -> Result<TransformKind, Error> {
        if self.is("(") {
            let (query, inner) = self.query_in(scope)?;
            self.expect("on")?;
            let left = self.name()?;
            self.check.check_field(scope, &left.text, left.span.start)?;
            self.expect("=")?;
            let right = self.name()?;
            self.check
                .check_field(&inner, &right.text, right.span.start)?;
            self.check.expose_inner(scope, &inner, None);
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
        self.check.expose_inner(scope, &inner, within.as_ref());
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
    /// `agg A, …`; `single` reads the one aggregate of `| count`.
    fn aggregates(&mut self, scope: &mut Scope, single: bool) -> Result<Vec<Aggregate>, Error> {
        let mut aggregates = Vec::new();
        let rows = self.check.begin_aggregates(scope);
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
            self.check
                .check_aggregate_arguments(&rows, &function, &arguments, t.span.start)?;
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
            self.check
                .add_aggregate(scope, &rows, &aggregate, aggregates.len());
            aggregates.push(aggregate);
            if single || !self.take(",") {
                break;
            }
        }
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
                    self.check.captures.len(),
                    self.check.query_stack.len(),
                    self.check.inner_scopes.clone(),
                );
                match self.extension_arguments(scope, slots) {
                    Ok(arguments) => ExtensionArgument::Optional(arguments),
                    Err(_) => {
                        self.at = checkpoint.0;
                        self.check.captures.truncate(checkpoint.1);
                        self.check.query_stack.truncate(checkpoint.2);
                        self.check.inner_scopes = checkpoint.3;
                        ExtensionArgument::Optional(Vec::new())
                    }
                }
            }
            ExtensionSlot::Field => {
                let t = self.name()?;
                self.check.check_field(scope, &t.text, t.span.start)?;
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
    crate::depth::deeper(|| match &condition.kind {
        ConditionKind::And { conditions } => {
            for c in conditions {
                top_level(c, visit);
            }
        }
        _ => visit(condition),
    })
}
/// The step units of a condition's top-level `exists` and `seq` patterns.
pub(crate) fn patterns(condition: Option<&Condition>) -> Vec<String> {
    crate::depth::deeper(|| {
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
    })
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
pub(crate) fn value_token(text: &str, quoted: bool) -> Value {
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
pub(crate) fn kind_name(kind: &FieldKind) -> String {
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
