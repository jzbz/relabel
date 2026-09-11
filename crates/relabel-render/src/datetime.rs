//! Dates and times for `{dt}`, formatted as niimblue formats them.
//!
//! niimblue fills `{dt|FORMAT}` with dayjs 1.11's `format()` in English, and a
//! label brought over from it has to print the same. So this is that
//! formatter: the same tokens, tried in the same order at each position, with
//! the same outputs and the same quirks. A letter that is not a token is copied
//! through, and one that is gets replaced, so an English word in a format
//! comes out mangled - `Today` prints as `To5pmy` - unless it is bracketed as
//! `[Today]`.
//!
//! relabel-render has no clock. The caller reads the time and the time zone
//! once for a whole job and passes a [`CivilTime`], so preflight checks exactly
//! the dates that print. niimblue reads the clock again for every placeholder
//! and every page, so its preview can disagree with its printout.
//!
//! One deliberate difference: the UTC offset prints exactly as given, where
//! dayjs rounds it to a quarter hour. They differ only for the local mean time
//! some zones used before 1972.

/// niimblue's default for a bare `{dt}`, which is not dayjs's own.
pub const DEFAULT_DT_FORMAT: &str = "YYYY-MM-DD HH:mm:ss";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

/// A local date and time, and the UTC offset in force at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilTime {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    milli: u16,
    offset_min: i16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClockError {
    #[error("{field} {value} is out of range")]
    OutOfRange { field: &'static str, value: i64 },
    #[error("{year:04}-{month:02}-{day:02} is not a date")]
    NoSuchDay { year: u16, month: u8, day: u8 },
}

fn is_leap(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a proleptic-Gregorian date: Howard Hinnant's
/// `days_from_civil`.
fn days_from_civil(year: u16, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(month) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

impl CivilTime {
    /// Years 0 to 9999, a real date, hours below 24, minutes and seconds below
    /// 60 - no leap second - milliseconds below 1000, and an offset of at most
    /// 23 hours 59 minutes either way.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        milli: u16,
        offset_min: i16,
    ) -> Result<CivilTime, ClockError> {
        let range = |field, value: i64, max: i64| {
            if (0..=max).contains(&value) {
                Ok(())
            } else {
                Err(ClockError::OutOfRange { field, value })
            }
        };
        range("year", i64::from(year), 9999)?;
        range("month", i64::from(month) - 1, 11).map_err(|_| ClockError::OutOfRange {
            field: "month",
            value: i64::from(month),
        })?;
        range("hour", i64::from(hour), 23)?;
        range("minute", i64::from(minute), 59)?;
        range("second", i64::from(second), 59)?;
        range("millisecond", i64::from(milli), 999)?;
        if offset_min.unsigned_abs() > 1439 {
            return Err(ClockError::OutOfRange {
                field: "UTC offset in minutes",
                value: i64::from(offset_min),
            });
        }
        if day == 0 || day > days_in_month(year, month) {
            return Err(ClockError::NoSuchDay { year, month, day });
        }
        Ok(CivilTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            milli,
            offset_min,
        })
    }

    /// The day of the week, 0 for Sunday. Derived, never supplied, so it
    /// cannot disagree with the date.
    pub fn weekday(&self) -> u8 {
        (days_from_civil(self.year, self.month, self.day) + 4).rem_euclid(7) as u8
    }

    fn zone(&self, colon: bool) -> String {
        let sign = if self.offset_min >= 0 { '+' } else { '-' };
        let m = self.offset_min.unsigned_abs();
        let sep = if colon { ":" } else { "" };
        format!("{sign}{:02}{sep}{:02}", m / 60, m % 60)
    }
}

/// Format a time as niimblue's `{dt|filter}` would. No filter, or an empty
/// one, means [`DEFAULT_DT_FORMAT`].
///
/// Linear in the filter's length: each token is at most four characters, and
/// the end of a `[...]` escape is found by a cursor that only moves forward.
pub fn format_dt(now: &CivilTime, filter: Option<&str>) -> String {
    let f = match filter {
        Some(f) if !f.is_empty() => f,
        _ => DEFAULT_DT_FORMAT,
    };
    let chars: Vec<char> = f.chars().collect();
    let mut out = String::with_capacity(f.len() + 8);
    // The next `]` at or after the scan position, or `chars.len()` for none.
    let mut close = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '[' {
            // `\[([^\]]+)]`: at least one character, up to the first `]`.
            if close <= i {
                close = (i + 1..chars.len())
                    .find(|&j| chars[j] == ']')
                    .unwrap_or(chars.len());
            }
            if close < chars.len() && close > i + 1 {
                out.extend(&chars[i + 1..close]);
                i = close + 1;
                continue;
            }
        }
        let run = chars[i..].iter().take(4).take_while(|&&x| x == c).count();
        let take = match c {
            'Y' if run >= 4 => 4,
            'Y' if run >= 2 => 2,
            'M' | 'd' => run,
            'D' | 'H' | 'h' | 'm' | 's' | 'Z' => run.min(2),
            'a' | 'A' => 1,
            'S' if run >= 3 => 3,
            _ => 0,
        };
        if take == 0 {
            out.push(c);
            i += 1;
            continue;
        }
        push_token(&mut out, now, c, take);
        i += take;
    }
    out
}

