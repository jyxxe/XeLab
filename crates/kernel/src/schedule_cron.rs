//! Evaluate the ScheduleSpec five-field cron grammar against bundled IANA time.
use crate::error::{Error, Result};
use chrono::{DateTime, Utc};
use jiff::{
    civil::{Date, Time},
    tz::{AmbiguousOffset, Offset, TimeZone, TimeZoneDatabase},
    Timestamp, ToSpan,
};
use serde::Serialize;
use std::collections::BTreeSet;

mod recovery;
pub use recovery::{
    MisfirePolicy, OverlapPolicy, RecoveryDecision, RecoveryInputs, RecoveryPlan, SkipReason,
};

pub const DEFAULT_TIMEZONE: &str = "Asia/Shanghai";
const MAX_PREVIEW_COUNT: usize = 100;
const SEARCH_YEARS: i64 = 500;

pub struct CronSchedule {
    timezone: TimeZone,
    times: Vec<Time>,
    days: Field,
    months: Field,
    weekdays: Field,
    match_either_day_field: bool,
}

#[derive(Serialize)]
pub struct SchedulePreview {
    pub timezone: String,
    pub fire_at: Vec<DateTime<Utc>>,
    pub warnings: Vec<PreviewWarning>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PreviewWarning {
    DayOfMonthOrDayOfWeek,
    SearchLimitReached,
}

impl CronSchedule {
    pub fn parse(expression: &str, timezone: &str) -> Result<Self> {
        if expression.len() > 128 {
            return Err(Error::invalid("Cron expression exceeds 128 characters"));
        }
        let fields: Vec<_> = expression.split_ascii_whitespace().collect();
        let &[minute, hour, day, month, weekday] = fields.as_slice() else {
            return Err(Error::invalid("Cron expression must contain five fields"));
        };
        let minutes = Field::parse(minute, 0, 59)
            .ok_or_else(|| Error::invalid("Invalid cron minute field"))?;
        let hours =
            Field::parse(hour, 0, 23).ok_or_else(|| Error::invalid("Invalid cron hour field"))?;
        let days =
            Field::parse(day, 1, 31).ok_or_else(|| Error::invalid("Invalid cron day field"))?;
        let months =
            Field::parse(month, 1, 12).ok_or_else(|| Error::invalid("Invalid cron month field"))?;
        let weekdays = Field::parse(weekday, 0, 6)
            .ok_or_else(|| Error::invalid("Invalid cron weekday field"))?;
        let timezone_error = || Error::invalid("Schedule requires a valid IANA time zone");
        if timezone.len() > 80 {
            return Err(timezone_error());
        }
        let timezone = TimeZoneDatabase::bundled()
            .get(timezone)
            .map_err(|_| timezone_error())?;
        // Jiff also recognizes its synthetic Etc/Unknown zone.
        if timezone.iana_name().is_none() {
            return Err(timezone_error());
        }

        let mut times = Vec::new();
        for hour in (0..=23).filter(|hour| hours.contains(*hour)) {
            for minute in (0..=59).filter(|minute| minutes.contains(*minute)) {
                times.push(jiff::civil::time(hour, minute, 0, 0));
            }
        }
        Ok(Self {
            timezone,
            times,
            days,
            months,
            weekdays,
            match_either_day_field: day != "*" && weekday != "*",
        })
    }

