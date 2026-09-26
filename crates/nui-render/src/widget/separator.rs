//! `Separator` — a one-line divider.
//!
//! The thinnest control in the set, and the cheapest: layout sizes the
//! element (`thickness` dp across, a full span along), and this module
//! paints exactly that box. Its default colour is the palette's track grey,
//! so a separator needs no properties at all to look right:
//!
//! ```text
//! Separator()
//! ```

use nui_core::Rect;
use nui_runtime::Element;

use super::part::WidgetPart;
use super::state::{Palette, VisualState};
use crate::props::{color_property, f_property};

/// Builds the part for a `Separator`.
pub fn parts(
    element: &Element,
    bounds: Rect,
    _state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let width = bounds.size.width;
    let height = bounds.size.height;
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    // A hairline is nicer rounded than square-ended; the default is half
    // the short side, i.e. a capsule that follows the line's thickness.
    let radius = f_property(element, "radius").unwrap_or(width.min(height) / 2.0);
    return vec![WidgetPart::Rect {
        x: 0.0,
        y: 0.0,
        width,
        height,
        radius,
        color: color_property(element, "color").unwrap_or(palette.track),
    }];
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::widget;
    use nui_core::{Color, Point, Size, Value};

    fn parts_of(element: &Element, width: f32, height: f32) -> Vec<WidgetPart> {
        let bounds = Rect::new(Point::ZERO, Size::new(width, height));
        return parts(element, bounds, VisualState::Normal, Palette::of(element));
    }

    #[test]
    fn a_separator_is_one_rect_over_its_whole_box() {
        let separator = widget("Separator", 200.0, 1.0);
        let parts = parts_of(&separator, 200.0, 1.0);
        assert_eq!(parts.len(), 1);
        let WidgetPart::Rect {
            width,
            height,
            color,
            radius,
            ..
        } = &parts[0]
        else {
            panic!("a rect")
        };
        assert_eq!((*width, *height), (200.0, 1.0));
        assert_eq!(*radius, 0.5, "a 1dp line rounds into a capsule");
        assert!(
            color.alpha() > 0.0,
            "the default colour must be visible: {color:?}"
        );
    }

    #[test]
    fn color_overrides_the_default() {
        let mut separator = widget("Separator", 200.0, 1.0);
        separator.set("color", Value::Color(Color::from_rgb8(0xab, 0xcd, 0xef)));
        let parts = parts_of(&separator, 200.0, 1.0);
        let WidgetPart::Rect { color, .. } = &parts[0] else {
            panic!("a rect")
        };
        assert_eq!(color.red8(), 0xab);
    }

    #[test]
    fn a_zero_sized_separator_paints_nothing() {
        let separator = widget("Separator", 0.0, 1.0);
        assert!(parts_of(&separator, 0.0, 1.0).is_empty());
    }
}
