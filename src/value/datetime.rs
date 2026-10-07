//! `datetime.date`, `datetime.datetime` and fixed-offset time zones.

use std::fmt::{self, Write};
use std::hash::{Hash, Hasher};

/// Error raised when constructing a date, time or time zone offset with an
/// out-of-range component.
///
/// The [`Display`](fmt::Display) text is the message of the `ValueError` that
/// CPython's `datetime` module raises for the same arguments, so a YAML loader
/// can surface it verbatim (PyYAML lets these errors escape, and cfgv's
/// `load_from_filename` reports them as the leaf message).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DateError {
    /// `year {year} is out of range` (valid years are `1..=9999`).
    YearOutOfRange(i64),
    /// `month must be in 1..12`.
    MonthOutOfRange,
    /// `day is out of range for month`.
    DayOutOfRange,
    /// `hour must be in 0..23`.
    HourOutOfRange,
    /// `minute must be in 0..59`.
    MinuteOutOfRange,
    /// `second must be in 0..59`.
    SecondOutOfRange,
    /// `microsecond must be in 0..999999`.
    MicrosecondOutOfRange,
    /// A UTC offset (in seconds) that is not strictly between -24 and +24
    /// hours.
    OffsetOutOfRange(i64),
}

impl fmt::Display for DateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::YearOutOfRange(year) => write!(f, "year {year} is out of range"),
            Self::MonthOutOfRange => f.write_str("month must be in 1..12"),
            Self::DayOutOfRange => f.write_str("day is out of range for month"),
            Self::HourOutOfRange => f.write_str("hour must be in 0..23"),
            Self::MinuteOutOfRange => f.write_str("minute must be in 0..59"),
            Self::SecondOutOfRange => f.write_str("second must be in 0..59"),
            Self::MicrosecondOutOfRange => f.write_str("microsecond must be in 0..999999"),
            Self::OffsetOutOfRange(seconds) => {
                f.write_str(
                    "offset must be a timedelta strictly between \
                     -timedelta(hours=24) and timedelta(hours=24), not ",
                )?;
                write_timedelta_repr(f, *seconds)?;
                f.write_char('.')
            }
        }
    }
}

impl std::error::Error for DateError {}

/// A calendar date, Python's `datetime.date`.
///
/// `repr()` is `datetime.date(2020, 1, 1)`; `str()` is `2020-01-01`; the type
/// name is `date`.
///
/// ```
/// use cfgv::{Date, Value};
///
/// let d = Date::new(2020, 1, 1).unwrap();
/// assert_eq!(Value::from(d).repr(), "datetime.date(2020, 1, 1)");
/// assert_eq!(d.to_string(), "2020-01-01");
/// assert_eq!(Date::new(2021, 2, 29).unwrap_err().to_string(), "day is out of range for month");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Date {
    year: u16,
    month: u8,
    day: u8,
}

impl Date {
    /// Creates a date, validating it like `datetime.date(year, month, day)`.
    ///
    /// # Errors
    ///
    /// Returns [`DateError::YearOutOfRange`], [`DateError::MonthOutOfRange`]
    /// or [`DateError::DayOutOfRange`] (checked in that order).
    pub fn new(year: i64, month: u32, day: u32) -> Result<Self, DateError> {
        let year16 = u16::try_from(year)
            .ok()
            .filter(|y| (1..=9999).contains(y))
            .ok_or(DateError::YearOutOfRange(year))?;
        let month8 = u8::try_from(month)
            .ok()
            .filter(|m| (1..=12).contains(m))
            .ok_or(DateError::MonthOutOfRange)?;
        let day8 = u8::try_from(day)
            .ok()
            .filter(|d| (1..=days_in_month(year16, month8)).contains(d))
            .ok_or(DateError::DayOutOfRange)?;
        Ok(Self {
            year: year16,
            month: month8,
            day: day8,
        })
    }

    /// The year (`1..=9999`).
    #[must_use]
    pub const fn year(self) -> u16 {
        self.year
    }

    /// The month (`1..=12`).
    #[must_use]
    pub const fn month(self) -> u8 {
        self.month
    }

    /// The day of the month (`1..=31`).
    #[must_use]
    pub const fn day(self) -> u8 {
        self.day
    }

    /// Days since 0001-01-01 (proleptic Gregorian calendar).
    fn ordinal(self) -> i64 {
        let y = i64::from(self.year) - 1;
        let before_year = y * 365 + y / 4 - y / 100 + y / 400;
        let mut before_month: i64 = 0;
        for m in 1..self.month {
            before_month += i64::from(days_in_month(self.year, m));
        }
        before_year + before_month + i64::from(self.day) - 1
    }

    pub(crate) fn write_repr<W: Write + ?Sized>(self, out: &mut W) -> fmt::Result {
        write!(
            out,
            "datetime.date({}, {}, {})",
            self.year, self.month, self.day
        )
    }
}

