//! `RadioButton` — a circular indicator, a filled dot when `selected`, and
//! the label.
//!
//! Group exclusivity is *not* here: that is interaction, and lives in
//! [`nui_runtime::widget::activate`]. This module only paints whatever
//! `selected` currently says.

use nui_core::Rect;
use nui_runtime::Element;

use super::part::{WidgetPart, control_label};
use super::state::{Palette, VisualState};
use crate::props::bool_property;

/// Builds the parts for a `RadioButton` filling `bounds`.
pub fn parts(
    element: &Element,
    bounds: Rect,
    state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let selected = bool_property(element, "selected");
    let diameter = bounds.size.height.min(20.0);
    let center_y = bounds.size.height / 2.0;
    // A circle is a rect whose radius is half its shorter side (the rect
    // pipeline's rounded-rect SDF degenerates to an ellipse).
    let mut parts = vec![WidgetPart::Rect {
        x: 0.0,
        y: center_y - diameter / 2.0,
        width: diameter,
        height: diameter,
        radius: diameter / 2.0,
        color: if selected {
            palette.accent
        } else {
            palette.surface_for(state)
        },
    }];
    if selected {
        // Inner dot: a second circle at 42% of the ring diameter.
        let inner = diameter * 0.42;
        parts.push(WidgetPart::Rect {
            x: (diameter - inner) / 2.0,
            y: center_y - inner / 2.0,
            width: inner,
            height: inner,
            radius: inner / 2.0,
            color: palette.foreground,
        });
    } else {
        // Ring outline: a stroked circle needs its own primitive, so draw
        // a slightly larger rect behind and punch it out with the
        // background — cheaper than adding an ellipse stroke pipeline.
        parts.push(WidgetPart::Outline {
            x: 0.0,
            y: center_y - diameter / 2.0,
            width: diameter,
            height: diameter,
            stroke: 1.0,
            radius: diameter / 2.0,
            color: palette.track,
        });
    }
    let foreground = palette.foreground_for(state);
    if let Some(label) = control_label(element, bounds, diameter + 8.0, foreground, 14.0) {
        parts.push(label);
    }
    return parts;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::{build, widget};
    use nui_core::Value;
    use nui_runtime::ElementTree;

    #[test]
    fn radio_button_ring_and_dot() {
        let mut tree = ElementTree::new();
        let plain = tree.insert(widget("RadioButton", 140.0, 20.0));
        tree.push_root(plain);
        let scene = build(&tree);
        // A circle is a rect with radius = half the shorter side.
        assert_eq!(scene.rects[0].corner_radius, 10.0);
        assert_eq!(scene.rects.len(), 1);

        let mut tree = ElementTree::new();
        let mut selected = widget("RadioButton", 140.0, 20.0);
        selected.set("selected", Value::Bool(true));
        let selected_id = tree.insert(selected);
        tree.push_root(selected_id);
        let scene = build(&tree);
        assert_eq!(scene.rects.len(), 2, "ring plus inner dot");
        assert_eq!(
            scene.rects[0].fill,
            crate::widget::Palette::default().accent
        );
        assert!(scene.rects[1].geometry.size.width < scene.rects[0].geometry.size.width);
    }
}
