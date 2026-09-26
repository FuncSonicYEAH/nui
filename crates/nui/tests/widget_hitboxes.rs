//! End-to-end hitbox tests: a control's *interactive* region must coincide
//! with its layout box, for the shipped `widgets` example.
//!
//! # The bug these pin down
//!
//! `nui-layout`'s `write_back` accumulates positions down the tree, so an
//! element's `x`/`y` is **already absolute**. Two consumers summed the
//! ancestor chain a second time:
//!
//! - `nui_runtime::widget::absolute_bounds` (drives `hovered`/`pressed`/
//!   `armed` and Slider dragging)
//! - `nui::app::element_bounds` (drives `cursor_in`, i.e. whether a
//!   release counts as a click)
//!
//! A control two levels deep therefore resolved to twice its real
//! position. Because the error equals the element's own absolute
//! coordinate, it grew with nesting depth and differed per control — the
//! reported symptom ("hitboxes drift downward, by different amounts").
//!
//! `hit_test` (the third coordinate path) walks the tree top-down keeping
//! the offset at zero, so it was always correct; that disagreement is what
//! made the drift visible as "I must click lower than the control".
#![allow(clippy::unwrap_used)]

use nui_core::{Point, Size, Value};
use nui_runtime::{ElementTree, Engine, WidgetStates};

/// The gallery's `widgets` page, in a document of its own (the page binds
/// nothing outside itself, so a bare window is enough to host it).
fn page_source() -> String {
    return format!(
        "component WidgetsPage {{\n    Window(id = root) {{\n{}\n    }}\n}}\n",
        include_str!("../examples/gallery/pages/widgets.nui")
    );
}

/// The viewport the gallery lays its pages out in.
const VIEWPORT: Size = Size {
    width: 1120.0,
    height: 760.0,
};

/// Instantiates and lays out the page the way the host does.
fn example_tree() -> ElementTree {
    let source = page_source();
    let outcome = nui_compiler::compile(&source);
    assert!(
        outcome.diagnostics.is_empty(),
        "the shipped page must compile clean: {:?}",
        outcome.diagnostics
    );
    let instance = nui_runtime::instantiate(&outcome.document);
    let mut tree = instance.tree;
    // Two passes: bindings settle on the first, geometry on the second.
    for _ in 0..3 {
        nui_layout::layout(&mut tree, VIEWPORT);
    }
    return tree;
}

fn f_of(tree: &ElementTree, id: nui_runtime::ElementId, name: &str) -> f32 {
    return match tree.arena[id].get(name) {
        Some(Value::Float(inner)) => *inner as f32,
        Some(Value::Int(inner)) => *inner as f32,
        Some(Value::Length(nui_core::Length::Dp(inner))) => *inner,
        other => panic!("expected a numeric {name}, got {other:?}"),
    };
}

/// Every element with the given type, in document order.
fn elements_of(tree: &ElementTree, ty: &str) -> Vec<nui_runtime::ElementId> {
    let mut found = Vec::new();
    tree.visit_pre_order(|id, element| {
        if element.ty == ty {
            found.push(id);
        }
    });
    return found;
}

fn label_of(tree: &ElementTree, id: nui_runtime::ElementId) -> String {
    return tree.arena[id]
        .get("label")
        .and_then(|value| return value.as_str().ok())
        .unwrap_or("")
        .to_string();
}

/// Whether the control accepts pointer input (`enabled`, defaulting true).
fn is_enabled(tree: &ElementTree, id: nui_runtime::ElementId) -> bool {
    return tree.arena[id]
        .get("enabled")
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(true);
}

/// Every control that is actually on screen: enabled, and not inside a
/// closed overlay (the example's dialog starts closed).
fn interactive_of(tree: &ElementTree, ty: &str) -> Vec<nui_runtime::ElementId> {
    return elements_of(tree, ty)
        .into_iter()
        .filter(|id| {
            return is_enabled(tree, *id)
                && !nui_runtime::widget::is_inside_a_closed_overlay(tree, *id);
        })
        .collect();
}

