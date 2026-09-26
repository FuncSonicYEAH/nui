//! Pointer state in, colors out: the two tables every control paints from.
//!
//! [`VisualState`] collapses the interaction properties the widget tracker
//! writes into the one state a control should draw. [`Palette`] maps that
//! state to a color, and derives the hover/press variants from a custom
//! `fill` so an overridden control still looks coherent.

use nui_core::Color;
use nui_runtime::Element;

use crate::props::{bool_property, bool_property_or, color_property};

/// The visual state a control paints in, most specific first
/// (`Disabled` wins over `Pressed`, which wins over `Hovered`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualState {
    /// `enabled = false`.
    Disabled,
    /// Pressed and the release would activate (`armed`).
    Pressed,
    /// Pointer over the control.
    Hovered,
    /// Idle.
    Normal,
}

impl VisualState {
    /// Resolves a control's visual state from the interaction properties
    /// written by [`nui_runtime::widget`].
    pub fn of(element: &Element) -> VisualState {
        if !bool_property_or(element, "enabled", true) {
            return VisualState::Disabled;
        }
        // `armed` (not `pressed`): a drag *out* of a held button drops the
        // pressed look even though the gesture is still captured — the
        // same convention as Qt Quick's `down && containsMouse`.
        if bool_property(element, "armed") {
            return VisualState::Pressed;
        }
        if bool_property(element, "hovered") {
            return VisualState::Hovered;
        }
        return VisualState::Normal;
    }

    /// Whether this state is the "nothing is happening" one.
    pub fn is_normal(self) -> bool {
        return self == VisualState::Normal;
    }
}

/// The color set a control variant paints with. Resolved once per control
/// per frame from the theme defaults plus the element's overrides.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Idle surface.
    pub surface: Color,
    /// Hovered surface.
    pub surface_hovered: Color,
    /// Pressed surface.
    pub surface_pressed: Color,
    /// Disabled surface.
    pub surface_disabled: Color,
    /// Text / indicator color on the surface.
    pub foreground: Color,
    /// Muted text (disabled labels, placeholders).
    pub muted: Color,
    /// Accent (checked indicator, slider fill, primary button surface).
    pub accent: Color,
    /// Track / border color behind a fill.
    pub track: Color,
}

impl Default for Palette {
    fn default() -> Palette {
        return Palette {
            surface: Color::from_rgb8(0x2a, 0x30, 0x3a),
            surface_hovered: Color::from_rgb8(0x33, 0x3b, 0x47),
            surface_pressed: Color::from_rgb8(0x22, 0x27, 0x30),
            surface_disabled: Color::from_rgb8(0x22, 0x26, 0x2d),
            foreground: Color::from_rgb8(0xe6, 0xea, 0xf2),
            muted: Color::from_rgb8(0x7a, 0x84, 0x95),
            accent: Color::from_rgb8(0x4a, 0x7b, 0xf7),
            track: Color::from_rgb8(0x3d, 0x44, 0x50),
        };
    }
}

impl Palette {
    /// The surface color for a visual state.
    pub fn surface_for(&self, state: VisualState) -> Color {
        return match state {
            VisualState::Disabled => self.surface_disabled,
            VisualState::Pressed => self.surface_pressed,
            VisualState::Hovered => self.surface_hovered,
            VisualState::Normal => self.surface,
        };
    }

    /// The foreground color for a visual state (`Disabled` dims to muted).
    pub fn foreground_for(&self, state: VisualState) -> Color {
        return match state {
            VisualState::Disabled => self.muted,
            _ => self.foreground,
        };
    }

    /// Reads the element's color overrides over the defaults. `fill`
    /// overrides the idle surface, `color` the foreground, `accent` the
    /// accent; the derived states shift the idle value instead of jumping
    /// to unrelated theme colors, so a custom `fill` still looks coherent
    /// when hovered.
    pub fn of(element: &Element) -> Palette {
        let mut palette = Palette::default();
        if let Some(fill) = color_property(element, "fill") {
            palette.surface = fill;
            palette.surface_hovered = lighten(fill, 0.08);
            palette.surface_pressed = darken(fill, 0.08);
        }
        if let Some(color) = color_property(element, "color") {
            palette.foreground = color;
        }
        if let Some(accent) = color_property(element, "accent") {
            palette.accent = accent;
        }
        return palette;
    }
}

