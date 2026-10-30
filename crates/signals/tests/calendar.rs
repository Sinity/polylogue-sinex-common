use jiff::{Timestamp, civil::Time, tz::TimeZone};
use query_language::time::DaySetting;
use signals::{Calendar, DayCalendar, QuietHours};
fn clock(zone: &str, start: &str) -> DayCalendar {
    DayCalendar::new(
        DaySetting::new(TimeZone::get(zone).unwrap(), start.parse::<Time>().unwrap()).unwrap(),
    )
}
fn at(text: &str) -> i64 {
    text.parse::<Timestamp>().unwrap().as_millisecond()
}
#[test]
fn scheduled_gap_uses_first_valid_instant_and_fold_uses_earlier_once() {
    let calendar = clock("Europe/Berlin", "04:00");
    assert_eq!(
        calendar.next_at(at("2024-03-31T00:00:00Z"), 150),
        at("2024-03-31T01:00:00Z")
    );
    assert_eq!(
        calendar.next_at(at("2024-10-27T00:00:00Z"), 150),
        at("2024-10-27T00:30:00Z")
    );
    assert_eq!(
        calendar.next_at(at("2024-10-27T00:45:00Z"), 150),
        at("2024-10-28T01:30:00Z")
    );
    assert_eq!(
        calendar.logical_day(at("2024-10-27T00:30:00Z")),
        "2024-10-26"
    );
}
#[test]
fn quiet_interval_does_not_reenter_after_earlier_fold_end() {
    let calendar = clock("Europe/Berlin", "04:00");
    let quiet = QuietHours { from: 60, to: 150 };
    assert_eq!(
        calendar.quiet_until(at("2024-10-27T00:15:00Z"), quiet),
        Some(at("2024-10-27T00:30:00Z"))
    );
    assert_eq!(calendar.minute(at("2024-10-27T01:15:00Z")), 135);
    assert_eq!(
        calendar.quiet_until(at("2024-10-27T01:15:00Z"), quiet),
        None
    );
    assert_eq!(
        calendar.quiet_until(at("2024-03-31T00:30:00Z"), quiet),
        Some(at("2024-03-31T01:00:00Z"))
    );
    assert_eq!(
        calendar.quiet_until(at("2024-03-31T01:00:00Z"), quiet),
        None
    );
}
#[test]
fn quiet_hours_wrap_midnight_and_skipped_dates_do_not_create_schedules() {
    let calendar = clock("Europe/Berlin", "04:00");
    let quiet = QuietHours {
        from: 22 * 60,
        to: 8 * 60,
    };
    assert_eq!(
        calendar.quiet_until(at("2024-01-15T22:00:00Z"), quiet),
        Some(at("2024-01-16T07:00:00Z"))
    );
    assert_eq!(
        calendar.quiet_until(at("2024-01-16T00:00:00Z"), quiet),
        Some(at("2024-01-16T07:00:00Z"))
    );
    let samoa = clock("Pacific/Apia", "04:00");
    assert_eq!(
        samoa.next_at(at("2011-12-29T22:00:00Z"), 10 * 60),
        at("2011-12-30T20:00:00Z")
    );
    assert_eq!(
        samoa.quiet_until(
            at("2011-12-30T11:00:00Z"),
            QuietHours {
                from: 22 * 60,
                to: 8 * 60
            }
        ),
        None
    );
}
