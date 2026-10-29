use jiff::{
    SignedDuration, Timestamp,
    civil::{Date, Time},
    tz::TimeZone,
};
use query_language::semantics::Truth;
use query_language::time::{
    self, Clock, DaySetting, MatchRank, Precision, ResolutionNote, TimeCondition, TimeRange,
    TimeValue,
};

fn ts(text: &str) -> Timestamp {
    text.parse().unwrap()
}
fn date(text: &str) -> Date {
    text.parse().unwrap()
}
fn setting(zone: &str, start: &str) -> DaySetting {
    DaySetting::new(TimeZone::get(zone).unwrap(), start.parse::<Time>().unwrap()).unwrap()
}
fn clock() -> Clock {
    Clock::new(
        ts("2026-10-07T10:00:00Z"),
        setting("Europe/Warsaw", "04:00"),
    )
}

#[test]
fn independent_natural_language_and_calendar_golden_table() {
    let c = clock();
    for (phrase, start, end) in [
        ("today", "2026-10-07T02:00Z", "2026-10-08T02:00Z"),
        ("yesterday", "2026-10-06T02:00Z", "2026-10-07T02:00Z"),
        ("this week", "2026-10-05T02:00Z", "2026-10-12T02:00Z"),
        ("last week", "2026-09-28T02:00Z", "2026-10-05T02:00Z"),
        ("this month", "2026-10-01T02:00Z", "2026-11-01T03:00Z"),
        ("last month", "2026-09-01T02:00Z", "2026-10-01T02:00Z"),
        ("last 3 days", "2026-10-05T02:00Z", "2026-10-08T02:00Z"),
        ("3 days ago", "2026-10-04T02:00Z", "2026-10-05T02:00Z"),
        ("monday", "2026-10-05T02:00Z", "2026-10-06T02:00Z"),
        ("last tuesday", "2026-10-06T02:00Z", "2026-10-07T02:00Z"),
        ("march", "2026-03-01T03:00Z", "2026-04-01T02:00Z"),
        ("2026-10", "2026-10-01T02:00Z", "2026-11-01T03:00Z"),
    ] {
        assert_eq!(
            time::resolve(phrase, &c).unwrap().value,
            time::Time::Span {
                start: ts(start),
                end: ts(end)
            },
            "{phrase}"
        );
    }
    assert_eq!(
        time::resolve("09:00", &c).unwrap().value.start(),
        ts("2026-10-07T07:00Z")
    );
    assert_eq!(
        time::resolve("01:30", &c).unwrap().value.start(),
        ts("2026-10-07T23:30Z")
    );
    assert_eq!(
        time::resolve("7d", &c).unwrap().value.start(),
        ts("2026-09-30T10:00Z")
    );
    assert_eq!(
        time::resolve("02/03/2026", &c).unwrap_err().kind,
        query_language::ErrorKind::Ambiguous
    );
}

#[test]
fn gaps_choose_first_existing_instant_and_folds_choose_earlier() {
    let s = setting("Europe/Warsaw", "02:30");
    let gap = s.local("2026-03-29T02:45".parse().unwrap()).unwrap();
    assert_eq!(gap.instant, ts("2026-03-29T01:00Z"));
    assert!(matches!(
        gap.notes.as_slice(),
        [ResolutionNote::SkippedTime { .. }]
    ));
    assert_eq!(
        s.start_of(date("2026-03-29")).unwrap(),
        ts("2026-03-29T01:00Z")
    );
    // 03:15 is already after the shifted 03:00 boundary, although before
    // the 03:30 that compatible disambiguation would have produced.
    assert_eq!(
        s.logical_day(ts("2026-03-29T01:15Z")).unwrap(),
        date("2026-03-29")
    );
    let fold = s.local("2026-10-25T02:45".parse().unwrap()).unwrap();
    assert_eq!(fold.instant, ts("2026-10-25T00:45Z"));
    assert!(matches!(
        fold.notes.as_slice(),
        [ResolutionNote::RepeatedTime { .. }]
    ));
    assert_eq!(
        s.logical_day(ts("2026-10-25T01:15Z")).unwrap(),
        date("2026-10-25")
    );
}

