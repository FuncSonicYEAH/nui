//! Numeric helpers: interpolation, normalization, and step-grid snapping.
//!
//! These are the pieces that were being written more than once. The
//! merge is deliberate: `lerp_f32` / `lerp_f64` replace a private
//! `lerp_component` in `nui-core/src/color.rs` and two closures spelled
//! `mix` (`nui-runtime/src/animation.rs`, `nui-render/src/scene.rs`), and
//! [`progress`] + [`inverse_lerp`] replace `slider_fraction`
//! (`nui-render/src/widget/slider.rs`) and `progress_along`
//! (`nui-render/src/widget/scrollbar.rs`).

/// Linear interpolation: `from` at `t = 0`, `to` at `t = 1`.
///
/// `t` is **not** clamped — extrapolation is a feature here, because an
/// overshooting easing curve hands out `t > 1` and the caller wants the
/// overshoot. Use [`inverse_lerp`] or [`progress`] when a `0..=1` result
/// is what is wanted.
pub fn lerp_f64(from: f64, to: f64, t: f64) -> f64 {
    return from + (to - from) * t;
}

/// [`lerp_f64`] for `f32`. See it for the clamping contract.
pub fn lerp_f32(from: f32, to: f32, t: f32) -> f32 {
    return from + (to - from) * t;
}

/// Where `value` sits between `minimum` and `maximum`, as `0..=1`.
///
/// The inverse of [`lerp_f32`]: `lerp_f32(minimum, maximum, inverse_lerp(value, minimum, maximum))`
/// recovers `value` for any `value` inside the range.
///
/// A zero-width range pins to `0.0` rather than producing `NaN`. That is
/// the interesting case: a range that has not been resolved yet (a
/// `Slider` whose `step` is still being read, a container whose content
/// height is not known on the first pass) reports a span of exactly zero,
/// and `0.0` is the only answer that keeps the caller drawing something
/// sane.
///
/// Infinities are *not* special-cased: an infinite span yields `0.0` for
/// a finite `value` (correct — it is an infinitesimal fraction of the
/// way along), and an infinite `value` yields `NaN`, which is the honest
/// answer for "infinity divided by infinity".
pub fn inverse_lerp(value: f32, minimum: f32, maximum: f32) -> f32 {
    let span = maximum - minimum;
    if span == 0.0 {
        return 0.0;
    }
    return ((value - minimum) / span).clamp(0.0, 1.0);
}

/// `offset / travel` clamped to `0..=1`, guarding the degenerate lane.
///
/// This is [`inverse_lerp`] with the origin shifted to zero, kept as its
/// own name because the scroll call sites read as "how far along the
/// travel is this offset" rather than as a range mapping, and because the
/// degenerate case is different: `travel` is a *length*, so a negative or
/// non-finite one is meaningless input rather than a legitimately empty
/// range, and it is rejected the same way zero is.
///
/// `NaN` travels through: `NaN.clamp(0.0, 1.0)` is `NaN`, and a `NaN`
/// offset is a bug in the caller that should stay visible rather than
/// silently become "at the start".
pub fn progress(offset: f32, travel: f32) -> f32 {
    if !travel.is_finite() || travel <= 0.0 {
        return 0.0;
    }
    return (offset / travel).clamp(0.0, 1.0);
}

/// Snaps `value` to the nearest multiple of `step` counted from `origin`.
///
/// `origin` is the grid's anchor, not an offset to add afterwards:
/// `step_grid(7.0, 5.0, 2.0)` is `7.0` (the grid is …5, 7, 9…), not
/// `7.0 + 5.0`. A `step` of zero or less is not a grid at all and
/// returns `value` unchanged, so a caller that has not resolved its step
/// yet gets its input back instead of a division by zero.
pub fn step_grid(value: f64, origin: f64, step: f64) -> f64 {
    if !step.is_finite() || step <= 0.0 {
        return value;
    }
    return origin + ((value - origin) / step).round() * step;
}

/// Decimal places `step` needs before it is an integer, capped at 9.
///
/// The cap is what stops a pathological value from asking for an unbounded
/// string; the tolerance is what absorbs the representation error of the
/// scaled value (`0.1 * 10` is not exactly `1`).
pub fn decimal_places(value: f64) -> u32 {
    if !value.is_finite() || value == 0.0 {
        return 0;
    }
    for places in 0..=9u32 {
        let scaled = value * 10f64.powi(places as i32);
        if (scaled - scaled.round()).abs() < 1e-6 {
            return places;
        }
    }
    return 9;
}

