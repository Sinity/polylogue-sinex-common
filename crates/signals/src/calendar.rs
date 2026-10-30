//! Signals use the query language's resolved logical day and local-time policy.
use crate::{Calendar, Instant, QuietHours};
use jiff::{
    Timestamp,
    civil::{Date, Time},
};
use query_language::time::DaySetting;

#[derive(Clone, Debug)]
pub struct DayCalendar {
    day: DaySetting,
}
impl DayCalendar {
    pub fn new(day: DaySetting) -> Self {
        Self { day }
    }
    pub fn setting(&self) -> &DaySetting {
        &self.day
    }
    fn timestamp(&self, at: Instant) -> Timestamp {
        Timestamp::from_millisecond(at).expect("product clock is a valid query-language instant")
    }
    fn resolve(&self, date: Date, minute: u16) -> Instant {
        let time = Time::new((minute / 60) as i8, (minute % 60) as i8, 0, 0)
            .expect("validated civil minute");
        self.day
            .local(date.to_datetime(time))
            .expect("validated zone and representable civil boundary")
            .instant
            .as_millisecond()
    }
    fn existing_date(&self, date: Date) -> bool {
        let range = self.day.day(date).expect("representable logical day");
        range.start < range.end
    }
}
impl Calendar for DayCalendar {
    fn logical_day(&self, at: Instant) -> String {
        self.day
            .logical_day(self.timestamp(at))
            .expect("product clock is in the logical-day range")
            .to_string()
    }
    fn minute(&self, at: Instant) -> u16 {
        let time = self.timestamp(at).to_zoned(self.day.zone().clone()).time();
        time.hour() as u16 * 60 + time.minute() as u16
    }
    fn next_at(&self, after: Instant, minute: u16) -> Instant {
        let mut date = self
            .timestamp(after)
            .to_zoned(self.day.zone().clone())
            .date();
        loop {
            if self.existing_date(date) {
                let candidate = self.resolve(date, minute);
                if candidate > after {
                    return candidate;
                }
            }
            date = date.tomorrow().expect("next civil day is representable");
        }
    }
    fn quiet_until(&self, at: Instant, hours: QuietHours) -> Option<Instant> {
        if hours.from == hours.to {
            return None;
        }
        let today = self.timestamp(at).to_zoned(self.day.zone().clone()).date();
        for date in [
            today
                .yesterday()
                .expect("previous civil day is representable"),
            today,
        ] {
            if !self.existing_date(date) {
                continue;
            }
            let end_date = if hours.from > hours.to {
                date.tomorrow().expect("next civil day is representable")
            } else {
                date
            };
            let start = self.resolve(date, hours.from);
            let end = self.resolve(end_date, hours.to);
            if start <= at && at < end {
                return Some(end);
            }
        }
        None
    }
}
