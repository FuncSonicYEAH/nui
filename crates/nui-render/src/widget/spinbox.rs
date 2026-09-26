//! `SpinBox` — a numeric field with a pair of stepper arrows.
//!
//! The value is *drawn*, not edited in place: the plan's "left half is a
//! `TextInput`" would mean the control synthesises a child element, and nui
//! instantiates children from the document only (a control cannot invent an
//! element in the tree). So v1 is a stepper — arrows, wheel and Up/Down —
//! over a read-only numeric display, which is the half of a `SpinBox` that
//! the plain `TextInput` cannot do. A document that wants both composes
//! them: `Row { TextInput(...)  SpinBox(...) }`.
//!
//! Geometry: the field is one surface with a hairline edge, the value sits
//! at the same inset a text field uses, and a divider separates the value
//! from the two arrow zones. Which zone a press landed in is the *host's*
//! business (it owns the pointer position), but *where* the zones are is
//! [`strip_width`] — shared with the stepper's stepping code so a press and
//! the glyph above it cannot disagree.

use nui_core::Rect;
use nui_runtime::Element;
use nui_runtime::widget::{display_value, strip_width};

use super::part::WidgetPart;
use super::state::{Palette, VisualState, with_alpha_scale};
use crate::props::{color_property, f_property};

/// Largest arrow glyph size, so a tall box does not draw a huge triangle.
const MAX_ARROW_SIZE: f32 = 10.0;