/// Rounds `value` to `step`'s own decimal precision, dropping the noise a
/// fraction accumulates.
///
/// Without this, stepping a `0.1` grid four times lands on
/// `0.30000000000000004` and the label grows a tail. This is a *display*
/// rounding and not a snap: it does not move the value onto the grid, it
/// only cleans up digits the grid already implies. Use [`step_grid`] when
/// the value itself has to land on the grid.
pub fn round_to_grid(value: f64, step: f64) -> f64 {
    let places = decimal_places(step);
    if places == 0 {
        return value.round();
    }
    let factor = 10f64.powi(places as i32);
    return (value * factor).round() / factor;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn lerp_hits_both_ends_and_extrapolates() {
        assert_eq!(lerp_f64(0.0, 10.0, 0.0), 0.0);
        assert_eq!(lerp_f64(0.0, 10.0, 1.0), 10.0);
        assert_eq!(lerp_f64(0.0, 10.0, 0.5), 5.0);
        // Beyond the ends: an overshooting easing curve relies on this.
        assert_eq!(lerp_f64(0.0, 10.0, 1.5), 15.0);
        assert_eq!(lerp_f32(2.0, 4.0, 0.25), 2.5);
        // Reversed endpoints ("from" above "to") are not special.
        assert_eq!(lerp_f64(10.0, 0.0, 0.25), 7.5);
    }

    #[test]
    fn inverse_lerp_clamps_and_survives_a_zero_span() {
        assert_eq!(inverse_lerp(50.0, 0.0, 100.0), 0.5);
        assert_eq!(inverse_lerp(-10.0, 0.0, 100.0), 0.0);
        assert_eq!(inverse_lerp(500.0, 0.0, 100.0), 1.0);
        // A range that has not resolved yet: pinned, not NaN.
        assert_eq!(inverse_lerp(5.0, 5.0, 5.0), 0.0);
        assert!(!inverse_lerp(5.0, 5.0, 5.0).is_nan());
        // The endpoints land exactly, which is what makes it a true inverse.
        assert_eq!(inverse_lerp(0.0, 0.0, 100.0), 0.0);
        assert_eq!(inverse_lerp(100.0, 0.0, 100.0), 1.0);
    }

    #[test]
    fn inverse_lerp_round_trips_through_lerp() {
        // The invariant that lets a painter and a hit test share one
        // mapping without drifting apart.
        for step in 0..=10 {
            let value = -20.0 + step as f32 * 4.5;
            let fraction = inverse_lerp(value, -20.0, 25.0);
            let back = lerp_f32(-20.0, 25.0, fraction);
            assert!(
                (back - value).abs() < 1e-4,
                "{value} -> {fraction} -> {back}"
            );
        }
    }

    #[test]
    fn a_negative_span_still_maps_monotonically() {
        // Not a use case today, but the clamp must not turn it into garbage:
        // a range written the wrong way round reads backwards, not randomly.
        assert_eq!(inverse_lerp(10.0, 100.0, 0.0), 0.9);
        assert_eq!(inverse_lerp(0.0, 100.0, 0.0), 1.0);
    }

    #[test]
    fn progress_guards_the_degenerate_lane() {
        assert_eq!(progress(50.0, 100.0), 0.5);
        assert_eq!(progress(-5.0, 100.0), 0.0, "above the top clamps");
        assert_eq!(progress(150.0, 100.0), 1.0, "past the bottom clamps");
        // No travel: an empty scrollbar, a slider with no lane.
        assert_eq!(progress(50.0, 0.0), 0.0);
        assert_eq!(progress(50.0, -1.0), 0.0);
        // A non-finite travel is bad input, not a wide lane.
        assert_eq!(progress(50.0, f32::NAN), 0.0);
        assert_eq!(progress(50.0, f32::INFINITY), 0.0);
        // But a non-finite offset stays visible rather than reading as 0.
        assert!(progress(f32::NAN, 100.0).is_nan());
    }

    #[test]
    fn progress_and_inverse_lerp_agree_where_they_overlap() {
        // The two existed separately; this is the evidence that merging
        // their arithmetic did not change either one's answers.
        for offset in [0.0f32, 12.5, 50.0, 99.0, 100.0, 1000.0] {
            let from_progress = progress(offset, 100.0);
            let from_inverse = inverse_lerp(offset, 0.0, 100.0);
            assert_eq!(from_progress, from_inverse, "offset {offset}");
        }
    }

    #[test]
    fn step_grid_snaps_to_the_origin_not_to_zero() {
        // The grid here is …, 5, 7, 9, … so each value snaps up or down to
        // whichever neighbour is nearer *in grid terms*.
        assert_eq!(step_grid(7.0, 5.0, 2.0), 7.0, "already on the grid");
        assert_eq!(step_grid(6.8, 5.0, 2.0), 7.0, "nearer 7");
        assert_eq!(step_grid(5.2, 5.0, 2.0), 5.0, "nearer 5");
        // The boundary is 6.0, and it is an exact tie: `(6.0 - 5.0) / 2.0`
        // is 0.5, and `f64::round` breaks halves away from zero, so a tie
        // goes *up*. Worth pinning — it is the reason `6.0` snaps to 7
        // while `5.9` snaps to 5, the two sitting either side of it.
        assert_eq!(step_grid(5.9, 5.0, 2.0), 5.0, "just below the tie");
        assert_eq!(step_grid(6.0, 5.0, 2.0), 7.0, "an exact tie rounds up");
        assert_eq!(step_grid(6.1, 5.0, 2.0), 7.0, "just above the tie");
        // Anchored at 0 the grid is the plain multiples.
        assert_eq!(step_grid(2.4, 0.0, 1.0), 2.0);
        assert_eq!(step_grid(2.6, 0.0, 1.0), 3.0);
        assert_eq!(step_grid(-2.4, 0.0, 1.0), -2.0, "ties round away from zero");
        // A grid offset from zero: …, 0.2, 0.7, 1.2, 1.7, … Note a point
        // below the origin still snaps to a grid line rather than to zero.
        assert_eq!(step_grid(1.6, 0.7, 0.5), 1.7);
        // And the arithmetic is exact only as far as `f64` allows: two
        // steps below the origin lands on 0.19999999999999996, not 0.2,
        // because 0.7 - 1.0 does not round to 0.2. That is this function
        // doing its job (`origin + ((-1.0).round() * 0.5)`), and it is why
        // `round_to_grid` exists alongside it — a *display* of this value
        // wants the digits cleaned, the value on the grid does not.
        let below = step_grid(0.2, 0.7, 0.5);
        assert!(
            (below - 0.2).abs() < 1e-15,
            "two steps below the origin, got {below}"
        );
        assert_eq!(round_to_grid(below, 0.5), 0.2);
        // Off-grid points choose the nearer line.
        assert_eq!(step_grid(0.65, 0.7, 0.5), 0.7, "nearest line above is 0.7");
    }

    #[test]
    fn step_grid_passes_through_a_missing_step() {
        // A caller whose step is not resolved yet must not get a panic or a
        // silent zero.
        assert_eq!(step_grid(3.7, 0.0, 0.0), 3.7);
        assert_eq!(step_grid(3.7, 0.0, -1.0), 3.7);
        assert_eq!(step_grid(3.7, 0.0, f64::NAN), 3.7);
        assert_eq!(step_grid(3.7, 0.0, f64::INFINITY), 3.7);
    }

    #[test]
    fn decimal_places_reads_the_grid_behind_a_step() {
        assert_eq!(decimal_places(1.0), 0);
        assert_eq!(decimal_places(0.5), 1);
        assert_eq!(decimal_places(0.25), 2);
        assert_eq!(decimal_places(0.1), 1);
        assert_eq!(decimal_places(0.001), 3);
        // Nothing sensible to report.
        assert_eq!(decimal_places(0.0), 0);
        assert_eq!(decimal_places(f64::NAN), 0);
        assert_eq!(decimal_places(f64::INFINITY), 0);
        // A value that never resolves is capped rather than looping.
        assert_eq!(decimal_places(1.0 / 3.0), 9);
    }

    #[test]
    fn round_to_grid_drops_the_noise_a_fraction_accumulates() {
        // The bug this exists for: a 0.1 step accumulating representation
        // error. Note it is *not* every multiple — 0.1 × 4 is exactly 0.4,
        // because 0.4 is a dyadic rational and the sum happens to be exact.
        // 0.3 is the one that drifts, so it is the one worth pinning.
        let drifted = 0.1f64 + 0.1 + 0.1;
        assert_ne!(drifted, 0.3, "the drift is real");
        assert_eq!(round_to_grid(drifted, 0.1), 0.3);
        assert_eq!(round_to_grid(0.30000000000000004, 0.1), 0.3);
        // An integer step rounds to whole numbers.
        assert_eq!(round_to_grid(2.6, 1.0), 3.0);
        assert_eq!(round_to_grid(2.4, 1.0), 2.0);
        // A quarter step keeps its quarters.
        assert_eq!(round_to_grid(1.25, 0.25), 1.25);
    }

    #[test]
    fn round_to_grid_is_a_rounding_not_a_snap() {
        // It cleans digits the grid implies; it does not move a value onto
        // the grid. `step_grid` is the one that does that.
        assert_eq!(round_to_grid(0.37, 0.1), 0.4);
        assert_eq!(step_grid(0.37, 0.0, 0.1), 0.4);
        // Off-grid in a way rounding cannot hide:
        assert_eq!(round_to_grid(0.34, 0.1), 0.3);
        assert_eq!(step_grid(0.34, 0.0, 0.25), 0.25);
    }

    #[test]
    fn a_lone_sign_is_not_a_number_but_it_is_not_an_error_either() {
        // Documented on the text predicates but asserted here too, because
        // the two spellings of "not a number" must not be confused: an
        // empty field is valid, a lone sign is invalid.
        assert!(crate::text::is_integer(""));
        assert!(!crate::text::is_integer("-"));
    }
}
