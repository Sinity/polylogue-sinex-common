use query_language::{
    bind::bind,
    catalog::*,
    format::format_query,
    identity::question_id,
    parse::parse,
    scope::{Origin, TypeRule},
    semantics::{Dataset, ReferenceEvaluator, Row, Scalar},
};
use std::collections::BTreeMap;

fn catalog() -> Catalog {
    let field = |name: &str, kind| Field {
        name: name.into(),
        kind,
        ..Default::default()
    };
    Catalog {
        default_unit: "rows".into(),
        units: vec![Unit {
            name: "rows".into(),
            identity: vec!["id".into()],
            fields: vec![
                field("id", FieldKind::Reference),
                field("n", FieldKind::Integer),
                field("cost", FieldKind::Money),
                field("duration", FieldKind::Duration),
                Field {
                    values: vec!["open".into(), "closed".into()],
                    ..field("state", FieldKind::Text)
                },
                field("label", FieldKind::Text),
            ],
            ..Default::default()
        }],
        ..Default::default()
    }
}

#[test]
fn conditional_values_keep_the_selected_inputs_kind_and_lineage_through_aggregation() {
    let catalog = catalog();
    let query = parse("rows | agg min(case_eq(n, 1, state)) as selected, min(case_eq(n, 1, id)) as selected_ref, min(case_eq(n, 1, cost)) as selected_cost", &catalog).unwrap();
    let bound = bind(&query, &catalog).unwrap();
    let output = &bound.typing().queries[0].transforms[0].after;
    for (name, kind) in [
        ("selected", FieldKind::Text),
        ("selected_ref", FieldKind::Reference),
        ("selected_cost", FieldKind::Money),
    ] {
        let info = output.field(name).unwrap();
        assert_eq!(info.field.kind, kind);
        let Origin::Aggregate {
            inputs,
            rule: TypeRule::Input(0),
            ..
        } = &info.origin
        else {
            panic!("{info:?}");
        };
        let Origin::Function {
            arguments,
            rule: TypeRule::Input(2),
            ..
        } = &inputs[0].origin
        else {
            panic!("{:?}", inputs[0]);
        };
        assert_eq!(arguments[2].field.kind, kind);
    }
    assert_eq!(
        output.field("selected").unwrap().field.values,
        ["open", "closed"]
    );
}

#[test]
fn conditional_equality_is_exact_unknown_aware_and_round_trips() {
    let catalog = catalog();
    let data: Dataset = BTreeMap::from([(
        "rows".into(),
        vec![
            Row::new("a")
                .with("n", 1.)
                .with("state", "open")
                .with("label", "001")
                .with("cost", 3.),
            Row::new("b")
                .with("n", 2.)
                .with("state", "closed")
                .with("label", "1")
                .with("cost", 4.),
            Row::new("c").with("state", "open").with("label", "001"),
        ],
    )]);
    let text = "rows | fields case_eq(n, 1, state) as selected, case_eq(label, 001, id) as reference, case_eq(n, unknown, state) as unknown_case";
    let query = parse(text, &catalog).unwrap();
    let formatted = format_query(&query, &catalog).unwrap();
    let reparsed = parse(&formatted, &catalog).unwrap();
    assert_eq!(
        question_id(&query, &catalog, &BTreeMap::new()).unwrap(),
        question_id(&reparsed, &catalog, &BTreeMap::new()).unwrap()
    );
    for query in [&query, &reparsed] {
        let rows = ReferenceEvaluator::new(&data).execute(query).unwrap().rows;
        assert_eq!(rows[0].value("selected"), Scalar::from("open"));
        assert_eq!(rows[1].value("selected"), Scalar::Unknown);
        assert_eq!(rows[2].value("selected"), Scalar::Unknown);
        assert_eq!(rows[0].value("reference"), Scalar::from("a"));
        assert_eq!(rows[1].value("reference"), Scalar::Unknown);
        assert_eq!(rows[2].value("reference"), Scalar::from("c"));
        assert!(
            rows.iter()
                .all(|row| row.value("unknown_case") == Scalar::Unknown)
        );
    }
}

#[test]
fn conditional_comparisons_keep_dimensions_and_closed_values() {
    let catalog = catalog();
    for text in [
        "rows | fields case_eq(cost, 1h, state)",
        "rows | fields case_eq(cost, duration, state)",
        "rows | fields case_eq(state, nonexistent, n)",
        "rows | fields case_eq(n, 1)",
    ] {
        assert!(parse(text, &catalog).is_err(), "{text}");
    }
    assert!(parse("rows | fields case_eq(cost, 1.00, state)", &catalog).is_ok());
}
