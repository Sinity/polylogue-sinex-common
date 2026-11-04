//! The timeline zoom fixture: one synthetic year of rows fitted at year, month,
//! day and hour windows. The fitted series are committed as
//! `timeline-zoom.json`, which the browser tests draw.
use std::collections::{BTreeMap, BTreeSet};
use web_shell::timeline::{
    Agency, Fit, Lane, Render, Series, SeriesRow, Shape, TimelineError, Window,
};

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
const YEAR_START: i64 = 1_767_225_600_000; // 2026-01-01T00:00:00Z
const OCTOBER_START: i64 = YEAR_START + 273 * DAY;
const OCTOBER_SEVENTH: i64 = OCTOBER_START + 6 * DAY;
const WIDTH: u32 = 480;
const MINIMUM_PIXELS: u32 = 4;

fn lanes() -> Vec<Lane> {
    vec![
        Lane {
            name: "sessions".into(),
            label: "Agent sessions".into(),
            query: "sessions".into(),
            render: Render::Interval,
            group_by: Some("repository".into()),
        },
        Lane {
            name: "prompts".into(),
            label: "Your prompts".into(),
            query: "prompts".into(),
            render: Render::Point,
            group_by: None,
        },
        Lane {
            name: "notices".into(),
            label: "Notices".into(),
            query: "notices".into(),
            render: Render::Point,
            group_by: None,
        },
    ]
}

/// A deterministic generator; the fixture never depends on a seed elsewhere.
struct Numbers(u64);
impl Numbers {
    fn below(&mut self, bound: i64) -> i64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        i64::try_from((self.0 >> 33) % u64::try_from(bound).unwrap()).unwrap()
    }
}

fn session(start: i64, minutes: i64, number: usize) -> SeriesRow {
    SeriesRow {
        lane: "sessions".into(),
        group: Some(
            if number.is_multiple_of(3) {
                "repository-b"
            } else {
                "repository-a"
            }
            .into(),
        ),
        start,
        shape: Shape::Interval {
            end: start + minutes * 60_000,
            label: Some(format!("session {number}")),
            agency: Some(if number.is_multiple_of(2) {
                Agency::Agent
            } else {
                Agency::Operator
            }),
            reference: format!("session:fixture:{number:04}"),
            href: format!("/session/fixture:{number:04}"),
        },
    }
}
fn point(lane: &str, start: i64, number: usize, agency: Agency) -> SeriesRow {
    SeriesRow {
        lane: lane.into(),
        group: None,
        start,
        shape: Shape::Point {
            label: None,
            agency: Some(agency),
            reference: format!("{lane}:fixture:{number:04}"),
            href: format!("/{lane}/fixture:{number:04}"),
        },
    }
}

fn rows() -> Vec<SeriesRow> {
    let mut numbers = Numbers(7);
    let mut rows = Vec::new();
    // Most activity is in the first two weeks of October.
    for number in 0..200 {
        let start = if number % 10 < 7 {
            OCTOBER_START + numbers.below(14 * DAY)
        } else {
            YEAR_START + numbers.below(365 * DAY)
        };
        rows.push(session(start, 5 + numbers.below(175), number));
    }
    for number in 0..600 {
        let start = if number % 10 < 7 {
            OCTOBER_START + numbers.below(14 * DAY)
        } else {
            YEAR_START + numbers.below(365 * DAY)
        };
        rows.push(point("prompts", start, number, Agency::Operator));
    }
    for number in 0..60 {
        rows.push(point(
            "notices",
            YEAR_START + numbers.below(365 * DAY),
            number,
            Agency::Unknown,
        ));
    }
    // The hour the fixture zooms into: rows far enough apart to be drawn one by one.
    let hour = OCTOBER_SEVENTH + 14 * HOUR;
    for (offset, minutes, number) in [(2, 25, 900), (31, 40, 901), (48, 10, 902)] {
        rows.push(session(hour + offset * 60_000, minutes, number));
    }
    for index in 0..12 {
        rows.push(point(
            "prompts",
            hour + 60_000 + index * 5 * 60_000,
            900 + usize::try_from(index).unwrap(),
            Agency::Operator,
        ));
    }
    rows
}

