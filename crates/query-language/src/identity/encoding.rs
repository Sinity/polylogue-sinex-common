//! The question identity vocabulary is independent of the Rust syntax wire format:
//! each construct has one tag and one shape, so two queries hash alike exactly
//! when their normal forms are equal.
use crate::ast::*;
use serde_json::Value as Json;

/// Builds JSON from owned parts. `serde_json::json!` turns each interpolated
/// value into JSON by serializing a reference to it, which copies an already
/// built child form, recursively and once per enclosing level; this moves it.
macro_rules! json {
    ({ $($key:literal : $value:expr),* $(,)? }) => {{
        let mut fields = serde_json::Map::new();
        $( fields.insert($key.to_owned(), IntoJson::into_json($value)); )*
        Json::Object(fields)
    }};
    ($value:expr) => {
        IntoJson::into_json($value)
    };
}

trait IntoJson {
    fn into_json(self) -> Json;
}
impl IntoJson for Json {
    fn into_json(self) -> Json {
        self
    }
}
impl IntoJson for Vec<Json> {
    fn into_json(self) -> Json {
        Json::Array(self)
    }
}
impl IntoJson for &str {
    fn into_json(self) -> Json {
        Json::String(self.to_owned())
    }
}
impl IntoJson for &String {
    fn into_json(self) -> Json {
        Json::String(self.clone())
    }
}
impl IntoJson for String {
    fn into_json(self) -> Json {
        Json::String(self)
    }
}
impl IntoJson for bool {
    fn into_json(self) -> Json {
        Json::Bool(self)
    }
}
impl IntoJson for &bool {
    fn into_json(self) -> Json {
        Json::Bool(*self)
    }
}
impl IntoJson for u64 {
    fn into_json(self) -> Json {
        self.into()
    }
}
impl IntoJson for &u64 {
    fn into_json(self) -> Json {
        (*self).into()
    }
}
impl IntoJson for usize {
    fn into_json(self) -> Json {
        self.into()
    }
}
impl IntoJson for &usize {
    fn into_json(self) -> Json {
        (*self).into()
    }
}
impl IntoJson for &f64 {
    fn into_json(self) -> Json {
        (*self).into()
    }
}
impl IntoJson for &u8 {
    fn into_json(self) -> Json {
        (*self).into()
    }
}
impl IntoJson for Option<Json> {
    fn into_json(self) -> Json {
        self.unwrap_or(Json::Null)
    }
}
impl IntoJson for &Option<String> {
    fn into_json(self) -> Json {
        self.as_ref()
            .map_or(Json::Null, |text| Json::String(text.clone()))
    }
}
impl IntoJson for Vec<String> {
    fn into_json(self) -> Json {
        Json::Array(self.into_iter().map(Json::String).collect())
    }
}
impl IntoJson for f64 {
    fn into_json(self) -> Json {
        self.into()
    }
}