/// Builds the parts for a `SpinBox` filling `bounds`.
pub fn parts(
    element: &Element,
    bounds: Rect,
    state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    let width = bounds.size.width;
    let height = bounds.size.height;
    let radius = f_property(element, "radius").unwrap_or(6.0);
    let fill = color_property(element, "fill").unwrap_or_else(|| return palette.surface_for(state));
    let edge = with_alpha_scale(palette.track, 1.0);
    let mut parts = vec![WidgetPart::Surface {
        x: 0.0,
        y: 0.0,
        width,
        height,
        radius,
        fill,
        // A stepper needs an edge where a text field does not: it is a
        // control you press, and the hairline is what says so.
        border: Some((1.0, edge)),
        shadow: None,
    }];

    // The value, at the text-field inset so a stepper row lines up with the
    // text fields beside it.
    let inset = nui_runtime::widget::chrome::text_inset(element);
    let size = f_property(element, "font.size").unwrap_or(14.0);
    let foreground = palette.foreground_for(state);
    parts.push(WidgetPart::Label {
        text: display_value(element),
        size,
        color: foreground,
        padding: inset,
        center_y: height / 2.0,
    });

    let strip = strip_width(height);
    let divider = (width - strip).max(0.0);
    parts.push(WidgetPart::Line {
        points: vec![(divider, 2.0), (divider, height - 2.0)],
        stroke: 1.0,
        color: edge,
    });

    // The arrows: brighter while the pointer is on the control, because the
    // tracker owns one hover state per element, not one per arrow.
    let arrow_color = if state.is_normal() {
        palette.muted
    } else {
        foreground
    };
    let arrow_size = (height * 0.3).clamp(6.0, MAX_ARROW_SIZE);
    // Glyphs are laid out from their left edge and are roughly 0.6em wide,
    // so this centres them in the strip.
    let arrow_x = divider + (strip - arrow_size * 0.6) / 2.0;
    for (glyph, center_y) in [("\u{25b2}", height * 0.28), ("\u{25bc}", height * 0.72)] {
        parts.push(WidgetPart::Label {
            text: glyph.to_string(),
            size: arrow_size,
            color: arrow_color,
            padding: arrow_x,
            center_y,
        });
    }
    return parts;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::widget;
    use nui_core::{Point, Size, Value};

    /// A `SpinBox` spanning `0..10` by 2, currently at 4.
    fn spin() -> Element {
        let mut element = widget("SpinBox", 120.0, 28.0);
        element.set("min", Value::Float(0.0));
        element.set("max", Value::Float(10.0));
        element.set("step", Value::Float(2.0));
        element.set("value", Value::Float(4.0));
        return element;
    }

    /// The parts of `element` in `state`.
    fn parts_of(element: &Element, state: VisualState) -> Vec<WidgetPart> {
        return parts(
            element,
            Rect::new(Point::ZERO, Size::new(120.0, 28.0)),
            state,
            Palette::of(element),
        );
    }

    /// The labels of a part list, as `(text, x, center_y)`.
    fn labels(parts: &[WidgetPart]) -> Vec<(String, f32, f32)> {
        return parts
            .iter()
            .filter_map(|part| {
                return match part {
                    WidgetPart::Label {
                        text,
                        padding,
                        center_y,
                        ..
                    } => Some((text.clone(), *padding, *center_y)),
                    _ => None,
                };
            })
            .collect();
    }

    #[test]
    fn a_stepper_paints_a_surface_a_divider_and_two_arrows() {
        let element = spin();
        let parts = parts_of(&element, VisualState::Normal);
        let WidgetPart::Surface { width, height, .. } = parts[0] else {
            panic!("a surface first")
        };
        assert_eq!((width, height), (120.0, 28.0));

        let labels = labels(&parts);
        assert_eq!(labels[0].0, "4", "the value, at its step's precision");
        assert!(labels[0].1 > 0.0, "inset from the left edge");
        assert_eq!(labels[1].0, "\u{25b2}", "up arrow");
        assert_eq!(labels[2].0, "\u{25bc}", "down arrow");
        // The arrows live in the right-hand strip, above and below centre.
        assert!(labels[1].1 > 120.0 - 26.0, "in the strip: {:?}", labels[1]);
        assert!(labels[1].2 < labels[2].2, "up is above down");
        assert!((labels[1].2 + labels[2].2 - 28.0).abs() < 0.01, "symmetric");
        // ... and a divider separates the strip from the value.
        let divider = parts
            .iter()
            .find_map(|part| {
                return match part {
                    WidgetPart::Line { points, .. } => Some(points[0].0),
                    _ => None,
                };
            })
            .expect("a divider");
        assert!(divider > labels[0].1 && divider < labels[1].1);
    }

    #[test]
    fn the_arrows_brighten_on_hover() {
        let element = spin();
        let color_of = |parts: &[WidgetPart], arrow: &str| {
            return parts
                .iter()
                .find_map(|part| {
                    return match part {
                        WidgetPart::Label { text, color, .. } if text == arrow => Some(*color),
                        _ => None,
                    };
                })
                .expect("arrow label");
        };
        let at_rest = parts_of(&element, VisualState::Normal);
        let hovered = parts_of(&element, VisualState::Hovered);
        assert_eq!(color_of(&at_rest, "\u{25b2}"), Palette::default().muted);
        assert_eq!(
            color_of(&hovered, "\u{25b2}"),
            Palette::default().foreground,
            "legible while the pointer is on the control"
        );
    }

    #[test]
    fn a_declared_fill_and_radius_are_honoured() {
        let mut element = spin();
        element.set("fill", Value::Color(nui_core::Color::from_rgb8(1, 2, 3)));
        element.set("radius", Value::Float(12.0));
        let parts = parts_of(&element, VisualState::Normal);
        let WidgetPart::Surface {
            radius,
            fill,
            border,
            ..
        } = parts[0]
        else {
            panic!("a surface first")
        };
        assert_eq!(radius, 12.0);
        assert_eq!(fill, nui_core::Color::from_rgb8(1, 2, 3));
        assert!(border.is_some(), "the stepper keeps its edge");
    }

    #[test]
    fn a_fractional_step_shows_its_decimals() {
        let mut element = widget("SpinBox", 120.0, 28.0);
        element.set("step", Value::Float(0.25));
        element.set("min", Value::Float(0.0));
        element.set("value", Value::Float(0.25));
        assert_eq!(
            labels(&parts_of(&element, VisualState::Normal))[0].0,
            "0.25"
        );
        // No value declared at all: the range's minimum stands in, so the
        // field never renders blank.
        let bare = widget("SpinBox", 120.0, 28.0);
        assert_eq!(labels(&parts_of(&bare, VisualState::Normal))[0].0, "0");
    }
}
