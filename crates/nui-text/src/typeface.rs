//! The typeface a run of text is shaped in.
//!
//! # Why this is a value and not three parameters
//!
//! Family, weight and italic all change *which glyphs* come out, and a glyph
//! is only correct for the typeface that produced it. So every one of them has
//! to reach the shaper, and every cache between the document and the glyph
//! atlas has to distinguish them. Threading three parameters through six
//! entry points and four cache keys is the version of this that goes wrong
//! twice: once where a signature is updated and the key is not, and once where
//! the key is updated and the value is not. A single owned value is the shape
//! that makes those two mistakes impossible to write.
//!
//! # Why weight is a number
//!
//! cosmic-text takes a `Weight(u16)` and CSS takes a number, and every
//! variable font axis is a number. Keeping the number rather than an enum means
//! a document can say `font.weight = 550` and a variable font can answer it,
//! without the language growing an enum it would have to grow again. A face
//! that has no such weight simply falls back to the nearest one it does have,
//! which is the same answer a browser gives.

use cosmic_text::{Attrs, Family, Style, Weight};

/// The typeface for a run of text: family, weight, slant.
///
/// [`Typeface::default`] is exactly what the shaper used unconditionally
/// before this existed, so a document that sets no font properties shapes
/// identically to one from the previous version.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Typeface {
    /// Family name, or empty for the shaper's default (sans-serif).
    ///
    /// Empty rather than a sentinel name, so "unset" and "the default" are the
    /// same state and a document that clears `font.family` gets the default back
    /// rather than a family literally named `""`.
    pub family: String,
    /// Weight on the CSS scale: 100 thin, 400 regular, 700 bold, 900 black.
    pub weight: u16,
    /// Whether the run is slanted.
    pub italic: bool,
}

impl Default for Typeface {
    fn default() -> Typeface {
        return Typeface {
            family: String::new(),
            weight: 400,
            italic: false,
        };
    }
}

impl Typeface {
    /// Builds a typeface from optional overrides, each defaulting on its own.
    ///
    /// The single place the defaults and the clamping are decided. Each layer
    /// that reads a typeface from an element must read the same three property
    /// names, and this cannot be what stops them disagreeing -- but it does
    /// mean "what does a malformed weight become" has one answer rather than
    /// one per layer. A weight that is not a positive finite number is dropped,
    /// because a document can write any expression there and a weight of zero
    /// or a NaN has no meaning as a font matching query.
    pub fn from_parts(family: Option<&str>, weight: Option<f32>, italic: Option<bool>) -> Typeface {
        let mut typeface = Typeface::default();
        if let Some(family) = family.filter(|family| return !family.is_empty()) {
            typeface.family = family.to_string();
        }
        if let Some(weight) = weight.filter(|weight| return weight.is_finite() && *weight > 0.0) {
            typeface.weight = weight.round().clamp(1.0, 1000.0) as u16;
        }
        if let Some(italic) = italic {
            typeface.italic = italic;
        }
        return typeface;
    }

    /// The shaper's attributes for this typeface.
    ///
    /// The family is borrowed from `self` for the lifetime of the returned
    /// `Attrs`, which is why this cannot return an owned one: cosmic-text's
    /// `Family` holds a `&str`. Callers shape with the result while the
    /// `Typeface` is still alive, which is the case everywhere — shaping is a
    /// synchronous call.
    pub fn attrs(&self) -> Attrs<'_> {
        let mut attrs = Attrs::new();
        if !self.family.is_empty() {
            attrs.family = Family::Name(self.family.as_str());
        }
        // cosmic-text's `Weight` is a CSS-scale u16, so this is a cast rather
        // than a mapping. The lower bound is clamped because a document can
        // write any integer it likes and `Weight(0)` is not a weight.
        attrs.weight = Weight(self.weight.max(1));
        if self.italic {
            attrs.style = Style::Italic;
        }
        return attrs;
    }

    /// The cache-key fragment that identifies this typeface.
    ///
    /// A string is carried whole rather than interned: a family name is a
    /// handful of bytes and the key already holds the text, which is longer.
    /// Interning would add a second lifetime to think about for no measurable
    /// saving.
    pub fn key(&self) -> (String, u16, bool) {
        return (self.family.clone(), self.weight, self.italic);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_the_previous_behaviour() {
        // A typeface that sets nothing must shape as `Attrs::new()` did, or
        // every existing snapshot moves for a change nobody asked for.
        let typeface = Typeface::default();
        assert_eq!(typeface.attrs(), Attrs::new());
    }

    #[test]
    fn an_empty_family_means_the_default_family() {
        // Not `Family::Name("")`, which asks for a family that does not exist
        // and falls back -- usually to the same face, but by accident rather
        // than by decision, and differently on different machines.
        let typeface = Typeface::default();
        assert_eq!(typeface.attrs().family, Family::SansSerif);
    }

    #[test]
    fn weight_and_slant_reach_the_shaper() {
        let typeface = Typeface {
            family: "Inter".to_string(),
            weight: 700,
            italic: true,
        };
        let attrs = typeface.attrs();
        assert_eq!(attrs.family, Family::Name("Inter"));
        assert_eq!(attrs.weight, Weight(700));
        assert_eq!(attrs.style, Style::Italic);
    }

    #[test]
    fn absent_parts_take_their_defaults() {
        assert_eq!(Typeface::from_parts(None, None, None), Typeface::default());
    }

    #[test]
    fn a_malformed_weight_is_dropped_rather_than_clamped() {
        // Zero, negative and NaN all mean "not a weight". Clamping them to 1
        // would be a legible lie: the run would ask for the thinnest face
        // available, which is worse than asking for the default.
        for weight in [0.0_f32, -300.0, f32::NAN, f32::INFINITY] {
            let typeface = Typeface::from_parts(None, Some(weight), None);
            assert_eq!(
                typeface.weight, 400,
                "weight {weight} should have been dropped"
            );
        }
    }

    #[test]
    fn a_fractional_weight_rounds_to_the_css_scale() {
        // Variable fonts are addressed in whole units, so 549.6 is 550 and not
        // 549. Rounding is on the value, not on the u16 cast, which would
        // truncate.
        assert_eq!(Typeface::from_parts(None, Some(549.6), None).weight, 550);
    }

    #[test]
    fn a_weight_beyond_the_scale_is_clamped() {
        assert_eq!(Typeface::from_parts(None, Some(9999.0), None).weight, 1000);
    }

    #[test]
    fn an_empty_family_is_the_default_not_a_named_family() {
        assert_eq!(Typeface::from_parts(Some(""), None, None).family, "");
    }

    #[test]
    fn a_zero_weight_is_clamped_rather_than_passed_through() {
        // `font.weight = 0` is a document's mistake, and a weight of zero has
        // no meaning; clamping keeps it from becoming a font-matching query
        // that silently resolves to something arbitrary.
        let typeface = Typeface {
            weight: 0,
            ..Typeface::default()
        };
        assert_eq!(typeface.attrs().weight, Weight(1));
    }
}
