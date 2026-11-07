mod support;
use query_language::{
    ErrorKind,
    bind::bind,
    catalog::{Catalog, Field, FieldKind, Unit, UnknownRule},
    parse::parse,
};

fn catalog() -> Catalog {
    let mut catalog = Catalog::new("records");
    let reference = |name: &str, target: &str| Field {
        name: name.into(),
        kind: FieldKind::Reference,
        unit: Some(target.into()),
        ..Field::default()
    };
    let mut same_owner = reference("same_owner", "person");
    same_owner.help = "another source's explanation".into();
    same_owner.read_as = "another phrase".into();
    let mut owners = reference("owners", "people");
    owners.kind = FieldKind::List(Box::new(FieldKind::Reference));
    let mut files = reference("files", "files");
    files.kind = FieldKind::List(Box::new(FieldKind::Reference));
    let state = |name: &str, values: &[&str], unknown| Field {
        name: name.into(),
        kind: FieldKind::Exact,
        values: values.iter().map(|value| (*value).into()).collect(),
        unknown,
        ..Field::default()
    };
    catalog.units.push(Unit {
        name: "records".into(),
        fields: vec![
            reference("owner", "people"),
            same_owner,
            reference("file", "files"),
            owners,
            files,
            Field {
                name: "label".into(),
                kind: FieldKind::Text,
                unit: Some("people".into()),
                ..Field::default()
            },
            state("state", &["open", "closed"], UnknownRule::Unknown),
            state("reordered_state", &["closed", "open"], UnknownRule::Unknown),
            state("other_state", &["busy", "done"], UnknownRule::Unknown),
            state("required_state", &["open", "closed"], UnknownRule::Never),
            state(
                "word_state",
                &["open", "closed"],
                UnknownRule::Word("NA".into()),
            ),
            state(
                "word_case_state",
                &["open", "closed"],
                UnknownRule::Word("na".into()),
            ),
            Field {
                name: "integer".into(),
                kind: FieldKind::Integer,
                ..Field::default()
            },
            Field {
                name: "number".into(),
                kind: FieldKind::Number,
                ..Field::default()
            },
        ],
        ..Unit::default()
    });
    catalog.units.push(Unit {
        name: "people".into(),
        singular: Some("person".into()),
        ..Unit::default()
    });
    catalog.units.push(Unit {
        name: "files".into(),
        ..Unit::default()
    });
    catalog
}

#[test]
fn projected_set_operators_refuse_incompatible_semantic_types() {
    let catalog = catalog();
    for operator in ["union", "intersect", "except", "diff"] {
        for (left, right) in [
            ("owner", "text(owner)"),
            ("text(owner)", "owner"),
            ("owner", "file"),
            ("owners", "files"),
            ("integer", "number"),
            ("state", "other_state"),
            ("state", "required_state"),
            ("state", "word_state"),
        ] {
            let query = format!(
                "records | fields {left} as value | {operator} (records | fields {right} as value)"
            );
            let error = parse(&query, &catalog).unwrap_err();
            assert_eq!(error.kind, ErrorKind::InvalidQuery, "{query}");
            assert!(
                error.message.contains("same projected fields and types"),
                "{query}: {error}"
            );
        }
    }
}

#[test]
fn projected_set_operators_accept_matching_types_from_different_origins() {
    let catalog = catalog();
    for operator in ["union", "intersect", "except", "diff"] {
        for (left, right) in [
            ("owner", "same_owner"),
            ("text(owner)", "text(file)"),
            ("text(owner)", "label"),
            ("owners", "owners"),
            ("state", "reordered_state"),
            ("word_state", "word_case_state"),
        ] {
            let query = format!(
                "records | fields {left} as value | {operator} (records | fields {right} as value)"
            );
            bind(&parse(&query, &catalog).unwrap(), &catalog).unwrap();
        }
    }
    // Fuse contributes ordering, so its operand's output columns are not
    // inserted into the left relation and need no projected-schema equality.
    let query = "records | fields owner | fuse (records | fields text(owner) as plain)";
    bind(&parse(query, &catalog).unwrap(), &catalog).unwrap();
}

#[test]
fn both_product_catalogs_require_matching_projected_reference_and_text_types() {
    for (catalog, unit) in [
        (support::polylogue(), "sessions"),
        (support::sinex(), "events"),
    ] {
        for operator in ["union", "intersect", "except", "diff"] {
            let incompatible =
                format!("{unit} | fields id | {operator} ({unit} | fields text(id) as id)");
            assert_eq!(
                parse(&incompatible, &catalog).unwrap_err().kind,
                ErrorKind::InvalidQuery
            );
            let compatible = format!(
                "{unit} | fields text(id) as id | {operator} ({unit} | fields text(id) as id)"
            );
            bind(&parse(&compatible, &catalog).unwrap(), &catalog).unwrap();
            let native = format!("{unit} | {operator} ({unit})");
            bind(&parse(&native, &catalog).unwrap(), &catalog).unwrap();
        }
    }
}

#[test]
fn binding_checks_captured_output_types_when_source_schemas_become_known() {
    use query_language::catalog::{OutputSchema, OutputSource, SourceOutput};
    let mut catalog = catalog();
    let syntax = parse("query:references | union (query:plain)", &catalog).unwrap();
    for (name, kind, unit) in [
        ("references", FieldKind::Reference, Some("people".into())),
        ("plain", FieldKind::Text, None),
    ] {
        catalog.source_outputs.push(SourceOutput {
            source: OutputSource::Saved {
                name: name.into(),
                arguments: vec![],
            },
            schema: OutputSchema {
                unit: Some("records".into()),
                fields: vec![Field {
                    name: "value".into(),
                    kind,
                    unit,
                    ..Field::default()
                }],
                ..OutputSchema::default()
            },
        });
    }
    let error = bind(&syntax, &catalog).unwrap_err();
    assert_eq!(error.kind, ErrorKind::InvalidQuery);
    catalog.source_outputs[0].schema.fields[0].kind = FieldKind::Text;
    catalog.source_outputs[0].schema.fields[0].unit = None;
    bind(&syntax, &catalog).unwrap();
}
