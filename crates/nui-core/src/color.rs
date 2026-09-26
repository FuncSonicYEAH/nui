//! Color type and hexadecimal literal parsing.

use std::fmt;

use crate::error::{Error, Result};

/// Full-scale f32 value of a u8 component (255.0), for byte/float conversion.
const U8_MAX_AS_F32: f32 = 255.0;
/// Radix of hexadecimal numbers.
const HEX_RADIX: u8 = 16;
/// Scale applied when expanding a single digit of the 3/4-digit short form
/// into two digits (0xF -> 0xFF, i.e. x17).
const SHORT_HEX_SCALE: u8 = 17;

/// RGBA color with components in 0.0..=1.0, stored in non-linear sRGB space
/// (matching CSS and nui-lang literals; linearization happens in the render
/// pipeline on demand).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    red: f32,
    green: f32,
    blue: f32,
    alpha: f32,
}

impl Color {
    /// Fully opaque white.
    pub const WHITE: Color = Color {
        red: 1.0,
        green: 1.0,
        blue: 1.0,
        alpha: 1.0,
    };
    /// Fully opaque black.
    pub const BLACK: Color = Color {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 1.0,
    };
    /// Fully transparent.
    pub const TRANSPARENT: Color = Color {
        red: 0.0,
        green: 0.0,
        blue: 0.0,
        alpha: 0.0,
    };

    /// Constructs from float components in 0.0..=1.0.
    pub const fn from_rgba(red: f32, green: f32, blue: f32, alpha: f32) -> Color {
        return Color {
            red,
            green,
            blue,
            alpha,
        };
    }

    /// Constructs from float components in 0.0..=1.0, alpha fixed at 1.0
    /// (opaque).
    pub const fn from_rgb(red: f32, green: f32, blue: f32) -> Color {
        return Color {
            red,
            green,
            blue,
            alpha: 1.0,
        };
    }

    /// Constructs from byte components in 0..=255.
    pub const fn from_rgba8(red: u8, green: u8, blue: u8, alpha: u8) -> Color {
        return Color {
            red: red as f32 / U8_MAX_AS_F32,
            green: green as f32 / U8_MAX_AS_F32,
            blue: blue as f32 / U8_MAX_AS_F32,
            alpha: alpha as f32 / U8_MAX_AS_F32,
        };
    }

    /// Constructs from byte components in 0..=255, alpha fixed at 255
    /// (opaque).
    pub const fn from_rgb8(red: u8, green: u8, blue: u8) -> Color {
        return Color::from_rgba8(red, green, blue, u8::MAX);
    }

    /// Parses an nui-lang color literal: `#RGB`, `#RGBA`, `#RRGGBB`,
    /// `#RRGGBBAA` (case-insensitive).
    ///
    /// # Examples
    /// ```
    /// let color = nui_core::Color::from_hex("#336699").unwrap();
    /// assert_eq!(color.to_rgba8(), [0x33, 0x66, 0x99, 0xFF]);
    /// ```
    pub fn from_hex(literal: &str) -> Result<Color> {
        let Some(digits) = literal.strip_prefix('#') else {
            return Err(Error::InvalidColorLiteral {
                literal: literal.to_string(),
                reason: "missing `#` prefix",
            });
        };
        let chars: Vec<char> = digits.chars().collect();
        return match chars.as_slice() {
            [red, green, blue] => Ok(Color::from_rgb8(
                short_component(literal, *red)?,
                short_component(literal, *green)?,
                short_component(literal, *blue)?,
            )),
            [red, green, blue, alpha] => Ok(Color::from_rgba8(
                short_component(literal, *red)?,
                short_component(literal, *green)?,
                short_component(literal, *blue)?,
                short_component(literal, *alpha)?,
            )),
            [
                red_high,
                red_low,
                green_high,
                green_low,
                blue_high,
                blue_low,
            ] => Ok(Color::from_rgb8(
                byte_component(literal, *red_high, *red_low)?,
                byte_component(literal, *green_high, *green_low)?,
                byte_component(literal, *blue_high, *blue_low)?,
            )),
            [
                red_high,
                red_low,
                green_high,
                green_low,
                blue_high,
                blue_low,
                alpha_high,
                alpha_low,
            ] => Ok(Color::from_rgba8(
                byte_component(literal, *red_high, *red_low)?,
                byte_component(literal, *green_high, *green_low)?,
                byte_component(literal, *blue_high, *blue_low)?,
                byte_component(literal, *alpha_high, *alpha_low)?,
            )),
            _ => Err(Error::InvalidColorLiteral {
                literal: literal.to_string(),
                reason: "expected 3, 4, 6, or 8 hex digits after `#`",
            }),
        };
    }

