//! Selectors: round trips, accepted spellings, bounds and finding moved spans.

use proptest::prelude::*;
use references::selector::{
    Bounds, LineRange, Rectangle, RectangleUnit, Seconds, SpanState, TextQuote, TextRange,
    TextUnit, TimeRange,
};
use references::{Selector, SelectorError};

fn parse(text: &str) -> Selector {
    Selector::parse(text).unwrap_or_else(|error| panic!("{text}: {error}"))
}

#[test]
fn selector_round_trip_of_other_spellings() {
    for (written, canonical) in [
        ("L40-L52", "L40-52"),
        ("L7-7", "L7"),
        ("L040", "L40"),
        ("t=npt:10,20", "t=10,20"),
        ("t=,20", "t=0,20"),
        ("t=0:01:30.250", "t=90.25"),
        ("t=01:30,02:00", "t=90,120"),
        ("t=10.", "t=10"),
        ("t=1.500,2", "t=1.5,2"),
        ("xywh=pixel:1,2,3,4", "xywh=1,2,3,4"),
        ("xywh=percent:0,0,50,100", "xywh=percent:0,0,50,100"),
        ("exact=a%20b&b=1-4", "b=1-4&exact=a%20b"),
        ("suffix=z&exact=y&prefix=x", "exact=y&prefix=x&suffix=z"),
        ("id=intro&t=1", "t=1&id=intro"),
        ("exact=100%", "exact=100%25"),
    ] {
        assert_eq!(parse(written).to_string(), canonical, "{written}");
        assert_eq!(parse(canonical).to_string(), canonical);
    }
}

#[test]
fn malformed_selectors_are_refused() {
    for text in [
        "",
        "L0",
        "L5-3",
        "t=20,10",
        "t=10,10",
        "t=1:5",
        "t=0:60",
        "t=abc",
        "t=smpte:00:00:01:00",
        "xywh=1,2,0,4",
        "xywh=1,2,3",
        "xywh=percent:60,0,50,10",
        "b=5-3",
        "b=x-3",
        "prefix=a",
        "exact=",
        "t=1&t=2",
        "b=1-2&c=1-2",
        "unknown=1",
        "chapter",
        "exact=%FF",
    ] {
        assert!(Selector::parse(text).is_err(), "{text} parsed");
    }
}

#[test]
fn selector_out_of_range_is_invalid_not_clamped() {
    let text = "one\ntwo\nthree\n";
    let bounds = Bounds {
        duration: Some(Seconds::new(60, "").unwrap()),
        width: Some(100),
        height: Some(50),
        ..Bounds::of_text(text)
    };
    assert_eq!(bounds.lines, Some(3));
    for inside in [
        "L1-3",
        "t=0,60",
        "t=59.9",
        "xywh=0,0,100,50",
        "b=0-14",
        "c=3-14",
        "u16=0-14",
    ] {
        assert_eq!(parse(inside).check(&bounds), Ok(()), "{inside}");
    }
    for outside in [
        "L3-4",
        "t=0,60.5",
        "t=60",
        "xywh=1,0,100,50",
        "xywh=0,1,100,50",
        "b=0-15",
        "c=0-15",
    ] {
        assert!(
            matches!(
                parse(outside).check(&bounds),
                Err(SelectorError::OutOfBounds { .. })
            ),
            "{outside}"
        );
    }
    // An unknown bound is not checked.
    assert_eq!(parse("L900&t=9999").check(&Bounds::default()), Ok(()));
}

#[test]
fn text_ranges_convert_between_units() {
    let text = "a😀b";
    let at = |unit, start, end| TextRange::new(unit, start, end).unwrap().byte_range(text);
    assert_eq!(at(TextUnit::Bytes, 1, 5), Some(1..5));
    assert_eq!(at(TextUnit::Bytes, 2, 5), None);
    assert_eq!(at(TextUnit::CodePoints, 1, 2), Some(1..5));
    assert_eq!(at(TextUnit::Utf16, 1, 3), Some(1..5));
    assert_eq!(at(TextUnit::Utf16, 2, 3), None);
    assert_eq!(at(TextUnit::CodePoints, 3, 3), Some(6..6));
    assert_eq!(at(TextUnit::CodePoints, 3, 4), None);
}

#[test]
fn quote_follows_moved_text() {
    let original = "The writer syncs once per group. Then it commits.";
    let start = original.find("syncs once").unwrap();
    let range = start..start + "syncs once".len();
    let quote = TextQuote::around(original, range.clone(), 8).unwrap();
    assert_eq!(quote.prefix(), " writer ");
    assert_eq!(quote.suffix(), " per gro");
    let selector = Selector {
        text_range: Some(
            TextRange::new(TextUnit::Bytes, range.start as u64, range.end as u64).unwrap(),
        ),
        quote: Some(quote),
        ..Selector::default()
    };
    let printed = parse(&selector.to_string());
    assert_eq!(printed, selector);
    assert_eq!(
        selector.span_in(original),
        Some(SpanState::Current(range.clone()))
    );
    let edited = format!("Note. {original}");
    assert_eq!(
        selector.span_in(&edited),
        Some(SpanState::Moved(range.start + 6..range.end + 6))
    );
    let changed = original.replace("syncs once", "syncs twice");
    assert_eq!(selector.span_in(&changed), Some(SpanState::ChangedSince));
    let twice = format!("{original} {original}");
    let shifted = Selector {
        text_range: Some(TextRange::new(TextUnit::Bytes, 0, 3).unwrap()),
        ..selector.clone()
    };
    assert_eq!(shifted.span_in(&twice), Some(SpanState::ChangedSince));
    assert_eq!(Selector::default().span_in(original), None);
}

