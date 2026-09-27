//! Easing curves: the three CSS defaults plus a table of named
//! cubic-bezier curves.
//!
//! # Why a table and not a `bezier(x1, y1, x2, y2)` builtin
//!
//! A general builtin is the better design and is the eventual one, but it
//! needs somewhere for four numbers to live in a dynamically typed value —
//! a new `Value` variant, or an opaque handle — and every consumer of the
//! `Value` enum grows a case for it. A design system, on the other hand,
//! arrives with a *fixed, named* set of curves, and a document that can
//! only say `emphasized` is a document whose motion is reviewable by
//! reading it. The table keeps the language unchanged; the builtin stays
//! available as a follow-up without having to migrate these names.
//!
//! # The segment form
//!
//! A CSS `cubic-bezier(x1, y1, x2, y2)` is the parametric curve
//! `P(u) = (x(u), y(u))` for `u` in `0..=1`, with `P(0) = (0, 0)`,
//! `P(1) = (1, 1)` and the two arguments as the control points. Progress
//! `t` is the *x* coordinate, so evaluating the curve means inverting
//! `x(u) = t` (Newton–Raphson, bisection as the fallback) and reading `y`
//! there.
//!
//! [`BezierSegment`] spells that out with **both** endpoints rather than
//! assuming `(0, 0)` and `(1, 1)`, because a curve can be several segments
//! long and a middle segment's start is neither. Material 3's emphasised
//! curve is exactly that: two segments meeting at `(1/6, 0.4)`.
//!
//! `x` is monotonic on every curve in the table, which is what makes the
//! inversion well-defined. `y` is not, and is not required to be — several
//! curves have a control point above `1`, which is the overshoot the
//! expressive curves exist for.
//!
//! # Where the numbers come from
//!
//! Transcribed from the design system's motion token table. The
//! six-number form those tokens are written in is a cubic from `(0, 0)` to
//! `(1, 1)`, and is spelled out here in full so the segment's meaning is
//! checkable by eye.
//!
//! Two consequences of reading them as CSS curves rather than as spline
//! knots are worth stating, because both are visible in the tests:
//!
//! - The **spatial** curves overshoot, by design — that is what a control
//!   point at `y = 1.67` means. `spatial-fast` and `spatial-default` also
//!   dip very slightly on the way down, because their second control point
//!   sits *below* the first; that is a real feature of the curve, not
//!   numerical noise, and
//!   `every_curve_advances_essentially_monotonically` bounds it.
//! - `emphasized` does **not** overshoot. Every control point of it lies
//!   within its own segment's span, so it reaches 1 from below.
//!
//! The two `emphasized` halves are the first and second segments of the
//! full curve, each rescaled from its own span to the full
//! `(0, 0) .. (1, 1)` so either can be used as a whole easing — which is
//! what "animate only the entry" means. That rescaling is why they come
//! out as the accelerate and decelerate curves, and
//! `the_two_emphasized_halves_are_the_accelerate_and_decelerate_curves`
//! pins the identity down.

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
/// Eight is enough for every curve in the table to land well inside a
/// pixel of progress at UI durations. The bisection fallback is what makes
/// a degenerate curve terminate: `standard-decelerate` has both control
/// points at `x = 0`, so `x(u) = u³` has zero slope at `u = 0` and Newton
/// cannot take a first step.
const NEWTON_ITERATIONS: usize = 8;

/// Bisection iterations, used once Newton stops making progress.
///
/// Each halves the interval, so 24 is past the point where `f64` runs out
/// of mantissa to tell the endpoints apart.
const BISECTION_ITERATIONS: usize = 24;

/// Easing curve of a tween.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    /// Linear (`x = t`).
    Linear,
    /// Quadratic ease-in-out (the default for an unknown name).
    EaseInOut,
    /// Cubic ease-out (fast start, slow end).
    EaseOut,
    /// A named curve from [`CURVES`], possibly several segments long.
    Bezier(BezierCurve),
}

