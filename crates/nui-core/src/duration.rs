//! Duration type: time spans for animations and timers.

use std::fmt;

/// Milliseconds in one second.
const MILLIS_PER_SEC: f64 = 1000.0;

/// Time span stored as `f64` milliseconds.
///
/// [`std::time::Duration`] is deliberately not reused: animation
/// interpolation needs fractional milliseconds and arbitrary scaling, and the
/// integer-nanosecond type would require conversion on every per-frame
/// operation. Callers must pass finite values; NaN makes every ordering
/// comparison false.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Duration {
    milliseconds: f64,
}

impl Duration {
    /// Zero duration (animation start point, timer default).
    pub const ZERO: Duration = Duration { milliseconds: 0.0 };

    /// Constructs from milliseconds.
    pub const fn from_millis(milliseconds: f64) -> Duration {
        return Duration { milliseconds };
    }

    /// Constructs from seconds.
    pub const fn from_secs(seconds: f64) -> Duration {
        return Duration {
            milliseconds: seconds * MILLIS_PER_SEC,
        };
    }

    /// Milliseconds.
    pub const fn as_millis_f64(&self) -> f64 {
        return self.milliseconds;
    }

    /// Seconds.
    pub const fn as_secs_f64(&self) -> f64 {
        return self.milliseconds / MILLIS_PER_SEC;
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(formatter, "{}ms", self.milliseconds);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secs_convert_to_millis_exactly() {
        let duration = Duration::from_secs(0.25);
        assert_eq!(duration.as_millis_f64(), 250.0);
        assert_eq!(duration.as_secs_f64(), 0.25);
    }

    #[test]
    fn zero_equals_default() {
        assert_eq!(Duration::ZERO, Duration::default());
    }

    #[test]
    fn display_appends_ms_unit() {
        assert_eq!(Duration::from_millis(200.0).to_string(), "200ms");
    }

    #[test]
    fn ordering_compares_magnitude() {
        assert!(Duration::from_millis(100.0) < Duration::from_millis(200.0));
    }
}