fn arguments(args: &[Argument]) -> Vec<Json> {
    args.iter()
        .map(|a| json!({"parameter":&a.parameter,"value":value(&a.value)}))
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
    crate::depth::deeper(|| match e {
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
    })
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
fn gap(g: &Gap) -> Json {
    match g {
        Gap::Later => json!("later"),
        Gap::Next => json!("next"),
        Gap::Within {
            distance: Distance::Time { duration },
        } => json!({"within":json!({"duration":value(duration)})}),
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

fn column(c: &Column) -> Json {
    json!({"expression":expression(&c.expression),"name":c.column()})
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

pub(super) fn query(q: &Query) -> Json {
    crate::depth::deeper(|| {
        json!({"source":source(&q.source),"attach":q.attach.iter().map(attachment).collect::<Vec<_>>(),
        "stages":q.transforms.iter().map(|t|json!({"kind":transform(&t.kind)})).collect::<Vec<_>>()})
    })
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
fn condition(c: &Condition) -> Json {
    json!({"kind":condition_kind(&c.kind)})
}
fn condition_kind(c: &ConditionKind) -> Json {
    crate::depth::deeper(|| match c {
        ConditionKind::And { conditions } => {
            json!({"kind":"and","conditions":conditions.iter().map(condition).collect::<Vec<_>>()})
        }
        ConditionKind::Or { conditions } => {
            json!({"kind":"or","conditions":conditions.iter().map(condition).collect::<Vec<_>>()})
        }
        ConditionKind::Not { condition: c } => {
            json!({"kind":"not","condition":condition(c)})
        }
        ConditionKind::Match {
            field,
            value: Value::Known,
        } => json!({"kind":"field_exists","operand":expression(&Expression::field(field))}),
        ConditionKind::Match { field, value: v } => {
            json!({"kind":"match","field":field,"value":value(v)})
        }
        ConditionKind::Compare { left, op: o, right } => {
            json!({"kind":"compare","left":expression(left),"op":op(*o),"right":expression(right)})
        }
        ConditionKind::Since { field, value: v } => {
            json!({"kind":"since","operand":expression(&Expression::field(field)),"value":value(v)})
        }
        ConditionKind::Until { field, value: v } => {
            json!({"kind":"until","operand":expression(&Expression::field(field)),"value":value(v)})
        }
        ConditionKind::Between { left, from, to } => {
            json!({"kind":"between","left":expression(left),"from":value(from),"to":value(to)})
        }
        ConditionKind::TimeRange { left, value: v } => {
            json!({"kind":"time_range","left":expression(left),"value":value(v)})
        }
        ConditionKind::In { left, query: q } => {
            json!({"kind":"in","left":expression(left),"query":query(q)})
        }
        ConditionKind::Text { text, mode } => {
            json!({"kind":"text","text":text,"mode":match mode {TextMode::Word=>"word",TextMode::Phrase=>"phrase",TextMode::Prefix=>"prefix",TextMode::Approximate=>"approximate"}})
        }
        ConditionKind::Reference { reference } => {
            json!({"kind":"reference","reference":reference})
        }
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
            if let MatchChoice::FirstBy { keys } = pick {
                form["pick"] = json!({"kind":"first_by","keys":keys.iter().map(|key| json!({"expression":expression(&key.expression),"descending":key.descending})).collect::<Vec<_>>()});
            }
            form
        }
        ConditionKind::Extension { name, arguments } => extension(name, arguments),
    })
}
fn attachment(a: &Attachment) -> Json {
    json!({"unit":&a.unit,"fields":a.fields.clone(),"windows":a.windows.iter().map(|w| match w {Window::First(n)=>json!({"first":n}),Window::Last(n)=>json!({"last":n}),Window::Where(c)=>json!({"where":condition(c)})}).collect::<Vec<_>>()})
}
fn transform(t: &TransformKind) -> Json {
    crate::depth::deeper(|| match t {
        TransformKind::Where { condition: c } => json!({"kind":"where","condition":condition(c)}),
        TransformKind::Sort { keys } => {
            json!({"kind":"sort","keys":keys.iter().map(|k|json!({"expression":expression(&k.expression),"descending":k.descending})).collect::<Vec<_>>()})
        }
        TransformKind::Group { keys } => {
            json!({"kind":"group","keys":keys.iter().map(column).collect::<Vec<_>>()})
        }
        TransformKind::Aggregate { aggregates } => {
            json!({"kind":"aggregate","aggregates":aggregates.iter().map(|a|json!({"function":aggregate_function(&a.function),"arguments":a.arguments.iter().map(expression).collect::<Vec<_>>(),"name":a.column()})).collect::<Vec<_>>()})
        }
        TransformKind::Limit { count } => json!({"kind":"limit","count":count}),
        TransformKind::Offset { count } => json!({"kind":"offset","count":count}),
        TransformKind::First { count } => json!({"kind":"first","count":count}),
        TransformKind::Distinct => json!({"kind":"distinct"}),
        TransformKind::Matches => json!({"kind":"match_steps"}),
        TransformKind::Fill => json!({"kind":"fill"}),
        TransformKind::Fields { columns } => {
            json!({"kind":"fields","columns":columns.iter().map(column).collect::<Vec<_>>()})
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
    })
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
