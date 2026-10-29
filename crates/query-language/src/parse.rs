//! Catalog-driven parsing for piped and clause-oriented surfaces.
use crate::ast::*;
use crate::catalog::{Catalog, ExtensionPosition, ExtensionSlot, Field, FieldKind, OutputSchema};
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
        let literal_end = text[i..]
            .find(|c: char| c.is_whitespace() || matches!(c, '|' | ')' | ']' | ','))
            .map_or(text.len(), |offset| i + offset);
        if !text[i..literal_end].contains('"')
            && (catalog.is_reference(&text[i..literal_end])
                || text[i..].starts_with("http://")
                || text[i..].starts_with("https://"))
        {
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
            // Corrections are complete executable questions, including when more than
            // one spelling error needs the same deterministic correction.
            let mut candidate = error.suggestion.take();
            let mut seen = BTreeSet::new();
            while let Some(corrected) = candidate {
                if !seen.insert(corrected.clone()) {
                    break;
                }
                match parse_once(&corrected, catalog) {
                    Ok(_) => {
                        error.suggestion = Some(corrected);
                        break;
                    }
                    Err(next)
                        if next.kind == ErrorKind::NotAQuery
                            && next.message.contains("is a command") =>
                    {
                        error.suggestion = Some(corrected);
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
    let mut p = Parser {
        tokens: tokens(text, catalog)?,
        at: 0,
        text,
        catalog,
    };
    let query = p.query()?;
    if p.peek().is_some() {
        return Err(p.error("expected a stage or the end of the query"));
    }
    Ok(query)
}
#[derive(Clone)]
struct Scope {
    unit: String,
    columns: Option<BTreeSet<String>>,
    extra: BTreeSet<String>,
    bucketed: bool,
    output: Option<OutputSchema>,
    deferred: bool,
    typed_extra: Vec<Field>,
}
impl Scope {
    fn new(unit: String) -> Self {
        Self {
            unit,
            columns: None,
            extra: BTreeSet::new(),
            bucketed: false,
            output: None,
            deferred: false,
            typed_extra: Vec::new(),
        }
    }
}
struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    text: &'a str,
    catalog: &'a Catalog,
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
            Err(self.error(format!("expected `{word}`")))
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
        let e = Error::new(
            ErrorKind::InvalidQuery,
            format!("`{}` is not a unit", t.text),
            t.span.start,
        );
        if let Some(fix) = closest(&t.text, self.catalog.units.iter().map(|u| u.name.as_str())) {
            e.suggestion(self.replace(t.span, &fix))
        } else {
            e
        }
    }
    fn replace(&self, span: Span, value: &str) -> String {
        format!(
            "{}{}{}",
            &self.text[..span.start],
            value,
            &self.text[span.end..]
        )
    }
    fn query(&mut self) -> Result<Query, Error> {
        self.query_in(None)
    }
    fn query_in(&mut self, inherited: Option<&Scope>) -> Result<Query, Error> {
        let mut scope = if let Some(parent) = inherited {
            // An omitted nested source inherits the row domain, rather than
            // the enclosing grouping/projection's visible output columns.
            let mut rows = Scope::new(parent.unit.clone());
            rows.deferred = self.catalog.unit(&parent.unit).is_none();
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
        if let Some(first) = self.peek().cloned()
            && !first.quoted
            && let Some((prefix, raw)) = first.text.split_once(':')
            && (prefix.eq_ignore_ascii_case("query") || prefix.eq_ignore_ascii_case("result"))
        {
            self.at += 1;
            source = if prefix.eq_ignore_ascii_case("query") {
                let name = self.saved_name(raw, first.span.start)?;
                Source::Saved {
                    name,
                    arguments: self.arguments()?,
                    condition: None,
                    output: OutputBinding::Deferred,
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
                    output: OutputBinding::Deferred,
                }
            };
            self.output_scope(&mut source, &mut scope);
            consumed_source = true;
        }
        if !consumed_source
            && let Some(first) = self.peek().cloned()
            && !first.quoted
            && !self.field_syntax()
        {
            let lower = first.text.to_ascii_lowercase();
            if matches!(lower.as_str(), "trace" | "descendants" | "parents")
                && self.catalog.commands.iter().any(|w| w == &lower)
            {
                self.at += 1;
                consumed_source = true;
                let reference = self.next()?.text;
                source = Source::Walk {
                    direction: match lower.as_str() {
                        "trace" => WalkDirection::Trace,
                        "parents" => WalkDirection::Parents,
                        _ => WalkDirection::Descendants,
                    },
                    reference,
                };
            } else if lower == "recall" && self.catalog.commands.iter().any(|w| w == &lower) {
                self.at += 1;
                consumed_source = true;
                source = if self.take("last") {
                    scope.unit = self
                        .catalog
                        .recall_last_unit
                        .clone()
                        .unwrap_or_else(|| scope.unit.clone());
                    let condition = if self.take("where") {
                        Some(self.condition(&scope)?)
                    } else {
                        None
                    };
                    Source::RecallLast {
                        unit: scope.unit.clone(),
                        condition,
                    }
                } else if self.take("mark") {
                    let number = if self.peek().is_some_and(|t| t.text.parse::<u64>().is_ok()) {
                        self.number()?
                    } else {
                        1
                    };
                    if number == 0 {
                        return Err(self.error("mark number must be positive"));
                    }
                    Source::RecallMark {
                        number,
                        window: self.recall_window()?,
                    }
                } else {
                    let time = if self.query_end() || self.stage_start() {
                        None
                    } else {
                        Some(self.value()?)
                    };
                    Source::Recall {
                        time,
                        window: self.recall_window()?,
                    }
                };
            } else if let Some(unit) = self.catalog.unit(&first.text) {
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
            } else if self
                .catalog
                .commands
                .iter()
                .any(|word| word.eq_ignore_ascii_case(&first.text))
            {
                return Err(Error::new(
                    ErrorKind::NotAQuery,
                    format!(
                        "`{}` is a command; quote it to search for the word",
                        first.text
                    ),
                    first.span.start,
                )
                .hint(format!("run the `{}` command", first.text)));
            } else if self.field_known(&scope, &first.text) {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!("`{}` is a field; give it an operator and value", first.text),
                    first.span.start,
                ));
            } else if let Some(word) = closest(&first.text, self.catalog.command_words()) {
                return Err(Error::new(
                    ErrorKind::NotAQuery,
                    format!(
                        "`{}` looks like `{word}`; quote it to search for the word",
                        first.text
                    ),
                    first.span.start,
                )
                .suggestion(self.replace(first.span, &word)));
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
            && !self.stage_start()
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
            && (!consumed_source || !self.stage_start())
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
                output: OutputBinding::Deferred,
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
                output: OutputBinding::Deferred,
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
            && !self.stage_start()
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
        let mut stages = Vec::new();
        while !self.query_end() {
            if self.take("|") {
            } else if !self.catalog.unpiped_stages {
                return Err(self.error("expected `|` before a stage"));
            }
            stages.push(self.stage(&mut scope)?);
        }
        Ok(Query {
            source,
            attach,
            stages,
        })
    }
    fn output_scope(&self, source: &mut Source, scope: &mut Scope) {
        scope.unit = match source {
            Source::Saved { name, .. } => format!("query:{name}"),
            Source::Result { reference, .. } => reference.clone(),
            _ => return,
        };
        scope.output = self.catalog.source_output(source).cloned();
        if let Some(unit) = scope
            .output
            .as_ref()
            .and_then(|output| output.unit.as_ref())
        {
            scope.unit = unit.clone();
        }
        scope.deferred = scope.output.is_none();
        scope.columns = scope
            .output
            .as_ref()
            .map(|output| output.fields.iter().map(|f| f.name.clone()).collect());
        if let Source::Saved { output, .. } | Source::Result { output, .. } = source {
            *output = if scope.deferred {
                OutputBinding::Deferred
            } else {
                OutputBinding::Known
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
                } else if scope.deferred {
                    None
                } else {
                    self.catalog.field(&scope.unit, name)
                }
            })
    }
    fn main_time(&self, scope: &Scope) -> Option<String> {
        if let Some(output) = &scope.output {
            output.main_time.clone()
        } else if scope.deferred {
            Some("$deferred_main_time".into())
        } else {
            self.catalog
                .unit(&scope.unit)
                .and_then(|unit| unit.main_time.clone())
        }
    }
    fn recall_window(&mut self) -> Result<Option<Value>, Error> {
        if let Some(t) = self.peek().cloned()
            && let Some(w) = t
                .text
                .strip_prefix('±')
                .or_else(|| t.text.strip_prefix("+-"))
        {
            self.at += 1;
            return Ok(Some(Value::scalar(w)));
        }
        Ok(None)
    }
    fn query_end(&self) -> bool {
        self.peek().is_none() || self.is(")")
    }
    fn field_syntax(&self) -> bool {
        self.peek()
            .is_some_and(|t| !t.quoted && t.text.contains(':'))
            || self.tokens.get(self.at + 1).is_some_and(|t| {
                !t.quoted
                    && (Op::parse(&t.text).is_some()
                        || matches!(t.text.as_str(), "between" | "in" | "exists" | "-"))
            })
    }
    fn stage_start(&self) -> bool {
        if !self.catalog.unpiped_stages || self.field_syntax() {
            return false;
        }
        let Some(t) = self.peek() else { return false };
        if t.quoted {
            return false;
        }
        if let Some(stage) = crate::catalog::STAGES
            .iter()
            .find(|stage| stage.name.eq_ignore_ascii_case(&t.text))
        {
            match stage.form {
                crate::catalog::StageForm::Plain => true,
                crate::catalog::StageForm::Count => !self.next_is(1, "("),
                crate::catalog::StageForm::Function => self.next_is(1, "("),
                crate::catalog::StageForm::Relation => {
                    self.next_is(1, "(") || (self.next_is(1, "all") && self.next_is(2, "("))
                }
            }
        } else {
            self.catalog.extensions.iter().any(|extension| {
                extension.position == ExtensionPosition::Stage
                    && extension.name.eq_ignore_ascii_case(&t.text)
            })
        }
    }
    fn condition_end(&self) -> bool {
        self.peek().is_none()
            || ["|", ")", "]", ",", "->"].iter().any(|s| self.is(s))
            || self.stage_start()
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
            if self
                .peek()
                .is_some_and(|t| !t.quoted && matches!(t.text.as_str(), "and" | "or"))
                && conditions
                    .iter()
                    .any(|c| !matches!(c.kind, ConditionKind::Text { .. }))
            {
                let t = self.peek().unwrap();
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    "boolean operators are AND, OR and NOT in capitals",
                    t.span.start,
                )
                .suggestion(self.replace(t.span, &t.text.to_uppercase())));
            }
            self.take_bool("AND");
            conditions.push(self.atom(scope)?);
        }
        Ok(join_conditions(conditions, true))
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
            .suggestion(self.replace(t.span, "NOT")));
        }
        if self.take_bool("NOT") {
            let c = self.atom(scope)?;
            return Ok(Condition {
                span: Span {
                    start,
                    end: c.span.end,
                },
                kind: ConditionKind::Not {
                    condition: Box::new(c),
                },
            });
        }
        if self.take("-") {
            let c = self.atom(scope)?;
            return Ok(Condition {
                span: Span {
                    start,
                    end: c.span.end,
                },
                kind: ConditionKind::Not {
                    condition: Box::new(c),
                },
            });
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
            return Ok(Condition {
                span: Span {
                    start,
                    end: c.span.end,
                },
                kind: ConditionKind::Not {
                    condition: Box::new(c),
                },
            });
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
            if is_reference(&token.text) || self.catalog.is_reference(&token.text) {
                return Ok(self.cond(
                    start,
                    ConditionKind::Reference {
                        reference: token.text,
                    },
                ));
            }
            if !["similar", "in", "since", "until"]
                .iter()
                .any(|name| field.eq_ignore_ascii_case(name))
            {
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
            if field.eq_ignore_ascii_case("similar") {
                return Ok(self.cond(start, ConditionKind::Similar { value }));
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
            return Ok(self.cond(
                start,
                ConditionKind::Match {
                    field: field.into(),
                    value,
                },
            ));
        }
        // Function operands and field syntax take precedence over text and keyword positions.
        self.at -= 1;
        let has_operand = self.field_syntax()
            || (self.next_is(1, "(")
                && builtin(&token.text, crate::catalog::KEY_FUNCTIONS, token.span.start)?
                    .is_some());
        if has_operand {
            let operand = self.operand(scope)?;
            if self.take("exists") {
                return Ok(self.cond(start, ConditionKind::FieldExists { operand }));
            }
            if self.take("between") {
                let mut from = self.value()?;
                self.expect("and")?;
                let mut to = self.value()?;
                self.normalize_operand_value(scope, &operand, &mut from);
                self.normalize_operand_value(scope, &operand, &mut to);
                self.check_operand_value(scope, &operand, &from, start)?;
                self.check_operand_value(scope, &operand, &to, start)?;
                return Ok(self.cond(start, ConditionKind::Between { operand, from, to }));
            }
            if self.take("in") {
                if self.take("(") {
                    let query = Box::new(self.query_in(Some(scope))?);
                    self.expect(")")?;
                    return Ok(self.cond(start, ConditionKind::In { operand, query }));
                }
                let value = self.value()?;
                return Ok(self.cond(start, ConditionKind::TimeRange { operand, value }));
            }
            if self.take("-") {
                let right = self.operand(scope)?;
                let op = self.op()?;
                let value = self.value()?;
                return Ok(self.cond(
                    start,
                    ConditionKind::TimeDifference {
                        left: operand,
                        right,
                        op,
                        value,
                    },
                ));
            }
            let op = self.op()?;
            let mut value = self.value()?;
            self.normalize_operand_value(scope, &operand, &mut value);
            if let Operand::Field { name } = &operand
                && self
                    .scope_field(scope, name)
                    .is_some_and(|f| f.kind == FieldKind::Time)
                && matches!(op, Op::Lt | Op::Le | Op::Gt | Op::Ge)
                && matches!(&value,Value::Scalar{text,quoted:false} if is_duration(text))
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
            self.check_operand_value(scope, &operand, &value, start)?;
            return Ok(self.cond(start, ConditionKind::Compare { operand, op, value }));
        }
        self.at += 1;
        if matches!(token.text.as_str(), "since" | "until") {
            let value = self.value()?;
            let kind = self.time_bound(scope, token.text == "until", value)?;
            return Ok(self.cond(start, kind));
        }
        if token.text == "overlaps" {
            let from = self.value()?;
            let to = if self.take("..") {
                Some(self.value()?)
            } else {
                None
            };
            return Ok(self.cond(start, ConditionKind::Overlaps { from, to }));
        }
        if token.text == "where" {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                "where starts a unit filter or a stage; use `| where` after conditions",
                start,
            ));
        }
        if is_reference(&token.text) || self.catalog.is_reference(&token.text) {
            return Ok(self.cond(
                start,
                ConditionKind::Reference {
                    reference: token.text,
                },
            ));
        }
        if self.field_known(scope, &token.text) {
            return Err(Error::new(
                ErrorKind::InvalidQuery,
                format!("`{}` is a field; give it an operator and value", token.text),
                start,
            ));
        }
        // Pasted calls remain text, including their parentheses, when no operand function is declared.
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
        Ok(self.cond(start, ConditionKind::Text { text, mode }))
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
        let operand = Operand::field(field);
        Ok(if until {
            ConditionKind::Until { operand, value }
        } else {
            ConditionKind::Since { operand, value }
        })
    }
    fn value(&mut self) -> Result<Value, Error> {
        if self.take("(") {
            let mut values = vec![self.value()?];
            while self.take("|") {
                values.push(self.value()?);
            }
            self.expect(")")?;
            return Ok(Value::Set { values });
        }
        if self.take("[") {
            let mut values = vec![self.value()?];
            while self.take(",") {
                values.push(self.value()?);
            }
            self.expect("]")?;
            return Ok(Value::Set { values });
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
        let scope = &Scope::new(unit.clone());
        let start = self.pos();
        self.expect("seq")?;
        let all = self.take("all");
        self.expect("(")?;
        let mut steps = vec![self.condition(scope)?];
        let mut gaps = Vec::new();
        while self.take("->") {
            let gap = if self.take("[") {
                let gap = if self.take("next") {
                    Gap::Next
                } else {
                    let t = self.next()?;
                    if let Some(d) = t.text.strip_prefix("within:") {
                        Gap::Within {
                            duration: if d.is_empty() {
                                self.value()?
                            } else {
                                Value::scalar(d)
                            },
                        }
                    } else {
                        return Err(Error::new(
                            ErrorKind::InvalidQuery,
                            "expected next or within:D",
                            t.span.start,
                        ));
                    }
                };
                self.expect("]")?;
                gap
            } else {
                Gap::Later
            };
            gaps.push(gap);
            steps.push(self.condition(scope)?);
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
    fn field_known(&self, scope: &Scope, field: &str) -> bool {
        if (scope.deferred && scope.columns.is_none()) || scope.extra.contains(field) {
            return true;
        }
        scope.columns.as_ref().map_or_else(
            || self.scope_field(scope, field).is_some() || scope.extra.contains(field),
            |c| c.contains(field),
        )
    }
    fn check_field(&self, scope: &Scope, field: &str, position: usize) -> Result<(), Error> {
        if self.field_known(scope, field) {
            return Ok(());
        }
        let names: Vec<&str> = if let Some(columns) = &scope.columns {
            columns.iter().map(String::as_str).collect()
        } else {
            self.catalog
                .unit(&scope.unit)
                .into_iter()
                .flat_map(|u| u.fields.iter().map(|f| f.name.as_str()))
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
        let e = Error::new(ErrorKind::UnknownField, message, position);
        Err(if let Some(fix) = closest(field, names.into_iter()) {
            e.suggestion(self.replace(
                Span {
                    start: position,
                    end: position + field.len(),
                },
                &fix,
            ))
        } else {
            e
        })
    }
    fn normalize_operand_value(&self, scope: &Scope, operand: &Operand, value: &mut Value) {
        if let Operand::Field { name } = operand {
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
            return Err(Error::new(ErrorKind::BadValue, message, position));
        }
        let valid = match spec.kind {
            FieldKind::Integer => text.parse::<i64>().is_ok(),
            FieldKind::Number => text.parse::<f64>().is_ok_and(f64::is_finite),
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
    fn check_operand_value(
        &self,
        scope: &Scope,
        operand: &Operand,
        value: &Value,
        position: usize,
    ) -> Result<(), Error> {
        if let Operand::Field { name } = operand {
            self.check_value(scope, name, value, position)
        } else {
            Ok(())
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
    fn operand(&mut self, scope: &Scope) -> Result<Operand, Error> {
        let t = self.name()?;
        if self.take("(") {
            let Some(name) = builtin(&t.text, crate::catalog::KEY_FUNCTIONS, t.span.start)? else {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!("unknown key function `{}`", t.text),
                    t.span.start,
                ));
            };
            let mut arguments = Vec::new();
            if !self.is(")") {
                loop {
                    arguments.push(self.operand(scope)?);
                    if !self.take(",") {
                        break;
                    }
                }
            }
            self.expect(")")?;
            return Ok(Operand::Function {
                name: name.into(),
                arguments,
            });
        }
        self.check_field(scope, &t.text, t.span.start)?;
        Ok(Operand::field(t.text))
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
    fn stage(&mut self, scope: &mut Scope) -> Result<Stage, Error> {
        let start = self.pos();
        let t = self.name()?;
        let word = t.text.to_ascii_lowercase();
        let kind = match word.as_str() {
            "where" => StageKind::Where {
                condition: self.condition(scope)?,
            },
            "since" | "until" => {
                self.at -= 1;
                StageKind::Where {
                    condition: self.condition(scope)?,
                }
            }
            "sort" => {
                self.take("by");
                let mut keys = Vec::new();
                loop {
                    let operand = if self.take("random") {
                        let mut arguments = Vec::new();
                        if self.take("(") {
                            let n = self.number()?;
                            arguments.push(Operand::field(n.to_string()));
                            self.expect(")")?;
                        }
                        Operand::Function {
                            name: "random".into(),
                            arguments,
                        }
                    } else if self.take("rank") {
                        Operand::field("rank")
                    } else {
                        self.operand(scope)?
                    };
                    let descending = self.take("desc");
                    if !descending {
                        self.take("asc");
                    }
                    keys.push(SortKey {
                        operand,
                        descending,
                    });
                    if !self.take(",") {
                        break;
                    }
                }
                StageKind::Sort { keys }
            }
            "group" => {
                self.expect("by")?;
                let mut keys = Vec::new();
                loop {
                    keys.push(self.operand(scope)?);
                    if !self.take(",") {
                        break;
                    }
                }
                scope.bucketed=keys.iter().any(|k|matches!(k,Operand::Function{name,..} if matches!(name.as_str(),"day"|"hour"|"week"|"month")));
                scope.columns = Some(
                    keys.iter()
                        .map(Operand::column)
                        .chain(std::iter::once("count".into()))
                        .collect(),
                );
                StageKind::Group { keys }
            }
            "agg" => {
                let aggregates = self.aggregates(scope)?;
                StageKind::Aggregate { aggregates }
            }
            "count" | "sum" | "avg" | "min" | "max" | "count_distinct" | "histogram" | "corr" => {
                self.at -= 1;
                let aggregates = self.aggregates(scope)?;
                StageKind::Aggregate { aggregates }
            }
            "limit" => StageKind::Limit {
                count: self.number()?,
            },
            "offset" => {
                if self.query_end() && self.field_known(scope, &t.text) {
                    return Err(Error::new(ErrorKind::InvalidQuery,"`offset` is a field and a stage; supply a stage count or quote it to search",t.span.start).suggestion(self.replace(t.span,"\"offset\"")));
                }
                StageKind::Offset {
                    count: self.number()?,
                }
            }
            "first" => StageKind::First {
                count: self.number()?,
            },
            "distinct" => StageKind::Distinct,
            "matches" => StageKind::Matches,
            "fill" => {
                if !scope.bucketed {
                    return Err(self.error("fill follows a time-bucketed group by"));
                }
                scope.columns.as_mut().unwrap().insert("coverage".into());
                StageKind::Fill
            }
            "fields" => {
                let mut fields = Vec::new();
                loop {
                    let f = self.name()?;
                    self.check_field(scope, &f.text, f.span.start)?;
                    fields.push(f.text);
                    if !self.take(",") {
                        break;
                    }
                }
                scope.columns = Some(fields.iter().cloned().collect());
                StageKind::Fields { fields }
            }
            "measure" => {
                let name = self.next()?.text;
                let mut by = Vec::new();
                if self.take("by") {
                    loop {
                        if self.peek().is_some_and(|t| {
                            matches!(t.text.as_str(), "day" | "week" | "month")
                                && !self.next_is(1, "(")
                                && !self.field_known(scope, &t.text)
                        }) {
                            let bucket = self.next()?;
                            let time = self.main_time(scope).ok_or_else(|| {
                                self.error("period measure keys need the unit's main time")
                            })?;
                            by.push(Operand::Function {
                                name: bucket.text,
                                arguments: vec![Operand::field(time)],
                            });
                        } else {
                            by.push(self.operand(scope)?);
                        }
                        if !self.take(",") {
                            break;
                        }
                    }
                }
                StageKind::Measure { name, by }
            }
            "union" | "intersect" | "except" | "fuse" => {
                self.expect("(")?;
                let query = Box::new(self.query_in(Some(scope))?);
                self.expect(")")?;
                StageKind::Combine {
                    how: match word.as_str() {
                        "union" => Combination::Union,
                        "intersect" => Combination::Intersect,
                        "except" => Combination::Except,
                        _ => Combination::Fuse,
                    },
                    query,
                }
            }
            "near" | "before" | "after" | "during" | "containing" => {
                let all = self.take("all");
                self.expect("(")?;
                let query = Box::new(self.query_in(Some(scope))?);
                self.expect(")")?;
                let at = if self.take("at") {
                    let point = self.next()?;
                    match point.text.as_str() {
                        "when" => TimePoint::When,
                        "start" => TimePoint::Start,
                        "end" => TimePoint::End,
                        _ => {
                            return Err(Error::new(
                                ErrorKind::BadValue,
                                "at takes when, start or end",
                                point.span.start,
                            ));
                        }
                    }
                } else {
                    TimePoint::When
                };
                let within = if matches!(word.as_str(), "near" | "before" | "after") {
                    self.expect("within")?;
                    Some(self.value()?)
                } else {
                    None
                };
                let number = scope
                    .extra
                    .iter()
                    .filter(|name| {
                        name.starts_with("near")
                            || name.starts_with("during")
                            || name.starts_with("containing")
                    })
                    .count()
                    + 1;
                let suffix = if number == 1 {
                    String::new()
                } else {
                    format!("_{number}")
                };
                let base = match word.as_str() {
                    "during" => "during",
                    "containing" => "containing",
                    _ => "near",
                };
                scope.extra.insert(format!("{base}{suffix}"));
                if base == "near" {
                    scope.extra.insert(format!("offset{suffix}"));
                }
                StageKind::Temporal {
                    relation: match word.as_str() {
                        "near" => TemporalRelation::Near,
                        "before" => TemporalRelation::Before,
                        "after" => TemporalRelation::After,
                        "during" => TemporalRelation::During,
                        _ => TemporalRelation::Containing,
                    },
                    query,
                    at,
                    within,
                    all,
                }
            }
            "as" => {
                self.expect("of")?;
                let sequence = self.take("seq");
                StageKind::AsOf {
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
                        e.position == ExtensionPosition::Stage
                            && e.name.eq_ignore_ascii_case(&t.text)
                    })
                    .cloned()
                {
                    let arguments = self.extension_arguments(scope, &extension.arguments)?;
                    StageKind::Extension {
                        name: extension.name,
                        arguments,
                    }
                } else {
                    let Some(unit) = self.catalog.unit(&t.text) else {
                        return Err(self.unknown_unit(&t));
                    };
                    *scope = Scope::new(unit.name.clone());
                    let condition = if self.take("where") {
                        Some(self.condition(scope)?)
                    } else {
                        None
                    };
                    StageKind::Scope {
                        unit: scope.unit.clone(),
                        condition,
                    }
                }
            }
        };
        Ok(Stage {
            span: Span {
                start,
                end: self.end(),
            },
            kind,
        })
    }
    fn aggregates(&mut self, scope: &mut Scope) -> Result<Vec<Aggregate>, Error> {
        let mut aggregates = Vec::new();
        loop {
            let t = self.name()?;
            let (name, field) = t
                .text
                .split_once(':')
                .map_or((t.text.as_str(), None), |(a, b)| (a, Some(b)));
            let function = match name.to_ascii_lowercase().as_str() {
                "count" => AggregateFunction::Count,
                "sum" => AggregateFunction::Sum,
                "avg" => AggregateFunction::Avg,
                "min" => AggregateFunction::Min,
                "max" => AggregateFunction::Max,
                "count_distinct" => AggregateFunction::CountDistinct,
                "histogram" => AggregateFunction::Histogram,
                "corr" => AggregateFunction::Correlation,
                n if n.starts_with('p') => {
                    let p = n[1..]
                        .parse::<u8>()
                        .ok()
                        .filter(|p| *p <= 100)
                        .ok_or_else(|| {
                            Error::new(
                                ErrorKind::BadValue,
                                "percentile must be p0 through p100",
                                t.span.start,
                            )
                        })?;
                    AggregateFunction::Percentile(p)
                }
                _ => {
                    return Err(Error::new(
                        ErrorKind::InvalidQuery,
                        "expected an aggregate function",
                        t.span.start,
                    ));
                }
            };
            let mut fields = Vec::new();
            let parenthesis = self.take("(");
            if let Some(field) = field {
                if field.is_empty() {
                    fields.push(self.name()?.text);
                } else {
                    fields.push(field.into());
                }
            } else if parenthesis {
                fields.push(self.name()?.text);
            } else if function != AggregateFunction::Count {
                return Err(self.error("aggregate needs a field"));
            }
            if function == AggregateFunction::Correlation {
                self.expect(",")?;
                fields.push(self.name()?.text);
            }
            if parenthesis {
                self.expect(")")?;
            }
            // Aggregates read the pre-group row fields; grouped columns belong to subsequent stages.
            for f in &fields {
                let mut rows = scope.clone();
                rows.columns = None;
                self.check_field(&rows, f, t.span.start)?;
            }
            let aggregate = Aggregate { function, fields };
            let kind = match aggregate.function {
                AggregateFunction::Count | AggregateFunction::CountDistinct => FieldKind::Integer,
                _ => aggregate
                    .fields
                    .first()
                    .and_then(|field| self.scope_field(scope, field))
                    .map(|field| field.kind.clone())
                    .unwrap_or(FieldKind::Number),
            };
            scope.typed_extra.push(Field {
                name: aggregate.column(),
                kind,
                ..Field::default()
            });
            if let Some(columns) = &mut scope.columns {
                columns.insert(aggregate.column());
            } else {
                scope.columns = Some(std::iter::once(aggregate.column()).collect());
            }
            aggregates.push(aggregate);
            if self.take(",") {
                continue;
            }
            if self.catalog.unpiped_stages && self.peek().is_some_and(|t| is_aggregate(&t.text)) {
                continue;
            }
            break;
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
            arguments.push(match slot {
                ExtensionSlot::Keyword(word) => {
                    self.expect(word)?;
                    ExtensionArgument::Keyword(word.clone())
                }
                ExtensionSlot::Value => ExtensionArgument::Value(self.value()?),
                ExtensionSlot::Name => ExtensionArgument::Name(self.name()?.text),
                ExtensionSlot::Optional(slots) => {
                    let checkpoint = self.at;
                    match self.extension_arguments(scope, slots) {
                        Ok(arguments) => ExtensionArgument::Optional(arguments),
                        Err(_) => {
                            self.at = checkpoint;
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
                    self.expect("(")?;
                    let query = self.query_in(Some(scope))?;
                    self.expect(")")?;
                    ExtensionArgument::Query(Box::new(query))
                }
            });
        }
        Ok(arguments)
    }
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
        if text == "*" {
            return Value::Any;
        }
        if text == "unknown" {
            return Value::Unknown;
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
fn builtin<'a>(
    name: &str,
    declarations: &'a [&str],
    position: usize,
) -> Result<Option<&'a str>, Error> {
    let mut matches = declarations
        .iter()
        .copied()
        .filter(|value| value.eq_ignore_ascii_case(name));
    let found = matches.next();
    if matches.next().is_some() {
        return Err(Error::new(
            ErrorKind::Ambiguous,
            format!("ambiguous builtin declaration `{name}`"),
            position,
        ));
    }
    Ok(found)
}
#[cfg(test)]
mod tests;

fn is_aggregate(text: &str) -> bool {
    let name = text.split(':').next().unwrap_or(text);
    crate::catalog::AGGREGATES.contains(&name)
        || name
            .strip_prefix('p')
            .is_some_and(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}
fn is_reference(text: &str) -> bool {
    text.starts_with("https://")
        || text.starts_with("http://")
        || text.starts_with("sinex:")
        || text.starts_with("polylogue:")
        || text.starts_with('@')
        || matches!(text, "self" | "last" | "latest")
        || (text.len() >= 12 && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
}
/// The shared command-word policy; transposition counts as one typing mistake.
pub(crate) fn closest<'a>(word: &str, candidates: impl Iterator<Item = &'a str>) -> Option<String> {
    let length = word.chars().count();
    let bound = if length < 4 {
        return None;
    } else if length <= 6 {
        1
    } else {
        2
    };
    candidates
        .filter_map(|candidate| {
            let distance = distance(&word.to_lowercase(), &candidate.to_lowercase());
            (distance <= bound).then_some((distance, candidate))
        })
        .min_by(|a, b| a.cmp(b))
        .map(|(_, word)| word.to_string())
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

fn is_duration(text: &str) -> bool {
    let at = text.find(|c: char| !c.is_ascii_digit() && c != '.');
    at.is_some_and(|at| {
        at > 0
            && matches!(&text[at..], "ms" | "s" | "m" | "h" | "d" | "w")
            && text[..at].parse::<f64>().is_ok()
    })
}
