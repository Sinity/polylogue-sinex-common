//! Typed series for the shared timeline element. The product serves one series
//! per visible window, fitted to the drawn width; the shell draws it in the
//! client and keeps the server-rendered table beneath it as the no-script view.
use maud::{Markup, html};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[cfg(feature = "typescript")]
use ts_rs::TS;

/// How one series row is drawn. A state is an interval with a label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum Render {
    Interval,
    Point,
    Count,
}

/// Who acted in a span.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(rename_all = "snake_case")]
pub enum Agency {
    Operator,
    Agent,
    Unknown,
}

/// A timeline row defined by a query: its rows carry a time or an interval.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(deny_unknown_fields)]
pub struct Lane {
    /// Stable name the series rows refer to.
    pub name: String,
    pub label: String,
    /// The product query whose rows this lane draws; shown in the footer.
    pub query: String,
    /// The render the lane's rows take by default.
    pub render: Render,
    /// The field that splits the lane into drawn rows, such as a repository.
    pub group_by: Option<String>,
}

/// What one series row draws. Count rows stand for several rows in one bin and
/// carry no reference: opening one zooms into its span.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(tag = "render", rename_all = "snake_case", deny_unknown_fields)]
pub enum Shape {
    Interval {
        #[cfg_attr(feature = "typescript", ts(type = "number"))]
        end: i64,
        label: Option<String>,
        agency: Option<Agency>,
        reference: String,
        href: String,
    },
    Point {
        label: Option<String>,
        agency: Option<Agency>,
        reference: String,
        href: String,
    },
    Count {
        #[cfg_attr(feature = "typescript", ts(type = "number"))]
        end: i64,
        #[cfg_attr(feature = "typescript", ts(type = "number"))]
        count: u64,
    },
}

/// One drawable row: `(lane, group, start, shape)`. Times are milliseconds since
/// the Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(deny_unknown_fields)]
pub struct SeriesRow {
    pub lane: String,
    pub group: Option<String>,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub start: i64,
    pub shape: Shape,
}
impl SeriesRow {
    pub fn render(&self) -> Render {
        match self.shape {
            Shape::Interval { .. } => Render::Interval,
            Shape::Point { .. } => Render::Point,
            Shape::Count { .. } => Render::Count,
        }
    }
    /// The end of the row; a point ends where it starts.
    pub fn end(&self) -> i64 {
        match self.shape {
            Shape::Interval { end, .. } | Shape::Count { end, .. } => end,
            Shape::Point { .. } => self.start,
        }
    }
    /// How many source rows this row stands for.
    pub fn weight(&self) -> u64 {
        match self.shape {
            Shape::Count { count, .. } => count,
            _ => 1,
        }
    }
    /// Whether the row touches the half-open window.
    pub fn overlaps(&self, window: Window) -> bool {
        self.start < window.end && (self.end() > window.start || self.start >= window.start)
    }
}

/// A half-open span of time, `[start, end)`, in milliseconds since the epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(deny_unknown_fields)]
pub struct Window {
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub start: i64,
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub end: i64,
}
impl Window {
    pub fn span(self) -> i64 {
        self.end - self.start
    }
}

/// The rows of every lane over one window, fitted to a drawn width.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "typescript", derive(TS))]
#[serde(deny_unknown_fields)]
pub struct Series {
    pub window: Window,
    /// Drawn width in CSS pixels the series was fitted to.
    pub width: u32,
    /// The narrowest drawn shape in CSS pixels; narrower rows share a bin.
    pub minimum_pixels: u32,
    /// Length of one bin in milliseconds: a power of two, aligned to the epoch.
    /// A row belongs to the bin its start falls in.
    #[cfg_attr(feature = "typescript", ts(type = "number"))]
    pub bin: i64,
    /// IANA time zone the axis is labelled in.
    pub time_zone: String,
    pub lanes: Vec<Lane>,
    pub rows: Vec<SeriesRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TimelineError {
    EmptyWindow,
    NoWidth,
    DuplicateLane(String),
    UndeclaredLane(String),
    /// A row's group does not match its lane's `group_by` declaration.
    Group {
        lane: String,
    },
    EndsBeforeStart {
        lane: String,
        start: i64,
    },
}
impl std::fmt::Display for TimelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyWindow => write!(f, "the window must end after it starts"),
            Self::NoWidth => write!(f, "the width and minimum pixels must be positive"),
            Self::DuplicateLane(lane) => write!(f, "lane {lane} is declared twice"),
            Self::UndeclaredLane(lane) => write!(f, "rows name lane {lane}, which is not declared"),
            Self::Group { lane } => write!(
                f,
                "rows of lane {lane} must carry a group exactly when the lane declares group_by"
            ),
            Self::EndsBeforeStart { lane, start } => {
                write!(f, "a row of lane {lane} at {start} ends before it starts")
            }
        }
    }
}
impl std::error::Error for TimelineError {}

