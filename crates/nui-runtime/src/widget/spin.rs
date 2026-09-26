//! `SpinBox` stepping: a direction in, a clamped value out.
//!
//! A stepper is the one control whose value is *quantised by construction*:
//! every interaction (an arrow press, a wheel notch, Up/Down on the
//! keyboard) means "one step from where you are", not "a position under the
//! pointer". The arithmetic therefore lives here, once, instead of in each
//! of the three call sites — they differ only in which direction they ask
//! for.
//!
//! Two rules keep stepping honest:
//!
//! - The step grid is measured **from `min`**, not from zero. `min = 5,
//!   step = 10` offers 5, 15, 25 — snapping from zero would offer 5, 10,
//!   20, and a value the user can never reach by stepping is a bug.
//! - Each step is snapped back onto the grid, so a fractional step
//!   (`0.1`) cannot drift: three steps store `0.3`, not
//!   `0.30000000000000004`.

use crate::binding::Engine;
use crate::element::{Element, ElementId, ElementTree};

use super::props::number64;

/// `min` when the stepper declares none.
pub const DEFAULT_MIN: f64 = 0.0;
/// `max` when the stepper declares none.
pub const DEFAULT_MAX: f64 = 100.0;
/// `step` when the stepper declares none (or declares a non-positive one).
pub const DEFAULT_STEP: f64 = 1.0;

/// Narrowest arrow strip (dp), so a short stepper still has a pressable
/// half for each arrow.
pub const MIN_STRIP_DP: f32 = 14.0;
/// Widest arrow strip (dp), so a tall stepper does not become all arrows.
pub const MAX_STRIP_DP: f32 = 22.0;

/// Width of the arrow strip on a stepper `height` dp tall.
///
/// Shared with the painter: the strip is both where the arrows are *drawn*
/// and where a press *means* something, and a press zone that disagrees
/// with the glyph above it is a control that lies about itself.
pub fn strip_width(height: f32) -> f32 {
    return (height * 0.9).clamp(MIN_STRIP_DP, MAX_STRIP_DP);
}

/// Which way a press at `(x, y)` — in the stepper's own dp, origin at its
/// top-left — means to step: `1.0` for the up arrow, `-1.0` for the down
/// one, `0.0` for a press on the value half.
pub fn direction_at(width: f32, height: f32, x: f32, y: f32) -> f64 {
    if x < width - strip_width(height) {
        return 0.0;
    }
    return if y < height / 2.0 { 1.0 } else { -1.0 };
}

/// The range a stepper works in: its bounds plus its grid spacing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpinRange {
    /// Lower bound (inclusive).
    pub min: f64,
    /// Upper bound (inclusive).
    pub max: f64,
    /// Grid spacing; always positive.
    pub step: f64,
}

impl SpinRange {
    /// Reads the range a `SpinBox` declares, filling in the defaults.
    ///
    /// A reversed range (`min > max`) is read as the interval the two
    /// numbers bound rather than as an error: a document that declares them
    /// from a model may hand them over in either order, and turning that
    /// into an unsteppable control would be worse than tolerating it. A
    /// non-positive `step` falls back to the default rather than making
    /// every step a no-op.
    pub fn of(element: &Element) -> SpinRange {
        let first = number64(element, "min").unwrap_or(DEFAULT_MIN);
        let second = number64(element, "max").unwrap_or(DEFAULT_MAX);
        let step = match number64(element, "step") {
            Some(step) if step.is_finite() && step > 0.0 => step,
            _ => DEFAULT_STEP,
        };
        return SpinRange {
            min: first.min(second),
            max: first.max(second),
            step,
        };
    }

    /// The value snapped to `[min, max]` and onto the step grid.
    pub fn snap(&self, value: f64) -> f64 {
        let snapped = self.min + ((value - self.min) / self.step).round() * self.step;
        return round_to_grid(snapped.clamp(self.min, self.max), self.step);
    }

    /// `value` advanced by `steps` steps from the grid.
    ///
    /// An off-grid `value` spends its first step reaching the grid *in the
    /// direction of travel*: on an even grid, stepping up from 3 lands on 4,
    /// not on 6 — a value the user cannot reach by stepping is a bug, and a
    /// skipped grid point looks like one.
    pub fn advance(&self, value: f64, steps: f64) -> f64 {
        let offset = (value - self.min) / self.step;
        let nearest = offset.round();
        // Within tolerance the seed *is* on the grid; only its float
        // representation says otherwise.
        let anchor = if (offset - nearest).abs() < 1e-6 {
            nearest + steps
        } else {
            let grid = if steps >= 0.0 {
                offset.ceil()
            } else {
                offset.floor()
            };
            grid + (steps.abs() - 1.0).max(0.0) * steps.signum()
        };
        return self.snap(self.min + anchor * self.step);
    }

    /// Whether `value` sits on this range's grid.
    pub fn holds(&self, value: f64) -> bool {
        let offset = (value - self.min) / self.step;
        return (offset - offset.round()).abs() < 1e-6;
    }
}