/// The named cubic-bezier curves, keyed by the `easing = ...` name a
/// document writes.
///
/// Every entry is spelled out in the full eight-number segment form so the
/// geometry can be read off the table. Note how many have a control point
/// outside `0..=1` on the y axis — `spatial-fast` reaches `1.67`, which is
/// a quarter of an overshoot before it settles.
static CURVES: &[(&str, &[BezierSegment])] = &[
    // Standard: restrained, for colour and opacity.
    ("standard", &[[0.0, 0.0, 0.2, 0.0, 0.0, 1.0, 1.0, 1.0]]),
    (
        "standard-accelerate",
        &[[0.0, 0.0, 0.3, 0.0, 1.0, 1.0, 1.0, 1.0]],
    ),
    (
        "standard-decelerate",
        &[[0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0]],
    ),
    // Spatial: for elements that move or resize. All three overshoot.
    (
        "spatial-fast",
        &[[0.0, 0.0, 0.42, 1.67, 0.21, 0.90, 1.0, 1.0]],
    ),
    (
        "spatial-default",
        &[[0.0, 0.0, 0.38, 1.21, 0.22, 1.00, 1.0, 1.0]],
    ),
    (
        "spatial-slow",
        &[[0.0, 0.0, 0.39, 1.29, 0.35, 0.98, 1.0, 1.0]],
    ),
    // Effects: short, for things that appear or respond to a press.
    ("effects", &[[0.0, 0.0, 0.34, 0.80, 0.34, 1.00, 1.0, 1.0]]),
    // Emphasized: the expressive curve. Two segments, meeting at (1/6, 0.4).
    (
        "emphasized",
        &[
            [0.0, 0.0, 0.05, 0.0, 0.13333333, 0.06, 0.16666667, 0.4],
            [0.16666667, 0.4, 0.20833333, 0.82, 0.25, 1.0, 1.0, 1.0],
        ],
    ),
    // The two halves, each rescaled to span the whole timeline. See the
    // module doc: rescaling the first segment is what makes these the
    // accelerate and decelerate curves.
    (
        "emphasized-first-half",
        &[[0.0, 0.0, 0.3, 0.0, 0.8, 0.15, 1.0, 1.0]],
    ),
    (
        "emphasized-last-half",
        &[[0.0, 0.0, 0.05, 0.70, 0.10, 1.0, 1.0, 1.0]],
    ),
    (
        "emphasized-accelerate",
        &[[0.0, 0.0, 0.3, 0.0, 0.8, 0.15, 1.0, 1.0]],
    ),
    (
        "emphasized-decelerate",
        &[[0.0, 0.0, 0.05, 0.70, 0.10, 1.0, 1.0, 1.0]],
    ),
];

impl Easing {
    /// Maps an `easing = ...` string.
    ///
    /// An unrecognised name falls back to [`Easing::EaseInOut`]. That is
    /// deliberate and pre-existing: `easing` is an `Enum`, and `Enum`
    /// carries its variant as a plain string, so the compiler checks the
    /// *type* and not the name. A typo in a curve name is therefore a
    /// silently different animation rather than a build failure — worth
    /// knowing when a motion looks wrong.
    pub fn from_name(name: &str) -> Easing {
        return match name {
            "linear" => Easing::Linear,
            "ease-out" => Easing::EaseOut,
            other => {
                return match CURVES.iter().find(|(curve, _)| return *curve == other) {
                    Some((_, curve)) => Easing::Bezier(curve),
                    None => Easing::EaseInOut,
                };
            }
        };
    }

    /// Every named curve, for tooling and tests.
    pub fn named_curves() -> impl Iterator<Item = &'static str> {
        return CURVES.iter().map(|(name, _)| return *name);
    }

    /// Progresses `t` (0..=1) through the curve.
    pub fn apply(self, t: f64) -> f64 {
        let clamped = t.clamp(0.0, 1.0);
        return match self {
            Easing::Linear => clamped,
            Easing::EaseInOut => {
                if clamped < 0.5 {
                    2.0 * clamped * clamped
                } else {
                    let mirrored = 2.0 - 2.0 * clamped;
                    1.0 - mirrored * mirrored / 2.0
                }
            }
            Easing::EaseOut => {
                let inverted = 1.0 - clamped;
                1.0 - inverted * inverted
            }
            Easing::Bezier(curve) => return apply_bezier(curve, clamped),
        };
    }
}