/// The drawn width and the narrowest shape a series is fitted to.
#[derive(Clone, Copy, Debug)]
pub struct Fit {
    pub window: Window,
    pub width: u32,
    pub minimum_pixels: u32,
}
impl Fit {
    /// The bin length: the smallest power of two milliseconds that is at least
    /// `minimum_pixels` wide. Bins are aligned to the epoch, so a bin at one
    /// zoom level lies inside one bin of every coarser level, and panning never
    /// moves a bin boundary.
    pub fn bin(self) -> i64 {
        // The window is non-empty, so the product is positive.
        let wanted = (u128::try_from(self.window.span()).unwrap_or(1)
            * u128::from(self.minimum_pixels))
        .div_ceil(u128::from(self.width))
        .max(1);
        let wanted = u64::try_from(wanted).unwrap_or(u64::MAX);
        i64::try_from(wanted.checked_next_power_of_two().unwrap_or(1 << 62)).unwrap_or(1 << 62)
    }
    /// Drawn width of a duration, in CSS pixels.
    pub fn pixels(self, duration: i64) -> f64 {
        duration as f64 * f64::from(self.width) / self.window.span() as f64
    }
}

impl Series {
    /// Fit rows to a window and width. Rows outside the window are dropped.
    /// An interval or count row at least `minimum_pixels` wide is kept as is.
    /// Narrower rows of one lane and group that start in the same bin become one
    /// count row for that bin; a narrow row alone in its bin is kept as is. The
    /// counts of the result add up to the rows that touch the window.
    pub fn fit(
        time_zone: impl Into<String>,
        lanes: Vec<Lane>,
        rows: impl IntoIterator<Item = SeriesRow>,
        fit: Fit,
    ) -> Result<Self, TimelineError> {
        if fit.window.end <= fit.window.start {
            return Err(TimelineError::EmptyWindow);
        }
        if fit.width == 0 || fit.minimum_pixels == 0 {
            return Err(TimelineError::NoWidth);
        }
        let mut order = BTreeMap::new();
        for (index, lane) in lanes.iter().enumerate() {
            if order.insert(lane.name.clone(), index).is_some() {
                return Err(TimelineError::DuplicateLane(lane.name.clone()));
            }
        }
        let bin = fit.bin();
        let minimum = f64::from(fit.minimum_pixels);
        let mut kept = Vec::new();
        let mut binned: BTreeMap<(usize, Option<String>, i64), Vec<SeriesRow>> = BTreeMap::new();
        for row in rows {
            let Some(&lane) = order.get(&row.lane) else {
                return Err(TimelineError::UndeclaredLane(row.lane));
            };
            if lanes[lane].group_by.is_some() != row.group.is_some() {
                return Err(TimelineError::Group { lane: row.lane });
            }
            if row.end() < row.start {
                return Err(TimelineError::EndsBeforeStart {
                    lane: row.lane,
                    start: row.start,
                });
            }
            if !row.overlaps(fit.window) {
                continue;
            }
            let wide =
                row.render() != Render::Point && fit.pixels(row.end() - row.start) >= minimum;
            if wide {
                kept.push((lane, row));
            } else {
                // The bin of the row's own start, even before the window, so that
                // a bin's members never depend on where the window begins.
                let index = row.start.div_euclid(bin);
                binned
                    .entry((lane, row.group.clone(), index))
                    .or_default()
                    .push(row);
            }
        }
        for ((lane, group, index), mut members) in binned {
            if members.len() == 1 {
                kept.push((lane, members.remove(0)));
                continue;
            }
            let start = index * bin;
            // A member may run past its bin; the count spans its members, so it
            // touches the window whenever one of them does.
            let end = members
                .iter()
                .map(SeriesRow::end)
                .fold(start + bin, i64::max);
            kept.push((
                lane,
                SeriesRow {
                    lane: lanes[lane].name.clone(),
                    group,
                    start,
                    shape: Shape::Count {
                        end,
                        count: members.iter().map(SeriesRow::weight).sum(),
                    },
                },
            ));
        }
        kept.sort_by(|(left_lane, left), (right_lane, right)| {
            (
                left_lane,
                &left.group,
                left.start,
                left.end(),
                left.render(),
            )
                .cmp(&(
                    right_lane,
                    &right.group,
                    right.start,
                    right.end(),
                    right.render(),
                ))
        });
        Ok(Self {
            window: fit.window,
            width: fit.width,
            minimum_pixels: fit.minimum_pixels,
            bin,
            time_zone: time_zone.into(),
            lanes,
            rows: kept.into_iter().map(|(_, row)| row).collect(),
        })
    }
}

