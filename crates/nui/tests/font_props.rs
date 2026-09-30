//! `font.family` / `font.weight` / `font.italic`, from a document.
//!
//! # Why this file exists when `nui-render/tests/font_props.rs` covers the rest
//!
//! That file proves the property reaches the measurement, using
//! `Element::set`. This one proves a *document* can write it — which is a
//! different question, because a document's `font.weight = 700` travels as an
//! attached property with an unvalidated prefix (`font.size` works the same
//! way), and nothing in the compiler stands between the source text and the
//! element property. If that ever changes, this is the test that says so.
//!
//! It also covers the one case a layout test cannot: a weight that arrives
//! through a *binding* rather than a literal, which means the value is
//! re-evaluated rather than seeded once.

#![allow(clippy::unwrap_used)]

use nui_core::{Size, Value};
use nui_runtime::{ElementId, ElementTree, Engine};

const SAMPLE: &str = "Handgloves";

const VIEWPORT: Size = Size {
    width: 800.0,
    height: 400.0,
};

/// Compiles, instantiates and lays out a document with one `Text` element, and
/// returns the width layout reserved for it.
fn measured(source: &str) -> (f32, ElementTree, Engine) {
    let outcome = nui_compiler::compile(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "the document should compile: {:?}",
        outcome.diagnostics
    );
    let instance = nui_runtime::instantiate(&outcome.document);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    let _ = engine.propagate(&mut tree);
    let mut text = nui_text::TextSystem::with_embedded_font();
    nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
    let label = tree
        .lookup_id("label")
        .expect("the document declared `label`");
    let Some(Value::Float(width)) = tree.arena[label].get("width") else {
        panic!("layout wrote no width back");
    };
    return (*width as f32, tree, engine);
}

/// [`measured`]'s width, for the tests that do not need the tree back.
fn measured_width(source: &str) -> f32 {
    let (width, _tree, _engine) = measured(source);
    return width;
}

/// A document that sets no weight is measured as the default face.
#[test]
fn an_unset_weight_is_the_default_face() {
    let source = format!(
        "component App {{
             Window(id = shell, width = 800dp, height = 400dp) {{
                 Text(id = label, content = \"{SAMPLE}\", font.size = 32dp, align_self = \"start\")
             }}
         }}"
    );
    let unset = measured_width(&source);
    let explicit = measured_width(&source.replace("align_self", "font.weight = 400, align_self"));
    assert!(
        (unset - explicit).abs() < 0.01,
        "unset measured {unset}, an explicit 400 measured {explicit}"
    );
}

/// A weight written in a document reaches the measurement.
#[test]
fn a_document_weight_reaches_the_measurement() {
    let plain = format!(
        "component App {{
             Window(id = shell, width = 800dp, height = 400dp) {{
                 Text(id = label, content = \"{SAMPLE}\", font.size = 32dp, align_self = \"start\")
             }}
         }}"
    );
    let bold = plain.replace("align_self", "font.weight = 700, align_self");
    let plain_width = measured_width(&plain);
    let bold_width = measured_width(&bold);
    assert!(
        bold_width > plain_width + 0.5,
        "a bold document run measured {bold_width}, not more than the regular {plain_width}"
    );
}

