//! End-to-end tests for the text-field family (FUTURE 批次 6), driven by
//! the gallery's `text_fields` page.
//!
//! The editing core is unit-tested in `nui-runtime`, the flags there too,
//! and the wrap in `nui-layout`. What is left — and what this file covers —
//! is the seam between them: the pieces the *host* has to get right because
//! only it owns both a font and the pointer. Specifically:
//!
//! - the line table a multi-line field's Up/Down walks is built from the
//!   field's real box, and the field's box is content-sized *by* that wrap
//!   (`nui-layout` ← `nui-text`);
//! - a stepper's press zone is the same geometry the arrow glyph is drawn
//!   at (`nui-render` ← `nui-runtime`);
//! - a label's `for` resolves to a real element, so a click has somewhere
//!   to send focus.
//!
//! Using the shipped page as the fixture keeps the demo and the test
//! from drifting apart.
#![allow(clippy::unwrap_used)]

use nui_core::{Key, Modifiers, Size, Value};
use nui_runtime::widget;
use nui_runtime::{ElementId, ElementTree, Engine};

/// The gallery's `text_fields` page, in a document of its own (the page
/// keeps its state on its own root element, so a bare window is enough
/// to host it).
fn page_source() -> String {
    return format!(
        "component TextFieldsPage {{\n    Window(id = root) {{\n{}\n    }}\n}}\n",
        include_str!("../examples/gallery/pages/text_fields.nui")
    );
}

/// The viewport the gallery lays its pages out in.
const VIEWPORT: Size = Size {
    width: 1120.0,
    height: 760.0,
};

/// Instantiates and lays out the page the way the host does.
fn example() -> (ElementTree, Engine) {
    let source = page_source();
    let outcome = nui_compiler::compile(&source);
    assert!(
        outcome.diagnostics.is_empty(),
        "the shipped page must compile clean: {:?}",
        outcome.diagnostics
    );
    let instance = nui_runtime::instantiate(&outcome.document);
    let mut tree = instance.tree;
    let mut text = nui_text::TextSystem::with_embedded_font();
    // Bindings settle on the first pass, geometry on the next.
    for _ in 0..3 {
        nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
    }
    return (tree, instance.engine);
}

/// Lays the tree out again (after a text change made it a different size).
fn relayout(tree: &mut ElementTree) {
    let mut text = nui_text::TextSystem::with_embedded_font();
    for _ in 0..3 {
        nui_layout::layout_with_text(tree, VIEWPORT, Some(&mut text));
    }
}

fn find(tree: &ElementTree, id_name: &str) -> ElementId {
    return tree
        .lookup_id(id_name)
        .unwrap_or_else(|| panic!("the page declares `{id_name}`"));
}

fn number(tree: &ElementTree, id: ElementId, name: &str) -> f32 {
    return match tree.arena[id].get(name) {
        Some(Value::Float(inner)) => *inner as f32,
        Some(Value::Int(inner)) => *inner as f32,
        Some(Value::Length(nui_core::Length::Dp(inner))) => *inner,
        other => panic!("expected a numeric {name}, got {other:?}"),
    };
}

fn text_of(tree: &ElementTree, id: ElementId) -> Option<String> {
    let value = tree.arena[id].get("text")?;
    return value.as_str().ok().map(str::to_string);
}

fn invalid_of(tree: &ElementTree, id: ElementId) -> Option<bool> {
    return tree.arena[id]
        .get("invalid")
        .and_then(|value| return value.as_bool().ok());
}

/// Sets a field's text the way a binding would (so the `<=>` partner moves
/// too), then lays out again.
fn set_text(tree: &mut ElementTree, engine: &mut Engine, id: ElementId, text: &str) {
    assert!(engine.set_direct(tree, id, "text", Value::String(text.to_string())));
    relayout(tree);
}

#[test]
fn tab_walks_the_fields_and_the_steppers_in_document_order() {
    // A stepper that cannot be focused can never be stepped from the
    // keyboard, and a label that names a `Button` would focus nothing —
    // so the focus order is part of this batch, not a detail of it.
    let (mut tree, mut engine) = example();
    for name in [
        "text_name",
        "text_secret",
        "text_frozen",
        "text_bio",
        "text_mail", // the fields
        "text_count",
        "text_fine",
        "text_save", // the controls
    ] {
        engine.handle_key(&mut tree, Key::Tab, Modifiers::NONE);
        assert_eq!(
            engine.focused(),
            Some(find(&tree, name)),
            "tab reaches `{name}`"
        );
    }
    // ... and wraps back to the first field.
    engine.handle_key(&mut tree, Key::Tab, Modifiers::NONE);
    assert_eq!(engine.focused(), Some(find(&tree, "text_name")));
}