/// The timeline element around its no-script table. The element fetches
/// `series_route?start=&end=&width=` and draws the answer; the table, its
/// links and the window links below it are the view without script.
pub struct Timeline<'a> {
    /// Stable element ID, unique on the page.
    pub id: &'a str,
    pub label: &'a str,
    pub series: &'a Series,
    /// Existing GET route returning `Series` JSON for `start`, `end` and `width`.
    pub series_route: &'a str,
    /// Element ID of the rows under the timeline, replaced on a selection.
    pub rows_target: Option<&'a str>,
    /// Existing GET view route returning rows HTML for `start` and `end`.
    pub rows_route: Option<&'a str>,
}
impl Timeline<'_> {
    /// `time` formats an instant for the table; `window_link` is the URL of this
    /// view over another window, used for zoom and pan without script and for
    /// opening a count row.
    pub fn render(
        &self,
        time: impl Fn(i64) -> String,
        window_link: impl Fn(Window) -> String,
    ) -> Markup {
        let window = self.series.window;
        let span = window.span();
        let earlier = Window {
            start: window.start - span / 2,
            end: window.end - span / 2,
        };
        let later = Window {
            start: window.start + span / 2,
            end: window.end + span / 2,
        };
        let zoom_out = Window {
            start: window.start - span / 2,
            end: window.end + span / 2,
        };
        let zoom_in = Window {
            start: window.start + span / 4,
            end: window.end - span / 4,
        };
        let lane_label = |name: &str| {
            self.series
                .lanes
                .iter()
                .find(|lane| lane.name == name)
                .map_or(name.to_owned(), |lane| lane.label.clone())
        };
        html! {
            shell-timeline id=(self.id) tabindex="0" aria-label=(self.label)
                data-series=(self.series_route) data-start=(window.start) data-end=(window.end)
                data-minimum=(self.series.minimum_pixels) data-time-zone=(self.series.time_zone)
                data-rows=[self.rows_target] data-rows-route=[self.rows_route] {
                nav class="timeline-window" aria-label="Timeline window" {
                    a href=(window_link(earlier)) { "← earlier" }
                    " · "
                    a href=(window_link(zoom_out)) { "zoom out" }
                    " · "
                    a href=(window_link(zoom_in)) { "zoom in" }
                    " · "
                    a href=(window_link(later)) { "later →" }
                }
                div class="table-scroll timeline-table" {
                    table {
                        caption { (self.label) ": " (time(window.start)) " to " (time(window.end)) }
                        thead { tr { th { "Lane" } th { "Group" } th { "Start" } th { "End" } th { "Row" } th { "Agency" } } }
                        tbody {
                            @for row in &self.series.rows {
                                tr {
                                    td { (lane_label(&row.lane)) }
                                    td { @if let Some(group) = &row.group { (group) } }
                                    td { (time(row.start)) }
                                    td { @if row.end() != row.start { (time(row.end())) } }
                                    @match &row.shape {
                                        Shape::Interval { label, agency, reference, href, .. }
                                        | Shape::Point { label, agency, reference, href } => {
                                            td { a href=(href) { (label.as_deref().unwrap_or(reference)) } }
                                            td { @if let Some(agency) = agency { (agency_word(*agency)) } }
                                        }
                                        Shape::Count { end, count } => {
                                            td { a href=(window_link(Window { start: row.start, end: *end })) { (count) " rows" } }
                                            td {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
fn agency_word(agency: Agency) -> &'static str {
    match agency {
        Agency::Operator => "operator",
        Agency::Agent => "agent",
        Agency::Unknown => "unknown",
    }
}
