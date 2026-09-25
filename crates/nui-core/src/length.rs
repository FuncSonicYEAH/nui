//! Length type: dp / percent / auto.

use std::fmt;

/// Denominator base of percentages (100%).
const PERCENT_BASE: f32 = 100.0;

/// Layout length: a definite dp value, a percentage relative to the parent's
/// available space, or a decision delegated to the layout engine.
///
/// The engine works in dp (density-independent pixels) throughout; physical
/// pixel conversion happens only at render submission.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Length {
    /// Density-independent pixels.
    Dp(f32),
    /// Percentage of the available space (50.0 means 50%).
    Percent(f32),
    /// Decided by the layout engine from content and constraints.
    Auto,
}

impl Length {
    /// Zero length.
    pub const ZERO: Length = Length::Dp(0.0);

    /// dp length.
    pub const fn dp(dp: f32) -> Length {
        return Length::Dp(dp);
    }

    /// Percentage length (`percent` counts in percent, 50.0 = 50%).
    pub const fn percent(percent: f32) -> Length {
        return Length::Percent(percent);
    }

    /// auto length.
    pub const fn auto() -> Length {
        return Length::Auto;
    }

    /// Whether this is auto.
    pub const fn is_auto(self) -> bool {
        return matches!(self, Length::Auto);
    }

    /// Returns the definite dp value if this is `Dp`, otherwise `None`.
    pub const fn to_dp(self) -> Option<f32> {
        return match self {
            Length::Dp(dp) => Some(dp),
            Length::Percent(_) | Length::Auto => None,
        };
    }

    /// Resolves against `available_dp` (the parent's available space) into a
    /// dp value.
    ///
    /// `Auto` cannot be resolved without the layout engine and returns
    /// `None`; `Dp` is returned unchanged (negative or non-finite values are
    /// left for the layout engine to clamp).
    ///
    /// # Examples
    /// ```
    /// use nui_core::Length;
    /// assert_eq!(Length::Percent(50.0).resolve(420.0), Some(210.0));
    /// assert_eq!(Length::Auto.resolve(420.0), None);
    /// ```
    pub fn resolve(self, available_dp: f32) -> Option<f32> {
        return match self {
            Length::Dp(dp) => Some(dp),
            Length::Percent(percent) => Some(available_dp * percent / PERCENT_BASE),
            Length::Auto => None,
        };
    }
}

impl fmt::Display for Length {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return match self {
            Length::Dp(dp) => write!(formatter, "{dp}dp"),
            Length::Percent(percent) => write!(formatter, "{percent}%"),
            Length::Auto => write!(formatter, "auto"),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_dp_returns_value_unchanged() {
        assert_eq!(Length::dp(10.5).resolve(420.0), Some(10.5));
    }

    #[test]
    fn resolve_percent_uses_available_space() {
        assert_eq!(Length::percent(50.0).resolve(420.0), Some(210.0));
        assert_eq!(Length::percent(100.0).resolve(420.0), Some(420.0));
    }

    #[test]
    fn resolve_auto_has_no_solution() {
        assert_eq!(Length::Auto.resolve(420.0), None);
    }

    #[test]
    fn to_dp_extracts_only_dp_variant() {
        assert_eq!(Length::dp(7.0).to_dp(), Some(7.0));
        assert_eq!(Length::percent(50.0).to_dp(), None);
        assert_eq!(Length::Auto.to_dp(), None);
    }

    #[test]
    fn is_auto_matches_auto_variant() {
        assert!(Length::auto().is_auto());
        assert!(!Length::ZERO.is_auto());
    }

    #[test]
    fn display_uses_language_literal_forms() {
        assert_eq!(Length::dp(10.0).to_string(), "10dp");
        assert_eq!(Length::percent(50.0).to_string(), "50%");
        assert_eq!(Length::Auto.to_string(), "auto");
    }
}
