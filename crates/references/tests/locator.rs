//! Locators: the pinned shared cases, idempotence, catalog folding, no
//! guessing, and catalog web addresses that fold back.

use proptest::prelude::*;
use references::locator::VERSION;
use references::shared_cases::shared_locator_cases;
use references::{Locator, LocatorKind, PatternError, Selector, UrlPattern};

fn canonical(text: &str) -> String {
    Locator::parse(text)
        .unwrap_or_else(|error| panic!("{text}: {error}"))
        .to_string()
}

#[test]
fn locator_shared_module() {
    let table = shared_locator_cases();
    assert_eq!(
        table.version, VERSION,
        "the shared cases were written for another canonicalizer version"
    );
    assert!(table.cases.len() > 40);
    let mut wrong = Vec::new();
    for case in &table.cases {
        let got = Locator::parse(&case.input)
            .ok()
            .map(|located| located.to_string());
        if got != case.canonical {
            wrong.push(format!(
                "{:?}: expected {:?}, got {:?}",
                case.input, case.canonical, got
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "outputs changed without a new version:\n{}",
        wrong.join("\n")
    );
}

#[test]
fn locator_catalog_folds() {
    let objects: &[(&str, &[&str])] = &[
        (
            "youtube:dQw4w9WgXcQ",
            &[
                "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
                "https://youtube.com/watch?v=dQw4w9WgXcQ&list=PL1&index=2",
                "http://m.youtube.com/watch?feature=share&v=dQw4w9WgXcQ",
                "https://music.youtube.com/watch?v=dQw4w9WgXcQ",
                "https://youtu.be/dQw4w9WgXcQ",
                "https://www.youtube.com/embed/dQw4w9WgXcQ?start=10",
                "https://www.youtube.com/shorts/dQw4w9WgXcQ",
                "https://www.youtube.com/live/dQw4w9WgXcQ",
                "https://www.youtube.com/v/dQw4w9WgXcQ",
                "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
                "url:https://youtu.be/dQw4w9WgXcQ",
                "youtube:dQw4w9WgXcQ",
            ],
        ),
        (
            "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
            &[
                "https://open.spotify.com/track/4uLU6hMCjMI75M1A2tKUQC",
                "https://open.spotify.com/intl-pl/track/4uLU6hMCjMI75M1A2tKUQC?si=1",
                "https://open.spotify.com/embed/track/4uLU6hMCjMI75M1A2tKUQC",
                "spotify:track:4uLU6hMCjMI75M1A2tKUQC",
            ],
        ),
        (
            "doi:10.1038/nphys1170",
            &[
                "https://doi.org/10.1038/NPHYS1170",
                "http://dx.doi.org/10.1038/nphys1170",
                "https://www.doi.org/10.1038/nphys1170",
                "doi:10.1038/NPhys1170",
            ],
        ),
        (
            "isbn:9780306406157",
            &[
                "isbn:0306406152",
                "isbn:978 0 306 40615 7",
                "isbn:0-306-40615-2",
            ],
        ),
        (
            "arxiv:1706.03762",
            &[
                "https://arxiv.org/abs/1706.03762",
                "https://arxiv.org/pdf/1706.03762",
                "https://arxiv.org/pdf/1706.03762.pdf",
                "https://export.arxiv.org/abs/1706.03762",
                "arxiv:1706.03762",
            ],
        ),
        (
            "booru:safebooru/77",
            &[
                "https://safebooru.org/index.php?page=post&s=view&id=77",
                "https://safebooru.org/index.php?id=77&s=view&page=post&tags=x",
                "booru:safebooru/077",
            ],
        ),
        (
            "github:rust-lang/cargo",
            &[
                "https://github.com/rust-lang/cargo",
                "https://github.com/Rust-Lang/Cargo/",
                "https://www.github.com/rust-lang/cargo",
                "github:rust-lang/cargo.git",
            ],
        ),
        (
            "wikipedia:en/Rust_(programming_language)",
            &[
                "https://en.wikipedia.org/wiki/Rust_(programming_language)",
                "https://en.m.wikipedia.org/wiki/Rust_%28programming_language%29",
                "https://en.wikipedia.org/wiki/rust_(programming_language)?useskin=vector",
                "wikipedia:en/rust (programming language)",
                "wikipedia:EN/Rust__(programming_language)_",
            ],
        ),
    ];
    for (expected, forms) in objects {
        for form in *forms {
            assert_eq!(canonical(form), *expected, "{form}");
        }
    }
}

#[test]
fn catalog_web_address_folds_back() {
    for text in [
        "youtube:dQw4w9WgXcQ",
        "youtube:dQw4w9WgXcQ#t=30",
        "spotify:episode:4rOoJ6Egrf8K2IrywzwOMk",
        "doi:10.1002/(sici)1097-4571(199806)49:8<693::aid-asi4>3.0.co;2-o",
        "arxiv:hep-th/9901001v3",
        "booru:danbooru/1",
        "booru:gelbooru/2",
        "booru:safebooru/3",
        "booru:e621/4",
        "booru:yandere/5",
        "booru:konachan/6",
        "github:rust-lang/rust",
        "wikipedia:de/Ärger",
        "wikipedia:en/AC/DC",
        "wikipedia:en/What?#id=History",
    ] {
        let located = Locator::parse(text).unwrap();
        assert_eq!(located.to_string(), text);
        let address = located.locator.web_url(located.selector.as_ref()).unwrap();
        assert_eq!(Locator::from_url(&address).unwrap(), located, "{address}");
    }
    for no_address in ["isbn:9780306406157", "file:h/r/p", "part:gmail/m/1"] {
        assert_eq!(
            Locator::parse(no_address).unwrap().locator.web_url(None),
            None
        );
    }
    assert_eq!(
        Locator::parse("url:https://example.com/a#id=x%20y")
            .unwrap()
            .locator
            .web_url(Some(&Selector::parse("id=x%20y").unwrap())),
        Some("https://example.com/a#x%20y".into())
    );
}

#[test]
fn file_and_part_constructors() {
    assert_eq!(
        Locator::file("Laptop", "root-1", "src/./lib.rs")
            .unwrap()
            .to_string(),
        "file:laptop/root-1/src/lib.rs"
    );
    assert!(Locator::file("h", "r", "").is_err());
    assert!(Locator::file("h", "r", "a/../..").is_err());
    assert_eq!(
        Locator::part("chatgpt", "m1", "uploads/a b.png")
            .unwrap()
            .to_string(),
        "part:chatgpt/m1/uploads/a%20b.png"
    );
    assert!(Locator::part("chatgpt", "", "x").is_err());
    assert_eq!(LocatorKind::from_name("booru"), Some(LocatorKind::Booru));
    assert!(LocatorKind::Youtube.is_catalog() && !LocatorKind::File.is_catalog());
}

#[test]
fn patterns_are_checked_when_declared() {
    assert_eq!(
        UrlPattern::new("x.com", "/{a}/{a}", &[], "k:{a}"),
        Err(PatternError::CapturedTwice("a".into()))
    );
    assert!(matches!(
        UrlPattern::new("x.com", "/{a}", &[], "k:{b}"),
        Err(PatternError::UncapturedOutput { .. })
    ));
    assert!(matches!(
        UrlPattern::new("x.com", "a", &[], "k"),
        Err(PatternError::PathNotAbsolute(_))
    ));
    assert!(matches!(
        UrlPattern::new("x.com", "/{a*}/b", &[], "k:{a}"),
        Err(PatternError::RestNotLast(_))
    ));
    assert!(matches!(
        UrlPattern::new("x.com", "/{a}{b}", &[], "k"),
        Err(PatternError::TwoCaptures(_))
    ));
    assert!(matches!(
        UrlPattern::new("x.com", "/{a", &[], "k"),
        Err(PatternError::Unbalanced(_))
    ));
    let pattern = UrlPattern::new(
        "{site}.example.org",
        "/p/{id}.html",
        &[("lang", "{lang}")],
        "k:{site}/{lang}/{id}",
    )
    .unwrap();
    let url = url::Url::parse("https://docs.example.org/p/x.html?lang={id}").unwrap();
    // A captured value is never read as a placeholder.
    assert_eq!(pattern.apply(&url), Some("k:docs/{id}/x".into()));
    assert_eq!(
        pattern.apply(&url::Url::parse("https://docs.example.org/p/x.htm?lang=en").unwrap()),
        None
    );
}

fn host() -> impl Strategy<Value = String> {
    proptest::collection::vec("[a-zA-Z][a-zA-Z0-9]{0,6}", 2..4).prop_map(|labels| labels.join("."))
}

fn segment() -> impl Strategy<Value = String> {
    proptest::collection::vec(
        prop_oneof![
            "[a-zA-Z0-9._~-]{1,5}",
            Just("%7e".to_owned()),
            Just("%3f".to_owned()),
            Just("@".to_owned()),
            Just("é".to_owned())
        ],
        1..4,
    )
    .prop_map(|parts| parts.concat())
}

fn parameter() -> impl Strategy<Value = (String, String)> {
    ("[a-z]{1,4}", "[a-zA-Z0-9]{0,4}")
}

fn tracking() -> impl Strategy<Value = (String, String)> {
    (
        prop_oneof![
            Just("utm_source"),
            Just("utm_campaign"),
            Just("fbclid"),
            Just("gclid"),
            Just("msclkid")
        ],
        "[a-z0-9]{1,6}",
    )
        .prop_map(|(name, value)| (name.to_owned(), value))
}

fn url(
    scheme: &str,
    host: &str,
    port: &str,
    path: &[String],
    query: &[(String, String)],
) -> String {
    let query: Vec<String> = query
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{}", query.join("&"))
    };
    format!("{scheme}://{host}{port}/{}{query}", path.join("/"))
}

proptest! {
    #[test]
    fn locator_idempotent(
        scheme in prop_oneof![Just("http"), Just("https")],
        host in host(),
        path in proptest::collection::vec(segment(), 0..4),
        query in proptest::collection::vec(prop_oneof![parameter(), tracking()], 0..5),
        fragment in proptest::option::of(prop_oneof!["[a-z]{1,6}", Just("t=5,9".to_owned()), Just("L3-4".to_owned())]),
    ) {
        let mut text = url(scheme, &host, "", &path, &query);
        if let Some(fragment) = fragment {
            text = format!("{text}#{fragment}");
        }
        let once = Locator::parse(&text).unwrap();
        let printed = once.to_string();
        let twice = Locator::parse(&printed).unwrap();
        prop_assert_eq!(&twice, &once);
        prop_assert_eq!(twice.to_string(), printed);
    }

    #[test]
    fn locator_no_guess(
        host in host(),
        path in proptest::collection::vec(segment(), 1..4),
        mut query in proptest::collection::vec(parameter(), 0..4),
        noise in proptest::collection::vec(tracking(), 0..3),
        extra in parameter(),
    ) {
        // Distinct names, so reordering them is a permutation of distinct parameters.
        query.sort();
        query.dedup_by(|a, b| a.0 == b.0);
        let base = canonical(&url("https", &host.to_lowercase(), "", &path, &query));
        // Same thing: host case, default port, parameter order, tracking parameters.
        let mut shuffled: Vec<(String, String)> = query.iter().rev().cloned().collect();
        shuffled.extend(noise);
        prop_assert_eq!(canonical(&url("https", &host.to_uppercase(), ":443", &path, &shuffled)), base.clone());
        // Different thing: a parameter that is not on the tracking list.
        prop_assume!(!query.iter().any(|(name, _)| *name == extra.0));
        let mut more = query.clone();
        more.push(extra);
        prop_assert_ne!(canonical(&url("https", &host, "", &path, &more)), base);
    }
}