#[test]
fn parts_check_their_own_rules() {
    assert!(LineRange::new(0, 1).is_err());
    assert!(
        TimeRange::new(
            Seconds::new(2, "").unwrap(),
            Some(Seconds::new(1, "9").unwrap())
        )
        .is_err()
    );
    assert!(Rectangle::new(RectangleUnit::Pixel, 0, 0, 0, 1).is_err());
    assert!(Rectangle::new(RectangleUnit::Percent, 50, 0, 51, 1).is_err());
    assert!(TextQuote::new("", "a", "b").is_err());
    assert!(Seconds::new(1, "5").unwrap() < Seconds::new(1, "51").unwrap());
    assert!(Seconds::new(1, "05").unwrap() < Seconds::new(1, "5").unwrap());
    assert_eq!(
        Seconds::new(1, "500").unwrap(),
        Seconds::new(1, "5").unwrap()
    );
}

fn seconds() -> impl Strategy<Value = Seconds> {
    (0u64..100_000, "[0-9]{0,4}")
        .prop_map(|(whole, fraction)| Seconds::new(whole, &fraction).unwrap())
}

fn selector() -> impl Strategy<Value = Selector> {
    let lines = proptest::option::of(
        (1u64..1000, 0u64..1000)
            .prop_map(|(first, extra)| LineRange::new(first, first + extra).unwrap()),
    );
    let time = proptest::option::of(
        (seconds(), proptest::option::of(seconds()))
            .prop_filter_map("end after start", |(start, end)| {
                TimeRange::new(start, end).ok()
            }),
    );
    let rectangle = proptest::option::of(
        (any::<bool>(), 0u64..50, 0u64..50, 1u64..50, 1u64..50).prop_map(
            |(percent, x, y, width, height)| {
                let unit = if percent {
                    RectangleUnit::Percent
                } else {
                    RectangleUnit::Pixel
                };
                Rectangle::new(unit, x, y, width, height).unwrap()
            },
        ),
    );
    let text_range = proptest::option::of(
        (
            prop_oneof![
                Just(TextUnit::Bytes),
                Just(TextUnit::CodePoints),
                Just(TextUnit::Utf16)
            ],
            0u64..500,
            0u64..500,
        )
            .prop_map(|(unit, start, extra)| TextRange::new(unit, start, start + extra).unwrap()),
    );
    let quote = proptest::option::of(
        ("\\PC{1,12}", "\\PC{0,6}", "\\PC{0,6}")
            .prop_map(|(exact, prefix, suffix)| TextQuote::new(&exact, &prefix, &suffix).unwrap()),
    );
    let id = proptest::option::of("\\PC{1,10}");
    (
        proptest::option::of(any::<u64>()),
        lines,
        time,
        rectangle,
        text_range,
        quote,
        id,
    )
        .prop_map(
            |(ordinal, lines, time, rectangle, text_range, quote, id)| Selector {
                ordinal,
                lines,
                time,
                rectangle,
                text_range,
                quote,
                id,
            },
        )
        .prop_filter("not empty", |selector| !selector.is_empty())
}

proptest! {
    #[test]
    fn selector_round_trip(selector in selector()) {
        let printed = selector.to_string();
        let parsed = Selector::parse(&printed).unwrap();
        prop_assert_eq!(&parsed, &selector);
        prop_assert_eq!(parsed.to_string(), printed);
    }

    #[test]
    fn quote_follows_moved_text_after_insertions(
        before in "[a-z ]{0,30}",
        span in "[A-Z]{1,8}",
        after in "[a-z ]{0,30}",
        inserted in "[a-z ]{0,30}",
    ) {
        let text = format!("{before}{span}{after}");
        let range = before.len()..before.len() + span.len();
        let selector = Selector {
            text_range: Some(TextRange::new(TextUnit::Bytes, range.start as u64, range.end as u64).unwrap()),
            quote: TextQuote::around(&text, range.clone(), 32),
            ..Selector::default()
        };
        prop_assert_eq!(selector.span_in(&text), Some(SpanState::Current(range.clone())));
        let edited = format!("{inserted}{text}");
        let expected = if inserted.is_empty() {
            SpanState::Current(range.clone())
        } else {
            let shifted = range.start + inserted.len()..range.end + inserted.len();
            // The quote's whole context names one place: the upper-case span
            // occurs once, so the match is unique.
            SpanState::Moved(shifted)
        };
        // Inserting text can leave the stored text range on the same letters by
        // chance; then the text range still holds the quote.
        let found = selector.span_in(&edited).unwrap();
        match found {
            SpanState::Current(range) => prop_assert_eq!(&edited[range], span.as_str()),
            other => prop_assert_eq!(other, expected),
        }
    }
}