fn push_token(out: &mut String, t: &CivilTime, c: char, n: usize) {
    let hour12 = match t.hour % 12 {
        0 => 12,
        h => h,
    };
    let weekday = WEEKDAYS[usize::from(t.weekday())];
    let month = MONTHS[usize::from(t.month - 1)];
    let s = match (c, n) {
        ('Y', 2) => {
            let y = t.year.to_string();
            y[y.len().saturating_sub(2)..].to_string()
        }
        ('Y', _) => format!("{:04}", t.year),
        ('M', 1) => t.month.to_string(),
        ('M', 2) => format!("{:02}", t.month),
        ('M', 3) => month[..3].to_string(),
        ('M', _) => month.to_string(),
        ('D', 1) => t.day.to_string(),
        ('D', _) => format!("{:02}", t.day),
        ('d', 1) => t.weekday().to_string(),
        ('d', 2) => weekday[..2].to_string(),
        ('d', 3) => weekday[..3].to_string(),
        ('d', _) => weekday.to_string(),
        ('H', 1) => t.hour.to_string(),
        ('H', _) => format!("{:02}", t.hour),
        ('h', 1) => hour12.to_string(),
        ('h', _) => format!("{hour12:02}"),
        ('a', _) => (if t.hour < 12 { "am" } else { "pm" }).to_string(),
        ('A', _) => (if t.hour < 12 { "AM" } else { "PM" }).to_string(),
        ('m', 1) => t.minute.to_string(),
        ('m', _) => format!("{:02}", t.minute),
        ('s', 1) => t.second.to_string(),
        ('s', _) => format!("{:02}", t.second),
        ('S', _) => format!("{:03}", t.milli),
        ('Z', 1) => t.zone(true),
        ('Z', _) => t.zone(false),
        _ => c.to_string(),
    };
    out.push_str(&s);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fri 2026-09-11 16:05:09.042 at +02:00, as the niimblue research ran it.
    fn n1() -> CivilTime {
        CivilTime::new(2026, 9, 11, 16, 5, 9, 42, 120).unwrap()
    }

    /// Sun 2025-01-05 00:07:03.007 at +01:00.
    fn n2() -> CivilTime {
        CivilTime::new(2025, 1, 5, 0, 7, 3, 7, 60).unwrap()
    }

    fn f(t: &CivilTime, s: &str) -> String {
        format_dt(t, Some(s))
    }

    #[test]
    fn every_token_at_two_instants() {
        let tokens = [
            "YY", "YYYY", "M", "MM", "MMM", "MMMM", "D", "DD", "d", "dd", "ddd", "dddd", "H", "HH",
            "h", "hh", "a", "A", "m", "mm", "s", "ss", "SSS", "Z", "ZZ",
        ];
        let at_n1 = [
            "26",
            "2026",
            "9",
            "09",
            "Sep",
            "September",
            "11",
            "11",
            "5",
            "Fr",
            "Fri",
            "Friday",
            "16",
            "16",
            "4",
            "04",
            "pm",
            "PM",
            "5",
            "05",
            "9",
            "09",
            "042",
            "+02:00",
            "+0200",
        ];
        let at_n2 = [
            "25", "2025", "1", "01", "Jan", "January", "5", "05", "0", "Su", "Sun", "Sunday", "0",
            "00", "12", "12", "am", "AM", "7", "07", "3", "03", "007", "+01:00", "+0100",
        ];
        for (i, tok) in tokens.iter().enumerate() {
            assert_eq!(f(&n1(), tok), at_n1[i], "{tok} at N1");
            assert_eq!(f(&n2(), tok), at_n2[i], "{tok} at N2");
        }
    }

    #[test]
    fn greedy_runs_escapes_and_literals() {
        let n1 = n1();
        for (fmt, want) in [
            ("YYYY[-]MM", "2026-09"),
            ("YYYY-MM-DDTHH:mm:ssZ", "2026-09-11T16:05:09+02:00"),
            ("YYY", "26Y"),
            ("YYYYY", "2026Y"),
            ("MMMMM", "September9"),
            ("DDD", "1111"),
            ("ddddd", "Friday5"),
            ("SSSS", "042S"),
            ("ZZZ", "+0200+02:00"),
            ("S SS Do Q X", "S SS 11o Q X"),
            ("[]", "[]"),
            ("[[a]", "[a"),
            ("[a]b]", "ab]"),
            ("[a", "[pm"),
            ("[\n]", "\n"),
            ("MMM MMMM ddd dddd d h a", "Sep September Fri Friday 5 4 pm"),
            ("Today", "To5pmy"),
            ("[Today]", "Today"),
        ] {
            assert_eq!(f(&n1, fmt), want, "{fmt:?}");
        }
        assert_eq!(f(&n2(), "hh:mm A dd"), "12:07 AM Su");
    }

    #[test]
    fn default_format() {
        assert_eq!(format_dt(&n1(), None), "2026-09-11 16:05:09");
        assert_eq!(format_dt(&n1(), Some("")), "2026-09-11 16:05:09");
    }

    #[test]
    fn zones_hours_and_years() {
        let at = |off| CivilTime::new(2026, 7, 1, 12, 0, 0, 0, off).unwrap();
        for (off, want) in [
            (330, "+05:30 +0530"),
            (-150, "-02:30 -0230"),
            (0, "+00:00 +0000"),
            (825, "+13:45 +1345"),
            (-1, "-00:01 -0001"),
        ] {
            assert_eq!(f(&at(off), "Z ZZ"), want, "{off}");
        }
        for (hour, want) in [
            (0, "12 am"),
            (11, "11 am"),
            (12, "12 pm"),
            (13, "1 pm"),
            (23, "11 pm"),
        ] {
            let t = CivilTime::new(2026, 7, 1, hour, 0, 0, 0, 0).unwrap();
            assert_eq!(f(&t, "h a"), want, "{hour}");
        }
        let y = |year| CivilTime::new(year, 3, 1, 0, 0, 0, 0, 0).unwrap();
        assert_eq!(f(&y(999), "YYYY YY"), "0999 99");
        assert_eq!(f(&y(5), "YYYY YY"), "0005 5");
    }

    #[test]
    fn weekdays_come_from_the_date() {
        let wd = |y, m, d| CivilTime::new(y, m, d, 0, 0, 0, 0, 0).unwrap().weekday();
        assert_eq!(wd(1970, 1, 1), 4);
        assert_eq!(wd(2000, 2, 29), 2);
        assert_eq!(wd(2026, 9, 11), 5);
        assert_eq!(wd(1, 1, 1), 1);
    }

    #[test]
    fn impossible_times_are_refused() {
        assert!(CivilTime::new(2026, 13, 1, 0, 0, 0, 0, 0).is_err());
        assert!(CivilTime::new(2026, 0, 1, 0, 0, 0, 0, 0).is_err());
        assert_eq!(
            CivilTime::new(2025, 2, 29, 0, 0, 0, 0, 0),
            Err(ClockError::NoSuchDay {
                year: 2025,
                month: 2,
                day: 29
            })
        );
        assert!(CivilTime::new(2024, 2, 29, 0, 0, 0, 0, 0).is_ok());
        assert!(CivilTime::new(1900, 2, 29, 0, 0, 0, 0, 0).is_err());
        assert!(CivilTime::new(2026, 1, 1, 0, 0, 60, 0, 0).is_err());
        assert!(CivilTime::new(2026, 1, 1, 24, 0, 0, 0, 0).is_err());
        assert!(CivilTime::new(2026, 1, 1, 0, 0, 0, 1000, 0).is_err());
        assert!(CivilTime::new(2026, 1, 1, 0, 0, 0, 0, 1440).is_err());
        assert!(CivilTime::new(2026, 1, 1, 0, 0, 0, 0, -1440).is_err());
        assert!(CivilTime::new(10_000, 1, 1, 0, 0, 0, 0, 0).is_err());
    }

    /// A megabyte of escapes that never close, or of plain letters, formats
    /// in linear time.
    #[test]
    fn the_formatter_is_linear() {
        let t = n1();
        for s in [
            "[".repeat(1 << 20),
            "x".repeat(1 << 20),
            "[a".repeat(1 << 19),
        ] {
            let start = std::time::Instant::now();
            let out = format_dt(&t, Some(&s));
            assert!(out.len() >= s.len());
            assert!(start.elapsed().as_secs() < 2, "{:?}", start.elapsed());
        }
    }
}