#[test]
fn independent_dst_lengths_and_skipped_date() {
    for (zone, start, label, seconds) in [
        ("Europe/Warsaw", "04:00", "2026-03-28", 23 * 3600),
        ("Europe/Warsaw", "04:00", "2026-10-24", 25 * 3600),
        (
            "Australia/Lord_Howe",
            "04:00",
            "2026-04-04",
            24 * 3600 + 1800,
        ),
        (
            "Australia/Lord_Howe",
            "04:00",
            "2026-10-03",
            24 * 3600 - 1800,
        ),
    ] {
        let s = setting(zone, start);
        assert_eq!(
            s.day(date(label)).unwrap().duration().as_secs(),
            seconds,
            "{zone} {label}"
        );
    }
    let s = setting("Pacific/Apia", "04:00");
    let skipped = s.day(date("2011-12-30")).unwrap();
    assert_eq!(skipped.start, skipped.end);
    assert_eq!(s.day(date("2011-12-29")).unwrap().end, skipped.start);
    assert_eq!(skipped.end, s.day(date("2011-12-31")).unwrap().start);
    assert_eq!(
        s.logical_day(ts("2011-12-30T12:00Z")).unwrap(),
        date("2011-12-29")
    );
    assert_eq!(
        s.days_between(date("2011-12-29"), date("2011-12-31"))
            .unwrap(),
        2
    );
}

#[test]
fn days_tile_time_and_instant_in_its_day_across_transition_tables() {
    for zone in [
        "Europe/Warsaw",
        "America/New_York",
        "Australia/Lord_Howe",
        "Pacific/Apia",
        "Asia/Kathmandu",
    ] {
        for start in ["00:00", "02:30", "04:00", "23:45"] {
            let s = setting(zone, start);
            for beginning in ["2026-03-25", "2026-10-22", "2011-12-27"] {
                let mut d = date(beginning);
                for _ in 0..8 {
                    let range = s.day(d).unwrap();
                    let next = d.tomorrow().unwrap();
                    assert_eq!(range.end, s.day(next).unwrap().start, "{zone} {start} {d}");
                    if range.start < range.end {
                        for at in [
                            range.start,
                            range.start.checked_add(range.duration() / 2).unwrap(),
                            range
                                .end
                                .checked_sub(SignedDuration::from_nanos(1))
                                .unwrap(),
                        ] {
                            assert_eq!(s.logical_day(at).unwrap(), d, "{zone} {start} {at}");
                            assert!(range.contains(at));
                        }
                    }
                    d = next;
                }
            }
        }
    }
}

#[test]
fn coarse_times_use_every_possible_instant_and_join_width() {
    let c = clock();
    let range = c.day.day(date("2026-10-07")).unwrap();
    let value = TimeValue::new(range.into(), Precision::Day, Some(c.day)).unwrap();
    assert_eq!(
        value.compare(&TimeCondition::AtOrAfter { at: range.start }),
        Truth::True
    );
    assert_eq!(
        value.compare(&TimeCondition::AtOrAfter {
            at: ts("2026-10-07T12:00Z")
        }),
        Truth::Unknown
    );
    assert_eq!(
        value.compare(&TimeCondition::AtOrAfter { at: range.end }),
        Truth::False
    );
    assert_eq!(
        value.compare(&TimeCondition::Before { at: range.end }),
        Truth::True
    );
    assert_eq!(value.compare(&TimeCondition::Within { range }), Truth::True);
    assert!(!value.join_eligible(SignedDuration::from_secs(60)));
    assert!(value.join_eligible(SignedDuration::from_hours(24)));
    assert!(TimeValue::instant(range.start).match_rank() < value.match_rank());
    assert_eq!(value.match_rank(), MatchRank::Coarse);
    assert_eq!(
        time::compare(None, &TimeCondition::AtOrAfter { at: range.start }),
        Truth::Unknown
    );
}