    /// Return UTC occurrences strictly after the supplied instant.
    pub fn preview(&self, after: DateTime<Utc>, count: usize) -> Result<SchedulePreview> {
        if !(1..=MAX_PREVIEW_COUNT).contains(&count) {
            return Err(Error::invalid("Schedule preview count must be 1 to 100"));
        }
        // All matched instants have whole-second precision, including
        // historical zones whose offsets are not a whole minute.
        let cutoff = schedule_timestamp(after)?;
        // Use the earliest possible local date at the cutoff. A large UTC
        // offset change can make the next occurrence's local date go backward.
        let start = Offset::MIN.to_datetime(cutoff).date();
        let end = start.saturating_add(SEARCH_YEARS.years());
        let mut instants = BTreeSet::new();
        for date in start.series(1.day()).take_while(|date| *date <= end) {
            // Until this lower bound passes the last retained instant, a
            // later local date could yield an earlier UTC instant.
            if instants.len() == count
                && Offset::MAX
                    .to_timestamp(date.at(0, 0, 0, 0))
                    .is_ok_and(|earliest| instants.last().is_some_and(|last| earliest > *last))
            {
                break;
            }
            if !self.matches_date(date) {
                continue;
            }
            for instant in self.instants_on(date).filter(|instant| *instant > cutoff) {
                instants.insert(instant);
                if instants.len() > count {
                    instants.pop_last();
                }
            }
        }

        let mut warnings = Vec::new();
        if self.match_either_day_field {
            warnings.push(PreviewWarning::DayOfMonthOrDayOfWeek);
        }
        if instants.len() < count {
            warnings.push(PreviewWarning::SearchLimitReached);
        }
        let fire_at = instants.into_iter().map(fire_datetime).collect();
        Ok(SchedulePreview {
            timezone: self.timezone.iana_name().unwrap().to_owned(),
            fire_at,
            warnings,
        })
    }

    fn instants_on(&self, date: Date) -> impl Iterator<Item = Timestamp> + '_ {
        self.times
            .iter()
            .filter_map(move |&time| self.first_instant(date, time))
    }

    fn first_instant(&self, date: Date, time: Time) -> Option<Timestamp> {
        let mapping = self.timezone.to_ambiguous_timestamp(date.to_datetime(time));
        if matches!(mapping.offset(), AmbiguousOffset::Gap { .. }) {
            return None;
        }
        // Select the first physical occurrence before applying UTC cutoffs.
        mapping.earlier().ok()
    }

    fn matches_date(&self, date: Date) -> bool {
        let day = self.days.contains(date.day());
        let weekday = self
            .weekdays
            .contains(date.weekday().to_sunday_zero_offset());
        let day_matches = if self.match_either_day_field {
            day || weekday
        } else {
            day && weekday
        };
        self.months.contains(date.month()) && day_matches
    }
}

fn schedule_timestamp(time: DateTime<Utc>) -> Result<Timestamp> {
    Timestamp::from_second(time.timestamp())
        .map_err(|_| Error::invalid("Schedule instant is outside the supported range"))
}

fn fire_datetime(instant: Timestamp) -> DateTime<Utc> {
    DateTime::from_timestamp(instant.as_second(), 0).unwrap()
}

struct Field(u64);

impl Field {
    fn parse(field: &str, min: u8, max: u8) -> Option<Self> {
        let mut bits = 0;
        for item in field.split(',') {
            let (range, step) = match item.split_once('/') {
                Some((range, step)) => (range, number(step)?),
                None => (item, 1),
            };
            let (first, last) = if range == "*" {
                (min, max)
            } else if let Some((first, last)) = range.split_once('-') {
                (number(first)?, number(last)?)
            } else if item == range {
                let value = number(range)?;
                (value, value)
            } else {
                return None;
            };
            if first < min || first > last || last > max || step == 0 {
                return None;
            }
            for value in (first..=last).step_by(usize::from(step)) {
                bits |= 1 << value;
            }
        }
        Some(Self(bits))
    }

    fn contains(&self, value: i8) -> bool {
        self.0 & (1 << value) != 0
    }
}

fn number(text: &str) -> Option<u8> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // All fields end at 59 or below. Saturation rejects oversized field
    // positions while still accepting positive steps larger than a field.
    Some(text.bytes().fold(0_u8, |value, digit| {
        value.saturating_mul(10).saturating_add(digit - b'0')
    }))
}

#[cfg(test)]
mod tests;
