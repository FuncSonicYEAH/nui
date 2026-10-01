//! Color arithmetic on raw components: hex digits and 0..=255 channels.
//!
//! Everything here takes and returns plain numbers. The `Color` *type*
//! stays in `nui-core` — it is a domain type with a grammar, formatting
//! and parsing rules, and moving it would be a different migration. What
//! belongs here is the arithmetic underneath it, which is the part with
//! no opinion about what a color is.

/// The largest channel value, as a float, for scaling into `0..=255`.
const U8_MAX_AS_F32: f32 = 255.0;

/// Single hex digit -> `0..=15`. Case-insensitive.
///
/// `None` rather than an error, because "is this a hex digit" and "what
/// went wrong with this color literal" are different questions, asked by
/// different callers.
pub fn hex_value(digit: char) -> Option<u8> {
    return match digit {
        '0'..='9' => Some(digit as u8 - b'0'),
        'a'..='f' => Some(digit as u8 - b'a' + 10),
        'A'..='F' => Some(digit as u8 - b'A' + 10),
        _ => None,
    };
}

/// A `0..=1` channel -> `0..=255`, clamped and rounded.
///
/// Both guards matter. The clamp means an out-of-range component (a
/// computed tint that overshot, say) saturates instead of wrapping; the
/// rounding means `0.5` becomes `128` rather than truncating to `127`,
/// which is what keeps a color and its inverse summing to a constant.
pub fn component_to_u8(component: f32) -> u8 {
    return (component.clamp(0.0, 1.0) * U8_MAX_AS_F32).round() as u8;
}

/// Blends the channel `from` toward `to` by `amount` (`0..=1`).
///
/// `amount` is **not** clamped, so an overshoot saturates: that is what
/// keeps a tint that overshot a valid channel rather than wrapping to the
/// other end of the range. The consequence to know about is that a `NaN`
/// amount yields `0` rather than a panic — `f32::NAN.round().clamp(..)`
/// is `NaN`, and `NaN as u8` is `0` in Rust. A caller whose amount can be
/// `NaN` (a computed mix ratio, say) has to check it; this function
/// degrades quietly by design so that a bad tint cannot take the frame
/// down.
pub fn blend_u8(from: u8, to: u8, amount: f32) -> u8 {
    let value = from as f32 + (to as f32 - from as f32) * amount;
    return value.round().clamp(0.0, f32::from(u8::MAX)) as u8;
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn hex_digits_parse_in_both_cases() {
        assert_eq!(hex_value('0'), Some(0));
        assert_eq!(hex_value('9'), Some(9));
        assert_eq!(hex_value('a'), Some(10));
        assert_eq!(hex_value('f'), Some(15));
        assert_eq!(hex_value('A'), Some(10));
        assert_eq!(hex_value('F'), Some(15));
        // Not a digit: the caller decides what that means.
        assert_eq!(hex_value('g'), None);
        assert_eq!(hex_value(' '), None);
        assert_eq!(hex_value('按'), None);
    }

    #[test]
    fn the_whole_byte_range_is_reachable() {
        assert_eq!(component_to_u8(0.0), 0);
        assert_eq!(component_to_u8(1.0), 255);
        // Rounding, not truncation: this is the one that would be 127.
        assert_eq!(component_to_u8(0.5), 128);
        assert_eq!(component_to_u8(255.0 / 2.0 / 255.0), 128);
    }

    #[test]
    fn an_out_of_range_component_saturates() {
        assert_eq!(component_to_u8(-1.0), 0);
        assert_eq!(component_to_u8(2.0), 255);
        assert_eq!(component_to_u8(f32::INFINITY), 255);
        assert_eq!(component_to_u8(f32::NEG_INFINITY), 0);
    }

    #[test]
    fn a_component_round_trips_through_the_byte() {
        // The invariant a palette relies on: encode then decode is stable.
        for byte in 0..=255u8 {
            let component = f32::from(byte) / 255.0;
            assert_eq!(component_to_u8(component), byte, "byte {byte}");
        }
    }

    #[test]
    fn blending_hits_both_ends() {
        assert_eq!(blend_u8(0, 255, 0.0), 0);
        assert_eq!(blend_u8(0, 255, 1.0), 255);
        assert_eq!(blend_u8(0, 255, 0.5), 128);
        assert_eq!(blend_u8(255, 0, 0.5), 128);
        // Blending toward itself is a no-op at any amount.
        assert_eq!(blend_u8(37, 37, 0.5), 37);
    }

    #[test]
    fn blending_past_either_end_saturates() {
        // An amount above 1 keeps pushing in the same direction, so it
        // sticks at the target rather than wrapping to the other end.
        assert_eq!(blend_u8(200, 255, 2.0), 255);
        assert_eq!(blend_u8(200, 0, 2.0), 0);
        // A blend *toward white* with a huge amount: `to` is the ceiling,
        // so it stays 255 — unlike the `2.0`-amplified round trip below.
        assert_eq!(blend_u8(200, 255, 100.0), 255);
        // A negative amount extrapolates the other way and sticks at 0.
        assert_eq!(blend_u8(40, 255, -1.0), 0);
    }

    #[test]
    fn a_huge_amount_wraps_through_the_saturation_boundary() {
        // Worth knowing, because it looks wrong and is not: the raw value
        // is computed first and *then* clamped, so `from = 1, to = 0`
        // with `amount = 1000` gives -999, and the clamp catches it at 0.
        // The trap is the middle: `round()` of a large negative f32
        // saturates to `i32::MIN`, and `as u8` on an out-of-range float is
        // a *saturating* cast in Rust, so the result is still 0. No wrap
        // can occur.
        assert_eq!(blend_u8(1, 0, 1000.0), 0);
        assert_eq!(blend_u8(0, 255, 1000.0), 255);
    }

    #[test]
    fn a_nan_amount_degrades_to_zero_rather_than_panicking() {
        // Documented on `blend_u8`: a bad tint must not take the frame
        // down. A caller whose amount can be NaN has to check it.
        assert_eq!(blend_u8(200, 255, f32::NAN), 0);
    }
}
