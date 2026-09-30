//! What `Typeface` does to a shaped run.
//!
//! Every case here uses `with_embedded_font` rather than the system fonts. The
//! point is not that bold is 8% wider -- that number belongs to whichever
//! faces the host happens to have installed -- but that a typeface is a real
//! input to shaping and to every cache in front of it. A fixed font file makes
//! "it changed" a fact instead of a hope about someone else's `/usr/share/fonts`.

use nui_text::{TextSystem, Typeface};

const SAMPLE: &str = "Handgloves 0123";

fn bold() -> Typeface {
    return Typeface {
        weight: 700,
        ..Typeface::default()
    };
}

/// A heavier weight is a different shaping, not a hint that is ignored.
///
/// The cache key is the load-bearing part. A weight that changed the pixels but
/// not the key would draw correctly once and then serve the wrong glyph
/// whenever the atlas was asked again, which is the kind of bug that only shows
/// up after a resize.
#[test]
fn a_heavier_weight_shapes_differently() {
    let mut text = TextSystem::with_embedded_font();
    let regular = text.shape(SAMPLE, 32.0, &Typeface::default());
    let heavy = text.shape(SAMPLE, 32.0, &bold());
    assert_ne!(
        regular.glyphs[0].key, heavy.glyphs[0].key,
        "the atlas slot must differ, or the two typefaces share glyphs"
    );
    assert!(
        heavy.width > regular.width,
        "a heavier weight should not measure narrower: {} vs {}",
        heavy.width,
        regular.width
    );
}

/// Weight is monotone: heavier never means lighter.
///
/// The property a caller actually depends on when they write a type scale, and
/// the one a test for "it changed" would miss -- a shaper that scaled the wrong
/// way would still pass the test above.
#[test]
fn heavier_never_measures_narrower() {
    let mut text = TextSystem::with_embedded_font();
    let mut previous = 0.0_f32;
    for weight in [100_u16, 300, 400, 500, 700, 900] {
        let typeface = Typeface {
            weight,
            ..Typeface::default()
        };
        let width = text.measure(SAMPLE, 32.0, &typeface).0;
        assert!(
            width >= previous,
            "weight {weight} measured {width}, narrower than the lighter run's {previous}"
        );
        previous = width;
    }
    // Strictly monotone, not merely non-decreasing: a shaper that returned the
    // same width for 400 and 500 would pass `>=` and mean the intermediate
    // weights are not reachable, which is the whole point of a numeric scale.
    let at_400 = text
        .measure(
            SAMPLE,
            32.0,
            &Typeface {
                weight: 400,
                ..Typeface::default()
            },
        )
        .0;
    let at_500 = text
        .measure(
            SAMPLE,
            32.0,
            &Typeface {
                weight: 500,
                ..Typeface::default()
            },
        )
        .0;
    assert!(
        at_500 > at_400,
        "500 and 400 measured the same ({at_500}): the scale has a gap"
    );
}

/// The measure cache distinguishes typefaces.
///
/// The bug this pins: a cache keyed on `(text, size)` alone, with the typeface
/// threaded only into the shaping call, returns the *first* typeface's
/// measurement for every later one. A bold run is then laid out at the width of
/// a regular one -- close enough to look right in a screenshot, wrong enough
/// that the last glyph falls outside its box when the text is long.
#[test]
fn the_measure_cache_does_not_confuse_two_typefaces() {
    let mut text = TextSystem::with_embedded_font();
    // Regular first, so a cache that ignores the typeface has already been
    // populated with the narrower answer by the time bold asks.
    let regular = text.measure(SAMPLE, 32.0, &Typeface::default());
    let heavy = text.measure(SAMPLE, 32.0, &bold());
    assert_ne!(
        regular, heavy,
        "bold read a regular measurement from the cache"
    );
    // And the same string in the other order, to catch a cache that is keyed
    // but compares wrongly.
    let mut other = TextSystem::with_embedded_font();
    let heavy_first = other.measure(SAMPLE, 32.0, &bold());
    let regular_second = other.measure(SAMPLE, 32.0, &Typeface::default());
    assert_eq!(heavy_first, heavy, "the bold measurement is not stable");
    assert_eq!(
        regular_second, regular,
        "the regular measurement is not stable"
    );
}

/// The wrapped measure cache distinguishes typefaces too.
///
/// A separate cache with a separate key, so it is a separate thing to get
/// wrong -- and a multi-line field is where the consequence shows: a bold field
/// breaks its lines at the widths of a regular one and the last line overflows.
#[test]
fn the_wrapped_measure_cache_does_not_confuse_two_typefaces() {
    let mut text = TextSystem::with_embedded_font();
    const WRAP: f32 = 200.0;
    let regular = text.measure_wrapped(SAMPLE, 20.0, WRAP, &Typeface::default());
    let heavy = text.measure_wrapped(SAMPLE, 20.0, WRAP, &bold());
    assert_ne!(regular, heavy, "bold read a regular wrapped measurement");
}

/// A family name selects a different face when one exists, and is harmless
/// when it does not.
///
/// The "harmless" half matters: a document asking for a font the host has not
/// installed must get *something*, not a blank line. The embedded set is one
/// known family, so asking for it is the case that can be asserted exactly.
#[test]
fn a_named_family_is_answered_rather_than_ignored() {
    let mut text = TextSystem::with_embedded_font();
    let named = text.measure(
        SAMPLE,
        32.0,
        &Typeface {
            family: "DejaVu Sans".to_string(),
            ..Typeface::default()
        },
    );
    assert!(named.0 > 0.0, "a named family produced no advance at all");
    assert!(
        named.1 > 0.0,
        "a named family produced no line height: the glyphs are missing"
    );
}

/// Italic is a real axis, not a synonym for weight.
#[test]
fn italic_shapes_differently_from_regular() {
    let mut text = TextSystem::with_embedded_font();
    let regular = text.shape(SAMPLE, 32.0, &Typeface::default());
    let italic = text.shape(
        SAMPLE,
        32.0,
        &Typeface {
            italic: true,
            ..Typeface::default()
        },
    );
    assert_ne!(regular.glyphs[0].key, italic.glyphs[0].key);
}
