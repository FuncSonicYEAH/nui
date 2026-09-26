//! `Dialog` — a window-covering backdrop plus a centred panel with a title
//! bar, then whatever children the document put inside it.
//!
//! The dialog's *layout box* is the panel, not the window — layout has no
//! notion of a modal scrim. So the backdrop is drawn here, sized from the
//! builder's viewport, and the panel fills the element's own box. The
//! scrim's coordinates are element-local and therefore offset by the
//! element's origin; the overlay walk leaves that at its declared `x`/`y`,
//! which for a dialog is (0, 0).
//!
//! `modal` (default true) dims the backdrop and swallows input (the host
//! enforces that); `modal = false` keeps the backdrop clear so a Popover
//! can reuse the same element.

use nui_core::{Color, Rect, Size};
use nui_runtime::Element;

use super::part::WidgetPart;
use super::state::{Palette, VisualState, with_alpha_scale};
use crate::props::f_property;

/// Builds the parts for a `Dialog`. `viewport` is the window box in dp.
pub fn parts(
    element: &Element,
    bounds: Rect,
    viewport: Size,
    _state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let modal = crate::props::bool_property_or(element, "modal", true);
    let mut parts = Vec::new();
    // Backdrop: covers the window from the dialog's own origin. Element-
    // local, so it starts at -origin to reach the window's top-left.
    if viewport.width > 0.0 && viewport.height > 0.0 {
        parts.push(WidgetPart::Rect {
            x: -bounds.origin.x,
            y: -bounds.origin.y,
            width: viewport.width,
            height: viewport.height,
            radius: 0.0,
            color: if modal {
                with_alpha_scale(Color::from_rgb8(0, 0, 0), 0.45)
            } else {
                Color::TRANSPARENT
            },
        });
    }
    // Panel: the element's own box, elevated by a surface fill and a
    // hairline edge so it reads as a card rather than a hole.
    let radius = f_property(element, "radius").unwrap_or(12.0);
    parts.push(WidgetPart::Rect {
        x: 0.0,
        y: 0.0,
        width: bounds.size.width,
        height: bounds.size.height,
        radius,
        color: palette.surface,
    });
    let title_size = f_property(element, "title.size").unwrap_or(16.0);
    let has_title = element
        .get("title")
        .and_then(|value| return value.as_str().ok())
        .is_some_and(|text| return !text.is_empty());
    parts.push(WidgetPart::Outline {
        x: 0.0,
        y: 0.0,
        width: bounds.size.width,
        height: bounds.size.height,
        stroke: 1.0,
        radius,
        color: palette.track,
    });
    if has_title {
        // A title labels the TOP of the panel, so it does not use the
        // centred `control_label` path the other controls share.
        let padding = f_property(element, "padding").unwrap_or(20.0);
        if let Some(text) = element
            .get("title")
            .and_then(|value| return value.as_str().ok())
        {
            parts.push(WidgetPart::Label {
                text: text.to_string(),
                size: title_size,
                color: palette.foreground,
                padding,
                // The label baseline sits at the title row's vertical
                // middle: padding down from the top, half the line box.
                center_y: padding + title_size * 0.5,
            });
        }
        // A hairline under the title separates it from the body.
        parts.push(WidgetPart::Line {
            points: vec![
                (padding, padding + title_size + 8.0),
                (bounds.size.width - padding, padding + title_size + 8.0),
            ],
            stroke: 1.0,
            color: palette.track,
        });
    }
    return parts;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::{build, widget, window_tree};
    use nui_core::Value;

    #[test]
    fn an_open_dialog_paints_a_backdrop_a_panel_and_a_title() {
        let mut dialog = widget("Dialog", 240.0, 160.0);
        dialog.set("open", Value::Bool(true));
        dialog.set("title", Value::String("Delete file?".to_string()));
        let tree = window_tree(vec![dialog]);
        let scene = build(&tree);
        // Backdrop (400x300) then panel (240x160), then the panel outline
        // and the title rule.
        assert_eq!(scene.rects.len(), 2, "{:?}", scene.rects);
        let backdrop = scene.rects[0].geometry;
        assert_eq!(backdrop.size.width, 400.0, "the scrim covers the window");
        assert_eq!(backdrop.size.height, 300.0);
        // The scrim is a translucent black: dark and not fully opaque.
        let scrim = scene.rects[0].fill;
        assert!(
            scrim.red8() < 40 && scrim.alpha() > 0.0 && scrim.alpha() < 1.0,
            "the modal scrim dims: {scrim:?}"
        );
        let panel = scene.rects[1].geometry;
        assert_eq!(panel.size.width, 240.0);
        assert_eq!(panel.size.height, 160.0);
        // One glyph quad per character, deduplicated by the atlas for the
        // repeats in "Delete file?" — so assert "some glyphs", not the
        // character count.
        assert!(
            !scene.texts.is_empty(),
            "the title is drawn, got {:?}",
            scene.texts.len()
        );
        assert!(
            scene.polylines.len() >= 2,
            "panel outline + title rule: {:?}",
            scene.polylines.len()
        );
    }

    #[test]
    fn a_non_modal_dialog_keeps_the_backdrop_clear() {
        let mut dialog = widget("Dialog", 240.0, 160.0);
        dialog.set("open", Value::Bool(true));
        dialog.set("modal", Value::Bool(false));
        let tree = window_tree(vec![dialog]);
        let scene = build(&tree);
        assert_eq!(scene.rects[0].fill.alpha(), 0.0, "no scrim when not modal");
    }
}