/// The y-band in which the widget tracker reports `hovered = true` for
/// `id`, probing the control's horizontal centre.
fn hovered_band(
    engine: &mut Engine,
    tree: &mut ElementTree,
    states: &mut WidgetStates,
    id: nui_runtime::ElementId,
    y: f32,
    height: f32,
) -> Vec<(u32, u32)> {
    let probe_x = f_of(tree, id, "x") + f_of(tree, id, "width") / 2.0;
    let mut band: Vec<(u32, u32)> = Vec::new();
    let mut start: Option<u32> = None;
    let from = (y - 4.0).max(0.0) as u32;
    let to = (y + height + 4.0) as u32;
    for probe_y in from..to {
        states.reset();
        states.update(
            engine,
            tree,
            nui_runtime::PointerInput {
                position: Point::new(probe_x, probe_y as f32),
                inside: true,
                down: false,
            },
        );
        let hovered = tree.arena[id]
            .get("hovered")
            .and_then(|value| return value.as_bool().ok())
            .unwrap_or(false);
        match (hovered, start) {
            (true, None) => start = Some(probe_y),
            (false, Some(s)) => {
                band.push((s, probe_y - 1));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        band.push((s, to - 1));
    }
    return band;
}

/// A control's hover band must be the whole box, top edge included.
#[test]
fn a_control_hovers_over_its_whole_declared_box() {
    let mut tree = example_tree();
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();

    for ty in ["Button", "CheckBox", "Switch"] {
        let ids = interactive_of(&tree, ty);
        assert!(!ids.is_empty(), "the example ships a visible {ty}");
        for id in ids {
            let label = label_of(&tree, id);
            let y = f_of(&tree, id, "y");
            let height = f_of(&tree, id, "height");
            let band = hovered_band(&mut engine, &mut tree, &mut states, id, y, height);
            // `Rect::contains` is bottom-right-open, so the top edge is
            // included and the row past the bottom is not.
            assert_eq!(
                band,
                vec![(y as u32, (y + height) as u32 - 1)],
                "{ty} `{label}` at y={y}..{} must hover exactly over its box",
                y + height
            );
        }
    }
}

/// `hit_test` and the widget tracker must agree: the element under the
/// pointer is the element that lights up.
#[test]
fn the_tracker_agrees_with_the_hit_test() {
    let mut tree = example_tree();
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();

    for ty in ["Button", "CheckBox", "Switch"] {
        for id in interactive_of(&tree, ty) {
            let label = label_of(&tree, id);
            let x = f_of(&tree, id, "x");
            let y = f_of(&tree, id, "y");
            let width = f_of(&tree, id, "width");
            let height = f_of(&tree, id, "height");
            for probe in [
                (x + 1.0, y + 1.0),
                (x + width / 2.0, y + height / 2.0),
                (x + width - 1.0, y + height - 1.0),
            ] {
                states.reset();
                states.update(
                    &mut engine,
                    &mut tree,
                    nui_runtime::PointerInput {
                        position: Point::new(probe.0, probe.1),
                        inside: true,
                        down: false,
                    },
                );
                let hovered_here = tree.arena[id]
                    .get("hovered")
                    .and_then(|value| return value.as_bool().ok())
                    .unwrap_or(false);
                let hit = nui::hit_test(&tree, Point::new(probe.0, probe.1))
                    .map(|target| return target.element);
                assert_eq!(
                    hovered_here,
                    hit == Some(id),
                    "{ty} `{label}` at {probe:?}: hovered={hovered_here}, hit_test={hit:?}"
                );
            }
            // One dp past each edge: the control must not report hover.
            // This is the assertion the double-counting bug failed — the
            // control lit up only *below* its real box.
            for probe in [
                Point::new(x + width / 2.0, y - 1.0),
                Point::new(x + width / 2.0, y + height),
                Point::new(x - 1.0, y + height / 2.0),
                Point::new(x + width, y + height / 2.0),
            ] {
                states.reset();
                states.update(
                    &mut engine,
                    &mut tree,
                    nui_runtime::PointerInput {
                        position: probe,
                        inside: true,
                        down: false,
                    },
                );
                assert_eq!(
                    tree.arena[id].get("hovered"),
                    Some(&Value::Bool(false)),
                    "{ty} `{label}` must not hover at {probe:?} (outside its box)"
                );
            }
        }
    }
}

/// The example's dialog starts closed, so its buttons are not on screen.
/// Opening it must make them hit-testable at their own boxes.
#[test]
fn opening_the_dialog_makes_its_buttons_interactive() {
    let mut tree = example_tree();
    let dialog = elements_of(&tree, "Dialog")
        .into_iter()
        .next()
        .expect("the example ships a dialog");
    // Closed: the inner buttons report no interaction.
    let inner: Vec<nui_runtime::ElementId> = elements_of(&tree, "Button")
        .into_iter()
        .filter(|id| return nui_runtime::widget::is_inside_a_closed_overlay(&tree, *id))
        .collect();
    assert_eq!(inner.len(), 2, "the dialog holds two buttons");

    // Open it and re-lay-out (the overlay leaves the flow when shown).
    tree.arena[dialog].set("open", Value::Bool(true));
    for _ in 0..2 {
        nui_layout::layout(&mut tree, Size::new(480.0, 640.0));
    }
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    for id in inner {
        let label = label_of(&tree, id);
        let x = f_of(&tree, id, "x");
        let y = f_of(&tree, id, "y");
        let width = f_of(&tree, id, "width");
        let height = f_of(&tree, id, "height");
        let centre = Point::new(x + width / 2.0, y + height / 2.0);
        assert_eq!(
            nui::hit_test(&tree, centre).map(|target| return target.element),
            Some(id),
            "opened dialog button `{label}` owns its centre"
        );
        states.reset();
        states.update(
            &mut engine,
            &mut tree,
            nui_runtime::PointerInput {
                position: centre,
                inside: true,
                down: false,
            },
        );
        assert_eq!(
            tree.arena[id].get("hovered"),
            Some(&Value::Bool(true)),
            "opened dialog button `{label}` hovers at its centre"
        );
    }
}

/// `element_bounds` — the path a release uses to decide whether it counts
/// as a click — must match the layout box too.
#[test]
fn element_bounds_matches_the_layout_box() {
    let tree = example_tree();
    for ty in ["Button", "CheckBox", "Switch", "Slider", "Dialog"] {
        for id in elements_of(&tree, ty) {
            let label = label_of(&tree, id);
            let bounds = nui::element_bounds(&tree, id)
                .unwrap_or_else(|| panic!("{ty} `{label}` must have bounds"));
            assert_eq!(bounds.origin.x, f_of(&tree, id, "x"), "{ty} `{label}` x");
            assert_eq!(bounds.origin.y, f_of(&tree, id, "y"), "{ty} `{label}` y");
            assert_eq!(
                bounds.size.width,
                f_of(&tree, id, "width"),
                "{ty} `{label}` w"
            );
            assert_eq!(
                bounds.size.height,
                f_of(&tree, id, "height"),
                "{ty} `{label}` h"
            );
        }
    }
}

/// A disabled control stays inert and does not capture the pointer for its
/// whole container row.
#[test]
fn a_disabled_control_does_not_hover() {
    let mut tree = example_tree();
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    let disabled = elements_of(&tree, "Button")
        .into_iter()
        .find(|id| return label_of(&tree, *id) == "Disabled")
        .expect("the example ships a Disabled button");
    let x = f_of(&tree, disabled, "x");
    let y = f_of(&tree, disabled, "y");
    let width = f_of(&tree, disabled, "width");
    let height = f_of(&tree, disabled, "height");
    states.reset();
    states.update(
        &mut engine,
        &mut tree,
        nui_runtime::PointerInput {
            position: Point::new(x + width / 2.0, y + height / 2.0),
            inside: true,
            down: false,
        },
    );
    assert_eq!(
        tree.arena[disabled].get("hovered"),
        Some(&Value::Bool(false)),
        "a disabled control never reports hover"
    );
}

/// Clicking a control at its visual centre must capture *that* control,
/// not an ancestor container.
#[test]
fn a_press_captures_the_control_not_its_container() {
    let mut tree = example_tree();
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    for ty in ["Button", "CheckBox", "Switch"] {
        for id in interactive_of(&tree, ty) {
            let label = label_of(&tree, id);
            let cx = f_of(&tree, id, "x") + f_of(&tree, id, "width") / 2.0;
            let cy = f_of(&tree, id, "y") + f_of(&tree, id, "height") / 2.0;
            let hit = nui::hit_test(&tree, Point::new(cx, cy)).map(|t| return t.element);
            assert_eq!(
                hit,
                Some(id),
                "{ty} `{label}` owns the pointer at its centre {cx},{cy}"
            );
            // ...and the tracker mirrors that into `armed` while held.
            states.reset();
            states.capture(hit);
            states.update(
                &mut engine,
                &mut tree,
                nui_runtime::PointerInput {
                    position: Point::new(cx, cy),
                    inside: true,
                    down: true,
                },
            );
            assert_eq!(
                tree.arena[id].get("armed"),
                Some(&Value::Bool(true)),
                "{ty} `{label}` is armed while the press holds inside it"
            );
        }
    }
}