/// Evaluates a piecewise cubic-bezier curve at progress `t`.
///
/// Global progress is mapped onto each segment in proportion to that
/// segment's share of the timeline, then inverted within the segment. The
/// segments share endpoints, so the value is continuous across a join even
/// though each segment is evaluated on its own `0..=1`.
fn apply_bezier(curve: BezierCurve, t: f64) -> f64 {
    if curve.is_empty() {
        return t;
    }
    let mut elapsed = 0.0;
    for (index, segment) in curve.iter().enumerate() {
        let span = segment[6] - segment[0];
        let is_last = index + 1 == curve.len();
        if t <= elapsed + span || is_last {
            if span <= 0.0 {
                // A zero-width segment is a pause in the motion. The
                // endpoints are where the value sits either side of it.
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
fn solve_segment_x(segment: &BezierSegment, x: f64) -> f64 {
    let mut low = 0.0;
    let mut high = 1.0;
    let mut guess = x;
    for _ in 0..NEWTON_ITERATIONS {
        let error = bezier_x(segment, guess) - x;
        if error.abs() < 1e-9 {
            return guess;
        }
        if error > 0.0 {
            high = guess;
        } else {
            low = guess;
        }
        let slope = bezier_x_slope(segment, guess);
        if slope.abs() > 1e-9 {
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
        if (value - x).abs() < 1e-9 {
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
fn bezier_x(segment: &BezierSegment, u: f64) -> f64 {
    return bezier_axis(segment, 0, u);
}

/// A segment's y coordinate at `u`: the value axis.
fn bezier_y(segment: &BezierSegment, u: f64) -> f64 {
    return bezier_axis(segment, 1, u);
}

/// One axis of a segment at `u`.
///
/// `axis` is 0 for x and 1 for y; the eight numbers interleave as
/// `x0 y0 x1 y1 x2 y2 x3 y3`, so the four points of the axis are
/// `segment[axis]`, `segment[2 + axis]`, `segment[4 + axis]`,
/// `segment[6 + axis]`.
fn bezier_axis(segment: &BezierSegment, axis: usize, u: f64) -> f64 {
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
fn bezier_x_slope(segment: &BezierSegment, u: f64) -> f64 {
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

    /// The curve registered under `name`.
    fn curve_of(name: &str) -> &'static [BezierSegment] {
        return CURVES
            .iter()
            .find(|(curve, _)| return *curve == name)
            .map(|(_, curve)| return *curve)
            .unwrap_or_else(|| panic!("no curve named `{name}`"));
    }

    /// Samples `name` at `count + 1` evenly spaced points.
    fn samples(name: &str, count: usize) -> Vec<f64> {
        let easing = Easing::from_name(name);
        let span = count as f64;
        return (0..=count)
            .map(|step| return easing.apply(step as f64 / span))
            .collect();
    }

    #[test]
    fn every_named_curve_is_reachable_by_name() {
        // Guards against a typo in the table's keys: each name must map to
        // a Bezier curve rather than falling through to the default.
        for name in Easing::named_curves() {
            assert!(
                matches!(Easing::from_name(name), Easing::Bezier(_)),
                "{name} fell through to the default"
            );
        }
    }

    #[test]
    fn every_curve_starts_at_zero_and_ends_at_one() {
        for name in Easing::named_curves() {
            let values = samples(name, 100);
            assert!(values[0].abs() < 1e-6, "{name} starts at {}", values[0]);
            let end = values[values.len() - 1];
            assert!((end - 1.0).abs() < 1e-6, "{name} ends at {end}, expected 1");
        }
    }

    #[test]
    fn every_curve_advances_essentially_monotonically() {
        // The timeline axis is monotone on every curve — that is what makes
        // the inversion in `solve_segment_x` well defined. The *value* is a
        // different matter: a control point above 1 is an overshoot, and an
        // overshoot whose shoulder turns back down is a small dip near the
        // peak. `spatial-fast` and `spatial-default` have one (their second
        // control point sits below the first), so the test is a bound on the
        // regression rather than a strict inequality — big enough to catch a
        // curve that is actually broken, small enough to record that the
        // dip is a designed part of the motion.
        const MAX_REGRESSION: f64 = 0.02;
        for name in Easing::named_curves() {
            let values = samples(name, 400);
            for (index, pair) in values.windows(2).enumerate() {
                assert!(
                    pair[1] >= pair[0] - MAX_REGRESSION,
                    "{name} regressed by {} at step {index}: {} then {}",
                    pair[0] - pair[1],
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    #[test]
    fn the_restrained_curves_are_strictly_monotonic() {
        // Every curve that does not overshoot must advance strictly — this
        // is the property a colour or opacity tween relies on.
        for name in [
            "standard",
            "standard-accelerate",
            "standard-decelerate",
            "effects",
            "emphasized",
            "emphasized-first-half",
            "emphasized-last-half",
        ] {
            let values = samples(name, 400);
            for (index, pair) in values.windows(2).enumerate() {
                assert!(
                    pair[1] >= pair[0] - 1e-9,
                    "{name} went backwards at step {index}: {} then {}",
                    pair[0],
                    pair[1]
                );
            }
        }
    }

    #[test]
    fn a_multi_segment_curve_is_continuous_across_the_join() {
        // The two segments of `emphasized` meet at (1/6, 0.4). Evaluated
        // as independent curves they would jump; sharing the endpoint is
        // what stops them.
        let curve = curve_of("emphasized");
        assert_eq!(curve.len(), 2, "emphasized is two segments");
        let (first, second) = (curve[0], curve[1]);
        assert_eq!(first[6], second[0], "segments share their x endpoint");
        assert_eq!(first[7], second[1], "segments share their y endpoint");
        let join = 1.0 / 6.0;
        let at_join = apply_bezier(curve, join);
        assert!(
            (at_join - first[7]).abs() < 1e-6,
            "the join reads {at_join}, the first segment ends at {}",
            first[7]
        );
    }

    #[test]
    fn a_curve_with_a_flat_start_still_terminates() {
        // `standard-decelerate` has both control points at x = 0, so
        // x(u) = u³: its slope is zero at u = 0 and Newton cannot start.
        let curve = curve_of("standard-decelerate");
        for step in 0..=20 {
            let value = apply_bezier(curve, step as f64 / 20.0);
            assert!((0.0..=1.0).contains(&value), "step {step} gave {value}");
        }
        // x = 0.125 is u³ at u = 0.5.
        assert!((apply_bezier(curve, 0.125) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_spatial_curves_overshoot_and_settle() {
        // The signature of the spatial curves: past the target, then back.
        // `emphasized` is deliberately *not* in this list — every one of its
        // control points is within its own segment's span, so it reaches 1
        // from below. Overshoot is a property of the spatial set.
        for name in ["spatial-fast", "spatial-default", "spatial-slow"] {
            let values = samples(name, 400);
            let peak = values.iter().copied().fold(f64::MIN, f64::max);
            assert!(peak > 1.0, "{name} should overshoot, peaked at {peak}");
            assert!(
                (values[values.len() - 1] - 1.0).abs() < 1e-6,
                "{name} settles exactly on its target"
            );
        }
    }

    #[test]
    fn a_restrained_curve_stays_inside_its_target() {
        // The standard and effects curves exist precisely because they do
        // not overshoot: a colour or opacity tween that went past its target
        // would show it.
        for name in [
            "standard",
            "standard-accelerate",
            "standard-decelerate",
            "effects",
            "emphasized",
            "emphasized-first-half",
            "emphasized-last-half",
        ] {
            for value in samples(name, 200) {
                assert!(
                    (-1e-9..=1.0 + 1e-9).contains(&value),
                    "{name} left 0..1 at {value}"
                );
            }
        }
    }

    #[test]
    fn the_two_emphasized_halves_are_the_accelerate_and_decelerate_curves() {
        // The claim the module doc makes about where the halves come from.
        // If a token is ever re-transcribed and breaks the identity, this
        // is the test that says so.
        for (half, whole) in [
            ("emphasized-first-half", "emphasized-accelerate"),
            ("emphasized-last-half", "emphasized-decelerate"),
        ] {
            assert_eq!(
                curve_of(half),
                curve_of(whole),
                "{half} and {whole} are documented as the same curve"
            );
        }
    }

    #[test]
    fn the_css_defaults_are_unchanged() {
        assert_eq!(Easing::from_name("linear"), Easing::Linear);
        assert_eq!(Easing::from_name("ease-out"), Easing::EaseOut);
        assert_eq!(Easing::from_name("nonsense"), Easing::EaseInOut);
        assert_eq!(Easing::Linear.apply(0.25), 0.25);
        assert_eq!(Easing::EaseOut.apply(0.0), 0.0);
        assert!((Easing::EaseOut.apply(0.5) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn an_unrecognised_name_falls_back_rather_than_panicking() {
        // A typo is a silently different animation, not a crash: `easing`
        // is an `Enum`, so the compiler checks the type and not the name.
        assert_eq!(Easing::from_name("emphasised"), Easing::EaseInOut);
        assert_eq!(Easing::from_name(""), Easing::EaseInOut);
    }

    #[test]
    fn out_of_range_progress_is_clamped() {
        let easing = Easing::from_name("emphasized");
        assert_eq!(easing.apply(-1.0), easing.apply(0.0));
        assert_eq!(easing.apply(2.0), easing.apply(1.0));
    }

    #[test]
    fn an_empty_curve_is_the_identity() {
        // Only reachable from a hand-built table, but it must not divide by
        // zero or index past the end.
        let empty: &[BezierSegment] = &[];
        assert_eq!(apply_bezier(empty, 0.25), 0.25);
    }
}
