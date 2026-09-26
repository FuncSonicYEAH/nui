//! `Slider` — a track, a filled portion up to the value, and a round thumb.
//!
//! The fill fraction comes from `value` between `min` and `max`. The
//! inverse mapping — pointer position back to a value — is interaction and
//! lives in [`nui_runtime::widget::drag_value`]; this module only reads the
//! value it produced.

use nui_core::Rect;
use nui_runtime::Element;

use super::part::WidgetPart;
use super::state::{Palette, VisualState};
use crate::props::f_property;

/// Builds the parts for a `Slider` filling `bounds`.
pub fn parts(
    element: &Element,
    bounds: Rect,
    state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let minimum = f_property(element, "min").unwrap_or(0.0);
    let maximum = f_property(element, "max").unwrap_or(100.0);
    let value = f_property(element, "value").unwrap_or(minimum);
    let fraction = slider_fraction(value, minimum, maximum);
    let track_height = f_property(element, "track.height").unwrap_or(4.0);
    let track_y = (bounds.size.height - track_height) / 2.0;
    let radius = track_height / 2.0;
    let thumb = f_property(element, "thumb.size").unwrap_or(16.0).max(4.0);
    // The thumb's travel is inset so it never overhangs either end.
    let travel = (bounds.size.width - thumb).max(0.0);
    let filled = travel * fraction + thumb / 2.0;
    let mut parts = vec![WidgetPart::Rect {
        x: 0.0,
        y: track_y,
        width: bounds.size.width,
        height: track_height,
        radius,
        color: palette.track,
    }];
    if filled > 0.0 {
        parts.push(WidgetPart::Rect {
            x: 0.0,
            y: track_y,
            width: filled,
            height: track_height,
            radius,
            color: palette.accent,
        });
    }
    let thumb_x = travel * fraction;
    let thumb_y = bounds.size.height / 2.0 - thumb / 2.0;
    parts.push(WidgetPart::Rect {
        x: thumb_x,
        y: thumb_y,
        width: thumb,
        height: thumb,
        radius: thumb / 2.0,
        color: if state == VisualState::Disabled {
            palette.muted
        } else {
            palette.foreground
        },
    });
    return parts;
}

/// The normalized position of `value` in `[minimum, maximum]`. A
/// zero-width range (bad input, or a step still being resolved) pins to 0
/// rather than producing NaN.
fn slider_fraction(value: f32, minimum: f32, maximum: f32) -> f32 {
    let span = maximum - minimum;
    if span.abs() < f32::EPSILON {
        return 0.0;
    }
    return ((value - minimum) / span).clamp(0.0, 1.0);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::{build, widget};
    use nui_core::Value;
    use nui_runtime::ElementTree;

    #[test]
    fn slider_geometry_follows_the_value() {
        let mut tree = ElementTree::new();
        let mut midpoint = widget("Slider", 200.0, 20.0);
        midpoint.set("min", Value::Float(0.0));
        midpoint.set("max", Value::Float(100.0));
        midpoint.set("value", Value::Float(50.0));
        let midpoint_id = tree.insert(midpoint);
        tree.push_root(midpoint_id);
        let scene = build(&tree);
        // Track + filled portion + thumb.
        assert_eq!(scene.rects.len(), 3);
        assert_eq!(scene.rects[0].geometry.size.width, 200.0, "full track");
        // Halfway: travel = 200 - 16 = 184, so the fill reaches the thumb's
        // center (184 * 0.5 + 16 / 2 = 100) and the thumb sits at 92.
        let filled = scene.rects[1].geometry.size.width;
        assert!((filled - 100.0).abs() < 0.01, "filled = {filled}");
        let thumb = scene.rects[2].geometry.origin.x;
        assert!((thumb - 92.0).abs() < 0.01, "thumb at {thumb}");
        assert_eq!(scene.rects[2].geometry.size.width, 16.0);
    }

    #[test]
    fn slider_fraction_clamps_and_survives_a_zero_span() {
        assert_eq!(super::slider_fraction(50.0, 0.0, 100.0), 0.5);
        assert_eq!(super::slider_fraction(-10.0, 0.0, 100.0), 0.0);
        assert_eq!(super::slider_fraction(500.0, 0.0, 100.0), 1.0);
        assert_eq!(super::slider_fraction(5.0, 5.0, 5.0), 0.0);
    }
}
