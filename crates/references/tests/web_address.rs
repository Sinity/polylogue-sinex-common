//! The route catalog: web addresses from references and back.

use proptest::prelude::*;
use references::{
    Catalog, Kind, Link, Locator, Object, Product, ProductCatalog, Reference, Selector, Unlinked,
    WebAddress,
};

fn catalog(home: Product, sinex_web: bool) -> Catalog {
    let polylogue = ProductCatalog::new(
        Product::Polylogue,
        vec![
            Kind::new("session", &["report", "share", "trace"]),
            Kind::new("assertion", &["history"]),
            Kind::new("page", &["history"]),
        ],
    )
    .with_web("http://127.0.0.1:8765/");
    let mut sinex = ProductCatalog::new(
        Product::Sinex,
        vec![
            Kind::new("ev", &["trace"]),
            Kind::new("record", &[]),
            Kind::new("item", &[]),
        ],
    );
    if sinex_web {
        sinex = sinex.with_web("https://sinex.example/web");
    }
    Catalog::new(home, vec![polylogue, sinex]).unwrap()
}

fn object(product: Product, kind: &str, id: &str, revision: Option<&str>) -> Reference {
    Reference::object(Object::new(product, kind, id, revision).unwrap())
}

#[test]
fn urls_follow_from_references() {
    let catalog = catalog(Product::Polylogue, true);
    let session = object(
        Product::Polylogue,
        "session",
        "claude-code:7f3a2c10-0002",
        None,
    );
    assert_eq!(
        catalog.link(&session),
        Link::Url("http://127.0.0.1:8765/session/claude-code:7f3a2c10-0002".into())
    );
    assert_eq!(
        catalog.web_address(&session, Some("trace")).unwrap(),
        "http://127.0.0.1:8765/session/claude-code:7f3a2c10-0002/trace"
    );
    let revision = object(Product::Polylogue, "assertion", "r1", Some("3"));
    assert_eq!(
        catalog.web_address(&revision, None).unwrap(),
        "http://127.0.0.1:8765/assertion/r1@3"
    );
    let record = object(Product::Sinex, "record", "m1/120/shell.command", None)
        .with_selector(Some(Selector::parse("b=0-10").unwrap()));
    assert_eq!(
        catalog.web_address(&record, None).unwrap(),
        "https://sinex.example/web/record/m1%2F120%2Fshell.command#b=0-10"
    );
    assert_eq!(
        catalog.web_address(&session, Some("raw")),
        Err(Unlinked::UnknownView {
            kind: "session".into(),
            view: "raw".into()
        })
    );
}

#[test]
fn unconfigured_product_is_text() {
    let catalog = catalog(Product::Polylogue, false);
    let event = object(Product::Sinex, "ev", "e1", None);
    let link = catalog.link(&event);
    assert_eq!(link, Link::Text(Unlinked::NotConfigured(Product::Sinex)));
    let Link::Text(reason) = link else {
        unreachable!()
    };
    assert_eq!(reason.to_string(), "Sinex not configured");
    let file = Reference::locator(Locator::parse("file:h/r/notes.md").unwrap());
    assert_eq!(catalog.link(&file), Link::Text(Unlinked::NoWebView));
    let video = Reference::locator(Locator::parse("youtube:dQw4w9WgXcQ#t=30").unwrap());
    assert_eq!(
        catalog.link(&video),
        Link::Url("https://www.youtube.com/watch?v=dQw4w9WgXcQ#t=30".into())
    );
}

#[test]
fn addresses_read_back() {
    let catalog = catalog(Product::Sinex, true);
    let route = catalog
        .read_web_address("http://127.0.0.1:8765/session/claude-code:abc/report?view=changes#L3")
        .unwrap();
    assert_eq!(
        route,
        WebAddress {
            reference: object(Product::Polylogue, "session", "claude-code:abc", None)
                .with_selector(Some(Selector::parse("L3").unwrap())),
            view: Some("report".into()),
            parameters: Some("view=changes".into()),
        }
    );
    assert_eq!(
        catalog.read_web_address("/ev/e1").unwrap().reference,
        object(Product::Sinex, "ev", "e1", None)
    );
    // A pasted web address of a product is that product's reference.
    assert_eq!(
        catalog
            .read_url("https://sinex.example/web/item/i9")
            .unwrap(),
        object(Product::Sinex, "item", "i9", None)
    );
    for not_a_route in [
        "/status/capture",
        "/ev",
        "/ev/e1/unknown-view",
        "/ev/e1/trace/more",
        "/q?q=x",
        "https://elsewhere.example/ev/e1",
        "https://sinex.example/webev/e1",
    ] {
        assert_eq!(catalog.read_web_address(not_a_route), None, "{not_a_route}");
    }
}

fn identity() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9]{1,4}",
            Just(":".to_owned()),
            Just("/".to_owned()),
            Just("@".to_owned()),
            Just("#".to_owned()),
            Just("?".to_owned()),
            Just("%".to_owned()),
            Just(" ".to_owned()),
            Just("ł".to_owned()),
        ],
        1..8,
    )
    .prop_map(|parts| parts.concat())
}

proptest! {
    #[test]
    fn web_address_round_trip(
        sinex in any::<bool>(),
        id in identity(),
        revision in proptest::option::of(identity()),
        view in proptest::option::of(Just("trace")),
        time in proptest::option::of(1u64..500),
    ) {
        let (product, kind) = if sinex { (Product::Sinex, "ev") } else { (Product::Polylogue, "session") };
        let selector = time.map(|t| Selector::parse(&format!("t={t}")).unwrap());
        let reference = object(product, kind, &id, revision.as_deref()).with_selector(selector);
        let catalog = catalog(Product::Polylogue, true);
        let url = catalog.web_address(&reference, view).unwrap();
        let route = catalog.read_web_address(&url).unwrap();
        prop_assert_eq!(route.reference, reference);
        prop_assert_eq!(route.view.as_deref(), view);
        prop_assert_eq!(route.parameters, None);
    }
}
