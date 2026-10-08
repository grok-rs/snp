//! The clock seam (S12): timing and timestamps are injectable in tests.

use std::time::{Instant, SystemTime};

/// A source of monotonic and wall-clock time.
pub trait Clock: Send + Sync + std::fmt::Debug {
    /// The current monotonic instant.
    fn now(&self) -> Instant;
    /// Seconds since the Unix epoch (0 for a clock before 1970).
    fn unix_secs(&self) -> u64;
}

/// The real clock.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn unix_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock() {
        let c = SystemClock;
        assert!(c.unix_secs() > 1_700_000_000);
        let a = c.now();
        assert!(c.now() >= a);
    }
}
