use query_language::{
    ErrorKind,
    ast::{ConditionKind, Source, Value},
    bind::bind,
    catalog::{Catalog, Field, FieldKind, Unit},
    identity::question_id,
    number::{is_decimal, is_integer},
    parse::parse,
};
use std::collections::BTreeMap;

#[test]
fn exact_decimal_validation_accepts_large_values_without_float_or_integer_caps() {
    for value in [
        "18446744073709551615",
        "340282366920938463463374607431768211455",
        "1.00000000000000000000000000000000001",
        "1e10000",
        "1e999999999999999999999999999999",
        "1e-999999999999999999999999999999",
        "+001.2500",
        ".5",
        "1.",
    ] {
        assert!(is_decimal(value), "{value}");
    }
    assert!(is_decimal(&"9".repeat(16384)));
    for value in [
        "18446744073709551615",
        "1.0",
        "1e3",
        "1000e-3",
        "-0.00e-999999999999999999999999",
        "1e999999999999999999999999999999",
        "+001",
    ] {
        assert!(is_integer(value), "{value}");
    }
    for value in [
        "1.25",
        "1e-1",
        "1000e-4",
        "1e-999999999999999999999999999999",
    ] {
        assert!(!is_integer(value), "{value}");
    }
}

#[test]
fn decimal_validation_refuses_nonfinite_or_malformed_lexemes() {
    for value in [
        "",
        "+",
        "-",
        ".",
        "NaN",
        "nan",
        "inf",
        "-Infinity",
        "0x1",
        "1_000",
        "1e",
        "1e+",
        "1e--1",
        "1e2e3",
        "1.2.3",
        " 1",
        "1 ",
    ] {
        assert!(!is_decimal(value), "{value:?}");
        assert!(!is_integer(value), "{value:?}");
    }
}

fn catalog() -> Catalog {
    let mut catalog = Catalog::new("rows");
    catalog.units.push(Unit {
        name: "rows".into(),
        fields: [
            ("integer", FieldKind::Integer),
            ("decimal", FieldKind::Number),
            ("text", FieldKind::Text),
        ]
        .into_iter()
        .map(|(name, kind)| Field {
            name: name.into(),
            kind,
            ..Default::default()
        })
        .collect(),
        ..Default::default()
    });
    catalog
}

#[test]
fn parser_and_binder_share_exact_validation_and_retain_the_raw_scalar() {
    let catalog = catalog();
    for (field, lexeme) in [
        ("integer", "18446744073709551615"),
        ("integer", "1.0"),
        ("integer", "1e999999999999999999999999999999"),
        ("decimal", "1.00000000000000000000000000000000001"),
        ("decimal", "1e-999999999999999999999999999999"),
        ("text", "001"),
    ] {
        let syntax = parse(&format!("{field}:{lexeme}"), &catalog).unwrap();
        let bound = bind(&syntax, &catalog).unwrap();
        let Source::Unit {
            condition: Some(condition),
            ..
        } = &bound.query().source
        else {
            panic!()
        };
        assert!(
            matches!(&condition.kind, ConditionKind::Match { value: Value::Scalar { text, .. }, .. } if text == lexeme)
        );
    }
    for question in ["integer:1.25", "decimal:NaN", "decimal:inf"] {
        assert_eq!(
            parse(question, &catalog).unwrap_err().kind,
            ErrorKind::BadValue
        );
    }
}

#[test]
fn high_precision_literals_have_distinct_identity_while_integral_spellings_agree() {
    let catalog = catalog();
    let identity =
        |text| question_id(&parse(text, &catalog).unwrap(), &catalog, &BTreeMap::new()).unwrap();
    assert_ne!(
        identity("decimal:1.00000000000000000000000000000000001"),
        identity("decimal:1.00000000000000000000000000000000002")
    );
    assert_eq!(identity("integer:1000"), identity("integer:1e3"));
    assert_ne!(identity("text:001"), identity("text:1"));
}

#[test]
fn huge_exponents_share_exact_canonical_identity_without_expanding_their_magnitude() {
    let catalog = catalog();
    let identity =
        |text| question_id(&parse(text, &catalog).unwrap(), &catalog, &BTreeMap::new()).unwrap();
    assert_eq!(
        identity("decimal:1e100000000000000000000000000000000"),
        identity("decimal:10e99999999999999999999999999999999")
    );
    assert_eq!(
        identity("decimal:1e-100000000000000000000000000000000"),
        identity("decimal:10e-100000000000000000000000000000001")
    );
    assert_eq!(
        identity("decimal:0e-99999999999999999999999999999999"),
        identity("decimal:0")
    );
    assert_ne!(
        identity("decimal:1e100000000000000000000000000000000"),
        identity("decimal:1e99999999999999999999999999999999")
    );
}
