use crate::{
    ast::Source,
    catalog::{Catalog, Unit},
};

#[test]
fn historical_native_source_and_identity_are_frozen_independently_of_current_linking() {
    let catalog = Catalog {
        default_unit: "events".into(),
        units: vec![Unit {
            name: "events".into(),
            ..Unit::default()
        }],
        unpiped_stages: true,
        ..Catalog::default()
    };
    let query = crate::parse::parse("recall yesterday", &catalog).unwrap();
    assert!(matches!(query.source, Source::Recall { .. }));
    assert_eq!(
        crate::identity::normal_form(&query, &catalog).unwrap(),
        serde_json::json!({
            "source":{"kind":"recall","time":{"kind":"scalar","text":"yesterday"},"window":null},
            "attach":[],"stages":[]
        })
    );
}
