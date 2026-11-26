//! Binding: the checker's rules over a finished tree, in the order the parser
//! applies them to text. The tree is checked as given, with the parser's
//! elaborations that depend on the rows in scope applied again now that every
//! source's output is known: a word that is not a field reads as a value, a
//! word the catalog declares a reference reads as that reference, a value
//! takes its field's canonical spelling, a time bound names the scope's main
//! time and a sequence steps through the scope's sequence unit. A product
//! that substitutes values into a tree it parsed gets the nodes the parser
//! would have built from the substituted text.
use super::*;

impl Checker<'_> {
    /// Check `query` and record its scopes; returns the scope it ends in.
    pub fn query(&mut self, query: &mut Query, inherited: Option<&Scope>) -> Result<Scope, Error> {
        crate::depth::deeper(|| {
            let capture = self.captures.len();
            if self.capture_types {
                self.captures.push(None);
            }
            self.query_stack.push((capture, 0));
            let mut scope = self.relation_scope(inherited);
            match &mut query.source {
                Source::Unit { unit, implicit, .. } => {
                    if *implicit {
                        unit.clone_from(&scope.unit);
                    } else {
                        let name = self.unit(unit, 0)?;
                        scope = Scope::new(name.clone());
                        *unit = name;
                    }
                }
                source => self.output_scope(source, &mut scope),
            }
            if let Source::Unit {
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
            } = &mut query.source
            {
                self.condition(condition, &scope)?;
            }
            for attachment in &mut query.attach {
                attachment.unit = self.unit(&attachment.unit, attachment.span.start)?;
                let rows = Scope::new(attachment.unit.clone());
                for field in &attachment.fields {
                    self.check_field(&rows, field, attachment.span.start)?;
                }
                for window in &mut attachment.windows {
                    if let Window::Where(condition) = window {
                        self.condition(condition, &rows)?;
                    }
                }
            }
            scope.patterns = self.source_patterns(&query.source);
            if let Source::Unit {
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
            } = &query.source
            {
                self.expose_order_match(&mut scope, condition);
            }
            let source_scope = self.capture_types.then(|| scope.clone());
            let mut transform_scopes = Vec::new();
            for (index, transform) in query.transforms.iter_mut().enumerate() {
                self.query_stack.last_mut().unwrap().1 = index;
                let before = self.capture_types.then(|| scope.clone());
                self.transform(transform, &mut scope)?;
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
            Ok(scope)
        })
    }
    /// A unit the catalog declares, by its declared name.
    fn unit(&self, name: &str, position: usize) -> Result<String, Error> {
        self.catalog
            .unit(name)
            .map(|unit| unit.name.clone())
            .ok_or_else(|| {
                self.unknown_unit(
                    name,
                    Span {
                        start: position,
                        end: position + name.len(),
                    },
                )
            })
    }
    fn transform(&mut self, transform: &mut Transform, scope: &mut Scope) -> Result<(), Error> {
        let at = transform.span.start;
        match &mut transform.kind {
            TransformKind::Where { condition } => {
                self.condition(condition, scope)?;
                self.where_scope(scope, condition);
            }
            TransformKind::Sort { keys } => {
                for key in keys {
                    match &key.expression {
                        Expression::Field { name }
                            if name == "random" && !self.field_known(scope, "random") =>
                        {
                            key.expression = Expression::Function {
                                name: "random".into(),
                                arguments: Vec::new(),
                            };
                        }
                        Expression::Field { name }
                            if name == "rank" && !self.field_known(scope, "rank") => {}
                        _ => self.expression(&mut key.expression, scope, Place::Condition, at)?,
                    }
                }
            }
            TransformKind::Group { keys } => {
                for key in keys.iter_mut() {
                    self.expression(&mut key.expression, scope, Place::Condition, at)?;
                }
                self.group_scope(scope, keys);
            }
            TransformKind::Aggregate { aggregates } => {
                let rows = self.begin_aggregates(scope);
                for (index, aggregate) in aggregates.iter_mut().enumerate() {
                    for argument in &mut aggregate.arguments {
                        self.expression(argument, &rows, Place::Column, at)?;
                    }
                    self.check_aggregate_arguments(
                        &rows,
                        &aggregate.function,
                        &aggregate.arguments,
                        at,
                    )?;
                    self.add_aggregate(scope, &rows, aggregate, index);
                }
            }
            TransformKind::Limit { .. }
            | TransformKind::Offset { .. }
            | TransformKind::Distinct
            | TransformKind::AsOf { .. } => {}
            TransformKind::First { .. } => self.first_scope(scope),
            TransformKind::Matches => self.matches_scope(scope, at)?,
            TransformKind::Fill => self.fill_scope(scope, at)?,
            TransformKind::Fields { columns } => {
                for column in columns.iter_mut() {
                    self.expression(&mut column.expression, scope, Place::Column, at)?;
                }
                self.fields_scope(scope, columns, at)?;
            }
            TransformKind::Combine { how, query } => {
                let inner = self.query(query, Some(scope))?;
                self.check_combination(scope, &inner, *how, at)?;
            }
            TransformKind::Join { query, on } => {
                let inner = self.query(query, Some(scope))?;
                match on {
                    JoinOn::Key { left, right } => {
                        self.check_field(scope, left, at)?;
                        self.check_field(&inner, right, at)?;
                        self.expose_inner(scope, &inner, None);
                    }
                    JoinOn::Order {
                        relation,
                        within,
                        at: point,
                        scope: order_scope,
                    } => {
                        self.check_order_options(
                            *relation,
                            scope,
                            &inner,
                            within.as_ref(),
                            *point,
                            order_scope.as_deref(),
                            at,
                        )?;
                        self.expose_inner(scope, &inner, within.as_ref());
                    }
                }
            }
            TransformKind::Diff { query } => {
                let inner = self.query(query, Some(scope))?;
                self.diff_scope(scope, &inner, at)?;
            }
            TransformKind::Scope { unit, condition } => {
                *unit = self.unit(unit, at)?;
                *scope = Scope::new(unit.clone());
                if let Some(condition) = condition {
                    self.condition(condition, scope)?;
                }
                self.unit_condition_scope(scope, condition.as_ref());
            }
            TransformKind::Extension { arguments, .. } => self.arguments(arguments, scope, at)?,
        }
        self.finish_transform(scope, &transform.kind);
        Ok(())
    }
    fn condition(&mut self, condition: &mut Condition, scope: &Scope) -> Result<(), Error> {
        crate::depth::deeper(|| {
            let key = self.order_key(condition);
            let span = condition.span;
            let start = span.start;
            let until = matches!(condition.kind, ConditionKind::Until { .. });
            let kind = &mut condition.kind;
            match kind {
                ConditionKind::And { conditions } | ConditionKind::Or { conditions } => {
                    for condition in conditions {
                        self.condition(condition, scope)?;
                    }
                }
                ConditionKind::Not { condition } => self.condition(condition, scope)?,
                ConditionKind::Match { field, value } => {
                    self.check_match(scope, field, value, start)?
                }
                ConditionKind::Compare { left, op, right } => {
                    self.expression(left, scope, Place::Condition, start)?;
                    if let Expression::Value { value } = right {
                        self.check_compare_value(scope, left, *op, value, span)?;
                    } else {
                        self.expression(right, scope, Place::Argument, start)?;
                        self.check_compare_expressions(scope, left, right, start)?;
                    }
                }
                ConditionKind::Since { value, .. } | ConditionKind::Until { value, .. } => {
                    let value = std::mem::replace(value, Value::Any);
                    *kind = self.time_bound(scope, until, value, start)?;
                }
                ConditionKind::Between { left, from, to } => {
                    self.expression(left, scope, Place::Condition, start)?;
                    self.check_between(scope, left, from, to, start)?;
                }
                ConditionKind::TimeRange { left, .. } => {
                    self.expression(left, scope, Place::Condition, start)?
                }
                ConditionKind::In { left, query } => {
                    self.expression(left, scope, Place::Condition, start)?;
                    self.query(query, Some(scope))?;
                }
                ConditionKind::Exists {
                    unit,
                    condition: inner,
                }
                | ConditionKind::Count {
                    unit,
                    condition: inner,
                    ..
                } => {
                    *unit = self.unit(unit, start)?;
                    if let Some(inner) = inner {
                        self.condition(inner, &Scope::new(unit.clone()))?;
                    }
                }
                ConditionKind::Sequence {
                    unit, steps, gaps, ..
                } => {
                    *unit = self.sequence_unit(scope);
                    let rows = Scope::new(unit.clone());
                    for (index, step) in steps.iter_mut().enumerate() {
                        if index > 0
                            && let Some(Gap::Within {
                                distance: Distance::Steps { .. },
                            }) = gaps.get(index - 1)
                        {
                            self.require_order_scope(unit, start)?;
                        }
                        self.condition(step, &rows)?;
                    }
                }
                ConditionKind::Order {
                    relation,
                    query,
                    within,
                    at,
                    scope: order_scope,
                    pick,
                } => {
                    let inner = self.query(query, Some(scope))?;
                    self.check_order_options(
                        *relation,
                        scope,
                        &inner,
                        within.as_ref(),
                        *at,
                        order_scope.as_deref(),
                        start,
                    )?;
                    if let MatchChoice::FirstBy { keys } = pick {
                        let candidate = self.match_scope(&inner, within.as_ref());
                        for key in keys {
                            self.expression(
                                &mut key.expression,
                                &candidate,
                                Place::Condition,
                                start,
                            )?;
                        }
                    }
                    self.inner_scopes.insert(key, inner);
                }
                ConditionKind::Extension { arguments, .. } => {
                    self.arguments(arguments, scope, start)?
                }
                ConditionKind::Text {
                    text,
                    mode: TextMode::Word,
                } if self.reads_as_reference(scope, text) => {
                    *kind = ConditionKind::Reference {
                        reference: std::mem::take(text),
                    };
                }
                _ => {}
            }
            Ok(())
        })
    }
    fn expression(
        &mut self,
        expression: &mut Expression,
        scope: &Scope,
        place: Place,
        at: usize,
    ) -> Result<(), Error> {
        crate::depth::deeper(|| {
            match expression {
                Expression::Value { .. } => {}
                Expression::Field { name } => {
                    if self.reads_as_value(scope, name, place) {
                        *expression = Expression::value(crate::parse::value_token(name, false));
                    } else {
                        self.check_field(scope, name, at)?;
                    }
                }
                Expression::Function { name, .. } if name == "random" => {}
                Expression::Function { name, arguments } => {
                    let function = self.catalog.function(name)?.ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidQuery,
                            format!("`{name}` is not a function"),
                            at,
                        )
                    })?;
                    name.clone_from(&function.name);
                    for argument in arguments.iter_mut() {
                        self.expression(argument, scope, Place::Argument, at)?;
                    }
                    self.check_function(scope, &function, arguments, at)?;
                }
                Expression::Arithmetic {
                    operator,
                    left,
                    right,
                } => {
                    self.expression(left, scope, place, at)?;
                    self.expression(right, scope, place, at)?;
                    self.check_arithmetic(scope, *operator, left, right, at)?;
                }
            }
            Ok(())
        })
    }
    fn arguments(
        &mut self,
        arguments: &mut [ExtensionArgument],
        scope: &Scope,
        at: usize,
    ) -> Result<(), Error> {
        crate::depth::deeper(|| {
            for argument in arguments {
                match argument {
                    ExtensionArgument::Field(name) => self.check_field(scope, name, at)?,
                    ExtensionArgument::Expression(expression) => {
                        self.expression(expression, scope, Place::Column, at)?
                    }
                    ExtensionArgument::Condition(condition) => self.condition(condition, scope)?,
                    ExtensionArgument::Query(query) => {
                        self.query(query, Some(scope))?;
                    }
                    ExtensionArgument::Optional(arguments) | ExtensionArgument::List(arguments) => {
                        self.arguments(arguments, scope, at)?
                    }
                    _ => {}
                }
            }
            Ok(())
        })
    }
}
