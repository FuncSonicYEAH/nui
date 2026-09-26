//! `Button` — a surface, an optional hairline edge, an optional leading
//! icon glyph, and the centered label.
//!
//! Variants change only the [`Palette`], so the geometry here is shared by
//! plain / `primary` / `ghost` / `danger`.

use nui_core::Rect;
use nui_runtime::Element;

use super::part::{WidgetPart, control_label};
use super::state::{Palette, VisualState, with_alpha_scale};
use crate::props::f_property;

/// Builds the parts for a `Button` filling `bounds`.
pub fn parts(
    element: &Element,
    bounds: Rect,
    state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let radius = f_property(element, "radius").unwrap_or(6.0);
    let surface = palette.surface_for(state);
    let mut parts = vec![WidgetPart::Rect {
        x: 0.0,
        y: 0.0,
        width: bounds.size.width,
        height: bounds.size.height,
        radius,
        color: surface,
    }];
    // A `ghost` button needs a visible edge to read as a control at rest.
    if surface.alpha() < 0.5 {
        parts.push(WidgetPart::Outline {
            x: 0.0,
            y: 0.0,
            width: bounds.size.width,
            height: bounds.size.height,
            stroke: 1.0,
            radius,
            color: with_alpha_scale(palette.track, 1.0),
        });
    }
    let foreground = palette.foreground_for(state);
    let mut padding = f_property(element, "padding").unwrap_or(12.0);
    if let Some(icon) = element
        .get("icon")
        .and_then(|value| return value.as_str().ok())
        && !icon.is_empty()
    {
        // The icon is a short glyph drawn at the leading edge; the label
        // shifts right by its width plus a gap.
        let size = f_property(element, "font.size").unwrap_or(14.0);
        parts.push(WidgetPart::Label {
            text: icon.to_string(),
            size,
            color: foreground,
            padding,
            center_y: bounds.size.height / 2.0,
        });
        padding += size + 6.0;
    }
    if let Some(label) = control_label(element, bounds, padding, foreground, 14.0) {
        parts.push(label);
    }
    return parts;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::testkit::{build, widget};
    use nui_core::{Color, Value};
    use nui_runtime::ElementTree;

    #[test]
    fn button_paints_a_surface_at_rest() {
        let mut tree = ElementTree::new();
        let id = tree.insert(widget("Button", 120.0, 36.0));
        tree.push_root(id);
        let scene = build(&tree);
        assert_eq!(scene.rects.len(), 1, "a default button is one surface");
        assert_eq!(scene.rects[0].geometry.size.width, 120.0);
        assert_eq!(scene.sources, vec![id]);
    }

    #[test]
    fn button_variant_changes_the_surface() {
        let mut tree = ElementTree::new();
        let plain = tree.insert(widget("Button", 120.0, 36.0));
        tree.push_root(plain);
        let mut primary = widget("Button", 120.0, 36.0);
        primary.set("variant", Value::Enum("primary".to_string()));
        let primary_id = tree.insert(primary);
        tree.push_root(primary_id);
        let mut ghost = widget("Button", 120.0, 36.0);
        ghost.set("variant", Value::Enum("ghost".to_string()));
        let ghost_id = tree.insert(ghost);
        tree.push_root(ghost_id);
        let scene = build(&tree);
        let default_color = crate::widget::Palette::default().surface;
        let accent = crate::widget::Palette::default().accent;
        assert_eq!(scene.rects[0].fill, default_color);
        assert_eq!(scene.rects[1].fill, accent);
        // Ghost: a transparent surface plus a hairline outline.
        assert_eq!(scene.rects[2].fill.alpha(), 0.0);
        assert!(
            scene
                .polylines
                .iter()
                .any(|draw| return draw.color != Color::WHITE),
            "the ghost hairline is stroked"
        );
    }

    #[test]
    fn button_hover_and_press_change_the_surface() {
        let mut tree = ElementTree::new();
        let mut button = widget("Button", 120.0, 36.0);
        button.set("hovered", Value::Bool(true));
        let hovered = tree.insert(button);
        tree.push_root(hovered);
        let mut pressed = widget("Button", 120.0, 36.0);
        pressed.set("armed", Value::Bool(true));
        let pressed_id = tree.insert(pressed);
        tree.push_root(pressed_id);
        let mut disabled = widget("Button", 120.0, 36.0);
        disabled.set("enabled", Value::Bool(false));
        let disabled_id = tree.insert(disabled);
        tree.push_root(disabled_id);
        let scene = build(&tree);
        let palette = crate::widget::Palette::default();
        assert_eq!(scene.rects[0].fill, palette.surface_hovered);
        assert_eq!(scene.rects[1].fill, palette.surface_pressed);
        assert_eq!(scene.rects[2].fill, palette.surface_disabled);
    }

    #[test]
    fn button_label_and_icon_become_glyphs() {
        let mut tree = ElementTree::new();
        let mut plain = widget("Button", 120.0, 36.0);
        plain.set("label", Value::String("Run".to_string()));
        let plain_id = tree.insert(plain);
        tree.push_root(plain_id);
        let mut iconic = widget("Button", 120.0, 36.0);
        iconic.set("label", Value::String("Run".to_string()));
        iconic.set("icon", Value::String("!".to_string()));
        let iconic_id = tree.insert(iconic);
        tree.push_root(iconic_id);
        let scene = build(&tree);
        let plain_glyphs = scene
            .texts
            .iter()
            .filter(|draw| return draw.origin.x < 120.0)
            .count();
        assert!(plain_glyphs > 0, "the label produced glyphs");
        // The icon shifts the label right, so the second button's glyphs
        // start further from the control's left edge.
        let first_x = scene.texts[0].origin.x;
        let second_x = scene
            .texts
            .iter()
            .map(|draw| return draw.origin.x)
            .find(|x| return *x > first_x + 1.0)
            .unwrap_or(first_x);
        assert!(second_x > first_x, "the icon pushes the label right");
    }
}
