//! `Panel` / `Card` — the two visual containers.
//!
//! A panel is a surface with a corner radius, an optional hairline border,
//! an optional drop shadow, and an optional title bar. Its *children* are
//! laid out by `nui-layout` as an ordinary column; this module only paints
//! the chrome around them.
//!
//! # Why the title bar is geometry, not a child
//!
//! The title is not an element: making it one would mean synthesising a
//! `Text` node at instantiation, giving the document a way to address it,
//! and duplicating its measurement. Instead `nui-runtime::widget::chrome`
//! owns the one formula for "how tall is the title bar", layout adds it to
//! the panel's top padding, and this module draws the text and the rule
//! from the same numbers.
//!
//! # Panel vs Card
//!
//! The same painter, two defaults: a `Panel` is flat (radius 10, no
//! shadow), a `Card` is elevated (radius 12, a soft shadow). Either can be
//! overridden — declaring *any* `shadow.*` property hands shadow control
//! to the document, so `shadow.color = #00000000` removes the card's
//! default shadow.

use nui_core::{Color, Rect};
use nui_runtime::Element;
use nui_runtime::widget::chrome;

use super::part::{DropShadow, WidgetPart};
use super::state::{Palette, VisualState};
use crate::props::{color_property, f_property};

/// Corner radius (dp) when the element declares none.
fn default_radius(ty: &str) -> f32 {
    return match ty {
        "Card" => 12.0,
        _ => 10.0,
    };
}

/// Whether the document said anything about the shadow.
fn declares_shadow(element: &Element) -> bool {
    return ["shadow.color", "shadow.dx", "shadow.dy", "shadow.blur"]
        .iter()
        .any(|name| return element.get(name).is_some());
}

/// The card's built-in elevation, used only when the document leaves the
/// shadow entirely alone.
fn default_shadow(ty: &str) -> Option<DropShadow> {
    if ty != "Card" {
        return None;
    }
    return Some(DropShadow {
        dx: 0.0,
        dy: 2.0,
        blur: 10.0,
        color: Color::from_rgba8(0, 0, 0, 90),
    });
}

/// Reads the declared shadow (`shadow.color` required, offset/blur
/// defaulting to a 2dp drop with an 8dp blur).
fn declared_shadow(element: &Element) -> Option<DropShadow> {
    let color = color_property(element, "shadow.color")?;
    return Some(DropShadow {
        dx: f_property(element, "shadow.dx").unwrap_or(0.0),
        dy: f_property(element, "shadow.dy").unwrap_or(2.0),
        blur: f_property(element, "shadow.blur").unwrap_or(8.0),
        color,
    });
}

