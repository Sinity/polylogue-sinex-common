//! The question identity vocabulary is independent of the Rust syntax wire format.
//! Existing isomorphic constructs retain their original identity spelling; new
//! relational meanings receive distinct tags rather than reusing old tuples.
use crate::ast::*;
use serde_json::{Value as Json, json};

pub(super) fn query(q: &Query) -> Json {
    json!({"source":source(&q.source),"attach":q.attach.iter().map(attachment).collect::<Vec<_>>(),
        "stages":q.transforms.iter().enumerate().map(|(i,t)|json!({"kind":transform(&t.kind,i + 1 == q.transforms.len())})).collect::<Vec<_>>()})
}
fn source(s: &Source) -> Json {
    match s {
        Source::Unit {
            unit,
            implicit,
            condition: c,
        } => {
            let mut out =
                json!({"kind":"unit","unit":unit,"condition":optional_condition(c.as_ref())});
            if *implicit {
                out["implicit"] = json!(true);
            }
            out
        }
        Source::Saved {
            name,
            arguments: args,
            condition: c,
            ..
        } => {
            json!({"kind":"saved","name":name,"arguments":arguments(args),"condition":optional_condition(c.as_ref())})
        }
        Source::Result {
            reference,
            part,
            condition: c,
            ..
        } => {
            json!({"kind":"result","reference":reference,"part":match part {ResultPart::Rows=>"rows",ResultPart::Matches=>"matches"},"condition":optional_condition(c.as_ref())})
        }
    }
}
fn optional_condition(c: Option<&Condition>) -> Json {
    c.map_or(Json::Null, condition)
}
fn arguments(args: &[Argument]) -> Vec<Json> {
    args.iter()
        .map(|a| json!({"parameter":a.parameter,"value":value(&a.value)}))
        .collect()
}
fn value(v: &Value) -> Json {
    match v {
        Value::Scalar { text, .. } => json!({"kind":"scalar","text":text}),
        Value::Set { values } => {
            json!({"kind":"set","values":values.iter().map(value).collect::<Vec<_>>()})
        }
        Value::Any => json!({"kind":"any"}),
        Value::Unknown => json!({"kind":"unknown"}),
        Value::Known => json!({"kind":"known"}),
        Value::Parameter { name } => json!({"kind":"parameter","name":name}),
    }
}
fn expression(e: &Expression) -> Json {
    match e {
        Expression::Field { name } => json!({"kind":"field","name":name}),
        Expression::Value { value: v } => json!({"kind":"value","value":value(v)}),
        Expression::Function { name, arguments } => {
            json!({"kind":"function","name":name,"arguments":arguments.iter().map(expression).collect::<Vec<_>>()})
        }
        Expression::Arithmetic {
            operator,
            left,
            right,
        } => {
            json!({"kind":"arithmetic","operator":match operator {Arithmetic::Add=>"add",Arithmetic::Subtract=>"subtract",Arithmetic::Multiply=>"multiply",Arithmetic::Divide=>"divide"},"left":expression(left),"right":expression(right)})
        }
    }
}
fn original_operand(e: &Expression) -> bool {
    match e {
        Expression::Field { .. } => true,
        Expression::Function { arguments, .. } => arguments.iter().all(original_operand),
        _ => false,
    }
}
fn op(o: Op) -> &'static str {
    match o {
        Op::Eq => "eq",
        Op::Ne => "ne",
        Op::Lt => "lt",
        Op::Le => "le",
        Op::Gt => "gt",
        Op::Ge => "ge",
        Op::Contains => "contains",
        Op::Prefix => "prefix",
    }
}
fn condition(c: &Condition) -> Json {
    json!({"kind":condition_kind(&c.kind)})
}
fn condition_kind(c: &ConditionKind) -> Json {
    match c {
        ConditionKind::And { conditions } => {
            json!({"kind":"and","conditions":conditions.iter().map(condition).collect::<Vec<_>>()})
        }
        ConditionKind::Or { conditions } => {
            json!({"kind":"or","conditions":conditions.iter().map(condition).collect::<Vec<_>>()})
        }
        ConditionKind::Not { condition: c } => json!({"kind":"not","condition":condition(c)}),
        ConditionKind::Match {
            field,
            value: Value::Known,
        } => json!({"kind":"field_exists","operand":expression(&Expression::field(field))}),
        ConditionKind::Match { field, value: v } => {
            json!({"kind":"match","field":field,"value":value(v)})
        }
        ConditionKind::Compare {
            left,
            op: o,
            right: Expression::Value { value: v },
        } if original_operand(left) => {
            json!({"kind":"compare","operand":expression(left),"op":op(*o),"value":value(v)})
        }
        ConditionKind::Compare {
            left:
                Expression::Arithmetic {
                    operator: Arithmetic::Subtract,
                    left,
                    right,
                },
            op: o,
            right: Expression::Value { value: v },
        } if original_operand(left)
            && original_operand(right)
            && matches!(v,Value::Scalar{text,..} if crate::time::duration(text).is_some()) =>
        {
            json!({"kind":"time_difference","left":expression(left),"right":expression(right),"op":op(*o),"value":value(v)})
        }
        ConditionKind::Compare { left, op: o, right } => {
            json!({"kind":"expression_compare","left":expression(left),"op":op(*o),"right":expression(right)})
        }
        ConditionKind::Since { field, value: v } => {
            json!({"kind":"since","operand":expression(&Expression::field(field)),"value":value(v)})
        }
        ConditionKind::Until { field, value: v } => {
            json!({"kind":"until","operand":expression(&Expression::field(field)),"value":value(v)})
        }
        ConditionKind::Between { left, from, to } if original_operand(left) => {
            json!({"kind":"between","operand":expression(left),"from":value(from),"to":value(to)})
        }
        ConditionKind::Between { left, from, to } => {
            json!({"kind":"expression_between","left":expression(left),"from":value(from),"to":value(to)})
        }
        ConditionKind::TimeRange { left, value: v } if original_operand(left) => {
            json!({"kind":"time_range","operand":expression(left),"value":value(v)})
        }
        ConditionKind::TimeRange { left, value: v } => {
            json!({"kind":"expression_time_range","left":expression(left),"value":value(v)})
        }
        ConditionKind::In { left, query: q } if original_operand(left) => {
            json!({"kind":"in","operand":expression(left),"query":query(q)})
        }
        ConditionKind::In { left, query: q } => {
            json!({"kind":"expression_in","left":expression(left),"query":query(q)})
        }
        ConditionKind::Text { text, mode } => {
            json!({"kind":"text","text":text,"mode":match mode {TextMode::Word=>"word",TextMode::Phrase=>"phrase",TextMode::Prefix=>"prefix",TextMode::Approximate=>"approximate"}})
        }
        ConditionKind::Reference { reference } => json!({"kind":"reference","reference":reference}),
        ConditionKind::Rank {
            list: RankList::Similar,
            value: v,
        } => json!({"kind":"similar","value":value(v)}),
        ConditionKind::Rank { list, value: v } => {
            json!({"kind":"rank","list":list.name(),"value":value(v)})
        }
        ConditionKind::Exists { unit, condition: c } => {
            json!({"kind":"exists","unit":unit,"condition":optional_condition(c.as_deref())})
        }
        ConditionKind::Count {
            unit,
            condition: c,
            op: o,
            count,
        } => {
            json!({"kind":"count","unit":unit,"condition":optional_condition(c.as_deref()),"op":op(*o),"count":count})
        }
        ConditionKind::Saved {
            name,
            arguments: args,
        } => json!({"kind":"saved","name":name,"arguments":arguments(args)}),
        ConditionKind::Sequence {
            unit,
            all,
            steps,
            gaps,
        } => {
            json!({"kind":"sequence","unit":unit,"all":all,"steps":steps.iter().map(condition).collect::<Vec<_>>(),"gaps":gaps.iter().map(gap).collect::<Vec<_>>()})
        }
        ConditionKind::Order {
            relation,
            query: q,
            within,
            at,
            scope,
            pick,
        } => {
            let mut form = json!({"kind":"order","relation":relation.name(),"query":query(q),"within":within.as_ref().map(distance),"at":point(*at),"scope":scope});
            if let WitnessChoice::FirstBy { keys } = pick {
                form["pick"] = json!({"kind":"first_by","keys":keys.iter().map(|key| json!({"expression":expression(&key.expression),"descending":key.descending})).collect::<Vec<_>>()});
            }
            form
        }
        ConditionKind::Extension { name, arguments } => extension(name, arguments),
    }
}
fn gap(g: &Gap) -> Json {
    match g {
        Gap::Later => json!("later"),
        Gap::Next => json!("next"),
        Gap::Within {
            distance: Distance::Time { duration },
        } => json!({"within":{"duration":value(duration)}}),
        Gap::Within { distance: d } => json!({"within_positions":distance(d)}),
    }
}
fn distance(d: &Distance) -> Json {
    match d {
        Distance::Time { duration } => json!({"kind":"time","duration":value(duration)}),
        Distance::Steps { count } => json!({"kind":"steps","count":count}),
    }
}
fn point(p: TimePoint) -> &'static str {
    match p {
        TimePoint::When => "when",
        TimePoint::Start => "start",
        TimePoint::End => "end",
        TimePoint::MatchWhen => "match_when",
        TimePoint::MatchStart => "match_start",
        TimePoint::MatchEnd => "match_end",
    }
}
fn attachment(a: &Attachment) -> Json {
    json!({"unit":a.unit,"fields":a.fields,"windows":a.windows.iter().map(|w| match w {Window::First(n)=>json!({"first":n}),Window::Last(n)=>json!({"last":n}),Window::Where(c)=>json!({"where":condition(c)})}).collect::<Vec<_>>()})
}
fn column(c: &Column) -> Json {
    json!({"expression":expression(&c.expression),"name":c.name})
}
fn original_columns(cs: &[Column]) -> bool {
    cs.iter()
        .all(|c| c.column() == c.expression.column() && original_operand(&c.expression))
}
fn transform(t: &TransformKind, terminal: bool) -> Json {
    match t {
        TransformKind::Where { condition: c } => json!({"kind":"where","condition":condition(c)}),
        TransformKind::Sort { keys } if keys.iter().all(|k| original_operand(&k.expression)) => {
            json!({"kind":"sort","keys":keys.iter().map(|k|json!({"operand":expression(&k.expression),"descending":k.descending})).collect::<Vec<_>>()})
        }
        TransformKind::Sort { keys } => {
            json!({"kind":"expression_sort","keys":keys.iter().map(|k|json!({"expression":expression(&k.expression),"descending":k.descending})).collect::<Vec<_>>()})
        }
        TransformKind::Group { keys } if original_columns(keys) => {
            json!({"kind":"group","keys":keys.iter().map(|k|expression(&k.expression)).collect::<Vec<_>>()})
        }
        TransformKind::Group { keys } => {
            json!({"kind":"named_group","keys":keys.iter().map(column).collect::<Vec<_>>()})
        }
        TransformKind::Aggregate { aggregates } if aggregates.iter().all(original_aggregate) => {
            json!({"kind":"aggregate","aggregates":aggregates.iter().map(|a|json!({"function":aggregate_function(&a.function),"fields":a.arguments.iter().map(|e|if let Expression::Field {name}=e {name.as_str()} else {unreachable!()}).collect::<Vec<_>>()})).collect::<Vec<_>>()})
        }
        TransformKind::Aggregate { aggregates } => {
            json!({"kind":"expression_aggregate","aggregates":aggregates.iter().map(|a|json!({"function":aggregate_function(&a.function),"arguments":a.arguments.iter().map(expression).collect::<Vec<_>>(),"name":a.name})).collect::<Vec<_>>()})
        }
        TransformKind::Limit { count } => json!({"kind":"limit","count":count}),
        TransformKind::Offset { count } => json!({"kind":"offset","count":count}),
        TransformKind::First { count } => json!({"kind":"first","count":count}),
        TransformKind::Distinct => json!({"kind":"distinct"}),
        TransformKind::Matches => json!({"kind":"match_steps"}),
        TransformKind::Fill => json!({"kind":"fill"}),
        TransformKind::Fields { columns }
            if terminal && columns.iter().all(|c| {
                matches!(&c.expression,Expression::Field {name} if c.column() == name.replace(['.','-'], "_"))
            }) =>
        {
            json!({"kind":"fields","fields":columns.iter().map(|c|if let Expression::Field {name}=&c.expression {name.as_str()} else {unreachable!()}).collect::<Vec<_>>()})
        }
        TransformKind::Fields { columns } => {
            json!({"kind":"projection","columns":columns.iter().map(column).collect::<Vec<_>>()})
        }
        TransformKind::Scope { unit, condition: c } => {
            json!({"kind":"scope","unit":unit,"condition":optional_condition(c.as_ref())})
        }
        TransformKind::Combine { how, query: q } => {
            json!({"kind":"combine","how":match how {Combination::Union=>"union",Combination::Intersect=>"intersect",Combination::Except=>"except",Combination::Fuse=>"fuse"},"query":query(q)})
        }
        TransformKind::Join { query: q, on } => {
            json!({"kind":"join","query":query(q),"on":match on {JoinOn::Key {left,right}=>json!({"kind":"key","left":left,"right":right}),JoinOn::Order {relation,within,at,scope}=>json!({"kind":"order","relation":relation.name(),"within":within.as_ref().map(distance),"at":point(*at),"scope":scope})}})
        }
        TransformKind::Diff { query: q } => json!({"kind":"diff","query":query(q)}),
        TransformKind::AsOf { value: v, sequence } => {
            json!({"kind":"as_of","value":value(v),"sequence":sequence})
        }
        TransformKind::Extension { name, arguments } => extension(name, arguments),
    }
}
fn original_aggregate(a: &Aggregate) -> bool {
    a.name.as_ref().is_none_or(|name| {
        let mut unnamed = a.clone();
        unnamed.name = None;
        *name == unnamed.column()
    }) && !matches!(a.function, AggregateFunction::Covered)
        && a.arguments
            .iter()
            .all(|a| matches!(a, Expression::Field { .. }))
}
fn aggregate_function(f: &AggregateFunction) -> Json {
    match f {
        AggregateFunction::Count => json!("count"),
        AggregateFunction::Sum => json!("sum"),
        AggregateFunction::Avg => json!("avg"),
        AggregateFunction::Min => json!("min"),
        AggregateFunction::Max => json!("max"),
        AggregateFunction::Percentile(p) => json!({"percentile":p}),
        AggregateFunction::CountDistinct => json!("count_distinct"),
        AggregateFunction::Histogram => json!("histogram"),
        AggregateFunction::Correlation => json!("correlation"),
        AggregateFunction::Covered => json!("covered"),
    }
}
fn extension(name: &str, args: &[ExtensionArgument]) -> Json {
    json!({"kind":"extension","name":name,"arguments":args.iter().map(argument).collect::<Vec<_>>()})
}
fn argument(a: &ExtensionArgument) -> Json {
    match a {
        ExtensionArgument::Value(v) => json!({"value":value(v)}),
        ExtensionArgument::Name(s) => json!({"name":s}),
        ExtensionArgument::Optional(args) => {
            json!({"optional":args.iter().map(argument).collect::<Vec<_>>()})
        }
        ExtensionArgument::Field(s) => json!({"field":s}),
        ExtensionArgument::Unit(s) => json!({"unit":s}),
        ExtensionArgument::Condition(c) => json!({"condition":condition(c)}),
        ExtensionArgument::Query(q) => json!({"query":query(q)}),
        ExtensionArgument::Keyword(s) => json!({"keyword":s}),
        ExtensionArgument::Expression(e) => json!({"expression":expression(e)}),
        ExtensionArgument::List(args) => {
            json!({"list":args.iter().map(argument).collect::<Vec<_>>()})
        }
    }
}
