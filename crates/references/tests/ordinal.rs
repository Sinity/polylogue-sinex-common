use proptest::prelude::*;
use references::{
    Catalog, Input, Kind, LocalForm, Locator, Object, Product, ProductCatalog, Reference, Selector,
    Target, UrlPattern,
};

fn catalog() -> Catalog {
    Catalog::new(
        Product::Polylogue,
        vec![
            ProductCatalog::new(
                Product::Polylogue,
                vec![
                    Kind::new("block", &[]).with_local_form(LocalForm::Ordinal),
                    Kind::new("result", &[]).with_local_form(LocalForm::OptionalOrdinal),
                    Kind::new("variant", &[]).with_local_form(LocalForm::RevisionOrdinal),
                ],
            )
            .with_web("https://polylogue.example")
            .with_patterns(vec![
                UrlPattern::new(
                    "results.example",
                    "/{id}/{row}",
                    &[],
                    "result:{id}#ordinal={row}",
                )
                .unwrap(),
                UrlPattern::new(
                    "literal-results.example",
                    "/{id}/{row}",
                    &[],
                    "result:{id}#ordinal={row}",
                )
                .unwrap()
                .with_literal_capture("id")
                .unwrap(),
            ]),
        ],
    )
    .unwrap()
}

#[test]
fn old_local_ordinals_keep_their_exact_identity_fields() {
    let catalog = catalog();
    for (local, kind, id, revision, ordinal, global) in [
        (
            "block:codex:m#2",
            "block",
            "codex:m",
            None,
            Some(2),
            "polylogue:block/codex:m#ordinal=2",
        ),
        (
            "result:kept#0",
            "result",
            "kept",
            None,
            Some(0),
            "polylogue:result/kept#ordinal=0",
        ),
        (
            "result:kept",
            "result",
            "kept",
            None,
            None,
            "polylogue:result/kept",
        ),
        (
            "result:k%40raw#0",
            "result",
            "k%40raw",
            None,
            Some(0),
            "polylogue:result/k%2540raw#ordinal=0",
        ),
        (
            "result:k@raw#0",
            "result",
            "k@raw",
            None,
            Some(0),
            "polylogue:result/k%40raw#ordinal=0",
        ),
        (
            "variant:root@v1#3",
            "variant",
            "root",
            Some("v1"),
            Some(3),
            "polylogue:variant/root@v1#ordinal=3",
        ),
        (
            "variant:root@v1",
            "variant",
            "root",
            Some("v1"),
            None,
            "polylogue:variant/root@v1",
        ),
        (
            "variant:root",
            "variant",
            "root",
            None,
            None,
            "polylogue:variant/root",
        ),
        (
            "variant:root@part@v1#0",
            "variant",
            "root@part",
            Some("v1"),
            Some(0),
            "polylogue:variant/root%40part@v1#ordinal=0",
        ),
    ] {
        let parsed = catalog.parse(local).unwrap();
        let Target::Object(object) = parsed.target() else {
            panic!("object expected")
        };
        assert_eq!(
            (
                object.product(),
                object.kind(),
                object.id(),
                object.revision()
            ),
            (Product::Polylogue, kind, id, revision)
        );
        assert_eq!(parsed.selector().and_then(|s| s.ordinal), ordinal);
        assert_eq!(parsed.global(), global);
        assert_eq!(catalog.written(&parsed), local);
        assert_eq!(Reference::parse(global).unwrap(), parsed);
        assert_eq!(
            serde_json::from_str::<Reference>(&serde_json::to_string(&parsed).unwrap()).unwrap(),
            parsed
        );
    }
}

#[test]
fn local_and_global_bad_ordinals_are_refused() {
    let catalog = catalog();
    for bad in ["", "01", "+1", "-1", "x", "18446744073709551616"] {
        assert!(catalog.parse(&format!("block:codex:m#{bad}")).is_err());
        assert!(catalog.parse(&format!("result:k#{bad}")).is_err());
        assert!(catalog.parse(&format!("variant:r@v#{bad}")).is_err());
        assert!(Reference::parse(&format!("polylogue:block/codex:m#ordinal={bad}")).is_err());
    }
    for bad in ["block:codex:m", "block:#0", "variant:r@#0", "variant:@v#0"] {
        assert!(catalog.parse(bad).is_err(), "{bad}");
    }
    assert!(Selector::parse("ordinal=0&ordinal=0").is_err());
}

