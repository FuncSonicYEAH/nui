//! `Switch` — a pill track and a round thumb that slides right when
//! `checked`.

use nui_core::Rect;
use nui_runtime::Element;

use super::part::{WidgetPart, control_label};
use super::state::{Palette, VisualState, lighten};
use crate::props::{bool_property, f_property};

/// Builds the parts for a `Switch` filling `bounds`.
pub fn parts(
    element: &Element,
    bounds: Rect,
    state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let checked = bool_property(element, "checked");
    // The track keeps a 2:1 aspect ratio from the control's height.
    let track_height = bounds.size.height.min(22.0);
    let track_width = f_property(element, "track.width").unwrap_or(track_height * 1.9);
    let track_y = (bounds.size.height - track_height) / 2.0;
    let radius = track_height / 2.0;
    let mut parts = vec![WidgetPart::Rect {
        x: 0.0,
        y: track_y,
        width: track_width,
        height: track_height,
        radius,
        color: if checked {
            palette.accent
        } else {
            palette.track
        },
    }];
    if !checked {
        // An unlit track reads better with a subtle inner edge.
        parts.push(WidgetPart::Outline {
            x: 0.0,
            y: track_y,
            width: track_width,
            height: track_height,
            stroke: 1.0,
            radius,
            color: lighten(palette.track, 0.12),
        });
    }
    let inset = 2.0;
    let thumb = (track_height - inset * 2.0).max(1.0);
    let thumb_x = if checked {
        track_width - thumb - inset
    } else {
        inset
    };
    parts.push(WidgetPart::Rect {
        x: thumb_x,
        y: track_y + inset,
        width: thumb,
        height: thumb,
        radius: thumb / 2.0,
        color: if checked {
            palette.foreground
        } else {
            lighten(palette.surface_for(state), 0.25)
        },
    });
    let foreground = palette.foreground_for(state);
    if let Some(label) = control_label(element, bounds, track_width + 10.0, foreground, 14.0) {
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
    fn switch_thumb_travels_with_checked() {
        let mut tree = ElementTree::new();
        let off = tree.insert(widget("Switch", 160.0, 22.0));
        tree.push_root(off);
        let scene = build(&tree);
        let off_thumb = scene.rects.last().unwrap();
        // Track + thumb (plus the unlit track outline as a stroke).
        assert_eq!(scene.rects.len(), 2);
        assert_eq!(off_thumb.geometry.origin.x, 2.0, "thumb sits at the left");

        let mut tree = ElementTree::new();
        let mut on = widget("Switch", 160.0, 22.0);
        on.set("checked", Value::Bool(true));
        let on_id = tree.insert(on);
        tree.push_root(on_id);
        let scene = build(&tree);
        let track_width = scene.rects[0].geometry.size.width;
        let on_thumb = scene.rects.last().unwrap();
        assert!(
            on_thumb.geometry.origin.x > track_width / 2.0,
            "checked thumb sits at the right ({})",
            on_thumb.geometry.origin.x
        );
        assert_eq!(
            scene.rects[0].fill,
            crate::widget::Palette::default().accent
        );
    }
}