impl fmt::Display for Date {
    /// `date.isoformat()`: `YYYY-MM-DD`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

const fn is_leap(year: u16) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

const fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// A fixed UTC offset, Python's `datetime.timezone(timedelta(...))`.
///
/// A zero offset is `datetime.timezone.utc` (CPython returns that singleton
/// for `timezone(timedelta(0))`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TzOffset {
    seconds: i32,
}

impl TzOffset {
    /// UTC (`datetime.timezone.utc`).
    pub const UTC: Self = Self { seconds: 0 };

    /// Creates an offset of `seconds` east of UTC.
    ///
    /// # Errors
    ///
    /// Returns [`DateError::OffsetOutOfRange`] unless the offset is strictly
    /// between -24 and +24 hours.
    pub fn from_seconds(seconds: i64) -> Result<Self, DateError> {
        i32::try_from(seconds)
            .ok()
            .filter(|s| s.unsigned_abs() < 86_400)
            .map(|seconds| Self { seconds })
            .ok_or(DateError::OffsetOutOfRange(seconds))
    }

    /// Creates the offset PyYAML builds for a `±HH[:MM]` timestamp suffix:
    /// `timezone(±timedelta(hours=hours, minutes=minutes))`.
    ///
    /// # Errors
    ///
    /// Returns [`DateError::OffsetOutOfRange`] unless the offset is strictly
    /// between -24 and +24 hours.
    pub fn from_hours_minutes(negative: bool, hours: u32, minutes: u32) -> Result<Self, DateError> {
        let magnitude = i64::from(hours) * 3600 + i64::from(minutes) * 60;
        Self::from_seconds(if negative { -magnitude } else { magnitude })
    }

    /// The offset in seconds east of UTC.
    #[must_use]
    pub const fn seconds(self) -> i32 {
        self.seconds
    }

    fn write_repr<W: Write + ?Sized>(self, out: &mut W) -> fmt::Result {
        if self.seconds == 0 {
            return out.write_str("datetime.timezone.utc");
        }
        out.write_str("datetime.timezone(")?;
        write_timedelta_repr(out, i64::from(self.seconds))?;
        out.write_char(')')
    }

    /// The `±HH:MM[:SS]` suffix of `isoformat()`.
    fn write_iso<W: Write + ?Sized>(self, out: &mut W) -> fmt::Result {
        let sign = if self.seconds < 0 { '-' } else { '+' };
        let total = self.seconds.unsigned_abs();
        let (hours, minutes, seconds) = (total / 3600, total % 3600 / 60, total % 60);
        write!(out, "{sign}{hours:02}:{minutes:02}")?;
        if seconds != 0 {
            write!(out, ":{seconds:02}")?;
        }
        Ok(())
    }
}

/// `repr(datetime.timedelta(seconds=seconds))`.
fn write_timedelta_repr<W: Write + ?Sized>(out: &mut W, seconds: i64) -> fmt::Result {
    let days = seconds.div_euclid(86_400);
    let secs = seconds.rem_euclid(86_400);
    out.write_str("datetime.timedelta(")?;
    match (days, secs) {
        (0, 0) => out.write_char('0')?,
        (0, s) => write!(out, "seconds={s}")?,
        (d, 0) => write!(out, "days={d}")?,
        (d, s) => write!(out, "days={d}, seconds={s}")?,
    }
    out.write_char(')')
}

/// A date and time, Python's `datetime.datetime`, optionally with a fixed
/// UTC offset.
///
/// Equality follows Python: naive values compare field by field, aware
/// values compare the instant they denote, and a naive value never equals
/// an aware one.
///
/// ```
/// use cfgv::{Date, DateTime, TzOffset, Value};
///
/// let tz = TzOffset::from_hours_minutes(true, 5, 0).unwrap();
/// let dt = DateTime::new(Date::new(2001, 12, 14).unwrap(), 21, 59, 43, 100_000, Some(tz)).unwrap();
/// assert_eq!(
///     Value::from(dt).repr(),
///     "datetime.datetime(2001, 12, 14, 21, 59, 43, 100000, \
///      tzinfo=datetime.timezone(datetime.timedelta(days=-1, seconds=68400)))",
/// );
/// assert_eq!(dt.to_string(), "2001-12-14 21:59:43.100000-05:00");
/// ```
#[derive(Debug, Clone, Copy)]
pub struct DateTime {
    date: Date,
    hour: u8,
    minute: u8,
    second: u8,
    microsecond: u32,
    tz: Option<TzOffset>,
}

