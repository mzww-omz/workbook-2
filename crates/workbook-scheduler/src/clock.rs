use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};

/// Injectable time source. Production uses the system clock; tests use
/// `FixedClock` so 1 minute, 20 minutes, and the 4 a.m. boundary can be tested
/// without sleeping.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug)]
pub struct FixedClock {
    now: Mutex<DateTime<Utc>>,
}

impl FixedClock {
    pub fn new(now: DateTime<Utc>) -> Self {
        Self {
            now: Mutex::new(now),
        }
    }

    pub fn set(&self, now: DateTime<Utc>) {
        *self.now.lock().expect("fixed clock poisoned") = now;
    }

    pub fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().expect("fixed clock poisoned");
        *now += duration;
    }
}

impl Clock for FixedClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("fixed clock poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_clock_can_advance_without_waiting() {
        let start = Utc::now();
        let clock = FixedClock::new(start);
        clock.advance(Duration::minutes(20));
        assert_eq!(clock.now(), start + Duration::minutes(20));
    }
}