#[test]
fn an_ordinal_and_text_range_survive_real_catalog_routes() {
    let catalog = catalog();
    let global = "polylogue:block/codex:m#ordinal=2&b=0-4";
    let reference = Reference::parse(global).unwrap();
    let selector = reference.selector().unwrap();
    assert_eq!(selector.ordinal, Some(2));
    assert_eq!(
        (
            selector.text_range.as_ref().unwrap().start(),
            selector.text_range.as_ref().unwrap().end()
        ),
        (0, 4)
    );
    let url = catalog.web_address(&reference, None).unwrap();
    assert_eq!(
        url,
        "https://polylogue.example/block/codex:m#ordinal=2&b=0-4"
    );
    assert_eq!(
        catalog.read_input(&url).unwrap(),
        Input::Reference(reference.clone())
    );
    assert_eq!(catalog.written(&reference), format!("<{global}>"));
    assert_eq!(
        catalog.parse(&catalog.written(&reference)).unwrap(),
        reference
    );
}

#[test]
fn producer_patterns_keep_the_ordinal_when_fragments_add_text_ranges() {
    let catalog = catalog();
    for (url, id, range) in [
        ("https://results.example/a%2Fb/0", "a/b", None),
        ("https://results.example/a%2Fb/0#b=0-4", "a/b", Some((0, 4))),
        (
            "https://literal-results.example/a%2fb/0#b=0-4",
            "a%2fb",
            Some((0, 4)),
        ),
    ] {
        let parsed = catalog.read_url(url).unwrap();
        let Target::Object(object) = parsed.target() else {
            panic!("object expected")
        };
        assert_eq!((object.kind(), object.id()), ("result", id));
        assert_eq!(parsed.selector().unwrap().ordinal, Some(0));
        assert_eq!(
            parsed
                .selector()
                .unwrap()
                .text_range
                .as_ref()
                .map(|r| (r.start(), r.end())),
            range
        );
    }
    assert!(
        catalog
            .read_url("https://results.example/k/0#ordinal=1")
            .is_err()
    );
}

#[test]
fn locator_web_addresses_keep_ordinals_beside_named_places() {
    let address = "https://docs.example/a#ordinal=2&id=section";
    let located = Locator::from_url(address).unwrap();
    assert_eq!(
        located
            .locator
            .web_url(located.selector.as_ref())
            .as_deref(),
        Some(address)
    );
    let selected = located.selector.unwrap();
    assert_eq!(selected.ordinal, Some(2));
    assert_eq!(selected.id.as_deref(), Some("section"));
}

proptest! {
    #[test]
    fn opaque_member_identities_round_trip_without_reinterpreting_punctuation(
        kind in prop::sample::select(vec!["block", "result", "variant"]),
        id in prop::collection::vec(any::<char>(), 1..40).prop_map(|chars| chars.into_iter().collect::<String>()),
        revision in prop::option::of(prop::collection::vec(any::<char>(), 1..20).prop_map(|chars| chars.into_iter().collect::<String>())),
        ordinal in any::<u64>(),
    ) {
        let revision = if kind == "variant" { revision } else { None };
        let reference = Reference::object(Object::new(Product::Polylogue, kind, &id, revision.as_deref()).unwrap())
            .with_selector(Some(Selector { ordinal: Some(ordinal), ..Default::default() }));
        let catalog = catalog();
        prop_assert_eq!(catalog.parse(&catalog.written(&reference)).unwrap(), reference.clone());
        prop_assert_eq!(catalog.read_input(&catalog.written(&reference)).unwrap(), Input::Reference(reference.clone()));
        prop_assert_eq!(Reference::parse(&reference.global()).unwrap(), reference);
    }
}
