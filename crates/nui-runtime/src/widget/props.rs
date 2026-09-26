//! Numeric property access for the interaction layer.
//!
//! `nui` values are dynamically typed, so a widget property may arrive as
//! an `Int`, a `Float` or a `Length`. These helpers read all three and
//! write back in the representation the document already used, so dragging
//! an integer slider does not turn it into a float.

use nui_core::Value;

use crate::binding::Engine;
use crate::element::{Element, ElementId, ElementTree};

/// Writes a numeric property as the same representation the element
/// already uses (`Int` stays `Int`), skipping no-op writes. Returns
/// whether the slot changed.
///
/// `f64` rather than `f32`: a control that steps by a fractional amount
/// (`0.1` three times) must store `0.3`, and going through `f32` on the way
/// would store `0.30000001192092896` instead and print it back to the user
/// in any `.nui` interpolation of `value`.
pub(super) fn write_number(
    engine: &mut Engine,
    tree: &mut ElementTree,
    id: ElementId,
    name: &str,
    value: f64,
) -> bool {
    let current = tree.arena[id].get(name);
    let new_value = match current {
        Some(Value::Int(_)) => Value::Int(value.round() as i64),
        Some(Value::Float(_)) => Value::Float(value),
        // No slot yet: a float reads as a float for every numeric spot.
        _ => Value::Float(value),
    };
    if current == Some(&new_value) {
        return false;
    }
    return engine.set_direct(tree, id, name, new_value);
}

/// Reads a numeric property as `f32` (Int/Float/Length).
pub(super) fn number(element: &Element, name: &str) -> Option<f32> {
    return element.get(name).and_then(|value| return number_of(value));
}

/// Reads a numeric property as `f64` (Int/Float/Length), for the controls
/// that quantise their value and must not lose the last digit doing it.
pub(super) fn number64(element: &Element, name: &str) -> Option<f64> {
    return element
        .get(name)
        .and_then(|value| return number_of_64(value));
}

/// Reads a `dp`-valued property with a fallback.
pub(super) fn dp_or(element: &Element, name: &str, default: f32) -> f32 {
    return number(element, name).unwrap_or(default);
}

/// The `f32` behind a numeric `Value`, if it is one.
pub(super) fn number_of(value: &Value) -> Option<f32> {
    return match value {
        Value::Int(inner) => Some(*inner as f32),
        Value::Float(inner) => Some(*inner as f32),
        Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
        _ => None,
    };
}

/// The `f64` behind a numeric `Value`, if it is one.
pub(super) fn number_of_64(value: &Value) -> Option<f64> {
    return match value {
        Value::Int(inner) => Some(*inner as f64),
        Value::Float(inner) => Some(*inner),
        Value::Length(nui_core::Length::Dp(inner)) => Some(f64::from(*inner)),
        _ => None,
    };
}