impl DateTime {
    /// Creates a date-time, validating the time components like
    /// `datetime.datetime(...)`.
    ///
    /// # Errors
    ///
    /// Returns [`DateError::HourOutOfRange`], [`DateError::MinuteOutOfRange`],
    /// [`DateError::SecondOutOfRange`] or
    /// [`DateError::MicrosecondOutOfRange`] (checked in that order).
    pub fn new(
        date: Date,
        hour: u32,
        minute: u32,
        second: u32,
        microsecond: u32,
        tz: Option<TzOffset>,
    ) -> Result<Self, DateError> {
        let in_range = |v: u32, max: u32, err| {
            if v <= max {
                u8::try_from(v).map_err(|_| err)
            } else {
                Err(err)
            }
        };
        let hour = in_range(hour, 23, DateError::HourOutOfRange)?;
        let minute = in_range(minute, 59, DateError::MinuteOutOfRange)?;
        let second = in_range(second, 59, DateError::SecondOutOfRange)?;
        if microsecond > 999_999 {
            return Err(DateError::MicrosecondOutOfRange);
        }
        Ok(Self {
            date,
            hour,
            minute,
            second,
            microsecond,
            tz,
        })
    }

    /// The date part.
    #[must_use]
    pub const fn date(self) -> Date {
        self.date
    }

    /// The hour (`0..=23`).
    #[must_use]
    pub const fn hour(self) -> u8 {
        self.hour
    }

    /// The minute (`0..=59`).
    #[must_use]
    pub const fn minute(self) -> u8 {
        self.minute
    }

    /// The second (`0..=59`).
    #[must_use]
    pub const fn second(self) -> u8 {
        self.second
    }

    /// The microsecond (`0..=999_999`).
    #[must_use]
    pub const fn microsecond(self) -> u32 {
        self.microsecond
    }

    /// The UTC offset, if the value is time-zone aware.
    #[must_use]
    pub const fn tz(self) -> Option<TzOffset> {
        self.tz
    }

    fn naive_fields(self) -> (Date, u8, u8, u8, u32) {
        (
            self.date,
            self.hour,
            self.minute,
            self.second,
            self.microsecond,
        )
    }

    /// Microseconds since 0001-01-01T00:00:00 UTC, for aware values.
    fn instant(self) -> Option<i64> {
        let tz = self.tz?;
        let seconds = self.date.ordinal() * 86_400
            + i64::from(self.hour) * 3600
            + i64::from(self.minute) * 60
            + i64::from(self.second)
            - i64::from(tz.seconds);
        Some(seconds * 1_000_000 + i64::from(self.microsecond))
    }

    /// Python's total-order fallback used when sorting values: naive values
    /// (by fields) before aware ones (by instant).
    pub(crate) fn py_cmp(self, other: Self) -> std::cmp::Ordering {
        match (self.instant(), other.instant()) {
            (None, None) => self.naive_fields().cmp(&other.naive_fields()),
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => a.cmp(&b),
        }
    }

    pub(crate) fn write_repr<W: Write + ?Sized>(self, out: &mut W) -> fmt::Result {
        let d = self.date;
        write!(
            out,
            "datetime.datetime({}, {}, {}, {}, {}",
            d.year, d.month, d.day, self.hour, self.minute
        )?;
        if self.microsecond != 0 {
            write!(out, ", {}, {}", self.second, self.microsecond)?;
        } else if self.second != 0 {
            write!(out, ", {}", self.second)?;
        }
        if let Some(tz) = self.tz {
            out.write_str(", tzinfo=")?;
            tz.write_repr(out)?;
        }
        out.write_char(')')
    }
}