/// Builds the parts for a `Panel` / `Card`.
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
    let radius =
        f_property(element, "radius").unwrap_or_else(|| return default_radius(&element.ty));
    let border = color_property(element, "border.color").map(|color| {
        return (
            f_property(element, "border.width").unwrap_or(1.0).max(0.0),
            color,
        );
    });
    let shadow = if declares_shadow(element) {
        declared_shadow(element)
    } else {
        default_shadow(&element.ty)
    };
    let mut parts = vec![WidgetPart::Surface {
        x: 0.0,
        y: 0.0,
        width,
        height,
        radius,
        fill: palette.surface,
        border,
        shadow,
    }];
    let padding = f_property(element, "padding")
        .unwrap_or_else(|| return chrome::default_padding(&element.ty));
    if let Some(title) = chrome::title_text(element) {
        parts.push(WidgetPart::Label {
            text: title.to_string(),
            size: chrome::title_size(element),
            color: palette.foreground,
            padding,
            center_y: chrome::title_center_y(element),
        });
        // The rule stops at the padding on both sides, so it reads as
        // underlining the title rather than cutting the panel in half.
        let rule_y = chrome::title_rule_y(element);
        parts.push(WidgetPart::Line {
            points: vec![(padding, rule_y), ((width - padding).max(padding), rule_y)],
            stroke: 1.0,
            color: palette.track,
        });
    }
    return parts;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::widget;
    use nui_core::Value;

    fn parts_of(element: &Element, width: f32, height: f32) -> Vec<WidgetPart> {
        let bounds = Rect::new(nui_core::Point::ZERO, nui_core::Size::new(width, height));
        return parts(element, bounds, VisualState::Normal, Palette::of(element));
    }

    #[test]
    fn a_plain_panel_is_one_flat_surface() {
        let panel = widget("Panel", 200.0, 120.0);
        let parts = parts_of(&panel, 200.0, 120.0);
        assert_eq!(parts.len(), 1);
        let WidgetPart::Surface {
            radius,
            border,
            shadow,
            ..
        } = &parts[0]
        else {
            panic!("a surface")
        };
        assert_eq!(*radius, 10.0);
        assert!(border.is_none(), "no border unless declared");
        assert!(shadow.is_none(), "a panel is flat");
    }

    #[test]
    fn a_card_carries_a_default_shadow() {
        let card = widget("Card", 200.0, 120.0);
        let parts = parts_of(&card, 200.0, 120.0);
        let WidgetPart::Surface { shadow, radius, .. } = &parts[0] else {
            panic!("a surface")
        };
        assert_eq!(*radius, 12.0);
        assert!(shadow.is_some(), "a card is elevated by default");
    }

    #[test]
    fn declaring_a_shadow_hands_control_to_the_document() {
        let mut card = widget("Card", 200.0, 120.0);
        // An odd colour so the assertion cannot pass by accident.
        card.set(
            "shadow.color",
            Value::Color(Color::from_rgb8(0x77, 0x11, 0x22)),
        );
        card.set("shadow.blur", Value::Float(0.0));
        let parts = parts_of(&card, 200.0, 120.0);
        let WidgetPart::Surface { shadow, .. } = &parts[0] else {
            panic!("a surface")
        };
        let shadow = shadow.expect("the declared shadow");
        assert_eq!(shadow.blur, 0.0, "the document's blur is used verbatim");
        assert_eq!(shadow.dx, 0.0, "and the offsets default, not the card's");
    }

    #[test]
    fn a_declared_border_becomes_an_outline() {
        let mut panel = widget("Panel", 200.0, 120.0);
        panel.set(
            "border.color",
            Value::Color(Color::from_rgb8(0x3d, 0x44, 0x50)),
        );
        let parts = parts_of(&panel, 200.0, 120.0);
        let WidgetPart::Surface { border, .. } = &parts[0] else {
            panic!("a surface")
        };
        let (stroke, color) = border.expect("the declared border");
        assert_eq!(stroke, 1.0, "a hairline by default");
        assert_eq!(color.red8(), 0x3d);
    }

    #[test]
    fn a_titled_panel_draws_the_title_and_its_rule() {
        let mut panel = widget("Panel", 200.0, 120.0);
        panel.set("title", Value::String("Details".to_string()));
        let parts = parts_of(&panel, 200.0, 120.0);
        assert_eq!(parts.len(), 3, "surface + title + rule: {parts:?}");
        let WidgetPart::Label {
            text,
            padding,
            center_y,
            ..
        } = &parts[1]
        else {
            panic!("a title label")
        };
        assert_eq!(text, "Details");
        // The default padding for a `Panel` is 16dp.
        assert_eq!(*padding, 16.0);
        assert_eq!(*center_y, 16.0 + chrome::DEFAULT_TITLE_SIZE_DP * 0.5);
        let WidgetPart::Line { points, .. } = &parts[2] else {
            panic!("a rule")
        };
        let rule_y = chrome::TITLE_GAP_DP + 16.0 + chrome::DEFAULT_TITLE_SIZE_DP;
        assert_eq!(points[0], (16.0, rule_y));
        assert_eq!(points[1].0, 184.0, "the rule stops at the padding");
        assert_eq!(points[1].1, rule_y, "and stays horizontal");
    }

    #[test]
    fn the_rule_is_never_inside_out_on_a_narrow_panel() {
        let mut panel = widget("Panel", 30.0, 120.0);
        panel.set("title", Value::String("Details".to_string()));
        let parts = parts_of(&panel, 30.0, 120.0);
        let WidgetPart::Line { points, .. } = &parts[2] else {
            panic!("a rule")
        };
        assert!(points[0].0 <= points[1].0, "clamped instead of reversed");
    }

    #[test]
    fn a_custom_fill_tints_the_surface() {
        let mut panel = widget("Panel", 200.0, 120.0);
        panel.set("fill", Value::Color(Color::from_rgb8(0x11, 0x22, 0x33)));
        let parts = parts_of(&panel, 200.0, 120.0);
        let WidgetPart::Surface { fill, .. } = &parts[0] else {
            panic!("a surface")
        };
        assert_eq!(fill.red8(), 0x11);
    }
}
