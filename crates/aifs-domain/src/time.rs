//! Millisecond UTC timestamps without pulling in a calendar library.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

const MS_PER_DAY: i64 = 86_400_000;
const CIVIL_EPOCH_OFFSET: i64 = 719_468;
const DAYS_PER_ERA: i64 = 146_097;

/// Milliseconds since the Unix epoch, UTC.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Returns the current wall-clock time.
    pub fn now() -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        Self(millis)
    }

    /// Builds a timestamp from a `SystemTime`, saturating outside the i64 range.
    pub fn from_system_time(value: SystemTime) -> Self {
        match value.duration_since(UNIX_EPOCH) {
            Ok(duration) => Self(i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)),
            Err(err) => Self(-i64::try_from(err.duration().as_millis()).unwrap_or(i64::MAX)),
        }
    }

    /// Returns the raw millisecond value.
    pub const fn as_millis(&self) -> i64 {
        self.0
    }
}

/// UTC civil `(year, month, day)` for unix milliseconds.
pub fn utc_ymd(millis: i64) -> Option<(i32, u8, u8)> {
    civil_from_days(millis.div_euclid(MS_PER_DAY))
}

/// `YYYY-MM` for unix milliseconds when the year fits a four-digit folder name.
pub fn utc_year_month_label(millis: i64) -> Option<String> {
    let (year, month, _) = utc_ymd(millis)?;
    if !(1..=9999).contains(&year) {
        return None;
    }
    Some(format!("{year:04}-{month:02}"))
}

/// Parses `YYYY-MM-DD` (month 01–12, day 01–31).
pub fn parse_iso_date(value: &str) -> Option<(i32, u8, u8)> {
    parse_ymd_parts(value, 10, 7)
}

/// Parses `YYYY-MM` (month 01–12).
pub fn parse_iso_year_month(value: &str) -> Option<(i32, u8)> {
    parse_ymd_parts(value, 7, usize::MAX).map(|(year, month, _)| (year, month))
}

fn parse_ymd_parts(value: &str, expected_len: usize, day_dash: usize) -> Option<(i32, u8, u8)> {
    if value.len() != expected_len || !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-' {
        return None;
    }
    if expected_len == 10 && (day_dash != 7 || bytes[7] != b'-') {
        return None;
    }
    if !value[..4].bytes().all(|byte| byte.is_ascii_digit())
        || !value[5..7].bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let year: i32 = value[..4].parse().ok()?;
    let month: u8 = value[5..7].parse().ok()?;
    if !(1..=12).contains(&month) {
        return None;
    }
    if expected_len == 7 {
        return Some((year, month, 1));
    }
    if !value[8..10].bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let day: u8 = value[8..10].parse().ok()?;
    if !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// Howard Hinnant's `civil_from_days` (days since 1970-01-01).
fn civil_from_days(days: i64) -> Option<(i32, u8, u8)> {
    let z = days.checked_add(CIVIL_EPOCH_OFFSET)?;
    let era = z.div_euclid(DAYS_PER_ERA);
    let doe = z - era * DAYS_PER_ERA;
    if !(0..=146_096).contains(&doe) {
        return None;
    }
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe.checked_add(era.checked_mul(400)?)?;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    let year = i32::try_from(year).ok()?;
    let month = u8::try_from(month).ok()?;
    let day = u8::try_from(day).ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn now_is_after_2020() {
        assert!(Timestamp::now().as_millis() > 1_577_836_800_000);
    }

    #[test]
    fn from_system_time_handles_pre_epoch() {
        let before = UNIX_EPOCH - std::time::Duration::from_millis(1500);
        assert_eq!(Timestamp::from_system_time(before), Timestamp(-1500));
    }

    #[test]
    fn utc_ymd_known_unix_instants() {
        assert_eq!(utc_ymd(0), Some((1970, 1, 1)));
        assert_eq!(utc_ymd(MS_PER_DAY - 1), Some((1970, 1, 1)));
        assert_eq!(utc_ymd(MS_PER_DAY), Some((1970, 1, 2)));
        assert_eq!(utc_ymd(-1), Some((1969, 12, 31)));
        assert_eq!(utc_ymd(1_626_307_200_000), Some((2021, 7, 15)));
        assert_eq!(
            utc_year_month_label(1_626_307_200_000).as_deref(),
            Some("2021-07")
        );
    }

    #[test]
    fn parse_iso_date_accepts_ymd_and_rejects_junk() {
        assert_eq!(parse_iso_date("2021-07-15"), Some((2021, 7, 15)));
        assert_eq!(parse_iso_year_month("2021-07"), Some((2021, 7)));
        assert!(parse_iso_date("2021-13-01").is_none());
        assert!(parse_iso_date("not-a-date").is_none());
        assert!(parse_iso_date("2021/07/15").is_none());
        assert!(parse_iso_year_month("2021-13").is_none());
    }
}