impl PartialEq for DateTime {
    fn eq(&self, other: &Self) -> bool {
        match (self.instant(), other.instant()) {
            (None, None) => self.naive_fields() == other.naive_fields(),
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}

impl Eq for DateTime {}

impl Hash for DateTime {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self.instant() {
            None => {
                state.write_u8(0);
                self.naive_fields().hash(state);
            }
            Some(instant) => {
                state.write_u8(1);
                instant.hash(state);
            }
        }
    }
}

impl fmt::Display for DateTime {
    /// `datetime.isoformat(sep=' ')`, e.g. `2001-12-14 21:59:43.100000-05:00`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {:02}:{:02}:{:02}",
            self.date, self.hour, self.minute, self.second
        )?;
        if self.microsecond != 0 {
            write!(f, ".{:06}", self.microsecond)?;
        }
        if let Some(tz) = self.tz {
            tz.write_iso(f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i64, m: u32, d: u32) -> Date {
        Date::new(y, m, d).unwrap()
    }

    #[test]
    fn date_validation_messages() {
        let msg = |y, m, d| Date::new(y, m, d).unwrap_err().to_string();
        assert_eq!(msg(0, 1, 1), "year 0 is out of range");
        assert_eq!(msg(10_000, 1, 1), "year 10000 is out of range");
        assert_eq!(msg(2020, 13, 1), "month must be in 1..12");
        assert_eq!(msg(2020, 0, 1), "month must be in 1..12");
        assert_eq!(msg(2021, 2, 29), "day is out of range for month");
        assert_eq!(msg(2020, 4, 31), "day is out of range for month");
        assert!(Date::new(2000, 2, 29).is_ok());
        assert!(Date::new(1900, 2, 29).is_err());
    }

    #[test]
    fn ordinal_matches_python() {
        // date(2020, 1, 1).toordinal() == 737425; ours is zero-based.
        assert_eq!(date(1, 1, 1).ordinal(), 0);
        assert_eq!(date(2020, 1, 1).ordinal(), 737_424);
        assert_eq!(date(9999, 12, 31).ordinal(), 3_652_058);
    }

    #[test]
    fn offsets() {
        assert_eq!(
            TzOffset::from_hours_minutes(false, 24, 0)
                .unwrap_err()
                .to_string(),
            "offset must be a timedelta strictly between -timedelta(hours=24) and \
             timedelta(hours=24), not datetime.timedelta(days=1)."
        );
        assert_eq!(
            TzOffset::from_hours_minutes(false, 99, 0)
                .unwrap_err()
                .to_string(),
            "offset must be a timedelta strictly between -timedelta(hours=24) and \
             timedelta(hours=24), not datetime.timedelta(days=4, seconds=10800)."
        );
        assert_eq!(
            TzOffset::from_hours_minutes(true, 24, 0)
                .unwrap_err()
                .to_string(),
            "offset must be a timedelta strictly between -timedelta(hours=24) and \
             timedelta(hours=24), not datetime.timedelta(days=-1)."
        );
        assert_eq!(TzOffset::from_hours_minutes(true, 0, 0), Ok(TzOffset::UTC));
    }

    #[test]
    fn datetime_repr_and_str() {
        let repr = |dt: DateTime| {
            let mut s = String::new();
            dt.write_repr(&mut s).unwrap();
            s
        };
        let d = date(2001, 12, 14);
        let naive = DateTime::new(d, 0, 0, 0, 0, None).unwrap();
        assert_eq!(repr(naive), "datetime.datetime(2001, 12, 14, 0, 0)");
        assert_eq!(naive.to_string(), "2001-12-14 00:00:00");
        let secs = DateTime::new(d, 0, 0, 5, 0, None).unwrap();
        assert_eq!(repr(secs), "datetime.datetime(2001, 12, 14, 0, 0, 5)");
        let micros = DateTime::new(d, 0, 0, 0, 5, Some(TzOffset::UTC)).unwrap();
        assert_eq!(
            repr(micros),
            "datetime.datetime(2001, 12, 14, 0, 0, 0, 5, tzinfo=datetime.timezone.utc)"
        );
        assert_eq!(micros.to_string(), "2001-12-14 00:00:00.000005+00:00");
        let ist = TzOffset::from_hours_minutes(false, 5, 30).unwrap();
        let aware = DateTime::new(d, 21, 59, 0, 0, Some(ist)).unwrap();
        assert_eq!(
            repr(aware),
            "datetime.datetime(2001, 12, 14, 21, 59, \
             tzinfo=datetime.timezone(datetime.timedelta(seconds=19800)))"
        );
        let odd = TzOffset::from_seconds(-(5 * 3600 + 30 * 60 + 15)).unwrap();
        let odd = DateTime::new(d, 1, 2, 3, 0, Some(odd)).unwrap();
        assert_eq!(odd.to_string(), "2001-12-14 01:02:03-05:30:15");
        assert_eq!(date(5, 1, 1).to_string(), "0005-01-01");
    }

    #[test]
    fn time_validation_messages() {
        let d = date(2020, 1, 1);
        let msg = |h, m, s, us| DateTime::new(d, h, m, s, us, None).unwrap_err().to_string();
        assert_eq!(msg(24, 0, 0, 0), "hour must be in 0..23");
        assert_eq!(msg(1, 60, 0, 0), "minute must be in 0..59");
        assert_eq!(msg(1, 1, 60, 0), "second must be in 0..59");
        assert_eq!(msg(1, 1, 1, 1_000_000), "microsecond must be in 0..999999");
    }

    #[test]
    fn aware_equality_uses_instant() {
        let d = date(2020, 1, 1);
        let plus1 = TzOffset::from_hours_minutes(false, 1, 0).unwrap();
        let a = DateTime::new(d, 10, 0, 0, 0, Some(plus1)).unwrap();
        let b = DateTime::new(d, 9, 0, 0, 0, Some(TzOffset::UTC)).unwrap();
        let naive = DateTime::new(d, 9, 0, 0, 0, None).unwrap();
        assert_eq!(a, b);
        assert_ne!(b, naive);
        let hash = |dt: &DateTime| {
            let mut h = std::hash::DefaultHasher::new();
            dt.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash(&a), hash(&b));
    }
}
