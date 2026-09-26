//! `CheckBox` — a rounded indicator box, a check mark when `checked`, and
//! the label to its right.

use nui_core::Rect;
use nui_runtime::Element;

use super::part::{WidgetPart, control_label};
use super::state::{Palette, VisualState};
use crate::props::{bool_property, f_property};

/// Builds the parts for a `CheckBox` filling `bounds`.
pub fn parts(
    element: &Element,
    bounds: Rect,
    state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let checked = bool_property(element, "checked");
    // The indicator is square, sized to the control's height with a cap so
    // a tall row does not produce an oversized box.
    let box_size = bounds.size.height.min(20.0);
    let box_y = (bounds.size.height - box_size) / 2.0;
    let radius = f_property(element, "radius")
        .unwrap_or(4.0)
        .min(box_size / 2.0);
    let mut parts = Vec::new();
    let background = if checked {
        palette.accent
    } else {
        palette.surface_for(state)
    };
    parts.push(WidgetPart::Rect {
        x: 0.0,
        y: box_y,
        width: box_size,
        height: box_size,
        radius,
        color: background,
    });
    if !checked {
        parts.push(WidgetPart::Outline {
            x: 0.0,
            y: box_y,
            width: box_size,
            height: box_size,
            stroke: 1.0,
            radius,
            color: palette.track,
        });
    } else {
        // The tick is the classic 2-segment polyline, sized as a fraction
        // of the box so it scales with the control.
        let inset = box_size * 0.24;
        let left = inset;
        let middle = box_size * 0.42;
        let right = box_size - inset;
        let top = box_y + inset;
        let bottom = box_y + box_size - inset;
        let mid_y = box_y + box_size * 0.55;
        parts.push(WidgetPart::Line {
            points: vec![(left, mid_y), (middle, bottom), (right, top)],
            stroke: (box_size * 0.14).max(1.5),
            color: palette.foreground,
        });
    }
    let foreground = palette.foreground_for(state);
    if let Some(label) = control_label(element, bounds, box_size + 8.0, foreground, 14.0) {
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
    fn checkbox_indicator_and_tick() {
        let mut tree = ElementTree::new();
        let unchecked = tree.insert(widget("CheckBox", 140.0, 20.0));
        tree.push_root(unchecked);
        let scene = build(&tree);
        // Unchecked: one square, and the ring outline stroked (a 4-segment
        // closed rect, one polyline) — but no tick.
        assert_eq!(scene.rects.len(), 1);
        assert_eq!(scene.rects[0].geometry.size.width, 20.0);
        assert_eq!(scene.polylines.len(), 1, "the ring outline");
        assert!(scene.polylines[0].points.len() > 3, "a closed rect ring");

        let mut tree = ElementTree::new();
        let mut checked = widget("CheckBox", 140.0, 20.0);
        checked.set("checked", Value::Bool(true));
        let checked_id = tree.insert(checked);
        tree.push_root(checked_id);
        let scene = build(&tree);
        // Checked: the box is accent-filled and a 3-point tick replaces
        // the ring.
        assert_eq!(
            scene.rects[0].fill,
            crate::widget::Palette::default().accent
        );
        assert_eq!(scene.polylines.len(), 1, "the tick");
        assert_eq!(scene.polylines[0].points.len(), 3, "a two-segment tick");
    }
}