#[test]
fn independent_discrete_oracle_for_whole_period_comparison() {
    let origin = ts("2026-01-01T00:00Z");
    for width in 1..8 {
        let end = origin
            .checked_add(SignedDuration::from_nanos(width))
            .unwrap();
        let value = TimeValue::new(
            time::Time::Span { start: origin, end },
            Precision::Bounded,
            None,
        )
        .unwrap();
        for cut in -2..10 {
            let at = origin.checked_add(SignedDuration::from_nanos(cut)).unwrap();
            let truths: Vec<bool> = (0..width).map(|i| i >= cut).collect();
            let expected = if truths.iter().all(|&x| x) {
                Truth::True
            } else if truths.iter().all(|&x| !x) {
                Truth::False
            } else {
                Truth::Unknown
            };
            assert_eq!(value.compare(&TimeCondition::AtOrAfter { at }), expected);
        }
    }
}

#[test]
fn serialization_preserves_resolved_clock_and_rejects_invalid_periods() {
    let c = clock();
    assert_eq!(
        serde_json::from_str::<time::ClockWire>(&serde_json::to_string(&c).unwrap())
            .unwrap()
            .resolve(c.day.zone().clone())
            .unwrap(),
        c
    );
    let resolved = time::resolve("last month", &c).unwrap();
    assert_eq!(
        serde_json::from_str::<time::ResolvedTime>(&serde_json::to_string(&resolved).unwrap())
            .unwrap(),
        resolved
    );
    assert!(
        serde_json::from_str::<TimeRange>(
            r#"{"start":"2026-02-02T00:00Z","end":"2026-02-01T00:00Z"}"#
        )
        .is_err()
    );
    assert!(time::duration_ms("999999999999999999999999999999d").is_none());
    assert_eq!(time::duration_ms("1.5h"), Some(5_400_000));
}

#[test]
fn exact_relative_days_differ_from_calendar_days_at_dst() {
    let c = Clock::new(ts("2026-03-29T10:00Z"), setting("Europe/Warsaw", "04:00"));
    assert_eq!(
        time::resolve("1d", &c).unwrap().value.start(),
        ts("2026-03-28T10:00Z")
    );
    assert_eq!(
        time::resolve("yesterday", &c).unwrap().value.start(),
        ts("2026-03-28T03:00Z")
    );
    let gap = time::resolve("2026-03-29T02:59", &c).unwrap();
    assert_eq!(gap.value.start(), ts("2026-03-29T01:00Z"));
    assert_eq!(gap.notes.len(), 1);
    assert!(time::resolve("last 9223372036854775807 days", &c).is_err());
    assert!(c.day.day(Date::MAX).is_err());
    assert!(
        DaySetting::new(
            TimeZone::fixed(jiff::tz::Offset::from_seconds(3600).unwrap()),
            Time::MIN
        )
        .is_err()
    );
}

#[test]
fn bounds_include_both_dates_and_overlap_without_boundary_touching() {
    let c = clock();
    let lower = time::resolve("2026-10-05", &c).unwrap();
    let upper = time::resolve("2026-10-07", &c).unwrap();
    let inclusive = time::between(&lower, &upper).unwrap();
    assert_eq!(inclusive.start, ts("2026-10-05T02:00Z"));
    assert_eq!(inclusive.end, ts("2026-10-08T02:00Z"));
    assert!(time::overlaps_bounds(
        inclusive.into(),
        Some(ts("2026-10-07T12:00Z")),
        None
    ));
    assert!(!time::overlaps_bounds(
        inclusive.into(),
        Some(inclusive.end),
        None
    ));
    assert!(!time::overlaps_bounds(
        inclusive.into(),
        None,
        Some(inclusive.start)
    ));
}

#[test]
fn whole_period_joins_require_all_endpoints_and_are_symmetric() {
    let at = ts("2026-01-01T00:00Z");
    let minute = SignedDuration::from_secs(60);
    let end = at.checked_add(minute).unwrap();
    let coarse =
        TimeValue::new(time::Time::Span { start: at, end }, Precision::Minute, None).unwrap();
    let near = TimeValue::instant(at.checked_add(SignedDuration::from_secs(30)).unwrap());
    let distant = TimeValue::instant(at.checked_add(SignedDuration::from_secs(90)).unwrap());
    assert!(coarse.joins(&near, minute));
    assert!(!coarse.joins(&distant, minute));
    assert_eq!(coarse.joins(&near, minute), near.joins(&coarse, minute));
    assert!(!coarse.joins(&near, SignedDuration::from_secs(59)));
}

