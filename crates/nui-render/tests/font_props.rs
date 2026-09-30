//! `font.weight` must reach the *measurement*, not only the renderer.
//!
//! # The bug this is about
//!
//! `nui-layout` and `nui-render` are siblings, so each reads the three
//! `font.*` properties itself (see `props::typeface_of` and
//! `nui_layout::typeface_of`). If only the renderer learns about a weight, the
//! run is *drawn* in bold and *measured* as regular. Nothing errors. The box is
//! a few percent too narrow, the text still renders, and the failure only shows
//! when the run is long enough that the last glyph falls outside the box -- at
//! which point it looks like a clipping bug rather than a font bug.
//!
//! So the assertion is an equality between what layout reserved and what the
//! shaper produced, for a bold run. That is the invariant both readers have to
//! hold, and it fails the moment either one stops reading the property.

use nui_core::{Size, Value};
use nui_runtime::{Element, ElementTree};

const SAMPLE: &str = "Handgloves 0123";

/// Lays out one `Text` element and returns the width its box was given.
fn laid_out_width(font_weight: Option<i64>) -> f32 {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Length(nui_core::Length::Dp(600.0)));
    root.set("height", Value::Length(nui_core::Length::Dp(200.0)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);

    let mut label = Element::new("Text", None);
    label.set("content", Value::String(SAMPLE.to_string()));
    label.set("font.size", Value::Length(nui_core::Length::Dp(32.0)));
    // Without this the `Column` stretches the label to its own 600dp and the
    // measurement is never observable.
    label.set("align_self", Value::String("start".to_string()));
    if let Some(weight) = font_weight {
        label.set("font.weight", Value::Int(weight));
    }
    let label_id = tree.insert(label);
    tree.append_child(root_id, label_id);

    let mut text = nui_text::TextSystem::with_embedded_font();
    nui_layout::layout_with_text(&mut tree, Size::new(600.0, 200.0), Some(&mut text));
    return laid_out_width_of(&tree, label_id);
}

/// The width layout wrote back onto an element.
///
/// Read from the tree rather than through a `Rect` helper so the test needs
/// only `nui-render` and its own dev-dependencies, and so it asserts on the
/// number the *next* layout pass will read.
fn laid_out_width_of(tree: &ElementTree, id: nui_runtime::ElementId) -> f32 {
    let Some(width) = tree.arena[id].get("width") else {
        panic!("the label was not laid out: no width was written back");
    };
    // `Value::Float` is an `f64` while layout works in `f32` dp, so the
    // narrowing happens here. `as` rather than a conversion that could fail: the
    // value came out of a `f32` layout pass one line above, so it is in range
    // by construction and a `NaN` check would only be testing the cast.
    let Value::Float(width) = width else {
        panic!("the written-back width is not a float: {width:?}");
    };
    return *width as f32;
}

/// The width a bold run is measured at is the width it is shaped at.
#[test]
fn font_weight_reaches_the_measurement() {
    for weight in [700_i64, 300, 900] {
        let declared = laid_out_width(None);
        let bold = laid_out_width(Some(weight));
        let mut text = nui_text::TextSystem::with_embedded_font();
        let typeface = nui_text::Typeface {
            weight: weight as u16,
            ..nui_text::Typeface::default()
        };
        let shaped = text.measure(SAMPLE, 32.0, &typeface).0;
        assert!(
            (bold - shaped).abs() < 0.5,
            "weight {weight}: layout reserved {bold} but the shaper produced {shaped}"
        );
        // Sanity: the run really is a different one, so the equality above is
        // not two default weights agreeing with each other.
        assert!(
            (bold - declared).abs() > 0.5,
            "weight {weight} measured the same as no weight at all ({bold})"
        );
    }
}

/// A document that sets no weight measures exactly as before.
///
/// The property is new and optional, so "unset" has to be the *old* behaviour
/// rather than merely a plausible one. This is also what keeps the golden
/// snapshots byte-identical, so it is the test that would notice if the
/// default drifted.
#[test]
fn an_unset_weight_measures_as_the_default_face() {
    let unset = laid_out_width(None);
    let explicit = laid_out_width(Some(400));
    assert!(
        (unset - explicit).abs() < 0.01,
        "unset measured {unset} but an explicit 400 measured {explicit}"
    );
}

/// A weight the document writes as a float is read the same as an integer.
///
/// A document computes a weight; the result is a `Float`, not an `Int`. A
/// reader that only accepted `Int` would silently give the default face, and
/// `font.size` has the same shape of trap already (`dp_of` accepts both).
#[test]
fn a_fractional_weight_is_not_silently_ignored() {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Length(nui_core::Length::Dp(600.0)));
    root.set("height", Value::Length(nui_core::Length::Dp(200.0)));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    let mut label = Element::new("Text", None);
    label.set("content", Value::String(SAMPLE.to_string()));
    label.set("font.size", Value::Length(nui_core::Length::Dp(32.0)));
    // Without this the `Column` stretches the label to its own 600dp and the
    // measurement is never observable.
    label.set("align_self", Value::String("start".to_string()));
    label.set("font.weight", Value::Float(700.0));
    let label_id = tree.insert(label);
    tree.append_child(root_id, label_id);

    let mut text = nui_text::TextSystem::with_embedded_font();
    nui_layout::layout_with_text(&mut tree, Size::new(600.0, 200.0), Some(&mut text));
    let fractional = laid_out_width_of(&tree, label_id);
    let integer = laid_out_width(Some(700));
    assert!(
        (fractional - integer).abs() < 0.01,
        "a Float 700 measured {fractional} but an Int 700 measured {integer}"
    );
}