#[test]
fn the_page_compiles_and_instantiates() {
    let (tree, _engine) = example();
    for id in [
        "text_fields_page",
        "text_name",
        "text_secret",
        "text_frozen",
        "text_bio",
        "text_mail",
        "text_count",
        "text_fine",
        "text_save",
    ] {
        let element = find(&tree, id);
        assert!(
            number(&tree, element, "width") > 0.0,
            "`{id}` must have been laid out"
        );
    }
}

#[test]
fn a_multiline_field_is_sized_by_its_wrapped_lines() {
    let (mut tree, mut engine) = example();
    let bio = find(&tree, "text_bio");
    assert!(
        tree.arena[bio].is_multiline(),
        "the area declares multiline"
    );
    let one_line = number(&tree, bio, "height");

    // Three hard breaks is four lines, so the box grows by roughly three
    // line heights — the point being that it grows at all: the field is
    // content-sized, which is the whole reason `nui-layout` had to learn
    // to measure a wrap.
    set_text(&mut tree, &mut engine, bio, "one\ntwo\nthree\nfour");
    let grown = number(&tree, bio, "height");
    assert!(
        grown > one_line + 30.0,
        "a four-line area must be taller than a one-line one: {one_line} -> {grown}"
    );

    // And it shrinks back: the height tracks the content, not just its peak.
    set_text(&mut tree, &mut engine, bio, "one");
    assert!(
        (number(&tree, bio, "height") - one_line).abs() < 1.0,
        "back to one line"
    );
}

#[test]
fn the_caret_line_table_matches_the_field_box() {
    let (mut tree, mut engine) = example();
    let bio = find(&tree, "text_bio");
    // Long enough to wrap at the field's width, with a short line after the
    // break so the vertical column has something to remember.
    let content = "wrap this sentence across the field, which is narrow\nok";
    set_text(&mut tree, &mut engine, bio, content);

    // The host builds the table from the laid-out box and the field's own
    // inset and font size — exactly this.
    let bounds = nui::element_bounds(&tree, bio).expect("the area has a box");
    let element = &tree.arena[bio];
    let inset = nui_runtime::widget::chrome::text_inset(element);
    let font_size = nui_runtime::widget::chrome::text_size(element);
    let mut text = nui_text::TextSystem::with_embedded_font();
    let lines = nui_layout::visual_lines(
        &mut text,
        content,
        font_size,
        (bounds.size.width - inset * 2.0).max(1.0),
    );
    assert!(
        lines.len() > 2,
        "the sentence wraps *and* has a hard break: {} lines",
        lines.len()
    );
    // The lines partition the text: every char belongs to exactly one.
    let mut next = 0;
    for line in &lines {
        assert_eq!(line.start, next, "lines are contiguous");
        next = line.end;
    }
    assert_eq!(next, content.chars().count(), "and they cover all of it");

    // The caret walks them: down from the middle of the first line.
    engine.focus(bio);
    engine.handle_key(&mut tree, Key::Home, Modifiers::NONE);
    for _ in 0..10 {
        engine.handle_key(&mut tree, Key::ArrowRight, Modifiers::NONE);
    }
    assert!(
        engine.move_focused_vertical(&mut tree, &lines, false, false),
        "there is a line below"
    );
    // Back up: the same visual column is remembered across the short line.
    assert!(engine.move_focused_vertical(&mut tree, &lines, true, false));
}

#[test]
fn a_label_names_a_real_control_and_marks_required_fields() {
    let (tree, _engine) = example();
    let mut labels = 0;
    let mut required: Vec<String> = Vec::new();
    tree.visit_pre_order(|_id, element| {
        let Some(target) = widget::label_target(element) else {
            return;
        };
        labels += 1;
        assert!(
            tree.lookup_id(target).is_some(),
            "`for = {target}` names an element that exists"
        );
        if widget::is_required(element) {
            required.push(target.to_string());
        }
    });
    assert_eq!(labels, 5, "the page labels five fields");
    // `required` is read off the label, and only when it names a control.
    assert_eq!(
        required,
        vec!["text_name".to_string(), "text_secret".to_string()]
    );
}

