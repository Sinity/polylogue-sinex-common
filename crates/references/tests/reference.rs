//! The reference grammar: global and local forms, the catalog's checks, input
//! classification and row picks.

use proptest::prelude::*;
use references::{
    Catalog, ContextWord, Input, Kind, Locator, Object, Product, ProductCatalog, Reference,
    ReferenceError, RowPick, RowSpan, Selector, Target, UrlPattern,
};

fn polylogue() -> ProductCatalog {
    ProductCatalog::new(
        Product::Polylogue,
        vec![
            Kind::new("session", &["report", "share", "trace"]),
            Kind::new("message", &[]),
            Kind::new("assertion", &["history"]),
            Kind::new("page", &["history"]),
            Kind::new("result", &[]),
            Kind::new("input", &[]),
        ],
    )
    .with_patterns(vec![
        UrlPattern::new("chatgpt.com", "/c/{id}", &[], "session:chatgpt:{id}").unwrap(),
    ])
}

fn sinex() -> ProductCatalog {
    ProductCatalog::new(
        Product::Sinex,
        vec![
            Kind::new("ev", &["trace"]),
            Kind::new("record", &[]),
            Kind::new("material", &[]),
            Kind::new("input", &[]),
            Kind::new("item", &[]),
        ],
    )
}

fn catalog(home: Product) -> Catalog {
    Catalog::new(home, vec![polylogue(), sinex()]).unwrap()
}

fn object(product: Product, kind: &str, id: &str, revision: Option<&str>) -> Reference {
    Reference::object(Object::new(product, kind, id, revision).unwrap())
}

#[test]
fn global_forms_parse_without_a_catalog() {
    let reference = Reference::parse("sinex:ev/0190a1b2").unwrap();
    assert_eq!(reference, object(Product::Sinex, "ev", "0190a1b2", None));
    let reference = Reference::parse("polylogue:assertion/r1@3#exact=hello&b=4-9").unwrap();
    let Target::Object(target) = reference.target() else {
        panic!("an object");
    };
    assert_eq!(target.revision(), Some("3"));
    assert_eq!(
        reference.global(),
        "polylogue:assertion/r1@3#b=4-9&exact=hello"
    );
    // Sinex's record coordinates keep their slashes inside the identity.
    let record = Reference::parse("sinex:record/m1/120/shell.command/0").unwrap();
    let Target::Object(record) = record.target() else {
        panic!("an object");
    };
    assert_eq!(record.id(), "m1/120/shell.command/0");
}

#[test]
fn local_forms_need_their_product() {
    assert!(matches!(
        Reference::parse("session:claude-code:abc"),
        Err(ReferenceError::LocalForm { .. })
    ));
    let polylogue = catalog(Product::Polylogue);
    assert_eq!(
        polylogue.parse("session:claude-code:abc").unwrap(),
        object(Product::Polylogue, "session", "claude-code:abc", None)
    );
}

#[test]
fn local_equals_global() {
    for home in Product::ALL {
        let catalog = catalog(home);
        for text in ["input:mail-1", "input:mail-1@4#L3"] {
            let local = catalog.parse(text).unwrap();
            let global = catalog
                .parse(&format!("{home}:{}", text.replacen(':', "/", 1)))
                .unwrap();
            assert_eq!(local, global);
            assert_eq!(local.written_in(home), text);
            assert_eq!(Reference::parse(&local.global()).unwrap(), local);
        }
    }
}

#[test]
fn other_products_kind_is_refused_with_the_global_form() {
    let polylogue = catalog(Product::Polylogue);
    let error = polylogue.parse("ev:0190").unwrap_err();
    assert_eq!(
        error,
        ReferenceError::OtherProductKind {
            kind: "ev".into(),
            home: Product::Polylogue,
            owner: Product::Sinex,
            suggestion: "sinex:ev/0190".into(),
        }
    );
    assert!(matches!(
        polylogue.parse("sinex:session/x"),
        Err(ReferenceError::UnknownKind {
            product: Product::Sinex,
            ..
        })
    ));
    // A product whose catalog is absent is not checked.
    let alone = Catalog::new(Product::Polylogue, vec![polylogue_only()]).unwrap();
    assert!(alone.parse("sinex:anything/x").is_ok());
}

