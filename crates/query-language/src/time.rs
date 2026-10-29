//! Pure time resolution, logical-day boundaries and whole-period comparisons.
//!
//! Callers supply one clock for a question and keep its resolved values for all
//! continuations. All intervals are half-open and retain nanosecond precision.
use crate::semantics::Truth;
use crate::{Error, ErrorKind};
use jiff::civil::{Date, DateTime, Time as CivilTime};
use jiff::tz::{AmbiguousOffset, TimeZone};
use jiff::{SignedDuration, Timestamp, ToSpan};
use serde::{Deserialize, Serialize};

fn bad(message: impl ToString) -> Error {
    Error::new(ErrorKind::BadValue, message.to_string(), 0)
}

/// A resolved named zone and a valid wall-clock start. Deployment owns defaults.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(into = "DaySettingWire")]
pub struct DaySetting {
    #[schemars(with = "String")]
    zone: TimeZone,
    #[schemars(with = "String")]
    start: CivilTime,
}
/// Pure transport data. The caller resolves the named zone before constructing
/// a runtime setting; deserialization performs no zone database lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DaySettingWire {
    pub zone: String,
    #[schemars(with = "String")]
    pub start: CivilTime,
}
impl DaySettingWire {
    pub fn resolve(self, zone: TimeZone) -> Result<DaySetting, Error> {
        if zone.iana_name() != Some(self.zone.as_str()) {
            return Err(bad(
                "the supplied resolved zone does not match the day setting's name",
            ));
        }
        DaySetting::new(zone, self.start)
    }
}
impl From<DaySetting> for DaySettingWire {
    fn from(value: DaySetting) -> Self {
        Self {
            zone: value.zone_name().to_string(),
            start: value.start,
        }
    }
}
impl DaySetting {
    pub fn wire(&self) -> DaySettingWire {
        self.clone().into()
    }
    pub fn new(zone: TimeZone, start: CivilTime) -> Result<Self, Error> {
        if zone.iana_name().is_none() {
            return Err(bad("a day setting requires a resolved IANA zone"));
        }
        Ok(Self { zone, start })
    }
    pub fn zone(&self) -> &TimeZone {
        &self.zone
    }
    pub fn zone_name(&self) -> &str {
        self.zone.iana_name().expect("validated named zone")
    }
    pub fn start(&self) -> CivilTime {
        self.start
    }

