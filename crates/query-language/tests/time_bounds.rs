mod support;
use jiff::{Timestamp, civil::Time as CivilTime, tz::TimeZone};
use query_language::{
    ast::{ConditionKind, Operand, Query, Source, Value},
    format::format_query,
    identity::{normal_form, question_id},
    parse::parse,
    time::{self, Clock, DaySetting, Time},
};
use std::collections::BTreeMap;

fn condition(query: &Query) -> &ConditionKind {
    match &query.source {
        Source::Unit {
            condition: Some(condition),
            ..
        } => &condition.kind,
        source => panic!("expected a unit condition: {source:?}"),
    }
}
fn ts(value: &str) -> Timestamp {
    value.parse().unwrap()
}

#[test]
fn both_catalogs_preserve_keyword_and_colon_bounds_as_typed_nodes() {
    for catalog in [support::polylogue(), support::sinex()] {
        let unit = catalog.default_unit.clone();
        let main_time = catalog.unit(&unit).unwrap().main_time.as_ref().unwrap();
        for (keyword, until) in [("since", false), ("until", true)] {
            for value in ["2026-10-05", "2026-10-05T12:00Z", "7d", "\"this week\""] {
                let keyword_query =
                    parse(&format!("{unit} where {keyword} {value}"), &catalog).unwrap();
                let colon_query =
                    parse(&format!("{unit} where {keyword}:{value}"), &catalog).unwrap();
                assert_eq!(
                    normal_form(&keyword_query, &catalog).unwrap(),
                    normal_form(&colon_query, &catalog).unwrap()
                );
                let (operand, parsed_value) = match (until, condition(&keyword_query)) {
                    (false, ConditionKind::Since { operand, value })
                    | (true, ConditionKind::Until { operand, value }) => (operand, value),
                    (_, other) => panic!("bound lost its operation: {other:?}"),
                };
                assert_eq!(operand, &Operand::field(main_time));
                assert!(matches!(parsed_value, Value::Scalar { .. }));
                let rendered = format_query(&colon_query, &catalog).unwrap();
                assert!(rendered.contains(&format!("{keyword} ")));
                let reparsed = parse(&rendered, &catalog).unwrap();
                assert_eq!(
                    normal_form(&keyword_query, &catalog).unwrap(),
                    normal_form(&reparsed, &catalog).unwrap()
                );
            }
        }
    }
}

#[test]
fn bound_identity_is_distinct_from_explicit_comparison_and_survives_serde() {
    let catalog = support::sinex();
    for (bound, comparison) in [
        ("until 2026-10-05", "when < 2026-10-05"),
        ("since 2026-10-05", "when >= 2026-10-05"),
    ] {
        let bound = parse(&format!("events where {bound}"), &catalog).unwrap();
        let comparison = parse(&format!("events where {comparison}"), &catalog).unwrap();
        assert!(matches!(
            condition(&comparison),
            ConditionKind::Compare { .. }
        ));
        assert_ne!(
            question_id(&bound, &catalog, &BTreeMap::new()).unwrap(),
            question_id(&comparison, &catalog, &BTreeMap::new()).unwrap()
        );
        let decoded: Query = serde_json::from_value(serde_json::to_value(&bound).unwrap()).unwrap();
        assert_eq!(decoded, bound);
    }
    let a = parse("events until 2026-10-05T12:00Z", &catalog).unwrap();
    let b = parse("events until 2026-10-05T14:00+02:00", &catalog).unwrap();
    assert_eq!(
        question_id(&a, &catalog, &BTreeMap::new()).unwrap(),
        question_id(&b, &catalog, &BTreeMap::new()).unwrap()
    );
}

#[test]
fn date_until_uses_complete_logical_day_and_instant_until_stays_exclusive() {
    let clock = Clock::new(
        ts("2026-10-25T12:00Z"),
        DaySetting::new(
            TimeZone::get("Europe/Warsaw").unwrap(),
            CivilTime::constant(4, 0, 0, 0),
        )
        .unwrap(),
    );
    // Independent UTC goldens: this logical day includes the autumn fold.
    let day = time::resolve("2026-10-24", &clock).unwrap().value;
    assert_eq!(day.start(), ts("2026-10-24T02:00Z"));
    assert_eq!(day.until_bound(), ts("2026-10-25T03:00Z"));
    let until = day.until_bound();
    assert!(ts("2026-10-25T02:59:59Z") < until);
    assert!(ts("2026-10-25T03:00Z") >= until);
    // Explicit `when < DATE` uses day.start(), excluding the whole date.
    assert!(ts("2026-10-24T02:00Z") >= day.start());
    let instant = time::resolve("2026-10-24T12:00Z", &clock).unwrap().value;
    assert_eq!(instant.until_bound(), ts("2026-10-24T12:00Z"));
    assert_eq!(instant.start(), instant.until_bound());
    let span = Time::Span {
        start: ts("2026-10-24T01:00Z"),
        end: ts("2026-10-24T05:00Z"),
    };
    assert!(time::overlaps_bounds(span, Some(day.start()), Some(until)));
    assert!(!time::overlaps_bounds(
        Time::Instant { at: until },
        None,
        Some(until)
    ));
}

#[test]
fn bounds_keep_unit_visitors_and_formatter_field_bindings() {
    let catalog = support::mixed();
    let query = parse("pl.messages where until:\"sx.events\"", &catalog).unwrap();
    assert_eq!(query.namespaces().into_iter().collect::<Vec<_>>(), ["pl"]);
    let mut invalid = parse("pl.messages where since:7d", &catalog).unwrap();
    if let Source::Unit {
        condition: Some(condition),
        ..
    } = &mut invalid.source
    {
        let ConditionKind::Since { operand, .. } = &mut condition.kind else {
            panic!("expected since")
        };
        *operand = Operand::field("id");
    }
    assert!(format_query(&invalid, &catalog).is_err());
    let sinex = support::sinex();
    assert!(parse("queries where until 2026-10-05", &sinex).is_err());
}
