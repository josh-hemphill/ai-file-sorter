//! Millisecond UTC timestamps without pulling in a calendar library.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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
}