/// A weight that arrives through a binding, not a literal.
///
/// The seeded path and the binding path are different code: a literal is
/// written once at instantiation, a binding is registered and re-evaluated.
/// A document that computes its weight gets the second, and if only the first
/// worked then `font.weight = shell.weight` would quietly render regular.
#[test]
fn a_bound_weight_reaches_the_measurement() {
    let source = format!(
        "component App {{
             property heavy: Bool = true
             Window(id = shell, width = 800dp, height = 400dp) {{
                 Text(id = label, content = \"{SAMPLE}\", font.size = 32dp,
                      font.weight <- shell.heavy ? 700 : 400, align_self = \"start\")
             }}
         }}"
    );
    let (bound, mut tree, mut engine) = measured(&source);
    let bold_direct = plain_width(700);
    assert!(
        (bound - bold_direct).abs() < 0.5,
        "a bound 700 measured {bound}, a literal 700 measured {bold_direct}"
    );
    // And flipping the source property really does change the measurement,
    // which is what makes it a binding rather than a seeded value. The engine
    // that laid the tree out is the one that has the binding, so it has to be
    // this one: a fresh `Engine` would have no bindings at all and this would
    // pass for the wrong reason.
    let shell = tree.lookup_id("shell").expect("the shell");
    engine.set_direct(&mut tree, shell, "heavy", Value::Bool(false));
    let _ = engine.propagate(&mut tree);
    let mut text = nui_text::TextSystem::with_embedded_font();
    nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
    let label = tree.lookup_id("label").expect("the label");
    let Some(Value::Float(light)) = tree.arena[label].get("width") else {
        panic!("no width after flipping");
    };
    let light = *light as f32;
    assert!(
        light < bound - 0.5,
        "flipping heavy to false left the width at {light} (was {bound})"
    );
}

/// The width a plain document measures at a given weight, for comparison.
fn plain_width(weight: u16) -> f32 {
    let source = format!(
        "component App {{
             Window(id = shell, width = 800dp, height = 400dp) {{
                 Text(id = label, content = \"{SAMPLE}\", font.size = 32dp,
                      font.weight = {weight}, align_self = \"start\")
             }}
         }}"
    );
    return measured_width(&source);
}

/// A family name in a document is answered rather than dropped.
///
/// Asking for a family the host does not have must still produce a run: the
/// failure mode to rule out is a document that sets `font.family` and gets an
/// empty line, which is invisible until someone notices the text is gone.
#[test]
fn a_document_family_still_produces_a_run() {
    let source = format!(
        "component App {{
             Window(id = shell, width = 800dp, height = 400dp) {{
                 Text(id = label, content = \"{SAMPLE}\", font.size = 32dp,
                      font.family = \"A Font Nobody Has Installed\", align_self = \"start\")
             }}
         }}"
    );
    let width = measured_width(&source);
    assert!(
        width > 1.0,
        "an unknown family measured {width}: the run came out empty"
    );
}

/// `font.italic` and `font.weight` are independent axes.
#[test]
fn italic_and_weight_are_independent() {
    let plain = plain_width(400);
    let italic = format!(
        "component App {{
             Window(id = shell, width = 800dp, height = 400dp) {{
                 Text(id = label, content = \"{SAMPLE}\", font.size = 32dp,
                      font.italic = true, align_self = \"start\")
             }}
         }}"
    );
    let italic_width = measured_width(&italic);
    assert!(
        (italic_width - plain).abs() > 0.5,
        "italic measured {italic_width}, the same as regular {plain}"
    );
}

/// The two `typeface_of` readers agree about the element they are given.
///
/// The invariant the whole design rests on. They are separate functions in
/// sibling crates, so this is the one place that can compare them directly.
#[test]
fn the_two_typeface_readers_agree() {
    let source = format!(
        "component App {{
             Window(id = shell, width = 800dp, height = 400dp) {{
                 Text(id = label, content = \"{SAMPLE}\", font.size = 32dp,
                      font.family = \"DejaVu Sans\", font.weight = 550,
                      font.italic = true, align_self = \"start\")
             }}
         }}"
    );
    let (_width, tree, _engine) = measured(&source);
    let label: ElementId = tree.lookup_id("label").expect("the label");
    let from_layout = nui_layout::typeface_of(&tree.arena[label]);
    let from_render = nui_render::typeface_of(&tree.arena[label]);
    assert_eq!(
        from_layout, from_render,
        "the two readers disagree: layout {from_layout:?} vs render {from_render:?}"
    );
    assert_eq!(from_layout.weight, 550);
    assert_eq!(from_layout.family, "DejaVu Sans");
    assert!(from_layout.italic);
}
