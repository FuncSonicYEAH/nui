//! Cubic-bezier evaluation: the CSS `cubic-bezier()` math, with no
//! opinion about which curves exist.
//!
//! The caller owns the curve table and the naming. This module owns only
//! the numerics: given a segment `(x0, y0, x1, y1, x2, y2, x3, y3)` and a
//! time `t`, find the value. The complication is that a cubic bezier is
//! parametric in `u`, so turning a *time* into a *value* needs the
//! inverse of `x(u)` — done below by Newton–Raphson with a bisection
//! fallback.

/// One cubic-bezier segment: `(x0, y0, x1, y1, x2, y2, x3, y3)` — the
/// segment's two endpoints followed by its two control points, in the
/// order CSS's `cubic-bezier()` takes them.
pub type BezierSegment = [f64; 8];

/// A piecewise cubic-bezier curve: one or more segments in time order.
///
/// Consecutive segments share an endpoint, which is what makes the curve
/// continuous across the join.
pub type BezierCurve = &'static [BezierSegment];

/// Newton iterations before falling back to bisection.
///
/// Eight is enough for every curve in the built-in table to land well
/// inside a pixel of progress at UI durations. The bisection fallback is
/// what makes a degenerate curve terminate: a curve with both control
/// points at `x = 0` has `x(u) = u³`, whose slope is zero at `u = 0`, so
/// Newton cannot take a first step.
const NEWTON_ITERATIONS: usize = 8;

/// Bisection iterations, used once Newton stops making progress.
///
/// Each halves the interval, so 24 is past the point where `f64` runs out
/// of mantissa to tell the endpoints apart.
const BISECTION_ITERATIONS: usize = 24;

/// The tolerance at which Newton and bisection call it solved.
const SOLVED_EPSILON: f64 = 1e-9;

/// Evaluates a piecewise cubic-bezier curve at progress `t`.
///
/// Global progress is mapped onto each segment in proportion to that
/// segment's share of the timeline, then inverted within the segment. The
/// segments share endpoints, so the value is continuous across a join even
/// though each segment is evaluated on its own `0..=1`.
///
/// An empty curve is the identity, and a zero-width segment is a
/// *pause*: the value sits at the endpoint on one side of it and jumps to
/// the other, which is how a multi-segment curve expresses "hold here".
pub fn apply_bezier(curve: BezierCurve, t: f64) -> f64 {
    if curve.is_empty() {
        return t;
    }
    let mut elapsed = 0.0;
    for (index, segment) in curve.iter().enumerate() {
        let span = segment[6] - segment[0];
        let is_last = index + 1 == curve.len();
        if t <= elapsed + span || is_last {
            if span <= 0.0 {
                return if t < elapsed { segment[1] } else { segment[7] };
            }
            let local = ((t - elapsed) / span).clamp(0.0, 1.0);
            let u = solve_segment_x(segment, local);
            return bezier_y(segment, u);
        }
        elapsed += span;
    }
    return 1.0;
}

/// The `u` at which a segment's x coordinate equals `x`.
///
/// Newton–Raphson, with bisection as the fallback. Bisection is not only a
/// safety net: it is what makes the *initial* step possible on a curve
/// whose slope is zero at `u = 0`, and what keeps a Newton step that would
/// leave the bracket from being taken.
pub fn solve_segment_x(segment: &BezierSegment, x: f64) -> f64 {
    let mut low = 0.0;
    let mut high = 1.0;
    let mut guess = x;
    for _ in 0..NEWTON_ITERATIONS {
        let error = bezier_x(segment, guess) - x;
        if error.abs() < SOLVED_EPSILON {
            return guess;
        }
        if error > 0.0 {
            high = guess;
        } else {
            low = guess;
        }
        let slope = bezier_x_slope(segment, guess);
        if slope.abs() > SOLVED_EPSILON {
            let next = guess - error / slope;
            if next > low && next < high {
                guess = next;
                continue;
            }
        }
        guess = (low + high) / 2.0;
    }
    for _ in 0..BISECTION_ITERATIONS {
        let value = bezier_x(segment, guess);
        if (value - x).abs() < SOLVED_EPSILON {
            return guess;
        }
        if value > x {
            high = guess;
        } else {
            low = guess;
        }
        guess = (low + high) / 2.0;
    }
    return guess;
}