#[test]
fn a_press_in_the_arrow_strip_steps_the_stepper() {
    let (mut tree, mut engine) = example();
    let count = find(&tree, "text_count");
    let bounds = nui::element_bounds(&tree, count).expect("the stepper has a box");
    // A point in the top-right corner: inside the strip, above the midline.
    let corner = nui_core::Point::new(bounds.size.width - 2.0, 4.0);
    let down = nui_core::Point::new(bounds.size.width - 2.0, bounds.size.height - 4.0);

    let steps = widget::direction_at(bounds.size.width, bounds.size.height, corner.x, corner.y);
    assert_eq!(steps, 1.0, "the up arrow");
    assert!(widget::step(&mut engine, &mut tree, count, steps));
    assert_eq!(tree.arena[count].get("value"), Some(&Value::Float(4.0)));
    // The `<=>` partner tracks the host's write.
    assert_eq!(
        tree.arena[find(&tree, "text_fields_page")].get("count"),
        Some(&Value::Float(4.0)),
        "the model follows the stepper"
    );

    let steps = widget::direction_at(bounds.size.width, bounds.size.height, down.x, down.y);
    assert_eq!(steps, -1.0, "the down arrow");
    assert!(widget::step(&mut engine, &mut tree, count, steps));
    assert_eq!(tree.arena[count].get("value"), Some(&Value::Float(3.0)));

    // The value half (and a press left of the strip) is not a step.
    assert_eq!(
        widget::direction_at(bounds.size.width, bounds.size.height, 4.0, 4.0),
        0.0
    );

    // Stepping past `max` clamps and says so by writing nothing.
    assert!(widget::step(&mut engine, &mut tree, count, 100.0));
    assert_eq!(tree.arena[count].get("value"), Some(&Value::Float(10.0)));
    assert!(!widget::step(&mut engine, &mut tree, count, 1.0));
}

#[test]
fn a_fractional_stepper_keeps_its_quarter_steps() {
    let (mut tree, mut engine) = example();
    let fine = find(&tree, "text_fine");
    for expected in [0.75, 1.0] {
        assert!(widget::step(&mut engine, &mut tree, fine, 1.0));
        assert_eq!(tree.arena[fine].get("value"), Some(&Value::Float(expected)));
    }
    // Clamped at the top of its range, and the display keeps the precision
    // a quarter step needs.
    assert_eq!(
        widget::display_value(&tree.arena[fine]),
        "1.00",
        "the step's precision, not the value's"
    );
    assert!(!widget::step(&mut engine, &mut tree, fine, 1.0));
}

#[test]
fn the_cap_and_the_mask_are_read_off_the_fields() {
    let (mut tree, mut engine) = example();
    let name = find(&tree, "text_name");
    let secret = find(&tree, "text_secret");
    let frozen = find(&tree, "text_frozen");

    assert!(tree.arena[secret].is_password());
    assert!(!tree.arena[name].is_password());
    assert!(tree.arena[frozen].is_read_only());

    // The page's `max_length = 16` is a real cap on typing.
    engine.focus_element(&mut tree, name);
    engine.handle_text_input(&mut tree, &"x".repeat(40));
    assert_eq!(
        text_of(&tree, name).map(|text| return text.chars().count()),
        Some(16)
    );

    // A read-only field refuses the same input and keeps its seed.
    engine.focus_element(&mut tree, frozen);
    assert!(!engine.handle_text_input(&mut tree, "more"));
    assert_eq!(text_of(&tree, frozen).as_deref(), Some("generated-0001"));
}

#[test]
fn a_validator_marks_the_seed_invalid_and_then_clears_it() {
    let (mut tree, mut engine) = example();
    let mail = find(&tree, "text_mail");
    assert_eq!(text_of(&tree, mail).as_deref(), Some("ada@"));

    // Focus reconciles, which runs the validator: `ada@` is not an address.
    engine.focus_element(&mut tree, mail);
    assert_eq!(invalid_of(&tree, mail), Some(true));

    // `.nui` reads that write back (`fill <- text_mail.invalid ? ...`), so
    // it must also be reported as a change, not just stored.
    let changes = engine.take_changes();
    assert!(
        changes
            .iter()
            .any(|change| return change.property == "invalid"),
        "the invalid write is observable: {changes:?}"
    );

    engine.handle_key(
        &mut tree,
        Key::Character('a'),
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
    );
    engine.handle_text_input(&mut tree, "ada@example.com");
    assert_eq!(
        text_of(&tree, mail).as_deref(),
        Some("ada@example.com"),
        "the selection was replaced"
    );
    assert_eq!(invalid_of(&tree, mail), Some(false), "now it validates");
}

#[test]
fn select_all_on_focus_replaces_the_seed_on_the_click_path() {
    let (mut tree, mut engine) = example();
    let name = find(&tree, "text_name");
    assert!(tree.arena[name].selects_all_on_focus());

    // A click focuses through `focus_element`, which runs the policy.
    set_text(&mut tree, &mut engine, name, "Ada Lovelace");
    engine.focus_element(&mut tree, name);
    assert_eq!(engine.copy_focused(&tree).as_deref(), Some("Ada Lovelace"));
    engine.handle_text_input(&mut tree, "B");
    assert_eq!(text_of(&tree, name).as_deref(), Some("B"), "replaced");
    // A field without the flag keeps the policy out of the way.
    let frozen = find(&tree, "text_frozen");
    engine.focus_element(&mut tree, frozen);
    assert!(engine.copy_focused(&tree).is_none());
}
