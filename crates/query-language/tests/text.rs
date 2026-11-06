use query_language::{
    ast::*,
    bind::bind,
    catalog::*,
    format::format_query,
    identity::normal_form,
    parse::parse,
    scope::{Origin, TypeRule},
    semantics::*,
};
use std::collections::BTreeMap;
fn catalog() -> Catalog {
    let mut catalog = Catalog::new("events");
    let fields = [
        ("plain", FieldKind::Text),
        ("exact", FieldKind::Exact),
        ("words", FieldKind::Words),
        ("path", FieldKind::Subtree),
        ("substring", FieldKind::Substring),
        ("reference", FieldKind::Reference),
        ("json", FieldKind::Json),
        ("integer", FieldKind::Integer),
        ("number", FieldKind::Number),
        ("money", FieldKind::Money),
        ("duration", FieldKind::Duration),
        ("when", FieldKind::Time),
        ("boolean", FieldKind::Bool),
        ("list", FieldKind::List(Box::new(FieldKind::Text))),
    ];
    catalog.units.push(Unit {
        name: "events".into(),
        fields: fields
            .into_iter()
            .map(|(name, kind)| Field {
                name: name.into(),
                kind,
                unit: Some("events".into()),
                values: if name == "exact" {
                    vec!["open".into()]
                } else {
                    vec![]
                },
                ..Field::default()
            })
            .collect(),
        ..Unit::default()
    });
    catalog
}
#[test]
fn text_cast_reads_strings_without_retaining_entity_path_or_closed_set_meaning() {
    let catalog = catalog();
    for input in [
        "plain",
        "exact",
        "words",
        "path",
        "substring",
        "reference",
        "json",
    ] {
        let query = parse(&format!("events | fields text({input}) as label"), &catalog).unwrap();
        let bound = bind(&query, &catalog).unwrap();
        let label = bound.typing().queries[0].transforms[0]
            .after
            .field("label")
            .unwrap();
        assert_eq!(label.field.kind, FieldKind::Text);
        assert_eq!(label.field.unit, None);
        assert!(label.field.values.is_empty());
        assert!(
            matches!(&label.origin,Origin::Function {name,rule:TypeRule::Fixed,..} if name=="text")
        );
        let rendered = format_query(&query, &catalog).unwrap();
        assert_eq!(
            normal_form(&query, &catalog).unwrap(),
            normal_form(&parse(&rendered, &catalog).unwrap(), &catalog).unwrap()
        );
        let filtered = parse(
            &format!("events | fields text({input}) as label | where label:open"),
            &catalog,
        )
        .unwrap();
        bind(&filtered, &catalog).unwrap();
    }
    for input in [
        "integer", "number", "money", "duration", "when", "boolean", "list",
    ] {
        assert!(
            parse(&format!("events | fields text({input})"), &catalog).is_err(),
            "{input}"
        );
    }
}
#[test]
fn text_cast_preserves_string_bytes_and_unknown_and_refuses_other_runtime_values() {
    let data = BTreeMap::new();
    let evaluator = ReferenceEvaluator::new(&data);
    let expression = Expression::Function {
        name: "text".into(),
        arguments: vec![Expression::field("input")],
    };
    for text in ["001", "event:Case", "/path/to/object", "Żółć 😀\0tail"] {
        let row = Row::new("row").with("input", text);
        assert_eq!(
            evaluator.expression(&row, &expression).unwrap(),
            Scalar::from(text)
        );
    }
    assert_eq!(
        evaluator
            .expression(&Row::new("unknown"), &expression)
            .unwrap(),
        Scalar::Unknown
    );
    for value in [
        Scalar::Number(123.),
        Scalar::Bool(true),
        Scalar::List(vec![Scalar::from("text")]),
    ] {
        assert!(
            evaluator
                .expression(&Row::new("row").with("input", value), &expression)
                .is_err()
        );
    }
}