    /// Red component (0.0..=1.0).
    pub const fn red(&self) -> f32 {
        return self.red;
    }

    /// Green component (0.0..=1.0).
    pub const fn green(&self) -> f32 {
        return self.green;
    }

    /// Blue component (0.0..=1.0).
    pub const fn blue(&self) -> f32 {
        return self.blue;
    }

    /// Alpha component (0.0..=1.0, 1 is opaque).
    pub const fn alpha(&self) -> f32 {
        return self.alpha;
    }

    /// Red component as a byte (0..=255), clamped and rounded — the
    /// integer counterpart of [`Color::red`] for palette math.
    pub fn red8(&self) -> u8 {
        return component_to_u8(self.red);
    }

    /// Green component as a byte (0..=255), clamped and rounded.
    pub fn green8(&self) -> u8 {
        return component_to_u8(self.green);
    }

    /// Blue component as a byte (0..=255), clamped and rounded.
    pub fn blue8(&self) -> u8 {
        return component_to_u8(self.blue);
    }

    /// Alpha component as a byte (0..=255), clamped and rounded.
    pub fn alpha8(&self) -> u8 {
        return component_to_u8(self.alpha);
    }

    /// Returns a copy with the alpha component replaced.
    pub const fn with_alpha(self, alpha: f32) -> Color {
        return Color {
            red: self.red,
            green: self.green,
            blue: self.blue,
            alpha,
        };
    }

    /// Byte components `[R, G, B, A]` (clamped to 0.0..=1.0, then rounded).
    pub fn to_rgba8(self) -> [u8; 4] {
        return [
            component_to_u8(self.red),
            component_to_u8(self.green),
            component_to_u8(self.blue),
            component_to_u8(self.alpha),
        ];
    }

    /// Premultiplied float components `[R, G, B, A]` for GPU submission
    /// (blend modes assume premultiplied alpha).
    pub fn to_rgba(self) -> [f32; 4] {
        let a = self.alpha.clamp(0.0, 1.0);
        return [self.red * a, self.green * a, self.blue * a, a];
    }

    /// Linearly interpolates towards `target`; `factor` is clamped to
    /// 0.0..=1.0 (used for animation gradients).
    pub fn lerp(self, target: Color, factor: f32) -> Color {
        let t = factor.clamp(0.0, 1.0);
        return Color::from_rgba(
            lerp_component(self.red, target.red, t),
            lerp_component(self.green, target.green, t),
            lerp_component(self.blue, target.blue, t),
            lerp_component(self.alpha, target.alpha, t),
        );
    }
}

/// Single hex digit -> 0..=15.
fn hex_value(digit: char) -> Option<u8> {
    return match digit {
        '0'..='9' => Some(digit as u8 - b'0'),
        'a'..='f' => Some(digit as u8 - b'a' + 10),
        'A'..='F' => Some(digit as u8 - b'A' + 10),
        _ => None,
    };
}

/// Short-form component: a single digit expanded to two digits (`#3` -> 0x33).
fn short_component(literal: &str, digit: char) -> Result<u8> {
    let Some(value) = hex_value(digit) else {
        return Err(Error::InvalidColorLiteral {
            literal: literal.to_string(),
            reason: "invalid hex digit",
        });
    };
    return Ok(value * SHORT_HEX_SCALE);
}