fn polylogue_only() -> ProductCatalog {
    polylogue()
}

#[test]
fn written_in_another_product_is_global() {
    let reference = object(Product::Sinex, "ev", "e1", None);
    assert_eq!(reference.written_in(Product::Sinex), "ev:e1");
    assert_eq!(reference.written_in(Product::Polylogue), "sinex:ev/e1");
}

#[test]
fn identities_escape_the_grammar_characters() {
    let reference = object(Product::Polylogue, "input", "a@b#c%d e", Some("r@1"));
    let written = reference.global();
    assert_eq!(written, "polylogue:input/a%40b%23c%25d%20e@r@1");
    assert_eq!(Reference::parse(&written).unwrap(), reference);
    // A `%` that starts no escape is a literal `%`.
    assert_eq!(
        Reference::parse("polylogue:input/100%").unwrap(),
        object(Product::Polylogue, "input", "100%", None)
    );
}

#[test]
fn malformed_references_are_refused() {
    for (text, expected) in [
        ("no-kind", "NoKind"),
        ("sinex:ev", "NoKindAfterProduct"),
        ("sinex:ev/", "EmptyId"),
        ("sinex:ev/x@", "EmptyRevision"),
        ("Session:x", "BadKind"),
        ("sinex:ev/x#nonsense", "Selector"),
    ] {
        let error = Reference::parse(text).unwrap_err();
        assert!(
            format!("{error:?}").starts_with(expected),
            "{text}: {error:?}"
        );
    }
    assert!(matches!(
        Object::new(Product::Sinex, "url", "x", None),
        Err(ReferenceError::BadKind(_))
    ));
}

#[test]
fn catalog_refuses_reserved_and_repeated_kinds() {
    let reserved = ProductCatalog::new(Product::Sinex, vec![Kind::new("file", &[])]);
    assert!(Catalog::new(Product::Sinex, vec![reserved]).is_err());
    let repeated = ProductCatalog::new(
        Product::Sinex,
        vec![Kind::new("ev", &[]), Kind::new("ev", &[])],
    );
    assert!(Catalog::new(Product::Sinex, vec![repeated]).is_err());
    assert!(Catalog::new(Product::Sinex, vec![sinex(), sinex()]).is_err());
    let bad_web = sinex().with_web("not a url");
    assert!(Catalog::new(Product::Sinex, vec![bad_web]).is_err());
}

#[test]
fn input_is_classified() {
    let polylogue = catalog(Product::Polylogue);
    let read = |text: &str| polylogue.read_input(text).unwrap();
    assert_eq!(read("self"), Input::Context(ContextWord::CallingSession));
    assert_eq!(read("here"), Input::Context(ContextWord::Here));
    assert_eq!(read(" last "), Input::Context(ContextWord::Last));
    assert_eq!(read("@last"), Input::Rows(RowPick::LastAnswer));
    assert_eq!(read("@*"), Input::Rows(RowPick::All));
    assert_eq!(
        read("@3.2,2-5,7"),
        Input::Rows(RowPick::Rows(vec![
            RowSpan::Hit { row: 3, hit: 2 },
            RowSpan::Range { first: 2, last: 5 },
            RowSpan::Row(7),
        ]))
    );
    assert!(polylogue.read_input("@0").is_err());
    assert!(polylogue.read_input("@5-2").is_err());
    assert_eq!(read("7f3a2c10"), Input::Bare("7f3a2c10".into()));
    assert_eq!(
        read("claude-code:7f3a2c10"),
        Input::Bare("claude-code:7f3a2c10".into())
    );
    assert_eq!(
        read("session:claude-code:7f3a2c10"),
        Input::Reference(object(
            Product::Polylogue,
            "session",
            "claude-code:7f3a2c10",
            None
        ))
    );
    assert_eq!(
        read("sinex:ev/e1"),
        Input::Reference(object(Product::Sinex, "ev", "e1", None))
    );
    assert_eq!(
        read("https://youtu.be/dQw4w9WgXcQ"),
        Input::Reference(Reference::locator(
            Locator::parse("youtube:dQw4w9WgXcQ").unwrap()
        ))
    );
    assert!(polylogue.read_input("ev:e1").is_err());
}