/// Advances a `SpinBox`'s `value` by `steps` steps (fractional steps are
/// allowed but a caller only ever asks for ±1), clamped to its range.
///
/// Always writes a `Float`: `value` is a numeric field, so it must be able
/// to hold a fractional step even when the document seeded it with an
/// integer literal (`value = 0, step = 0.5`). Unlike the slider, which maps
/// a position onto an existing range and so preserves the representation it
/// finds, a stepper *quantises by construction* and owns the type.
///
/// Returns whether the stored value changed.
pub fn step(engine: &mut Engine, tree: &mut ElementTree, id: ElementId, steps: f64) -> bool {
    let range = SpinRange::of(&tree.arena[id]);
    let current = number64(&tree.arena[id], "value").unwrap_or(range.min);
    let value = range.advance(current, steps);
    return engine.set_direct(tree, id, "value", nui_core::Value::Float(value));
}

/// Steps the *focused* element by `steps`, if it is an enabled stepper.
///
/// Up/Down is the keyboard half of a stepper, and which element a key
/// reaches is the engine's business (it owns focus), not the host's. Kept
/// here so the decision is testable without a window: the host only has to
/// ask "did the focused thing take the key", and a `false` leaves Up/Down
/// free for whatever encloses the stepper.
pub fn step_focused(engine: &mut Engine, tree: &mut ElementTree, steps: f64) -> bool {
    let Some(id) = engine.focused() else {
        return false;
    };
    if tree.arena[id].ty != "SpinBox" || !super::is_enabled(&tree.arena[id]) {
        return false;
    }
    return step(engine, tree, id, steps);
}

/// Formats a value for a stepper's text: as many decimals as the `step` or
/// `value` need, so `0.1` × 3 shows `0.3` and an integer step shows none.
///
/// Shared with the painter, which is the only other place the number is
/// turned into text — a control whose displayed value disagrees with the
/// value it steps to is worse than no display at all.
pub fn format_value(value: f64, step: f64) -> String {
    let places = decimal_places(step).max(decimal_places(value)) as usize;
    if places == 0 {
        return format!("{}", value.round() as i64);
    }
    return format!("{value:.places$}");
}

/// The text a stepper shows for its current value: its `value` at its
/// range's precision, or the range's minimum when it has none yet.
pub fn display_value(element: &Element) -> String {
    let range = SpinRange::of(element);
    let value = number64(element, "value").unwrap_or(range.min);
    return format_value(value, range.step);
}

/// Rounds to the step's own decimal precision, dropping the noise a
/// fraction accumulates.
fn round_to_grid(value: f64, step: f64) -> f64 {
    let places = decimal_places(step);
    if places == 0 {
        return value.round();
    }
    let factor = 10f64.powi(places as i32);
    return (value * factor).round() / factor;
}