/// Long-form component: two digits combined into one byte (`33` -> 0x33).
fn byte_component(literal: &str, high: char, low: char) -> Result<u8> {
    let (Some(high_value), Some(low_value)) = (hex_value(high), hex_value(low)) else {
        return Err(Error::InvalidColorLiteral {
            literal: literal.to_string(),
            reason: "invalid hex digit",
        });
    };
    return Ok(high_value * HEX_RADIX + low_value);
}

/// Float component -> byte (clamp + round).
fn component_to_u8(component: f32) -> u8 {
    return (component.clamp(0.0, 1.0) * U8_MAX_AS_F32).round() as u8;
}

/// Float linear interpolation.
fn lerp_component(from: f32, to: f32, t: f32) -> f32 {
    return from + (to - from) * t;
}

impl fmt::Display for Color {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [red, green, blue, alpha] = self.to_rgba8();
        return write!(formatter, "#{red:02x}{green:02x}{blue:02x}{alpha:02x}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn rgba8_roundtrip() {
        let color = Color::from_rgba8(0x33, 0x66, 0x99, 0xCC);
        assert_eq!(color.to_rgba8(), [0x33, 0x66, 0x99, 0xCC]);
    }

    #[test]
    fn constants_are_full_range() {
        assert_eq!(Color::WHITE.to_rgba8(), [255, 255, 255, 255]);
        assert_eq!(Color::BLACK.to_rgba8(), [0, 0, 0, 255]);
        assert_eq!(Color::TRANSPARENT.alpha(), 0.0);
    }

    #[test]
    fn hex_short_and_long_forms_agree() {
        let short = Color::from_hex("#369").unwrap();
        let long = Color::from_hex("#336699").unwrap();
        assert_eq!(short.to_rgba8(), [51, 102, 153, 255]);
        assert_eq!(long.to_rgba8(), [51, 102, 153, 255]);
    }

    #[test]
    fn hex_with_alpha_channel() {
        let short = Color::from_hex("#369C").unwrap();
        let long = Color::from_hex("#336699CC").unwrap();
        assert_eq!(short.to_rgba8(), [51, 102, 153, 204]);
        assert_eq!(long.to_rgba8(), [51, 102, 153, 204]);
    }

    #[test]
    fn hex_is_case_insensitive() {
        let color = Color::from_hex("#FFaa00").unwrap();
        assert_eq!(color.to_rgba8(), [255, 170, 0, 255]);
    }

    #[test]
    fn hex_missing_prefix_is_rejected() {
        let error = Color::from_hex("336699").unwrap_err();
        assert!(error.to_string().contains("missing `#` prefix"));
    }

    #[test]
    fn hex_invalid_digit_is_rejected() {
        let error = Color::from_hex("#GGG").unwrap_err();
        assert!(error.to_string().contains("invalid hex digit"));
    }

    #[test]
    fn hex_unsupported_length_is_rejected() {
        let error = Color::from_hex("#12345").unwrap_err();
        assert!(error.to_string().contains("3, 4, 6, or 8"));
    }

    #[test]
    fn with_alpha_replaces_alpha_only() {
        let base = Color::from_rgb8(10, 20, 30);
        let adjusted = base.with_alpha(0.5);
        assert_eq!(adjusted.to_rgba8(), [10, 20, 30, 128]);
    }

    #[test]
    fn lerp_interpolates_and_clamps() {
        let middle = Color::BLACK.lerp(Color::WHITE, 0.5);
        assert_eq!(middle.to_rgba8(), [128, 128, 128, 255]);
        let clamped = Color::BLACK.lerp(Color::WHITE, 2.0);
        assert_eq!(clamped.to_rgba8(), [255, 255, 255, 255]);
    }

    #[test]
    fn display_uses_pound_hex_form() {
        let color = Color::from_hex("#336699").unwrap();
        assert_eq!(color.to_string(), "#336699ff");
    }
}
