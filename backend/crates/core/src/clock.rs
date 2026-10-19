//! Injected "now". All time-dependent code (jobs, replay shifting, tests) goes through [`Clock`].

use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// Wall clock.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A settable clock for tests.
#[derive(Debug)]
pub struct FixedClock(Mutex<DateTime<Utc>>);

impl FixedClock {
    pub fn new(at: DateTime<Utc>) -> Self {
        Self(Mutex::new(at))
    }

    pub fn set(&self, at: DateTime<Utc>) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = at;
    }

    pub fn advance(&self, by: Duration) {
        let mut g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        *g += by;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap_or_else(|p| p.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn fixed_clock_is_settable() {
        let t0 = Utc.with_ymd_and_hms(2026, 10, 17, 12, 0, 0).unwrap();
        let c = FixedClock::new(t0);
        assert_eq!(c.now(), t0);
        c.advance(Duration::minutes(5));
        assert_eq!(c.now(), t0 + Duration::minutes(5));
        c.set(t0);
        assert_eq!(c.now(), t0);
    }

    #[test]
    fn system_clock_is_close_to_utc_now() {
        let d = (SystemClock.now() - Utc::now()).num_seconds().abs();
        assert!(d < 2);
    }
}
