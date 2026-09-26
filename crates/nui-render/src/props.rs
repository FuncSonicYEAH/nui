//! Dynamic property readers shared by the scene walk and the control
//! modules.
//!
//! Every property in a `nui` document is a [`nui_core::Value`] resolved at
//! runtime, so no reader can be typed. These helpers coerce and fall back
//! instead of failing: a binding that has not resolved yet must not make an
//! element disappear or jump.
//!
//! This is deliberately the lowest layer of the crate — [`crate::scene`]
//! walks the tree with them and [`crate::widget`] paints controls with
//! them, so neither has to depend on the other for property access.

use nui_core::{Color, Value};
use nui_runtime::Element;

/// Extracts a dp `f32` from a property value (`Int`, `Float` or
/// `Length::Dp`). Anything else — a string, a color, a bool — is `None`.
pub fn dp_of(value: &Value) -> Option<f32> {
    return match value {
        Value::Int(inner) => Some(*inner as f32),
        Value::Float(inner) => Some(*inner as f32),
        Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
        _ => None,
    };
}

/// Reads an `f32`-shaped property.
pub fn f_property(element: &Element, name: &str) -> Option<f32> {
    return element.get(name).and_then(|value| return dp_of(value));
}

/// Reads a color property.
pub fn color_property(element: &Element, name: &str) -> Option<Color> {
    return element.get(name).and_then(|value| {
        return match value {
            Value::Color(color) => Some(*color),
            _ => None,
        };
    });
}

/// Reads a bool property with an explicit fallback.
///
/// `enabled` is the one flag that defaults to `true` (an element is live
/// unless it says otherwise); every other flag reads as `false` when it is
/// missing or not a bool.
pub fn bool_property_or(element: &Element, name: &str, default: bool) -> bool {
    return element
        .get(name)
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(default);
}

/// Reads a bool property; a missing or non-bool value reads as `false`.
pub fn bool_property(element: &Element, name: &str) -> bool {
    return bool_property_or(element, name, false);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn element() -> Element {
        let mut element = Element::new("Button", None);
        element.set("height", Value::Length(nui_core::Length::Dp(24.0)));
        element.set("opacity", Value::Float(0.5));
        element.set("count", Value::Int(3));
        element.set("fill", Value::Color(Color::from_rgb8(1, 2, 3)));
        element.set("label", Value::String("hi".to_string()));
        return element;
    }

    #[test]
    fn numbers_read_from_every_numeric_shape() {
        let element = element();
        assert_eq!(f_property(&element, "height"), Some(24.0));
        assert_eq!(f_property(&element, "opacity"), Some(0.5));
        assert_eq!(f_property(&element, "count"), Some(3.0));
        // Non-numeric values and missing names are `None`, not 0.
        assert_eq!(f_property(&element, "label"), None);
        assert_eq!(f_property(&element, "nope"), None);
    }

    #[test]
    fn colors_read_only_from_colors() {
        let element = element();
        assert_eq!(
            color_property(&element, "fill"),
            Some(Color::from_rgb8(1, 2, 3))
        );
        assert_eq!(color_property(&element, "height"), None);
    }

    #[test]
    fn bools_fall_back_to_the_given_default() {
        let element = element();
        assert!(!bool_property(&element, "checked"));
        assert!(bool_property_or(&element, "enabled", true));
        let mut disabled = Element::new("Button", None);
        disabled.set("enabled", Value::Bool(false));
        assert!(!bool_property_or(&disabled, "enabled", true));
    }
}