#[test]
fn decimal_relative_durations_preserve_nanoseconds_without_rounding() {
    let c = clock();
    assert_eq!(
        time::resolve("0.5ms", &c).unwrap().value.start(),
        c.now
            .checked_sub(SignedDuration::from_nanos(500_000))
            .unwrap()
    );
    assert_eq!(
        time::resolve("0.000000001s", &c).unwrap().value.start(),
        c.now.checked_sub(SignedDuration::from_nanos(1)).unwrap()
    );
    assert!(time::resolve("0.0000000001s", &c).is_err());
    assert_eq!(time::duration_ms("0.5ms"), None);
    assert_eq!(
        time::duration("1.0000000000000000000000000000000000h"),
        Some(SignedDuration::from_hours(1))
    );
}

#[test]
fn precision_widths_refuse_mislabeled_periods_in_constructors_and_json() {
    let start = ts("2026-01-01T00:00Z");
    for (precision, seconds) in [(Precision::Second, 1), (Precision::Minute, 60)] {
        let end = start
            .checked_add(SignedDuration::from_secs(seconds))
            .unwrap();
        let valid = TimeValue::new(time::Time::Span { start, end }, precision, None).unwrap();
        assert_eq!(
            deserialize_time_value(
                serde_json::to_value(&valid).unwrap(),
                valid.boundary.as_ref().map(|day| day.zone().clone())
            )
            .unwrap(),
            valid
        );
        for invalid_seconds in [0, seconds - 1, seconds + 1, 3600] {
            let invalid_end = start
                .checked_add(SignedDuration::from_secs(invalid_seconds))
                .unwrap();
            assert!(
                TimeValue::new(
                    time::Time::Span {
                        start,
                        end: invalid_end
                    },
                    precision,
                    None
                )
                .is_err()
            );
            let mut json = serde_json::to_value(&valid).unwrap();
            json["period"]["end"] = serde_json::to_value(invalid_end).unwrap();
            assert!(
                deserialize_time_value(json, valid.boundary.as_ref().map(|day| day.zone().clone()))
                    .is_err()
            );
        }
    }
}

#[test]
fn calendar_precision_requires_the_complete_period_under_its_recorded_setting() {
    let day = setting("Europe/Warsaw", "04:00");
    for (precision, start, end) in [
        (Precision::Day, "2026-03-28T03:00Z", "2026-03-29T02:00Z"),
        (Precision::Month, "2026-03-01T03:00Z", "2026-04-01T02:00Z"),
        (Precision::Year, "2026-01-01T03:00Z", "2027-01-01T03:00Z"),
    ] {
        let period = time::Time::Span {
            start: ts(start),
            end: ts(end),
        };
        let valid = TimeValue::new(period, precision, Some(day.clone())).unwrap();
        assert_eq!(
            deserialize_time_value(
                serde_json::to_value(&valid).unwrap(),
                valid.boundary.as_ref().map(|day| day.zone().clone())
            )
            .unwrap(),
            valid
        );
        assert!(TimeValue::new(period, precision, None).is_err());
        assert!(
            TimeValue::new(period, precision, Some(setting("Europe/Warsaw", "05:00"))).is_err()
        );
        let mut json = serde_json::to_value(&valid).unwrap();
        json["period"]["end"] =
            serde_json::to_value(ts(end).checked_sub(SignedDuration::from_nanos(1)).unwrap())
                .unwrap();
        assert!(
            deserialize_time_value(json, valid.boundary.as_ref().map(|day| day.zone().clone()))
                .is_err()
        );
    }
    let apia = setting("Pacific/Apia", "04:00");
    let skipped = time::Time::Span {
        start: ts("2011-12-30T14:00Z"),
        end: ts("2011-12-30T14:00Z"),
    };
    let empty = TimeValue::new(skipped, Precision::Day, Some(apia)).unwrap();
    assert_eq!(
        deserialize_time_value(
            serde_json::to_value(&empty).unwrap(),
            empty.boundary.as_ref().map(|day| day.zone().clone())
        )
        .unwrap(),
        empty
    );
    let arbitrary_empty = time::Time::Span {
        start: ts("2026-01-01T03:00Z"),
        end: ts("2026-01-01T03:00Z"),
    };
    assert!(TimeValue::new(arbitrary_empty, Precision::Day, Some(day)).is_err());
}