fn levels() -> Vec<Window> {
    vec![
        Window {
            start: YEAR_START,
            end: YEAR_START + 365 * DAY,
        },
        Window {
            start: OCTOBER_START,
            end: OCTOBER_START + 31 * DAY,
        },
        Window {
            start: OCTOBER_SEVENTH,
            end: OCTOBER_SEVENTH + DAY,
        },
        Window {
            start: OCTOBER_SEVENTH + 14 * HOUR,
            end: OCTOBER_SEVENTH + 15 * HOUR,
        },
    ]
}

fn fit(window: Window) -> Series {
    Series::fit(
        "Europe/Warsaw",
        lanes(),
        rows(),
        Fit {
            window,
            width: WIDTH,
            minimum_pixels: MINIMUM_PIXELS,
        },
    )
    .unwrap()
}

fn drawn_as_itself(series: &Series) -> BTreeSet<String> {
    series
        .rows
        .iter()
        .filter_map(|row| match &row.shape {
            Shape::Interval { reference, .. } | Shape::Point { reference, .. } => {
                Some(reference.clone())
            }
            Shape::Count { .. } => None,
        })
        .collect()
}

#[test]
fn timeline_fit_conserves_rows() {
    for window in levels() {
        let series = fit(window);
        let touching = rows().iter().filter(|row| row.overlaps(window)).count();
        let drawn: u64 = series.rows.iter().map(SeriesRow::weight).sum();
        assert_eq!(drawn, u64::try_from(touching).unwrap(), "{window:?}");
        assert!(series.rows.iter().all(|row| row.overlaps(window)));
    }
}

#[test]
fn timeline_fit_draws_one_narrow_shape_per_bin() {
    for window in levels() {
        let series = fit(window);
        let fit = Fit {
            window,
            width: WIDTH,
            minimum_pixels: MINIMUM_PIXELS,
        };
        assert_eq!(series.bin, fit.bin());
        assert!(series.bin.count_ones() == 1, "the bin is a power of two");
        assert!(fit.pixels(series.bin) >= f64::from(MINIMUM_PIXELS));
        assert!(fit.pixels(series.bin) < 2.0 * f64::from(MINIMUM_PIXELS) + 1.0);
        let mut narrow = BTreeMap::<(String, Option<String>, i64), usize>::new();
        for row in &series.rows {
            if fit.pixels(row.end() - row.start) < f64::from(MINIMUM_PIXELS) {
                *narrow
                    .entry((
                        row.lane.clone(),
                        row.group.clone(),
                        row.start.div_euclid(series.bin),
                    ))
                    .or_default() += 1;
            }
        }
        assert!(narrow.values().all(|&count| count == 1), "{window:?}");
    }
}

#[test]
fn timeline_zoom_refines() {
    let windows = levels();
    for pair in windows.windows(2) {
        let (coarse, fine) = (fit(pair[0]), fit(pair[1]));
        let coarse_rows = drawn_as_itself(&coarse);
        let fine_rows = drawn_as_itself(&fine);
        for row in rows()
            .iter()
            .filter(|row| row.start >= pair[1].start && row.start < pair[1].end)
        {
            let (Shape::Interval { reference, .. } | Shape::Point { reference, .. }) = &row.shape
            else {
                continue;
            };
            if coarse_rows.contains(reference) {
                assert!(
                    fine_rows.contains(reference),
                    "{reference} merged after zooming in"
                );
            }
        }
    }
    // At the hour, every row is drawn as itself.
    let hour = fit(windows[3]);
    assert!(hour.rows.iter().all(|row| row.render() != Render::Count));
    let drawn = drawn_as_itself(&hour);
    for number in 900..903 {
        assert!(drawn.contains(&format!("session:fixture:{number:04}")));
    }
    for number in 900..912 {
        assert!(drawn.contains(&format!("prompts:fixture:{number:04}")));
    }
}