/// The built-in `variant` names and the palette they select.
///
/// `primary` is accent-filled, `ghost` is transparent with a hairline
/// outline, `danger` is a muted red surface. An unknown name falls back to
/// `default` rather than erroring: `variant` is a plain `Enum` property and
/// the registry validates nothing about it yet.
pub fn variant_palette(name: &str, base: Palette) -> Palette {
    return match name {
        "primary" => Palette {
            surface: base.accent,
            surface_hovered: lighten(base.accent, 0.1),
            surface_pressed: darken(base.accent, 0.1),
            foreground: Color::WHITE,
            ..base
        },
        "ghost" => Palette {
            surface: base.surface.with_alpha(0.0),
            surface_hovered: base.surface.with_alpha(0.35),
            surface_pressed: base.surface.with_alpha(0.6),
            ..base
        },
        "danger" => {
            let danger = Color::from_rgb8(0xc0, 0x43, 0x43);
            return Palette {
                surface: danger,
                surface_hovered: lighten(danger, 0.1),
                surface_pressed: darken(danger, 0.1),
                foreground: Color::WHITE,
                ..base
            };
        }
        _ => base,
    };
}

/// Blends `color` toward white by `amount` (0..=1).
pub fn lighten(color: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    return Color::from_rgba8(
        blend(color.red8(), 255, amount),
        blend(color.green8(), 255, amount),
        blend(color.blue8(), 255, amount),
        color.alpha8(),
    );
}

/// Blends `color` toward black by `amount` (0..=1).
pub fn darken(color: Color, amount: f32) -> Color {
    let amount = amount.clamp(0.0, 1.0);
    return Color::from_rgba8(
        blend(color.red8(), 0, amount),
        blend(color.green8(), 0, amount),
        blend(color.blue8(), 0, amount),
        color.alpha8(),
    );
}

fn blend(from: u8, to: u8, amount: f32) -> u8 {
    let value = from as f32 + (to as f32 - from as f32) * amount;
    return value.round().clamp(0.0, 255.0) as u8;
}

/// Multiplies a color's alpha.
pub fn with_alpha_scale(color: Color, scale: f32) -> Color {
    return color.with_alpha(color.alpha() * scale.clamp(0.0, 1.0));
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::Value;

    fn element() -> Element {
        return Element::new("Button", None);
    }

    #[test]
    fn visual_state_precedence() {
        let mut button = element();
        assert_eq!(VisualState::of(&button), VisualState::Normal);
        button.set("hovered", Value::Bool(true));
        assert_eq!(VisualState::of(&button), VisualState::Hovered);
        button.set("armed", Value::Bool(true));
        assert_eq!(VisualState::of(&button), VisualState::Pressed);
        button.set("enabled", Value::Bool(false));
        assert_eq!(VisualState::of(&button), VisualState::Disabled);
    }

    #[test]
    fn pressed_requires_armed_not_pressed() {
        // A captured drag that left the box is `pressed` but not `armed`.
        let mut button = element();
        button.set("pressed", Value::Bool(true));
        assert_eq!(VisualState::of(&button), VisualState::Normal);
    }

    #[test]
    fn focus_alone_is_not_a_visual_state() {
        let mut button = element();
        button.set("focused", Value::Bool(true));
        assert_eq!(VisualState::of(&button), VisualState::Normal);
    }

    #[test]
    fn palette_derives_states_from_a_custom_fill() {
        let mut button = element();
        button.set("fill", Value::Color(Color::from_rgb8(0x40, 0x40, 0x40)));
        let palette = Palette::of(&button);
        assert_eq!(palette.surface.red8(), 0x40);
        assert!(
            palette.surface_hovered.red8() > palette.surface.red8(),
            "hover lightens"
        );
        assert!(
            palette.surface_pressed.red8() < palette.surface.red8(),
            "press darkens"
        );
    }

    #[test]
    fn variant_palettes() {
        let base = Palette::default();
        let primary = variant_palette("primary", base);
        assert_eq!(primary.surface, base.accent);
        assert_eq!(primary.foreground, Color::WHITE);
        let ghost = variant_palette("ghost", base);
        assert_eq!(ghost.surface.alpha(), 0.0, "ghost is transparent at rest");
        assert!(ghost.surface_hovered.alpha() > 0.0, "but tints on hover");
        let danger = variant_palette("danger", base);
        assert!(danger.surface.red8() > danger.surface.green8());
        // Unknown variants fall back to the base palette.
        let unknown = variant_palette("sparkle", base);
        assert_eq!(unknown.surface, base.surface);
    }

    #[test]
    fn lighten_and_darken_clamp() {
        let grey = Color::from_rgb8(0x80, 0x80, 0x80);
        assert_eq!(lighten(grey, 0.0), grey);
        assert_eq!(darken(grey, 0.0), grey);
        assert_eq!(lighten(grey, 1.0).red8(), 255);
        assert_eq!(darken(grey, 1.0).red8(), 0);
        // Out-of-range amounts clamp instead of wrapping.
        assert_eq!(lighten(grey, 5.0).red8(), 255);
    }

    #[test]
    fn alpha_scale() {
        let color = Color::from_rgba8(10, 20, 30, 200);
        assert_eq!(with_alpha_scale(color, 0.5).alpha8(), 100);
        assert_eq!(with_alpha_scale(color, 0.0).alpha(), 0.0);
    }
}
