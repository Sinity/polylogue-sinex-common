//! Catalog-driven parsing for piped and clause-oriented surfaces.
use crate::ast::*;
use crate::catalog::{Catalog, ExtensionPosition, ExtensionSlot, FieldKind};
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
        if catalog.is_reference(&text[i..literal_end])
            || text[i..].starts_with("http://")
            || text[i..].starts_with("https://")
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
    Ok(out)
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
}
impl Scope {
    fn new(unit: String) -> Self {
        Self {
            unit,
            columns: None,
            extra: BTreeSet::new(),
            bucketed: false,
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
        let mut scope = Scope::new(self.catalog.default_unit.clone());
        let mut source = Source::Unit {
            unit: scope.unit.clone(),
            condition: None,
        };
        let mut consumed_source = false;
        if let Some(first) = self.peek().cloned() {
            if !first.quoted && !self.field_syntax() {
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
                        let condition = if self.take("where") {
                            Some(self.condition(&scope)?)
                        } else {
                            None
                        };
                        Source::RecallLast { condition }
                    } else if self.take("mark") {
                        let number = if self.peek().is_some_and(|t| t.text.parse::<u64>().is_ok()) {
                            self.number()?
                        } else {
                            1
                        };
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
                    scope.unit = unit.name.clone();
                    let condition = if self.take("where") {
                        Some(self.condition(&scope)?)
                    } else {
                        None
                    };
                    source = Source::Unit {
                        unit: scope.unit.clone(),
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
        {
            if let Source::Unit { condition, .. } = &mut source {
                if condition.is_none() {
                    *condition = Some(self.condition(&scope)?);
                }
            }
        }
        source = match source {
            Source::Unit {
                condition:
                    Some(Condition {
                        kind: ConditionKind::Saved { name, arguments },
                        ..
                    }),
                ..
            } => Source::Saved { name, arguments },
            Source::Unit {
                condition:
                    Some(Condition {
                        kind: ConditionKind::Reference { reference },
                        ..
                    }),
                ..
            } if reference.starts_with("result:") => Source::Result { reference },
            other => other,
        };
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
    fn recall_window(&mut self) -> Result<Option<Value>, Error> {
        if let Some(t) = self.peek().cloned() {
            if let Some(w) = t
                .text
                .strip_prefix('±')
                .or_else(|| t.text.strip_prefix("+-"))
            {
                self.at += 1;
                return Ok(Some(Value::scalar(w)));
            }
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
        if self.is("not")
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
            if is_reference(&token.text) || self.catalog.is_reference(&token.text) {
                return Ok(self.cond(
                    start,
                    ConditionKind::Reference {
                        reference: token.text,
                    },
                ));
            }
            if field.eq_ignore_ascii_case("query") {
                let name = raw.to_string();
                let arguments = self.arguments()?;
                return Ok(self.cond(start, ConditionKind::Saved { name, arguments }));
            }
            if field.eq_ignore_ascii_case("result") {
                return Ok(self.cond(
                    start,
                    ConditionKind::Reference {
                        reference: token.text,
                    },
                ));
            }
            let value = if raw.is_empty() {
                if self.peek().is_some_and(|t| Op::parse(&t.text).is_some()) {
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
            self.check_field(scope, field, start)?;
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
        let has_operand = self.field_syntax() || (self.next_is(1, "(") && is_function(&token.text));
        if has_operand {
            let operand = self.operand(scope)?;
            if self.take("exists") {
                return Ok(self.cond(start, ConditionKind::FieldExists { operand }));
            }
            if self.take("between") {
                let from = self.value()?;
                self.expect("and")?;
                let to = self.value()?;
                self.check_operand_value(scope, &operand, &from, start)?;
                self.check_operand_value(scope, &operand, &to, start)?;
                return Ok(self.cond(start, ConditionKind::Between { operand, from, to }));
            }
            if self.take("in") {
                if self.take("(") {
                    let query = Box::new(self.query()?);
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
            let value = self.value()?;
            if let Operand::Field { name } = &operand {
                if self
                    .catalog
                    .field(&scope.unit, name)
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
            }
            self.check_operand_value(scope, &operand, &value, start)?;
            return Ok(self.cond(start, ConditionKind::Compare { operand, op, value }));
        }
        self.at += 1;
        if matches!(token.text.as_str(), "since" | "until") {
            let field = self
                .catalog
                .unit(&scope.unit)
                .and_then(|u| u.main_time.clone())
                .ok_or_else(|| self.error("this unit has no main time"))?;
            let value = self.value()?;
            return Ok(self.cond(
                start,
                ConditionKind::Compare {
                    operand: Operand::field(field),
                    op: if token.text == "since" {
                        Op::Ge
                    } else {
                        Op::Lt
                    },
                    value,
                },
            ));
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
        Op::parse(&t.text).ok_or_else(|| {
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
                if let Some(next) = self.peek() {
                    if !next.quoted && !is_symbol(&next.text) {
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
            .sequence_unit
            .clone()
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
        scope.columns.as_ref().map_or_else(
            || self.catalog.field(&scope.unit, field).is_some() || scope.extra.contains(field),
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
        let Some(spec) = self.catalog.field(&scope.unit, field) else {
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
    fn operand(&mut self, scope: &Scope) -> Result<Operand, Error> {
        let t = self.name()?;
        if self.take("(") {
            if !is_function(&t.text) {
                return Err(Error::new(
                    ErrorKind::InvalidQuery,
                    format!("unknown key function `{}`", t.text),
                    t.span.start,
                ));
            }
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
                name: t.text,
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
                scope.columns = Some(keys.iter().map(Operand::column).collect());
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
            "offset" => StageKind::Offset {
                count: self.number()?,
            },
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
                StageKind::Fields { fields }
            }
            "measure" => StageKind::Measure {
                name: self.next()?.text,
            },
            "union" | "intersect" | "except" | "fuse" => {
                self.expect("(")?;
                let query = Box::new(self.query()?);
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
                let query = Box::new(self.query()?);
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
                    scope.unit = unit.name.clone();
                    scope.columns = None;
                    scope.extra.clear();
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
                let rows = Scope::new(scope.unit.clone());
                self.check_field(&rows, f, t.span.start)?;
            }
            let aggregate = Aggregate { function, fields };
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
                    let query = self.query()?;
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
fn is_function(name: &str) -> bool {
    crate::catalog::KEY_FUNCTIONS.contains(&name)
}
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