#[test]
fn last_compact_duration_is_elapsed_span_and_last_days_is_calendar_span() {
    let c = Clock::new(ts("2026-03-29T10:00Z"), setting("Europe/Warsaw", "04:00"));
    assert_eq!(
        time::resolve("last 7d", &c).unwrap().value,
        time::Time::Span {
            start: ts("2026-03-22T10:00Z"),
            end: ts("2026-03-29T10:00Z"),
        }
    );
    assert_eq!(
        time::resolve("last 7 days", &c).unwrap().value,
        time::Time::Span {
            start: ts("2026-03-23T03:00Z"),
            end: ts("2026-03-30T02:00Z"),
        }
    );
    assert_eq!(
        time::resolve("last 0.5ms", &c).unwrap().value,
        time::Time::Span {
            start: c
                .now
                .checked_sub(SignedDuration::from_nanos(500_000))
                .unwrap(),
            end: c.now,
        }
    );
    for text in [
        "last 0d",
        "last -7d",
        "last 0 days",
        "last 99999999999999999999999999999d",
    ] {
        assert_eq!(
            time::resolve(text, &c).unwrap_err().kind,
            query_language::ErrorKind::BadValue,
            "{text}"
        );
    }
}

#[test]
fn bare_dates_are_whole_logical_days_and_datetimes_keep_their_clock_time() {
    let c = clock();
    assert_eq!(
        time::resolve("2026-10-05", &c).unwrap().value,
        time::Time::Span {
            start: ts("2026-10-05T02:00Z"),
            end: ts("2026-10-06T02:00Z"),
        }
    );
    assert_eq!(
        time::resolve("2026-10-05T00:00", &c).unwrap().value,
        time::Time::Instant {
            at: ts("2026-10-04T22:00Z"),
        }
    );
}

fn deserialize_time_value(
    value: serde_json::Value,
    zone: Option<TimeZone>,
) -> Result<TimeValue, Box<dyn std::error::Error>> {
    Ok(serde_json::from_value::<time::TimeValueWire>(value)?.resolve(zone)?)
}

#[test]
fn wire_deserialization_never_resolves_a_zone_and_callers_must_supply_its_identity() {
    let source = r#"{"zone":"Synthetic/Uninstalled","start":"04:00:00"}"#;
    let wire: time::DaySettingWire = serde_json::from_str(source).unwrap();
    assert_eq!(wire.zone, "Synthetic/Uninstalled");
    assert!(wire.resolve(TimeZone::UTC).is_err());

    let day = setting("Europe/Warsaw", "04:00");
    let wire: time::DaySettingWire =
        serde_json::from_value(serde_json::to_value(&day).unwrap()).unwrap();
    assert!(wire.clone().resolve(TimeZone::UTC).is_err());
    assert!(
        wire.clone()
            .resolve(TimeZone::fixed(
                jiff::tz::Offset::from_seconds(3600).unwrap()
            ))
            .is_err()
    );
    assert_eq!(wire.resolve(day.zone().clone()).unwrap(), day);

    let value = TimeValue::new(
        day.day(date("2026-03-28")).unwrap().into(),
        Precision::Day,
        Some(day.clone()),
    )
    .unwrap();
    let wire: time::TimeValueWire =
        serde_json::from_value(serde_json::to_value(&value).unwrap()).unwrap();
    assert!(wire.clone().resolve(None).is_err());
    assert!(wire.clone().resolve(Some(TimeZone::UTC)).is_err());
    assert_eq!(wire.resolve(Some(day.zone().clone())).unwrap(), value);
}