#[test]
fn provider_urls_become_product_references() {
    for home in Product::ALL {
        let catalog = catalog(home);
        assert_eq!(
            catalog
                .read_url("https://chatgpt.com/c/abc-123?model=x")
                .unwrap(),
            object(Product::Polylogue, "session", "chatgpt:abc-123", None)
        );
    }
    // Without a matching pattern a URL is a `url:` locator.
    let reference = catalog(Product::Sinex)
        .read_url("https://chatgpt.com/g/abc")
        .unwrap();
    assert_eq!(reference.global(), "url:https://chatgpt.com/g/abc");
}

#[test]
fn row_picks_print_back() {
    for text in ["@last", "@*", "@3", "@3.2", "@2-5", "@1,4", "@1,2-3,4.5"] {
        assert_eq!(RowPick::parse(text).unwrap().to_string(), text);
    }
}

#[test]
fn serde_uses_the_global_form() {
    let reference = object(Product::Sinex, "ev", "e1", None)
        .with_selector(Some(Selector::parse("t=3").unwrap()));
    let json = serde_json::to_string(&reference).unwrap();
    assert_eq!(json, "\"sinex:ev/e1#t=3\"");
    assert_eq!(serde_json::from_str::<Reference>(&json).unwrap(), reference);
    assert!(serde_json::from_str::<Reference>("\"ev:e1\"").is_err());
}

fn identity() -> impl Strategy<Value = String> {
    // Every character class the grammar treats specially, plus ordinary text.
    proptest::collection::vec(
        prop_oneof![
            "[a-z0-9]{1,4}",
            Just(":".to_owned()),
            Just("/".to_owned()),
            Just("@".to_owned()),
            Just("#".to_owned()),
            Just("%".to_owned()),
            Just("%41".to_owned()),
            Just(" ".to_owned()),
            Just("ä".to_owned()),
            Just("&".to_owned()),
        ],
        1..8,
    )
    .prop_map(|parts| parts.concat())
}

fn product() -> impl Strategy<Value = Product> {
    prop_oneof![Just(Product::Polylogue), Just(Product::Sinex)]
}

proptest! {
    #[test]
    fn round_trip(
        product in product(),
        kind in "[a-z][a-z0-9_-]{0,6}",
        id in identity(),
        revision in proptest::option::of(identity()),
        lines in proptest::option::of((1u64..50, 0u64..50)),
    ) {
        prop_assume!(Object::new(product, &kind, &id, revision.as_deref()).is_ok());
        let selector = lines.map(|(first, extra)| Selector::parse(&format!("L{first}-{}", first + extra)).unwrap());
        let reference = object(product, &kind, &id, revision.as_deref()).with_selector(selector);
        let global = reference.global();
        prop_assert_eq!(&Reference::parse(&global).unwrap(), &reference);
        prop_assert_eq!(Reference::parse(&global).unwrap().global(), global);
        // Written locally in its own product, it parses to the same reference.
        let home = Catalog::new(product, vec![ProductCatalog::new(product, vec![Kind::new(&kind, &[])])]).unwrap();
        let local = reference.written_in(product);
        prop_assert_eq!(home.parse(&local).unwrap(), reference);
    }
}
