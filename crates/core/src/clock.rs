//! The [`Clock`] port. Time is injected, never read ambiently, so SLA-expiry logic in
//! the `consequence` engine is deterministic and testable without `sleep` (TESTING.md).

use chrono::{DateTime, Utc};

/// A source of the current time. Production uses [`SystemClock`]; tests use a fixed clock.
pub trait Clock: Send + Sync {
    /// The current instant in UTC.
    fn now(&self) -> DateTime<Utc>;
}

/// The real wall-clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic clock for tests (mirrors what each crate uses for SLA tests).
    struct FixedClock(DateTime<Utc>);
    impl Clock for FixedClock {
        fn now(&self) -> DateTime<Utc> {
            self.0
        }
    }

    #[test]
    fn fixed_clock_is_deterministic() {
        let t = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let c = FixedClock(t);
        assert_eq!(c.now(), t);
        assert_eq!(c.now(), c.now());
    }
}
