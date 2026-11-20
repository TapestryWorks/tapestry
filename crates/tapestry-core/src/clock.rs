//! Injectable wall clock used to give the model the current date and time zone.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, FixedOffset, Local};

/// Local calendar date and time zone at one instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockReading {
    /// Local calendar date, `YYYY-MM-DD`.
    pub local_date: String,
    /// IANA name when available, otherwise a UTC offset such as `+08:00`.
    pub timezone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("clock unavailable: {0}")]
pub struct ClockError(pub String);

/// Source of the current local date, time zone, and event timestamps.
pub trait Clock: Send + Sync {
    fn now(&self) -> Result<ClockReading, ClockError>;
    /// Milliseconds since the Unix epoch, used to timestamp events.
    fn unix_millis(&self) -> u64;
}

/// Reads the operating system clock and time zone.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl SystemClock {
    fn reading_from(
        now: DateTime<FixedOffset>,
        iana: Result<String, iana_time_zone::GetTimezoneError>,
    ) -> ClockReading {
        let timezone = match iana {
            Ok(name) if !name.trim().is_empty() => name,
            _ => now.format("%:z").to_string(),
        };
        ClockReading {
            local_date: now.format("%Y-%m-%d").to_string(),
            timezone,
        }
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Result<ClockReading, ClockError> {
        let now = Local::now().fixed_offset();
        Ok(Self::reading_from(now, iana_time_zone::get_timezone()))
    }

    fn unix_millis(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default()
    }
}

/// Programmable clock for tests.
#[derive(Debug)]
pub struct FixedClock {
    state: Mutex<Result<ClockReading, ClockError>>,
    millis: AtomicU64,
}

impl FixedClock {
    pub fn new(local_date: impl Into<String>, timezone: impl Into<String>) -> Self {
        Self {
            state: Mutex::new(Ok(ClockReading {
                local_date: local_date.into(),
                timezone: timezone.into(),
            })),
            millis: AtomicU64::new(0),
        }
    }

    /// Set the value returned by [`Clock::unix_millis`] (defaults to 0).
    pub fn set_unix_millis(&self, millis: u64) {
        self.millis.store(millis, Ordering::SeqCst);
    }

    pub fn set(&self, local_date: impl Into<String>, timezone: impl Into<String>) {
        *self.state.lock().unwrap() = Ok(ClockReading {
            local_date: local_date.into(),
            timezone: timezone.into(),
        });
    }

    pub fn fail(&self) {
        *self.state.lock().unwrap() = Err(ClockError("fixed clock set to fail".into()));
    }
}

impl Clock for FixedClock {
    fn now(&self) -> Result<ClockReading, ClockError> {
        self.state.lock().unwrap().clone()
    }

    fn unix_millis(&self) -> u64 {
        self.millis.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(s).unwrap()
    }

    #[test]
    fn reading_uses_iso_date_and_iana_name() {
        let r =
            SystemClock::reading_from(at("2026-11-05T00:30:00+08:00"), Ok("Asia/Shanghai".into()));
        assert_eq!(r.local_date, "2026-11-05");
        assert_eq!(r.timezone, "Asia/Shanghai");
    }

    #[test]
    fn reading_falls_back_to_utc_offset() {
        let err = iana_time_zone::GetTimezoneError::OsError;
        let r = SystemClock::reading_from(at("2026-11-05T00:30:00+08:00"), Err(err));
        assert_eq!(r.timezone, "+08:00");
        let r = SystemClock::reading_from(at("2026-11-05T00:30:00-03:30"), Ok(" ".into()));
        assert_eq!(r.timezone, "-03:30");
    }

    #[test]
    fn system_clock_returns_well_formed_reading() {
        let r = SystemClock.now().unwrap();
        assert_eq!(r.local_date.len(), 10);
        assert!(!r.timezone.is_empty());
        // After 2020-01-01.
        assert!(SystemClock.unix_millis() > 1_577_836_800_000);
    }

    #[test]
    fn fixed_clock_returns_configured_millis() {
        let clock = FixedClock::new("2026-11-04", "UTC");
        assert_eq!(clock.unix_millis(), 0);
        clock.set_unix_millis(1_700_000_000_123);
        assert_eq!(clock.unix_millis(), 1_700_000_000_123);
    }

    #[test]
    fn fixed_clock_set_and_fail() {
        let clock = FixedClock::new("2026-11-04", "UTC");
        assert_eq!(clock.now().unwrap().local_date, "2026-11-04");
        clock.set("2026-11-05", "Asia/Shanghai");
        assert_eq!(clock.now().unwrap().timezone, "Asia/Shanghai");
        clock.fail();
        assert!(clock.now().is_err());
        clock.set("2026-11-06", "UTC");
        assert!(clock.now().is_ok());
    }
}
