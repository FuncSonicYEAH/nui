//! The text-field family (FUTURE 批次 6), end to end through the engine.
//!
//! These are the *shaped* halves of the field: the pure cursor/insert
//! arithmetic is unit-tested in `nui-runtime`'s `text_input` module, and
//! what is left here is what only a real document can show — that the flags
//! reach the editing core, that a submit and a newline are told apart, that
//! `invalid` is written back as a *host* change, and that a widget write
//! reaches its `<=>` partner.
#![allow(clippy::unwrap_used)]

use nui_compiler::compile;
use nui_core::{Key, Modifiers, Value};
use nui_runtime::text_input::VisualLine;
use nui_runtime::{Engine, instantiate};

/// Compiles and instantiates a source document.
fn build(source: &str) -> (nui_runtime::ElementTree, Engine) {
    let outcome = compile(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "test source must compile cleanly: {:?}",
        outcome.diagnostics
    );
    let instance = instantiate(&outcome.document);
    return (instance.tree, instance.engine);
}

/// The `text` property of an element, or `None`.
fn text_of(tree: &nui_runtime::ElementTree, id: nui_runtime::ElementId) -> Option<String> {
    let value = tree.arena[id].get("text")?;
    return value.as_str().ok().map(str::to_string);
}

/// The `invalid` property of an element, or `None` when it has no slot.
fn invalid_of(tree: &nui_runtime::ElementTree, id: nui_runtime::ElementId) -> Option<bool> {
    return tree.arena[id]
        .get("invalid")
        .and_then(|value| return value.as_bool().ok());
}

/// The `accepted` signal handler both submit tests share.
const SUBMIT: &str = "
    component Form {
        property submitted: Bool = false
        property body: String = \"\"

        Window(id = root) {
            TextInput(id = line, text = \"hello\") {
                on accepted => submitted = true
            }
            TextInput(id = area, multiline = true, text = \"first\") {
                on accepted => submitted = true
            }
        }
    }
";

#[test]
fn enter_submits_one_line_and_breaks_the_other() {
    let (mut tree, mut engine) = build(SUBMIT);
    let line = tree.lookup_id("line").unwrap();
    let area = tree.lookup_id("area").unwrap();
    let root = tree.lookup_id("root").unwrap();

    // A one-line field: Enter is a submit, and the text is untouched.
    engine.focus(line);
    assert!(engine.handle_key(&mut tree, Key::Enter, Modifiers::NONE));
    assert_eq!(tree.arena[root].get("submitted"), Some(&Value::Bool(true)));
    assert_eq!(text_of(&tree, line).as_deref(), Some("hello"));

    tree.arena[root].set("submitted", Value::Bool(false));

    // A multi-line field: Enter is a newline (the end of the last line
    // still counts as a line, so the cursor stays where it was put).
    engine.focus(area);
    engine.handle_key(&mut tree, Key::End, Modifiers::NONE);
    assert!(engine.handle_key(&mut tree, Key::Enter, Modifiers::NONE));
    assert_eq!(text_of(&tree, area).as_deref(), Some("first\n"));
    assert_eq!(
        tree.arena[root].get("submitted"),
        Some(&Value::Bool(false)),
        "Enter did not submit"
    );

    // ... and Ctrl+Enter is the submit.
    assert!(engine.handle_key(
        &mut tree,
        Key::Enter,
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    ));
    assert_eq!(tree.arena[root].get("submitted"), Some(&Value::Bool(true)));
    assert_eq!(
        text_of(&tree, area).as_deref(),
        Some("first\n"),
        "the submitting Enter added no newline"
    );
}

#[test]
fn a_pasted_line_break_does_not_split_a_one_line_field() {
    let (mut tree, mut engine) = build(SUBMIT);
    let line = tree.lookup_id("line").unwrap();
    let area = tree.lookup_id("area").unwrap();

    engine.focus(line);
    engine.handle_key(&mut tree, Key::End, Modifiers::NONE);
    engine.handle_text_input(&mut tree, " one\ntwo");
    assert_eq!(
        text_of(&tree, line).as_deref(),
        Some("hello onetwo"),
        "the newline is dropped, the rest is not"
    );

    engine.focus(area);
    engine.handle_key(&mut tree, Key::End, Modifiers::NONE);
    engine.handle_text_input(&mut tree, "\nsecond");
    assert_eq!(text_of(&tree, area).as_deref(), Some("first\nsecond"));
}

