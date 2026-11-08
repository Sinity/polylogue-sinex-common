use query_language::{
    ast::{Combination, TransformKind},
    bind::{BoundQuery, bind},
    catalog::{Catalog, Field, FieldKind, OutputSchema, OutputSource, SourceOutput, Unit},
    format::format_query,
    identity::question_id,
    parse::parse,
};

fn catalog() -> Catalog {
    let mut catalog = Catalog::new("sessions");
    for unit in ["sessions", "events"] {
        catalog.units.push(Unit {
            name: unit.into(),
            singular: if unit == "sessions" {
                Some("conversation".into())
            } else {
                None
            },
            fields: vec![
                Field {
                    name: "id".into(),
                    kind: FieldKind::Reference,
                    unit: Some(unit.into()),
                    ..Default::default()
                },
                Field {
                    name: "title".into(),
                    kind: FieldKind::Text,
                    ..Default::default()
                },
                Field {
                    name: "n".into(),
                    kind: FieldKind::Integer,
                    ..Default::default()
                },
            ],
            ..Default::default()
        });
    }
    catalog
}
fn checked(text: &str, catalog: &Catalog) -> BoundQuery {
    bind(&parse(text, catalog).unwrap(), catalog).unwrap()
}

#[test]
fn native_membership_accepts_projected_rhs_and_preserves_left_columns() {
    let catalog = catalog();
    for (name, kind) in [
        ("intersect", Combination::Intersect),
        ("except", Combination::Except),
    ] {
        let query = checked(
            &format!(
                "sessions | {name} (sessions where n > 0 | fields id) | where title:open | fields id,title"
            ),
            &catalog,
        );
        assert!(
            matches!(&query.query().transforms[0].kind,TransformKind::Combine{how,..} if *how==kind)
        );
        let scope = query
            .typing()
            .queries
            .iter()
            .find(|query| query.path.0.is_empty())
            .unwrap();
        assert_eq!(scope.transforms[0].after.unit.as_deref(), Some("sessions"));
        assert!(scope.transforms[0].after.field("title").is_some());
        let rendered = format_query(query.query(), &catalog).unwrap();
        let round_trip = checked(&rendered, &catalog);
        assert_eq!(
            question_id(query.query(), &catalog, &Default::default()).unwrap(),
            question_id(round_trip.query(), &catalog, &Default::default()).unwrap()
        );
        let projected = checked(
            &format!("sessions | fields title | {name} (conversation | fields id)"),
            &catalog,
        );
        let scope = projected
            .typing()
            .queries
            .iter()
            .find(|query| query.path.0.is_empty())
            .unwrap();
        assert!(scope.transforms[1].after.field("title").is_some());
        assert!(scope.transforms[1].after.field("id").is_none());
        assert!(
            parse(
                &format!("sessions | fields title | {name} (sessions | fields id) | where id:one"),
                &catalog
            )
            .is_err()
        );
    }
}

#[test]
fn native_membership_uses_the_saved_outputs_actual_domain() {
    let mut catalog = catalog();
    catalog.source_outputs.push(SourceOutput {
        source: OutputSource::Saved {
            name: "chosen".into(),
            arguments: vec![],
        },
        schema: OutputSchema {
            unit: Some("sessions".into()),
            fields: vec![Field {
                name: "id".into(),
                kind: FieldKind::Reference,
                unit: Some("sessions".into()),
                ..Default::default()
            }],
            ..Default::default()
        },
    });
    for operation in ["intersect", "except"] {
        checked(
            &format!("sessions | {operation} (query:chosen) | fields title"),
            &catalog,
        );
        catalog.source_outputs[0].schema.unit = Some("events".into());
        assert!(parse(&format!("sessions | {operation} (query:chosen)"), &catalog).is_err());
        catalog.source_outputs[0].schema.unit = None;
        assert!(parse(&format!("sessions | {operation} (query:chosen)"), &catalog).is_err());
        catalog.source_outputs[0].schema.unit = Some("sessions".into());
    }
}

#[test]
fn native_membership_preserves_projected_compatibility_and_union_checks() {
    let catalog = catalog();
    for operation in ["intersect", "except"] {
        for text in [
            format!("sessions | {operation} (events | fields id)"),
            format!("sessions | group by title | {operation} (sessions | fields id)"),
        ] {
            assert!(parse(&text, &catalog).is_err(), "{text}");
        }
        // Deliberately equal projected values across domains remain supported.
        checked(
            &format!("sessions | fields title | {operation} (events | fields title)"),
            &catalog,
        );
    }
    assert!(parse("sessions | union (sessions | fields id)", &catalog).is_err());
    checked(
        "sessions | fields title | union (events | fields title)",
        &catalog,
    );
}