    /// Gaps resolve to the transition itself, rather than preserving minutes
    /// into the skipped hour. Folds select the first occurrence.
    pub fn local(&self, local: DateTime) -> Result<LocalResolution, Error> {
        let ambiguous = self.zone.to_ambiguous_timestamp(local);
        let (instant, note) = match ambiguous.offset() {
            AmbiguousOffset::Unambiguous { .. } => (ambiguous.unambiguous().map_err(bad)?, None),
            AmbiguousOffset::Fold { .. } => (
                ambiguous.earlier().map_err(bad)?,
                Some(ResolutionNote::RepeatedTime { local }),
            ),
            AmbiguousOffset::Gap { .. } => {
                let before_gap = ambiguous.earlier().map_err(bad)?;
                let transition = self
                    .zone
                    .following(before_gap)
                    .next()
                    .ok_or_else(|| bad("local gap has no representable following boundary"))?;
                (
                    transition.timestamp(),
                    Some(ResolutionNote::SkippedTime { local }),
                )
            }
        };
        Ok(LocalResolution {
            instant,
            notes: note.into_iter().collect(),
        })
    }
    pub fn start_of(&self, date: Date) -> Result<Timestamp, Error> {
        // An entirely skipped civil date has no logical day, even when the
        // configured start would otherwise move partway into the next date.
        let midnight = self.local(date.at(0, 0, 0, 0))?;
        if midnight.instant.to_zoned(self.zone.clone()).date() > date {
            return self.start_of(midnight.instant.to_zoned(self.zone.clone()).date());
        }
        Ok(self.local(date.to_datetime(self.start))?.instant)
    }
    pub fn day(&self, date: Date) -> Result<TimeRange, Error> {
        let end_date = date.tomorrow().map_err(bad)?;
        let start = self.start_of(date)?;
        let end = self.start_of(end_date)?;
        TimeRange::new(start, end)
    }
    /// Defined by boundaries, including when a gap moved today's start.
    pub fn logical_day(&self, instant: Timestamp) -> Result<Date, Error> {
        let mut date = instant.to_zoned(self.zone.clone()).date();
        loop {
            let range = self.day(date)?;
            if instant < range.start {
                date = date.yesterday().map_err(bad)?;
            } else if instant >= range.end {
                date = date.tomorrow().map_err(bad)?;
            } else {
                return Ok(date);
            }
        }
    }
    pub fn days_between(&self, from: Date, to: Date) -> Result<i64, Error> {
        days_between(from, to)
    }
    pub fn days(&self, from: Date, through: Date) -> Result<TimeRange, Error> {
        if through < from {
            return Err(bad("date range ends before it starts"));
        }
        TimeRange::new(
            self.start_of(from)?,
            self.start_of(through.tomorrow().map_err(bad)?)?,
        )
    }
}
/// Count civil labels, independent of offset changes and skipped dates.
pub fn days_between(from: Date, to: Date) -> Result<i64, Error> {
    Ok(i64::from(from.until(to).map_err(bad)?.get_days()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolutionNote {
    SkippedTime {
        #[schemars(with = "String")]
        local: DateTime,
    },
    RepeatedTime {
        #[schemars(with = "String")]
        local: DateTime,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LocalResolution {
    #[schemars(with = "String")]
    pub instant: Timestamp,
    pub notes: Vec<ResolutionNote>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "RangeWire", into = "RangeWire")]
pub struct TimeRange {
    #[schemars(with = "String")]
    pub start: Timestamp,
    #[schemars(with = "String")]
    pub end: Timestamp,
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
struct RangeWire {
    #[schemars(with = "String")]
    start: Timestamp,
    #[schemars(with = "String")]
    end: Timestamp,
}
impl TryFrom<RangeWire> for TimeRange {
    type Error = Error;
    fn try_from(v: RangeWire) -> Result<Self, Error> {
        Self::new(v.start, v.end)
    }
}
impl From<TimeRange> for RangeWire {
    fn from(v: TimeRange) -> Self {
        Self {
            start: v.start,
            end: v.end,
        }
    }
}
impl TimeRange {
    pub fn new(start: Timestamp, end: Timestamp) -> Result<Self, Error> {
        if end < start {
            return Err(bad("time range ends before it starts"));
        }
        Ok(Self { start, end })
    }
    pub fn contains(self, at: Timestamp) -> bool {
        self.start <= at && at < self.end
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.start < self.end
            && other.start < other.end
            && self.start < other.end
            && other.start < self.end
    }
    pub fn duration(self) -> SignedDuration {
        self.end.duration_since(self.start)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "TimeWire", into = "TimeWire")]
pub enum Time {
    Instant {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    Span {
        #[schemars(with = "String")]
        start: Timestamp,
        #[schemars(with = "String")]
        end: Timestamp,
    },
}
#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "time", rename_all = "snake_case")]
enum TimeWire {
    Instant {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    Span {
        #[schemars(with = "String")]
        start: Timestamp,
        #[schemars(with = "String")]
        end: Timestamp,
    },
}
impl TryFrom<TimeWire> for Time {
    type Error = Error;
    fn try_from(value: TimeWire) -> Result<Self, Error> {
        match value {
            TimeWire::Instant { at } => Ok(Self::Instant { at }),
            TimeWire::Span { start, end } => Ok(TimeRange::new(start, end)?.into()),
        }
    }
}
impl From<Time> for TimeWire {
    fn from(value: Time) -> Self {
        match value {
            Time::Instant { at } => Self::Instant { at },
            Time::Span { start, end } => Self::Span { start, end },
        }
    }
}
impl Time {
    pub fn start(self) -> Timestamp {
        match self {
            Self::Instant { at } => at,
            Self::Span { start, .. } => start,
        }
    }
    /// An exclusive upper bound for `until`: a period includes its whole
    /// extent; an instant remains exclusive at exactly that instant. Explicit
    /// comparisons instead use `start`, so `< DATE` stops at the day start.
    pub fn until_bound(self) -> Timestamp {
        match self {
            Self::Instant { at } => at,
            Self::Span { end, .. } => end,
        }
    }
    pub fn range(self) -> Option<TimeRange> {
        match self {
            Self::Span { start, end } => Some(TimeRange { start, end }),
            _ => None,
        }
    }
}
impl From<TimeRange> for Time {
    fn from(v: TimeRange) -> Self {
        Self::Span {
            start: v.start,
            end: v.end,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(into = "ClockWire")]
pub struct Clock {
    #[schemars(with = "String")]
    pub now: Timestamp,
    pub day: DaySetting,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ClockWire {
    #[schemars(with = "String")]
    pub now: Timestamp,
    pub day: DaySettingWire,
}
impl ClockWire {
    pub fn resolve(self, zone: TimeZone) -> Result<Clock, Error> {
        Ok(Clock::new(self.now, self.day.resolve(zone)?))
    }
}
impl From<Clock> for ClockWire {
    fn from(value: Clock) -> Self {
        Self {
            now: value.now,
            day: value.day.into(),
        }
    }
}
impl Clock {
    pub fn wire(&self) -> ClockWire {
        self.clone().into()
    }
    pub fn new(now: Timestamp, day: DaySetting) -> Self {
        Self { now, day }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ResolvedTime {
    pub expression: String,
    pub value: Time,
    pub notes: Vec<ResolutionNote>,
}

/// A duration represented exactly at nanosecond precision. Decimal input
/// uses integer arithmetic; overflow and fractional nanoseconds are refused.
pub fn duration(text: &str) -> Option<SignedDuration> {
    let text = text.trim();
    let split = text.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (number, unit) = text.split_at(split);
    let scale: i128 = match unit {
        "ms" => 1_000_000,
        "s" => 1_000_000_000,
        "m" => 60_000_000_000,
        "h" => 3_600_000_000_000,
        "d" => 86_400_000_000_000,
        "w" => 604_800_000_000_000,
        _ => return None,
    };
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    if whole.is_empty()
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let fraction = fraction.trim_end_matches('0');
    let factor = 10i128.checked_pow(fraction.len().try_into().ok()?)?;
    let fractional_numerator = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i128>().ok()?.checked_mul(scale)?
    };
    if fractional_numerator % factor != 0 {
        return None;
    }
    let nanos = whole
        .parse::<i128>()
        .ok()?
        .checked_mul(scale)?
        .checked_add(fractional_numerator / factor)?;
    SignedDuration::try_from_nanos_i128(nanos)
}
/// The exact duration only when it can be represented in whole milliseconds.
pub fn duration_ms(text: &str) -> Option<i64> {
    let nanos = duration(text)?.as_nanos();
    if nanos % 1_000_000 != 0 {
        return None;
    }
    (nanos / 1_000_000).try_into().ok()
}

/// Resolve against exactly the supplied clock. Calendar periods use logical
/// boundaries; relative units count exact elapsed time.
pub fn resolve(text: &str, clock: &Clock) -> Result<ResolvedTime, Error> {
    let normalized = text.trim().to_ascii_lowercase();
    let day = &clock.day;
    let today = day.logical_day(clock.now)?;
    let mut notes = Vec::new();
    let mut local = |dt| -> Result<Time, Error> {
        let result = day.local(dt)?;
        notes.extend(result.notes);
        Ok(Time::Instant { at: result.instant })
    };
    let value = if normalized == "now" {
        Time::Instant { at: clock.now }
    } else if let Some(elapsed) = duration(&normalized) {
        Time::Instant {
            at: clock.now.checked_sub(elapsed).map_err(bad)?,
        }
    } else if let Some(elapsed) = normalized.strip_prefix("last ").and_then(duration) {
        if elapsed <= SignedDuration::ZERO {
            return Err(bad("last duration requires a positive duration"));
        }
        TimeRange::new(clock.now.checked_sub(elapsed).map_err(bad)?, clock.now)?.into()
    } else if normalized
        .chars()
        .all(|c| c.is_ascii_digit() || c == '-' || c == '+')
        && let Ok(date) = normalized.parse::<Date>()
    {
        // Jiff's DateTime parser also accepts a bare date as midnight. A
        // date in the query language denotes the configured logical day.
        notes.extend(day.local(date.to_datetime(day.start))?.notes);
        notes.extend(
            day.local(date.tomorrow().map_err(bad)?.to_datetime(day.start))?
                .notes,
        );
        day.day(date)?.into()
    } else if let Ok(at) = text.trim().parse::<Timestamp>() {
        Time::Instant { at }
    } else if let Ok(dt) = text.trim().parse::<DateTime>() {
        local(dt)?
    } else if !(normalized.len() == 4 && normalized.bytes().all(|byte| byte.is_ascii_digit()))
        && let Ok(time) = normalized.parse::<CivilTime>()
    {
        // Basic civil-clock parsing also accepts HHMM. Four digits denote a
        // calendar year here; explicit HH:MM and T-prefixed clocks stay clocks.
        let date = if time < day.start {
            today.tomorrow().map_err(bad)?
        } else {
            today
        };
        local(date.to_datetime(time))?
    } else {
        let (from, through) = calendar_period(&normalized, today)?;
        // Explain adjustments on both resolved boundaries as well as locals.
        notes.extend(day.local(from.to_datetime(day.start))?.notes);
        notes.extend(
            day.local(through.tomorrow().map_err(bad)?.to_datetime(day.start))?
                .notes,
        );
        day.days(from, through)?.into()
    };
    Ok(ResolvedTime {
        expression: text.to_string(),
        value,
        notes,
    })
}

fn calendar_period(text: &str, today: Date) -> Result<(Date, Date), Error> {
    let one = |date| Ok((date, date));
    if let Ok(date) = text.parse::<Date>() {
        return one(date);
    }
    match text {
        "today" => return one(today),
        "yesterday" => return one(today.yesterday().map_err(bad)?),
        "tomorrow" => return one(today.tomorrow().map_err(bad)?),
        "this week" | "last week" => {
            let offset = i64::from(today.weekday().to_monday_zero_offset());
            let first = today
                .checked_sub((offset + if text == "last week" { 7 } else { 0 }).days())
                .map_err(bad)?;
            return Ok((first, first.checked_add(6.days()).map_err(bad)?));
        }
        "this month" | "last month" => {
            let first = Date::new(today.year(), today.month(), 1).map_err(bad)?;
            let first = if text == "last month" {
                first.checked_sub(1.months()).map_err(bad)?
            } else {
                first
            };
            return Ok((first, first.last_of_month()));
        }
        "this year" | "last year" => {
            let year = today
                .year()
                .checked_sub(if text == "last year" { 1 } else { 0 })
                .ok_or_else(|| bad("year out of range"))?;
            return Ok((
                Date::new(year, 1, 1).map_err(bad)?,
                Date::new(year, 12, 31).map_err(bad)?,
            ));
        }
        _ => {}
    }
    if let Some(n) = text
        .strip_prefix("last ")
        .and_then(|s| s.strip_suffix(" days"))
    {
        let count: i64 = n.parse().map_err(bad)?;
        if count < 1 {
            return Err(bad("last N days requires a positive count"));
        }
        return Ok((
            today
                .checked_sub(jiff::Span::new().try_days(count - 1).map_err(bad)?)
                .map_err(bad)?,
            today,
        ));
    }
    if let Some(n) = text
        .strip_suffix(" days ago")
        .or_else(|| text.strip_suffix(" day ago"))
    {
        let count: i64 = n.parse().map_err(bad)?;
        if count < 0 {
            return Err(bad("days ago requires a nonnegative count"));
        }
        return one(today
            .checked_sub(jiff::Span::new().try_days(count).map_err(bad)?)
            .map_err(bad)?);
    }
    let weekdays = [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ];
    let (name, previous) = text
        .strip_prefix("last ")
        .map_or((text, false), |s| (s, true));
    if let Some(target) = weekdays.iter().position(|&s| s == name) {
        let mut offset =
            (i64::from(today.weekday().to_monday_zero_offset()) - target as i64).rem_euclid(7);
        if previous && offset == 0 {
            offset = 7;
        }
        return one(today.checked_sub(offset.days()).map_err(bad)?);
    }
    let months = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    if let Some(month) = months.iter().position(|&s| s == text) {
        let first = Date::new(today.year(), (month + 1) as i8, 1).map_err(bad)?;
        return Ok((first, first.last_of_month()));
    }
    if text.len() == 7 && text.as_bytes().get(4) == Some(&b'-') {
        let first = format!("{text}-01").parse::<Date>().map_err(bad)?;
        return Ok((first, first.last_of_month()));
    }
    if text.len() == 4 && text.bytes().all(|c| c.is_ascii_digit()) {
        let year = text.parse::<i16>().map_err(bad)?;
        return Ok((
            Date::new(year, 1, 1).map_err(bad)?,
            Date::new(year, 12, 31).map_err(bad)?,
        ));
    }
    if text.contains('/') {
        let parts: Vec<_> = text.split('/').collect();
        if let [a, b, year] = parts.as_slice()
            && let (Ok(a), Ok(b), Ok(year)) =
                (a.parse::<i8>(), b.parse::<i8>(), year.parse::<i16>())
        {
            let mut readings = Vec::new();
            for (month, day) in [(a, b), (b, a)] {
                if let Ok(date) = Date::new(year, month, day)
                    && !readings.contains(&date)
                {
                    readings.push(date);
                }
            }
            match readings.as_slice() {
                [date] => return one(*date),
                [first, second] => {
                    return Err(Error::new(
                        ErrorKind::Ambiguous,
                        format!("ambiguous date {text:?}: {first} or {second}"),
                        0,
                    )
                    .hint("Use an ISO date YYYY-MM-DD to select a reading".to_string()));
                }
                _ => {}
            }
        }
    }
    Err(bad(format!("unknown time expression {text:?}")))
}

/// Where an event time came from. Producers classify their own evidence;
/// quality never substitutes for precision or invents an unknown instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimeQuality {
    Intrinsic,
    Captured,
    FileMtime,
    Operator,
    Unknown,
}
impl TimeQuality {
    pub const ALL: [Self; 5] = [
        Self::Intrinsic,
        Self::Captured,
        Self::FileMtime,
        Self::Operator,
        Self::Unknown,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Intrinsic => "intrinsic",
            Self::Captured => "captured",
            Self::FileMtime => "file_mtime",
            Self::Operator => "operator",
            Self::Unknown => "unknown",
        }
    }
}
impl std::fmt::Display for TimeQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
impl std::str::FromStr for TimeQuality {
    type Err = Error;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|value| value.as_str() == text)
            .ok_or_else(|| bad(format!("unknown time quality {text:?}")))
    }
}

/// Precision is carried separately from the represented half-open period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Precision {
    Instant,
    Second,
    Minute,
    Day,
    Month,
    Year,
    Bounded,
}
impl Precision {
    pub const ALL: [Self; 7] = [
        Self::Instant,
        Self::Second,
        Self::Minute,
        Self::Day,
        Self::Month,
        Self::Year,
        Self::Bounded,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Instant => "instant",
            Self::Second => "second",
            Self::Minute => "minute",
            Self::Day => "day",
            Self::Month => "month",
            Self::Year => "year",
            Self::Bounded => "bounded",
        }
    }
}
/// Boundary provenance is mandatory for day and hour-derived values. Hour
/// grains may use bounded precision and retain the same setting here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(into = "TimeValueWire")]
pub struct TimeValue {
    pub period: Time,
    pub precision: Precision,
    pub boundary: Option<DaySetting>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TimeValueWire {
    pub period: Time,
    pub precision: Precision,
    pub boundary: Option<DaySettingWire>,
}
impl TimeValueWire {
    pub fn resolve(self, zone: Option<TimeZone>) -> Result<TimeValue, Error> {
        let boundary = match self.boundary {
            Some(boundary) => Some(boundary.resolve(zone.ok_or_else(|| {
                bad("the caller must supply the boundary's resolved named zone")
            })?)?),
            None => None,
        };
        TimeValue::new(self.period, self.precision, boundary)
    }
}
impl From<TimeValue> for TimeValueWire {
    fn from(v: TimeValue) -> Self {
        Self {
            period: v.period,
            precision: v.precision,
            boundary: v.boundary.map(Into::into),
        }
    }
}
impl TimeValue {
    pub fn wire(&self) -> TimeValueWire {
        self.clone().into()
    }
    pub fn new(
        period: Time,
        precision: Precision,
        boundary: Option<DaySetting>,
    ) -> Result<Self, Error> {
        match (period, precision) {
            (Time::Instant { .. }, Precision::Instant) => {}
            (Time::Span { start, end }, p) if p != Precision::Instant && start <= end => {}
            _ => return Err(bad("time precision and period disagree")),
        }
        if let Time::Span { start, end } = period {
            let width = end.duration_since(start);
            match precision {
                Precision::Second if width != SignedDuration::from_secs(1) => {
                    return Err(bad("second precision requires a one-second period"));
                }
                Precision::Minute if width != SignedDuration::from_secs(60) => {
                    return Err(bad("minute precision requires a sixty-second period"));
                }
                Precision::Day | Precision::Month | Precision::Year => {
                    let day = boundary
                        .as_ref()
                        .ok_or_else(|| bad("calendar precision requires its day setting"))?;
                    let label = day.logical_day(start)?;
                    let expected = match precision {
                        Precision::Day if start == end => {
                            // An empty skipped date shares the following
                            // extant day's start. Its label is the predecessor
                            // of logical_day(start), which cannot return it.
                            day.day(label.yesterday().map_err(bad)?)?
                        }
                        Precision::Day => day.day(label)?,
                        Precision::Month => {
                            let first = Date::new(label.year(), label.month(), 1).map_err(bad)?;
                            day.days(first, first.last_of_month())?
                        }
                        Precision::Year => day.days(
                            Date::new(label.year(), 1, 1).map_err(bad)?,
                            Date::new(label.year(), 12, 31).map_err(bad)?,
                        )?,
                        _ => unreachable!("calendar precision match"),
                    };
                    if expected.start != start || expected.end != end {
                        return Err(bad("calendar period disagrees with its day setting"));
                    }
                }
                _ => {}
            }
        }
        Ok(Self {
            period,
            precision,
            boundary,
        })
    }
    pub fn instant(at: Timestamp) -> Self {
        Self {
            period: Time::Instant { at },
            precision: Precision::Instant,
            boundary: None,
        }
    }
    /// All possible instants must satisfy a condition to return true; some
    /// satisfying and some failing returns unknown. Empty dates are unknown.
    pub fn compare(&self, condition: &TimeCondition) -> Truth {
        let (first, last) = match self.period {
            Time::Instant { at } => (at, at),
            Time::Span { start, end } => {
                if start >= end {
                    return Truth::Unknown;
                }
                let Ok(last) = end.checked_sub(SignedDuration::from_nanos(1)) else {
                    return Truth::Unknown;
                };
                (start, last)
            }
        };
        match *condition {
            TimeCondition::Before { at } => classify(last < at, first >= at),
            TimeCondition::AtOrBefore { at } => classify(last <= at, first > at),
            TimeCondition::After { at } => classify(first > at, last <= at),
            TimeCondition::AtOrAfter { at } => classify(first >= at, last < at),
            TimeCondition::Within { range } => classify(
                range.contains(first) && range.contains(last),
                last < range.start || first >= range.end || range.start == range.end,
            ),
            TimeCondition::Equal { at } => {
                classify(first == at && last == at, at < first || at > last)
            }
            TimeCondition::NotEqual { at } => {
                classify(at < first || at > last, first == at && last == at)
            }
        }
    }
    /// A coarse observation establishes proximity only for a window wide
    /// enough to contain its entire uncertainty period.
    pub fn join_eligible(&self, window: SignedDuration) -> bool {
        if window < SignedDuration::ZERO {
            return false;
        }
        match self.period {
            Time::Instant { .. } => true,
            Time::Span { start, end } => start < end && end.duration_since(start) <= window,
        }
    }
    pub fn match_rank(&self) -> MatchRank {
        if self.precision == Precision::Instant {
            MatchRank::Precise
        } else {
            MatchRank::Coarse
        }
    }
    /// Whole-period proximity: every possible pair must fit in the window.
    pub fn joins(&self, other: &Self, window: SignedDuration) -> bool {
        if !self.join_eligible(window) || !other.join_eligible(window) {
            return false;
        }
        let endpoints = |time: Time| match time {
            Time::Instant { at } => (at, at),
            Time::Span { start, end } => (start, end),
        };
        let (a, b) = endpoints(self.period);
        let (c, d) = endpoints(other.period);
        b.duration_since(c) <= window && d.duration_since(a) <= window
    }
}
fn classify(all: bool, none: bool) -> Truth {
    if all {
        Truth::True
    } else if none {
        Truth::False
    } else {
        Truth::Unknown
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "condition", rename_all = "snake_case")]
pub enum TimeCondition {
    Before {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    AtOrBefore {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    After {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    AtOrAfter {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    Equal {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    NotEqual {
        #[schemars(with = "String")]
        at: Timestamp,
    },
    Within {
        range: TimeRange,
    },
}
/// Derived order puts precise matches first, independently of enumeration.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MatchRank {
    Precise,
    Coarse,
}
/// Unknown row times never satisfy a filter.
pub fn compare(value: Option<&TimeValue>, condition: &TimeCondition) -> Truth {
    value.map_or(Truth::Unknown, |value| value.compare(condition))
}
/// Since and until constrain overlap of spans, including open-ended bounds.
pub fn overlaps_bounds(value: Time, since: Option<Timestamp>, until: Option<Timestamp>) -> bool {
    if since.zip(until).is_some_and(|(a, b)| a >= b) {
        return false;
    }
    match value {
        Time::Instant { at } => since.is_none_or(|s| at >= s) && until.is_none_or(|u| at < u),
        Time::Span { start, end } => {
            start < end && since.is_none_or(|s| end > s) && until.is_none_or(|u| start < u)
        }
    }
}
/// Inclusive date bounds use the upper expression's exclusive end.
pub fn between(lower: &ResolvedTime, upper: &ResolvedTime) -> Result<TimeRange, Error> {
    let end = match upper.value {
        Time::Span { end, .. } => end,
        Time::Instant { at } => at.checked_add(SignedDuration::from_nanos(1)).map_err(bad)?,
    };
    TimeRange::new(lower.value.start(), end)
}