#[test]
fn a_read_only_field_refuses_edits_but_still_moves() {
    let source = r#"
        component R {
            Window(id = root) {
                TextInput(id = field, read_only = true, text = "locked")
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let field = tree.lookup_id("field").unwrap();
    engine.focus(field);

    assert!(
        !engine.handle_text_input(&mut tree, "x"),
        "a read-only field consumes nothing, so the key can reach elsewhere"
    );
    assert!(!engine.handle_key(&mut tree, Key::Character('x'), Modifiers::NONE));
    assert!(!engine.handle_key(&mut tree, Key::Backspace, Modifiers::NONE));
    assert!(!engine.handle_key(&mut tree, Key::Delete, Modifiers::NONE));
    assert_eq!(text_of(&tree, field).as_deref(), Some("locked"));

    // Movement and selection stay live: a read-only field is readable.
    assert!(engine.handle_key(&mut tree, Key::Home, Modifiers::NONE));
    assert!(engine.handle_key(
        &mut tree,
        Key::ArrowRight,
        Modifiers {
            shift: true,
            ..Modifiers::NONE
        }
    ));
    assert_eq!(engine.copy_focused(&tree).as_deref(), Some("l"));
    // A cut is still an edit, so the selection survives it.
    assert!(engine.cut_focused(&mut tree).is_none());
    assert_eq!(text_of(&tree, field).as_deref(), Some("locked"));
}

#[test]
fn max_length_caps_typing_and_pasting() {
    let source = r#"
        component C {
            Window(id = root) {
                TextInput(id = field, max_length = 4)
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let field = tree.lookup_id("field").unwrap();
    engine.focus(field);

    engine.handle_text_input(&mut tree, "ab");
    engine.handle_key(&mut tree, Key::Character('c'), Modifiers::NONE);
    assert_eq!(text_of(&tree, field).as_deref(), Some("abc"));

    // A paste that would overflow is truncated to the cap.
    engine.handle_text_input(&mut tree, "defgh");
    assert_eq!(text_of(&tree, field).as_deref(), Some("abcd"));
    // At the cap, nothing more is accepted.
    engine.handle_text_input(&mut tree, "!");
    assert_eq!(text_of(&tree, field).as_deref(), Some("abcd"));
    // Backspace frees a slot.
    engine.handle_key(&mut tree, Key::Backspace, Modifiers::NONE);
    engine.handle_text_input(&mut tree, "!e");
    assert_eq!(text_of(&tree, field).as_deref(), Some("abc!"));
}

#[test]
fn a_validator_writes_invalid_back_and_clears_it() {
    let source = r#"
        component V {
            Window(id = root) {
                TextInput(id = checked, validator = "int")
                TextInput(id = unchecked)
                TextInput(id = mailed, validator = "email", text = "nope@")
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let checked = tree.lookup_id("checked").unwrap();
    let unchecked = tree.lookup_id("unchecked").unwrap();
    let mailed = tree.lookup_id("mailed").unwrap();

    // A field with no validator never grows an `invalid` slot.
    engine.focus(unchecked);
    engine.handle_text_input(&mut tree, "anything");
    assert_eq!(invalid_of(&tree, unchecked), None);

    engine.focus(checked);
    engine.handle_text_input(&mut tree, "4a");
    assert_eq!(invalid_of(&tree, checked), Some(true));
    // The write is the host's, not the user's, and it is reported.
    engine.handle_key(&mut tree, Key::Backspace, Modifiers::NONE);
    assert_eq!(invalid_of(&tree, checked), Some(false), "4 is an int");
    let changes = engine.take_changes();
    assert!(
        changes
            .iter()
            .any(|change| return change.property == "invalid"),
        "the invalid write is observable: {changes:?}"
    );

    // A seed that is already wrong is caught on the first reconcile,
    // without the user touching the field.
    assert_eq!(invalid_of(&tree, mailed), None, "not reconciled yet");
    engine.focus(mailed);
    engine.handle_key(&mut tree, Key::Home, Modifiers::NONE);
    assert_eq!(invalid_of(&tree, mailed), Some(true));
    engine.handle_key(
        &mut tree,
        Key::Character('a'),
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        },
    );
    engine.handle_text_input(&mut tree, "user@example.com");
    assert_eq!(text_of(&tree, mailed).as_deref(), Some("user@example.com"));
    assert_eq!(invalid_of(&tree, mailed), Some(false));
}

#[test]
fn select_all_on_focus_applies_to_both_focus_paths() {
    let source = r#"
        component S {
            Window(id = root) {
                TextInput(id = first, select_all_on_focus = true, text = "aaaa")
                TextInput(id = second, select_all_on_focus = true, text = "bbbb")
                TextInput(id = plain, text = "cccc")
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let first = tree.lookup_id("first").unwrap();
    let second = tree.lookup_id("second").unwrap();
    let plain = tree.lookup_id("plain").unwrap();

    // A direct focus (a click, in the host) selects the whole field.
    engine.focus_element(&mut tree, first);
    assert_eq!(engine.copy_focused(&tree).as_deref(), Some("aaaa"));
    engine.handle_text_input(&mut tree, "z");
    assert_eq!(text_of(&tree, first).as_deref(), Some("z"), "replaced");

    // Tab lands with the same policy: tabbing in and typing replaces.
    engine.focus_element(&mut tree, first);
    engine.handle_key(&mut tree, Key::Tab, Modifiers::NONE);
    assert_eq!(engine.focused(), Some(second));
    engine.handle_text_input(&mut tree, "y");
    assert_eq!(text_of(&tree, second).as_deref(), Some("y"));

    // A field without the flag keeps its text and appends.
    engine.focus_element(&mut tree, plain);
    assert!(engine.copy_focused(&tree).is_none());
    engine.handle_text_input(&mut tree, "d");
    assert_eq!(text_of(&tree, plain).as_deref(), Some("ccccd"));
}

#[test]
fn vertical_movement_through_the_engine_uses_the_line_table() {
    let source = r#"
        component M {
            Window(id = root) {
                TextInput(id = area, multiline = true, text = "abcdefghij")
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let area = tree.lookup_id("area").unwrap();
    engine.focus(area);

    // Two visual lines, five chars each, at 10dp per char.
    let lines = vec![
        VisualLine {
            start: 0,
            end: 5,
            caret_x: vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0],
        },
        VisualLine {
            start: 5,
            end: 10,
            caret_x: vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0],
        },
    ];
    engine.handle_key(&mut tree, Key::Home, Modifiers::NONE);
    for _ in 0..3 {
        engine.handle_key(&mut tree, Key::ArrowRight, Modifiers::NONE);
    }
    assert!(engine.move_focused_vertical(&mut tree, &lines, false, false));
    // The caret moved, and the text is untouched (a move is not an edit).
    assert!(engine.handle_key(&mut tree, Key::End, Modifiers::NONE));
    assert_eq!(engine.copy_focused(&tree), None, "no selection was made");

    // Up from the first line is unhandled, so an enclosing control can
    // take the key.
    engine.handle_key(&mut tree, Key::Home, Modifiers::NONE);
    assert!(!engine.move_focused_vertical(&mut tree, &lines, true, false));

    // An empty table (no shaped geometry yet) is not a crash.
    assert!(!engine.move_focused_vertical(&mut tree, &[], false, false));
}

#[test]
fn a_widget_write_reaches_its_two_way_partner() {
    let source = r#"
        component N {
            property amount: Float = 0.0
            property open: Bool = false

            Window(id = root) {
                SpinBox(id = spin, value <=> root.amount, min = 0.0, max = 4.0, step = 0.5)
                Slider(id = bar, value <=> root.amount, min = 0.0, max = 4.0)
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let root = tree.lookup_id("root").unwrap();
    let spin = tree.lookup_id("spin").unwrap();
    let bar = tree.lookup_id("bar").unwrap();

    // The stepper is the host's write path (`set_direct`); the partner must
    // see it, exactly as a bound write would.
    for expected in [0.5, 1.0, 1.5] {
        assert!(nui_runtime::widget::step(&mut engine, &mut tree, spin, 1.0));
        assert_eq!(
            tree.arena[root].get("amount"),
            Some(&Value::Float(expected)),
            "the model tracks the stepper"
        );
    }
    assert_eq!(
        tree.arena[spin].get("value"),
        Some(&Value::Float(1.5)),
        "and the stepper holds what it wrote"
    );

    // Characterization of a *known* limit: `<=>` follows one hop. `bar` is
    // linked to the same model property, but no link pushes a value it did
    // not itself write, so it stays where it was born. Fanning a write out
    // to every element linked to the partner needs a reverse index in the
    // value channel (see FUTURE.md).
    assert_eq!(
        tree.arena[bar].get("value"),
        Some(&Value::Float(0.0)),
        "a sibling pair is not resynced from someone else's write"
    );

    // The model still routes a write from the *other* control one hop: the
    // slider steps from its own stale 0 to 1, and the model follows.
    assert!(nui_runtime::widget::step(&mut engine, &mut tree, bar, 1.0));
    assert_eq!(tree.arena[root].get("amount"), Some(&Value::Float(1.0)));
    assert_eq!(tree.arena[bar].get("value"), Some(&Value::Float(1.0)));
    let _ = engine.take_changes();
}
