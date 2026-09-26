//! The primitive vocabulary a control is made of.
//!
//! Parts are geometry in dp relative to the control's own top-left; the
//! scene builder offsets them into window space when it paints them. That
//! split is what lets a control be pure geometry math with no access to the
//! draw list.

use nui_core::{Color, Rect};
use nui_runtime::Element;

use crate::props::f_property;

/// A soft drop shadow on a [`WidgetPart::Surface`].
///
/// The renderer has its own `Shadow` (with a `Point` offset); this one
/// stays a flat record so the widget vocabulary does not have to import
/// scene types. The scene builder converts one into the other.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropShadow {
    /// Horizontal offset in dp (positive = right).
    pub dx: f32,
    /// Vertical offset in dp (positive = down).
    pub dy: f32,
    /// Softness in dp.
    pub blur: f32,
    /// Shadow color.
    pub color: Color,
}

/// One painted piece of a control, geometry in dp relative to the
/// control's own top-left (the caller offsets into absolute space).
#[derive(Debug, Clone)]
pub enum WidgetPart {
    /// A filled rectangle with a corner radius.
    Rect {
        /// Offset from the control's origin.
        x: f32,
        /// Offset from the control's origin.
        y: f32,
        /// Width in dp.
        width: f32,
        /// Height in dp.
        height: f32,
        /// Corner radius in dp.
        radius: f32,
        /// Fill color.
        color: Color,
    },
    /// A container surface: a fill plus an optional outline and an optional
    /// drop shadow.
    ///
    /// [`WidgetPart::Rect`] deliberately stays the minimal "flat fill" the
    /// controls use; a container is the one part that wants the whole
    /// rect-pipeline feature set (`shadow.*`, `border.*`), and growing
    /// `Rect` would have made every existing control spell out two `None`s.
    Surface {
        /// Offset from the control's origin.
        x: f32,
        /// Offset from the control's origin.
        y: f32,
        /// Width in dp.
        width: f32,
        /// Height in dp.
        height: f32,
        /// Corner radius in dp.
        radius: f32,
        /// Fill color.
        fill: Color,
        /// `(stroke width, color)` hairline around the surface.
        border: Option<(f32, Color)>,
        /// Drop shadow behind the surface.
        shadow: Option<DropShadow>,
    },
    /// A stroked outline of a rectangle (pill outlines for switches).
    Outline {
        /// Offset from the control's origin.
        x: f32,
        /// Offset from the control's origin.
        y: f32,
        /// Width in dp.
        width: f32,
        /// Height in dp.
        height: f32,
        /// Stroke width in dp.
        stroke: f32,
        /// Corner radius in dp.
        radius: f32,
        /// Stroke color.
        color: Color,
    },
    /// A stroked polyline (check marks, radio dots are circles).
    Line {
        /// Points in control-local dp.
        points: Vec<(f32, f32)>,
        /// Stroke width in dp.
        stroke: f32,
        /// Stroke color.
        color: Color,
    },
    /// A single centered text label.
    Label {
        /// Text to draw.
        text: String,
        /// Font size in dp.
        size: f32,
        /// Text color.
        color: Color,
        /// Horizontal inset from the control's origin.
        padding: f32,
        /// Vertical centering offset (usually the control's height / 2).
        center_y: f32,
    },
}

/// Builds a plain surface: a fill with a radius, no border, no shadow.
pub fn surface(x: f32, y: f32, width: f32, height: f32, radius: f32, fill: Color) -> WidgetPart {
    return WidgetPart::Surface {
        x,
        y,
        width,
        height,
        radius,
        fill,
        border: None,
        shadow: None,
    };
}

/// A control's text label, horizontally inset by `padding` and centered in
/// the control's height (or the given indicator box).
///
/// Shared by every labelled control, so the `Button` / `CheckBox` /
/// `RadioButton` / `Switch` labels cannot drift apart. A `Dialog` title is
/// *not* built here: it is top-aligned rather than centered.
pub fn control_label(
    element: &Element,
    bounds: Rect,
    padding: f32,
    color: Color,
    default_size: f32,
) -> Option<WidgetPart> {
    let content = element
        .get("label")
        .and_then(|value| return value.as_str().ok())?;
    if content.is_empty() {
        return None;
    }
    let size = f_property(element, "font.size").unwrap_or(default_size);
    return Some(WidgetPart::Label {
        text: content.to_string(),
        size,
        color,
        padding,
        center_y: bounds.size.height / 2.0,
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::{Size, Value};

    fn bounds(width: f32, height: f32) -> Rect {
        return Rect::new(nui_core::Point::ZERO, Size::new(width, height));
    }

    #[test]
    fn a_missing_or_empty_label_builds_no_part() {
        let element = Element::new("Button", None);
        assert!(control_label(&element, bounds(80.0, 20.0), 0.0, Color::WHITE, 14.0).is_none());
        let mut empty = Element::new("Button", None);
        empty.set("label", Value::String(String::new()));
        assert!(control_label(&empty, bounds(80.0, 20.0), 0.0, Color::WHITE, 14.0).is_none());
    }

    #[test]
    fn the_label_is_vertically_centered_in_the_box() {
        let mut element = Element::new("Button", None);
        element.set("label", Value::String("Save".to_string()));
        let part = control_label(&element, bounds(80.0, 36.0), 12.0, Color::WHITE, 14.0).unwrap();
        let WidgetPart::Label {
            text,
            size,
            padding,
            center_y,
            ..
        } = part
        else {
            panic!("a label part")
        };
        assert_eq!(text, "Save");
        assert_eq!(size, 14.0, "the default size when font.size is unset");
        assert_eq!(padding, 12.0);
        assert_eq!(center_y, 18.0);
    }

    #[test]
    fn font_size_overrides_the_default() {
        let mut element = Element::new("Button", None);
        element.set("label", Value::String("Save".to_string()));
        element.set("font.size", Value::Float(20.0));
        let part = control_label(&element, bounds(80.0, 36.0), 0.0, Color::WHITE, 14.0).unwrap();
        let WidgetPart::Label { size, .. } = part else {
            panic!("a label part")
        };
        assert_eq!(size, 20.0);
    }
}