#[test]
fn timeline_pan_keeps_bins() {
    let day = levels()[2];
    let series = fit(day);
    let shift = 7 * series.bin;
    let panned = fit(Window {
        start: day.start + shift,
        end: day.end + shift,
    });
    assert_eq!(series.bin, panned.bin);
    let inside = |row: &&SeriesRow| row.start >= day.start + shift && row.end() < day.end;
    let before: Vec<_> = series.rows.iter().filter(inside).collect();
    let after: Vec<_> = panned.rows.iter().filter(inside).collect();
    assert_eq!(before, after);
}

#[test]
fn timeline_fit_refuses_inconsistent_rows() {
    let window = levels()[3];
    let fit = |rows: Vec<SeriesRow>| {
        Series::fit(
            "UTC",
            lanes(),
            rows,
            Fit {
                window,
                width: WIDTH,
                minimum_pixels: MINIMUM_PIXELS,
            },
        )
    };
    let mut undeclared = point("prompts", window.start, 1, Agency::Operator);
    undeclared.lane = "absent".into();
    assert_eq!(
        fit(vec![undeclared]),
        Err(TimelineError::UndeclaredLane("absent".into()))
    );
    let mut ungrouped = session(window.start, 10, 1);
    ungrouped.group = None;
    assert_eq!(
        fit(vec![ungrouped]),
        Err(TimelineError::Group {
            lane: "sessions".into()
        })
    );
    let mut backwards = session(window.start, 10, 1);
    backwards.shape = Shape::Count {
        end: window.start - 1,
        count: 2,
    };
    assert!(matches!(
        fit(vec![backwards]),
        Err(TimelineError::EndsBeforeStart { .. })
    ));
    assert_eq!(
        Series::fit(
            "UTC",
            lanes(),
            vec![],
            Fit {
                window: Window { start: 5, end: 5 },
                width: WIDTH,
                minimum_pixels: MINIMUM_PIXELS
            }
        ),
        Err(TimelineError::EmptyWindow)
    );
    let mut twice = lanes();
    twice.push(twice[0].clone());
    assert_eq!(
        Series::fit(
            "UTC",
            twice,
            vec![],
            Fit {
                window,
                width: WIDTH,
                minimum_pixels: MINIMUM_PIXELS
            }
        ),
        Err(TimelineError::DuplicateLane("sessions".into()))
    );
}

#[test]
fn timeline_zoom_fixture_matches_committed() {
    let levels: Vec<Series> = levels().into_iter().map(fit).collect();
    let text = format!(
        "[\n{}\n]\n",
        levels
            .iter()
            .map(|series| serde_json::to_string(series).unwrap())
            .collect::<Vec<_>>()
            .join(",\n")
    );
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/timeline-zoom.json");
    if std::env::var_os("WEB_SHELL_WRITE_FIXTURE").is_some() {
        std::fs::write(path, &text).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(path).unwrap_or_default(),
        text,
        "regenerate with WEB_SHELL_WRITE_FIXTURE=1 cargo xtask test -p web-shell"
    );
    let decoded: Vec<Series> = serde_json::from_str(&text).unwrap();
    assert_eq!(decoded, levels);
}

#[test]
fn timeline_table_works_without_script() {
    let series = fit(levels()[2]);
    let markup = web_shell::Timeline {
        id: "day-timeline",
        label: "Day",
        series: &series,
        series_route: "/timeline/series",
        rows_target: Some("rows"),
        rows_route: Some("/timeline/rows"),
    }
    .render(
        |time| time.to_string(),
        |window| format!("/day?start={}&end={}", window.start, window.end),
    )
    .into_string();
    assert!(markup.starts_with("<shell-timeline id=\"day-timeline\""));
    assert!(markup.contains("data-series=\"/timeline/series\""));
    assert!(markup.contains("<table>"));
    assert!(markup.contains("zoom in"));
    // Every row is reachable: references link to their view, counts to their window.
    let links = markup.matches("<td><a href=").count();
    assert_eq!(links, series.rows.len());
    assert!(!markup.contains("<script"));
    assert!(!markup.contains(" style="));
}