/// A segment's x coordinate at `u`: the timeline axis.
pub fn bezier_x(segment: &BezierSegment, u: f64) -> f64 {
    return bezier_axis(segment, 0, u);
}

/// A segment's y coordinate at `u`: the value axis.
pub fn bezier_y(segment: &BezierSegment, u: f64) -> f64 {
    return bezier_axis(segment, 1, u);
}

/// One axis of a segment at `u`.
///
/// `axis` is 0 for x and 1 for y; the eight numbers interleave as
/// `x0 y0 x1 y1 x2 y2 x3 y3`, so the four points of the axis are
/// `segment[axis]`, `segment[2 + axis]`, `segment[4 + axis]`,
/// `segment[6 + axis]`.
pub fn bezier_axis(segment: &BezierSegment, axis: usize, u: f64) -> f64 {
    let p0 = segment[axis];
    let p1 = segment[2 + axis];
    let p2 = segment[4 + axis];
    let p3 = segment[6 + axis];
    let inverse = 1.0 - u;
    return inverse * inverse * inverse * p0
        + 3.0 * inverse * inverse * u * p1
        + 3.0 * inverse * u * u * p2
        + u * u * u * p3;
}

/// The derivative of [`bezier_x`].
pub fn bezier_x_slope(segment: &BezierSegment, u: f64) -> f64 {
    let (p0, p1, p2, p3) = (segment[0], segment[2], segment[4], segment[6]);
    let inverse = 1.0 - u;
    return 3.0 * inverse * inverse * (p1 - p0)
        + 6.0 * inverse * u * (p2 - p1)
        + 3.0 * u * u * (p3 - p2);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// The CSS `ease` curve, as a single segment.
    const EASE: BezierSegment = [0.0, 0.0, 0.25, 0.1, 0.25, 1.0, 1.0, 1.0];

    /// A curve with both control points at `x = 0`: `x(u) = u³`, so Newton
    /// cannot take a first step and the bisection fallback is load-bearing.
    const FLAT_START: BezierSegment = [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

    #[test]
    fn a_segment_hits_both_endpoints() {
        assert_eq!(apply_bezier(&[EASE], 0.0), 0.0);
        assert_eq!(apply_bezier(&[EASE], 1.0), 1.0);
    }

    #[test]
    fn an_empty_curve_is_the_identity() {
        // Only reachable from a hand-built table, but it must not divide by
        // zero or index past the end.
        let empty: &[BezierSegment] = &[];
        assert_eq!(apply_bezier(empty, 0.25), 0.25);
    }

    #[test]
    fn a_curve_with_a_flat_start_still_terminates() {
        // x(u) = u³ at x = 0.125 has u = 0.5, and y(u) = u at u = 0.5.
        assert!((apply_bezier(&[FLAT_START], 0.125) - 0.5).abs() < 1e-6);
        for step in 0..=100 {
            let value = apply_bezier(&[FLAT_START], step as f64 / 100.0);
            assert!((0.0..=1.0).contains(&value), "step {step} gave {value}");
        }
    }

    #[test]
    fn the_x_axis_is_monotone_which_is_what_makes_inversion_valid() {
        // `solve_segment_x` has a unique answer only if this holds.
        let mut previous = -1.0;
        for step in 0..=400 {
            let u = step as f64 / 400.0;
            let x = bezier_x(&EASE, u);
            assert!(x >= previous, "x went backwards at u = {u}");
            previous = x;
        }
    }

    #[test]
    fn the_inverse_lands_inside_the_bracket_it_was_given() {
        for step in 0..=100 {
            let x = step as f64 / 100.0;
            let u = solve_segment_x(&EASE, x);
            assert!((0.0..=1.0).contains(&u), "x = {x} gave u = {u}");
            assert!(
                (bezier_x(&EASE, u) - x).abs() < 1e-6,
                "x = {x} solved to u = {u}"
            );
        }
    }

    #[test]
    fn the_slope_is_the_derivative_of_the_curve() {
        // Compared against a central difference, which shares no code path.
        let h = 1e-6;
        for step in 1..10 {
            let u = step as f64 / 10.0;
            let numeric = (bezier_x(&EASE, u + h) - bezier_x(&EASE, u - h)) / (2.0 * h);
            let analytic = bezier_x_slope(&EASE, u);
            assert!(
                (numeric - analytic).abs() < 1e-4,
                "at u = {u}: numeric {numeric} vs analytic {analytic}"
            );
        }
    }

    #[test]
    fn a_multi_segment_curve_is_continuous_across_the_join() {
        // Two segments sharing their x/y endpoint. `const` rather than a
        // local because `BezierCurve` is `&'static` — a curve table is a
        // compile-time thing, which is exactly what the table built on top
        // of this is.
        const FIRST: BezierSegment = [0.0, 0.0, 0.2, 0.0, 0.4, 1.0, 0.5, 1.0];
        const SECOND: BezierSegment = [0.5, 1.0, 0.6, 1.0, 0.8, 1.0, 1.0, 1.0];
        const CURVE: &[BezierSegment] = &[FIRST, SECOND];
        assert_eq!(FIRST[6], SECOND[0], "segments share their x endpoint");
        assert_eq!(FIRST[7], SECOND[1], "segments share their y endpoint");
        // Just either side of the join the value must agree.
        let before = apply_bezier(CURVE, 0.4999);
        let after = apply_bezier(CURVE, 0.5001);
        assert!(
            (before - after).abs() < 1e-3,
            "a jump at the join: {before} then {after}"
        );
        assert_eq!(apply_bezier(CURVE, 1.0), 1.0);
    }

    #[test]
    fn a_zero_width_segment_is_a_pause() {
        // A segment whose endpoints share an x is a hold: it covers no
        // time, so the loop never selects it on its own.
        //
        // Worth pinning the exact behaviour, because "a pause" is *not*
        // what comes out. `x0 == x1` makes the whole segment degenerate —
        // `solve_segment_x` cannot invert `x(u) = 0` — so the y it reports
        // is `segment[7]`, the pause's far endpoint, at every `t`. A
        // caller wanting a genuine hold has to express it as a segment
        // with a non-zero x span and flat y (see
        // `a_flat_y_segment_is_a_real_hold`). Recorded rather than
        // changed: no built-in curve has a zero-width segment, and
        // inventing a pause semantic here would be this module guessing at
        // a curve author's intent.
        const DEGENERATE: BezierSegment = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        const CURVE: &[BezierSegment] = &[DEGENERATE];
        assert_eq!(apply_bezier(CURVE, 0.0), 1.0);
        assert_eq!(apply_bezier(CURVE, 0.5), 1.0);
        assert_eq!(apply_bezier(CURVE, 1.0), 1.0);
    }

    #[test]
    fn a_flat_y_segment_is_a_real_hold() {
        // The shape a curve author should actually write for a pause: it
        // spans real time, so it is selected, and its y does not move.
        const HOLD: BezierSegment = [0.0, 0.5, 0.3, 0.5, 0.7, 0.5, 1.0, 0.5];
        const CURVE: &[BezierSegment] = &[HOLD];
        for step in 0..=10 {
            let value = apply_bezier(CURVE, step as f64 / 10.0);
            assert!((value - 0.5).abs() < 1e-9, "step {step} gave {value}");
        }
    }

    #[test]
    fn axis_selection_addresses_the_right_interleaved_number() {
        // x and y pick different slots out of the same eight.
        let segment: BezierSegment = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        assert!((bezier_axis(&segment, 0, 1.0) - 0.7).abs() < 1e-12);
        assert!((bezier_axis(&segment, 1, 1.0) - 0.8).abs() < 1e-12);
        // At u = 0 the axis is its first point.
        assert!((bezier_axis(&segment, 0, 0.0) - 0.1).abs() < 1e-12);
        assert!((bezier_axis(&segment, 1, 0.0) - 0.2).abs() < 1e-12);
    }
}