/// How many decimals a number needs before it is an integer, capped at 9 so
/// a pathological value cannot ask for an unbounded string. The tolerance
/// is what absorbs the representation error of the scaled value.
fn decimal_places(value: f64) -> u32 {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::Value;

    fn spin(properties: &[(&str, f64)]) -> Element {
        let mut element = Element::new("SpinBox", None);
        for (name, value) in properties {
            element.set(name, Value::Float(*value));
        }
        return element;
    }

    #[test]
    fn the_default_range_is_zero_to_a_hundred_by_one() {
        let element = Element::new("SpinBox", None);
        let range = SpinRange::of(&element);
        assert_eq!(range.min, 0.0);
        assert_eq!(range.max, 100.0);
        assert_eq!(range.step, 1.0);
    }

    #[test]
    fn a_reversed_range_still_bounds_an_interval() {
        let element = spin(&[("min", 10.0), ("max", 0.0)]);
        let range = SpinRange::of(&element);
        assert_eq!((range.min, range.max), (0.0, 10.0));
    }

    #[test]
    fn a_non_positive_step_falls_back_to_one() {
        for step in [0.0, -2.0] {
            let element = spin(&[("step", step)]);
            assert_eq!(SpinRange::of(&element).step, 1.0, "step {step}");
        }
    }

    #[test]
    fn the_grid_runs_from_min() {
        let element = spin(&[("min", 5.0), ("max", 100.0), ("step", 10.0)]);
        let range = SpinRange::of(&element);
        assert_eq!(range.advance(5.0, 1.0), 15.0);
        assert_eq!(range.advance(15.0, 1.0), 25.0);
        assert!(range.holds(15.0));
        assert!(!range.holds(20.0), "20 is off the min-anchored grid");
    }

    #[test]
    fn fractional_steps_do_not_drift() {
        let element = spin(&[("min", 0.0), ("max", 10.0), ("step", 0.1)]);
        let range = SpinRange::of(&element);
        let mut value = 0.0;
        for _ in 0..3 {
            value = range.advance(value, 1.0);
        }
        assert_eq!(value, 0.3, "three tenths, not 0.30000000000000004");
        assert_eq!(format_value(value, range.step), "0.3");
    }

    #[test]
    fn stepping_clamps_at_both_ends() {
        let element = spin(&[("min", 0.0), ("max", 2.0), ("step", 1.0)]);
        let range = SpinRange::of(&element);
        assert_eq!(range.advance(2.0, 1.0), 2.0);
        assert_eq!(range.advance(0.0, -1.0), 0.0);
    }

    #[test]
    fn an_off_grid_seed_converges_on_the_first_step() {
        let element = spin(&[("min", 0.0), ("max", 10.0), ("step", 2.0)]);
        let range = SpinRange::of(&element);
        // 3 sits between the grid points 2 and 4: stepping up takes the
        // first step to 4 (not to 6, which would skip a grid point), and
        // stepping down takes it to 2.
        assert_eq!(range.advance(3.0, 1.0), 4.0);
        assert_eq!(range.advance(3.0, -1.0), 2.0);
        // Now on the grid, a step is a step.
        assert_eq!(range.advance(4.0, 1.0), 6.0);
        // A nearly-on-grid seed counts as on it: 0.1 × 3 must not be
        // pushed to the *next* tenth.
        let tenths = SpinRange::of(&spin(&[("min", 0.0), ("max", 1.0), ("step", 0.1)]));
        assert_eq!(tenths.advance(0.3, 1.0), 0.4);
    }

    #[test]
    fn values_format_to_their_own_precision() {
        assert_eq!(format_value(2.0, 1.0), "2");
        assert_eq!(format_value(2.5, 1.0), "2.5", "a fractional seed shows");
        assert_eq!(format_value(0.25, 0.25), "0.25");
        assert_eq!(format_value(-3.0, 1.0), "-3");
    }

    #[test]
    fn the_strip_has_two_halves_and_the_value_half_steps_nothing() {
        let (width, height) = (120.0, 28.0);
        let strip = strip_width(height);
        // Inside the strip: above the midline is up, below is down.
        assert_eq!(direction_at(width, height, width - 2.0, 6.0), 1.0);
        assert_eq!(direction_at(width, height, width - 2.0, 22.0), -1.0);
        // The value half (and anything left of the strip) is not a step.
        assert_eq!(direction_at(width, height, 10.0, 6.0), 0.0);
        assert_eq!(direction_at(width, height, width - strip - 1.0, 6.0), 0.0);
        // A short stepper still has a pressable strip.
        assert_eq!(strip_width(10.0), MIN_STRIP_DP);
        assert_eq!(strip_width(200.0), MAX_STRIP_DP);
    }

    #[test]
    fn only_a_focused_enabled_stepper_takes_the_arrow_key() {
        let mut tree = ElementTree::new();
        let mut engine = Engine::new();
        let mut spin = Element::new("SpinBox", None);
        spin.set("min", Value::Int(0));
        spin.set("max", Value::Int(4));
        spin.set("value", Value::Int(2));
        let stepper = tree.insert(spin);
        tree.push_root(stepper);
        let mut button = Element::new("Button", None);
        button.set("label", Value::String("ok".to_string()));
        let plain = tree.insert(button);
        tree.append_child(stepper, plain);

        // Nothing focused: the key belongs to whatever else wants it.
        assert!(!step_focused(&mut engine, &mut tree, 1.0));

        // A focused non-stepper does not step.
        engine.focus(plain);
        assert!(!step_focused(&mut engine, &mut tree, 1.0));
        assert_eq!(tree.arena[stepper].get("value"), Some(&Value::Int(2)));

        engine.focus(stepper);
        assert!(step_focused(&mut engine, &mut tree, 1.0));
        assert_eq!(tree.arena[stepper].get("value"), Some(&Value::Float(3.0)));
        assert!(step_focused(&mut engine, &mut tree, -1.0));
        assert_eq!(tree.arena[stepper].get("value"), Some(&Value::Float(2.0)));

        // Disabled: the key passes through instead of moving a dead control.
        tree.arena[stepper].set("enabled", Value::Bool(false));
        assert!(!step_focused(&mut engine, &mut tree, 1.0));
    }

    #[test]
    fn stepping_a_live_tree_writes_the_value() {
        let mut tree = ElementTree::new();
        let mut element = Element::new("SpinBox", None);
        element.set("min", Value::Int(0));
        element.set("max", Value::Int(10));
        element.set("step", Value::Float(2.0));
        // Seeded with an integer literal on purpose: a stepper must still
        // hold a fractional value, so the write is a `Float`.
        element.set("value", Value::Int(4));
        let id = tree.insert(element);
        tree.push_root(id);
        let mut engine = Engine::new();
        assert!(step(&mut engine, &mut tree, id, 1.0));
        assert_eq!(tree.arena[id].get("value"), Some(&Value::Float(6.0)));
        assert!(step(&mut engine, &mut tree, id, -1.0));
        assert_eq!(tree.arena[id].get("value"), Some(&Value::Float(4.0)));
        // Clamped, and a further step is a no-op.
        assert!(step(&mut engine, &mut tree, id, 10.0));
        assert_eq!(tree.arena[id].get("value"), Some(&Value::Float(10.0)));
        assert!(!step(&mut engine, &mut tree, id, 1.0));
    }
}
