//! Unit tests for the widget interaction layer.
//!
//! The tree fixtures are built by hand rather than laid out, because the
//! `x`/`y`/`width`/`height` a widget reads are **exactly** what layout
//! writes: absolute window coordinates. Hand-setting them keeps these
//! tests independent of `nui-layout` while still exercising the one
//! convention the two crates share — see
//! [`super::bounds::absolute_bounds`] for why that convention matters.

use nui_core::Value;

use crate::binding::Engine;
use crate::element::{Element, ElementId, ElementTree};
use crate::notify::ChangeSource;
use crate::registry::{BehaviorContext, ElementBehavior};

use super::PointerInput;
use super::WidgetKind;
use super::WidgetStates;
use super::activate::{activate, is_activatable, toggle_property};
use super::bounds::absolute_bounds;
use super::drag::drag_value;
use super::is_enabled;
use super::is_focusable_type;
use super::is_visible;
use super::is_widget_type;
use super::overlay::{
    active_modal, dismisses_on_backdrop, is_behind_modal, is_inside_a_closed_overlay, is_open,
    is_overlay,
};

/// A pointer at `(x, y)` inside the window, primary button `down` or not.
fn input(x: f32, y: f32, down: bool) -> PointerInput {
    return PointerInput {
        position: nui_core::Point::new(x, y),
        inside: true,
        down,
    };
}

/// An element of `ty` with layout's four geometry properties set.
///
/// `x`/`y` are the element's absolute window position, which is what
/// layout writes (`nui-layout`'s `write_back` accumulates as it descends).
fn widget(ty: &str, x: f32, y: f32, width: f32, height: f32) -> Element {
    let mut element = Element::new(ty, None);
    element.set("x", Value::Float(f64::from(x)));
    element.set("y", Value::Float(f64::from(y)));
    element.set("width", Value::Float(f64::from(width)));
    element.set("height", Value::Float(f64::from(height)));
    return element;
}

/// A `Slider` spanning `0..100`.
fn slider(x: f32, y: f32, width: f32, height: f32) -> Element {
    let mut element = widget("Slider", x, y, width, height);
    element.set("min", Value::Float(0.0));
    element.set("max", Value::Float(100.0));
    element.set("value", Value::Float(0.0));
    return element;
}

/// A tree whose only root is `element`.
fn tree_of(element: Element) -> (ElementTree, ElementId) {
    let mut tree = ElementTree::new();
    let id = tree.insert(element);
    tree.push_root(id);
    return (tree, id);
}

/// One of the tracker's Boolean state properties, or `None` when layout
/// never ran (the slot is absent until the first update writes it).
fn bool_of(tree: &ElementTree, id: ElementId, name: &str) -> Option<bool> {
    return tree.arena[id]
        .get(name)
        .and_then(|value| return value.as_bool().ok());
}

/// A numeric property as `f32` (Int/Float/Length).
fn read_number(tree: &ElementTree, id: ElementId, name: &str) -> Option<f32> {
    return tree.arena[id].get(name).and_then(|value| {
        return match value {
            Value::Int(inner) => Some(*inner as f32),
            Value::Float(inner) => Some(*inner as f32),
            Value::Length(nui_core::Length::Dp(inner)) => Some(*inner),
            _ => None,
        };
    });
}

/// Records the signals that reach one element.
///
/// Attached directly to `Element::behavior`, which is the same hook a
/// host-registered component would install — so this observes the real
/// delivery path rather than a stand-in for it.
#[derive(Debug)]
struct SignalLog(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

impl ElementBehavior for SignalLog {
    fn on_signal(&mut self, _context: &mut BehaviorContext<'_>, _element: ElementId, signal: &str) {
        self.0.borrow_mut().push(signal.to_string());
    }
}

#[test]
fn widget_kind_maps_types() {
    assert_eq!(WidgetKind::of("Button"), Some(WidgetKind::Momentary));
    assert_eq!(WidgetKind::of("Dialog"), Some(WidgetKind::Momentary));
    for ty in ["CheckBox", "Switch", "RadioButton"] {
        assert_eq!(WidgetKind::of(ty), Some(WidgetKind::Toggle), "{ty}");
    }
    assert_eq!(WidgetKind::of("Slider"), Some(WidgetKind::Drag));
    assert_eq!(WidgetKind::of("Column"), None);
    for ty in [
        "Button",
        "CheckBox",
        "Switch",
        "Slider",
        "RadioButton",
        "Dialog",
    ] {
        assert!(is_widget_type(ty), "{ty} is a widget type");
    }
    assert!(!is_widget_type("Row"));
    assert!(!is_widget_type("Text"));
}

#[test]
fn the_keyboard_focus_order_is_not_the_widget_list() {
    // Every control a user can *operate* is a tab stop...
    for ty in [
        "Button",
        "CheckBox",
        "Switch",
        "Slider",
        "RadioButton",
        "SpinBox",
    ] {
        assert!(is_focusable_type(ty), "{ty} takes focus");
    }
    // ... but a `Dialog` is interactive without being one, and a container
    // or a text label is neither.
    assert!(is_widget_type("Dialog") && !is_focusable_type("Dialog"));
    for ty in ["Dialog", "Panel", "Card", "Separator", "Row", "Text"] {
        assert!(!is_focusable_type(ty), "{ty} is not a tab stop");
    }
    // A text field is a tab stop without being a widget type (it has no
    // pointer behaviour of its own); `instantiate` marks it separately.
    assert!(!is_widget_type("TextInput"));
}

#[test]
fn enabled_defaults_to_true() {
    assert!(is_enabled(&Element::new("Button", None)));
    let mut off = Element::new("Button", None);
    off.set("enabled", Value::Bool(false));
    assert!(!is_enabled(&off));
}

#[test]
fn visible_defaults_to_true_and_reads_either_way() {
    assert!(is_visible(&Element::new("Rectangle", None)));
    let mut hidden = Element::new("Rectangle", None);
    hidden.set("visible", Value::Bool(false));
    assert!(!is_visible(&hidden));
    hidden.set("visible", Value::Bool(true));
    assert!(is_visible(&hidden));
    // A non-bool `visible` (a binding that resolved to something else, or
    // a typo'd literal) reads as shown: the flag has to be *set* to false
    // to hide, never merely present.
    hidden.set("visible", Value::Int(0));
    assert!(is_visible(&hidden));
    // It is not the overlay switch: `open` says nothing about `visible`.
    let dialog = Element::new("Dialog", None);
    assert!(is_visible(&dialog) && is_open(&dialog));
}

#[test]
fn hover_follows_the_pointer_in_and_out() {
    let (mut tree, id) = tree_of(widget("Button", 10.0, 10.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, false);
    assert_eq!(bool_of(&tree, id, "hovered"), Some(true));
    assert_eq!(bool_of(&tree, id, "pressed"), Some(false));
    assert_eq!(bool_of(&tree, id, "armed"), Some(false));
    state_update(&mut states, &mut engine, &mut tree, 200.0, 20.0, false);
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
    // The box edges are half-open at the far side: 110 is outside.
    state_update(&mut states, &mut engine, &mut tree, 110.0, 20.0, false);
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
    state_update(&mut states, &mut engine, &mut tree, 109.0, 20.0, false);
    assert_eq!(bool_of(&tree, id, "hovered"), Some(true));
}

#[test]
fn press_inside_sets_pressed_and_armed() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, true);
    assert_eq!(bool_of(&tree, id, "hovered"), Some(true));
    assert_eq!(bool_of(&tree, id, "pressed"), Some(true));
    assert_eq!(bool_of(&tree, id, "armed"), Some(true));
}

#[test]
fn dragging_out_of_a_captured_widget_keeps_press_but_drops_arm() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, true);
    assert_eq!(bool_of(&tree, id, "armed"), Some(true));
    // Still down, but the pointer left the box: the gesture continues
    // (still pressed) and releasing here would *not* activate (not armed).
    state_update(&mut states, &mut engine, &mut tree, 50.0, 200.0, true);
    assert_eq!(bool_of(&tree, id, "pressed"), Some(true));
    assert_eq!(bool_of(&tree, id, "armed"), Some(false));
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
    // Coming back re-arms without a new press.
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, true);
    assert_eq!(bool_of(&tree, id, "armed"), Some(true));
}

#[test]
fn capture_survives_the_pointer_leaving_the_window() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    let outside = PointerInput {
        position: nui_core::Point::new(-40.0, -40.0),
        inside: false,
        down: true,
    };
    states.update(&mut engine, &mut tree, outside);
    // `inside == false` clears every geometric state but the gesture.
    assert_eq!(bool_of(&tree, id, "pressed"), Some(true));
    assert_eq!(bool_of(&tree, id, "armed"), Some(false));
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
}

#[test]
fn release_capture_clears_pressed() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, true);
    assert_eq!(bool_of(&tree, id, "pressed"), Some(true));
    assert_eq!(states.captured(), Some(id));
    states.release_capture();
    assert_eq!(states.captured(), None);
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, false);
    assert_eq!(bool_of(&tree, id, "pressed"), Some(false));
    assert_eq!(bool_of(&tree, id, "armed"), Some(false));
    // Hover survives the release: the pointer is still over the box.
    assert_eq!(bool_of(&tree, id, "hovered"), Some(true));
}

#[test]
fn stale_capture_is_dropped() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    // A `For` rebuild or a hot reload retires the handles.
    tree.remove_subtree(id);
    states.update(&mut engine, &mut tree, input(50.0, 20.0, true));
    assert_eq!(states.captured(), None);
}

#[test]
fn disabled_widgets_never_hover_or_arm() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    tree.arena[id].set("enabled", Value::Bool(false));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, true);
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
    assert_eq!(bool_of(&tree, id, "pressed"), Some(false));
    assert_eq!(bool_of(&tree, id, "armed"), Some(false));
    assert_eq!(bool_of(&tree, id, "disabled"), Some(true));
}

#[test]
fn focus_is_mirrored_into_the_property() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    state_update(&mut states, &mut engine, &mut tree, 500.0, 500.0, false);
    assert_eq!(bool_of(&tree, id, "focused"), Some(false));
    engine.focus(id);
    state_update(&mut states, &mut engine, &mut tree, 500.0, 500.0, false);
    assert_eq!(bool_of(&tree, id, "focused"), Some(true));
    // Focus is orthogonal to hover: the pointer is nowhere near.
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
    engine.blur();
    state_update(&mut states, &mut engine, &mut tree, 500.0, 500.0, false);
    assert_eq!(bool_of(&tree, id, "focused"), Some(false));
}

#[test]
fn update_reports_only_visual_changes() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    // The first pass seeds every slot, so it is a change by definition.
    assert_eq!(
        states.update(&mut engine, &mut tree, input(50.0, 20.0, false)),
        vec![id]
    );
    // An idle pointer must not dirty the tree every frame.
    assert!(
        states
            .update(&mut engine, &mut tree, input(50.0, 20.0, false))
            .is_empty()
    );
    // Hover out *is* visible.
    assert_eq!(
        states.update(&mut engine, &mut tree, input(200.0, 20.0, false)),
        vec![id]
    );
    // `pressed`/`armed` are gesture bookkeeping, not visuals: a capture
    // that does not move the pointer changes neither `hovered`, `armed`,
    // `focused` nor `disabled`.
    states.capture(Some(id));
    assert!(
        states
            .update(&mut engine, &mut tree, input(200.0, 20.0, true))
            .is_empty()
    );
    // Arming does move `armed`, so it is reported.
    assert_eq!(
        states.update(&mut engine, &mut tree, input(50.0, 20.0, true)),
        vec![id]
    );
}

#[test]
fn update_writes_are_host_sourced() {
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.update(&mut engine, &mut tree, input(50.0, 20.0, false));
    let changes = engine.take_changes();
    assert!(!changes.is_empty(), "the first pass seeds the state slots");
    for change in &changes {
        // Host-sourced writes do not clobber a user's `<-` binding.
        assert_eq!(change.source, ChangeSource::Host);
        assert_eq!(change.element, id);
    }
}

#[test]
fn update_ignores_non_widget_elements() {
    let (mut tree, id) = tree_of(widget("Column", 0.0, 0.0, 400.0, 400.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    assert!(
        states
            .update(&mut engine, &mut tree, input(50.0, 20.0, false))
            .is_empty()
    );
    // Not even the slots are created: a `Column` has no `hovered`.
    assert_eq!(tree.arena[id].get("hovered"), None);
}

#[test]
fn nesting_depth_does_not_shift_the_hitbox() {
    let mut tree = ElementTree::new();
    let page = widget("Column", 0.0, 0.0, 400.0, 400.0);
    let page_id = tree.insert(page);
    tree.push_root(page_id);
    let toggles = widget("Column", 28.0, 120.0, 344.0, 56.0);
    let toggles_id = tree.insert(toggles);
    tree.append_child(page_id, toggles_id);
    let check = widget("CheckBox", 28.0, 120.0, 200.0, 22.0);
    let check_id = tree.insert(check);
    tree.append_child(toggles_id, check_id);

    // A container and a child inside it legitimately share a coordinate:
    // layout writes the child's *window* position, not an offset. Summing
    // the chain a second time resolved this CheckBox to y=240.
    let bounds = absolute_bounds(&tree, check_id).unwrap();
    assert_eq!(bounds.origin.x, 28.0);
    assert_eq!(bounds.origin.y, 120.0);

    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    // The box is 120..142.
    state_update(&mut states, &mut engine, &mut tree, 100.0, 130.0, false);
    assert_eq!(bool_of(&tree, check_id, "hovered"), Some(true));
    state_update(&mut states, &mut engine, &mut tree, 100.0, 160.0, false);
    assert_eq!(bool_of(&tree, check_id, "hovered"), Some(false));
}

#[test]
fn a_layout_written_origin_is_used_as_is() {
    let mut tree = ElementTree::new();
    let parent_id = tree.insert(widget("Column", 10.0, 10.0, 200.0, 200.0));
    tree.push_root(parent_id);
    let child_id = tree.insert(widget("Button", 15.0, 15.0, 50.0, 20.0));
    tree.append_child(parent_id, child_id);

    let bounds = absolute_bounds(&tree, child_id).unwrap();
    assert_eq!(bounds.origin.x, 15.0);
    assert_eq!(bounds.origin.y, 15.0);
    assert_eq!(bounds.size.width, 50.0);
    assert_eq!(bounds.size.height, 20.0);

    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.update(&mut engine, &mut tree, input(20.0, 20.0, false));
    assert_eq!(bool_of(&tree, child_id, "hovered"), Some(true));
    // 10dp further down is outside a box that ends at 35.
    states.update(&mut engine, &mut tree, input(20.0, 60.0, false));
    assert_eq!(bool_of(&tree, child_id, "hovered"), Some(false));
}

#[test]
fn a_scroll_ancestor_translates_the_hitbox() {
    let mut tree = ElementTree::new();
    let mut scroll = widget("Scroll", 0.0, 0.0, 300.0, 200.0);
    scroll.set("scroll_y", Value::Float(60.0));
    let scroll_id = tree.insert(scroll);
    tree.push_root(scroll_id);
    let child_id = tree.insert(widget("Button", 0.0, 100.0, 200.0, 40.0));
    tree.append_child(scroll_id, child_id);

    // `scroll_y` is a viewport translation applied at draw and hit-test
    // time, not something layout bakes into the child's `y` — so it is
    // the one adjustment the bounds calculation still owes.
    let bounds = absolute_bounds(&tree, child_id).unwrap();
    assert_eq!(bounds.origin.y, 40.0);

    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.update(&mut engine, &mut tree, input(20.0, 45.0, false));
    assert_eq!(bool_of(&tree, child_id, "hovered"), Some(true));
    states.update(&mut engine, &mut tree, input(20.0, 130.0, false));
    assert_eq!(bool_of(&tree, child_id, "hovered"), Some(false));
}

#[test]
fn activate_toggles_and_emits() {
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (mut tree, id) = tree_of(widget("CheckBox", 0.0, 0.0, 200.0, 22.0));
    tree.arena[id].behavior = Some(Box::new(SignalLog(log.clone())));
    let mut engine = Engine::new();

    assert!(activate(&mut engine, &mut tree, id));
    assert_eq!(bool_of(&tree, id, "checked"), Some(true));
    assert_eq!(
        *log.borrow(),
        vec!["changed".to_string(), "click".to_string()],
        "a toggle signals `changed` before `click`"
    );

    assert!(activate(&mut engine, &mut tree, id));
    assert_eq!(bool_of(&tree, id, "checked"), Some(false));
    assert_eq!(log.borrow().len(), 4);

    assert_eq!(toggle_property("CheckBox"), Some("checked"));
    assert_eq!(toggle_property("Switch"), Some("checked"));
    assert_eq!(toggle_property("RadioButton"), Some("selected"));
    assert_eq!(toggle_property("Button"), None);
}

#[test]
fn momentaries_do_not_toggle() {
    let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    tree.arena[id].behavior = Some(Box::new(SignalLog(log.clone())));
    let mut engine = Engine::new();
    // A Button emits `click` and writes nothing: there is no Boolean to
    // flip, so `activate` reports no write.
    assert!(!activate(&mut engine, &mut tree, id));
    assert_eq!(*log.borrow(), vec!["click".to_string()]);
    assert_eq!(tree.arena[id].get("checked"), None);
    assert_eq!(tree.arena[id].get("selected"), None);
}

#[test]
fn disabled_widgets_are_not_activatable() {
    let (mut tree, id) = tree_of(widget("CheckBox", 0.0, 0.0, 200.0, 22.0));
    assert!(is_activatable(&tree, id, "CheckBox"));
    tree.arena[id].set("enabled", Value::Bool(false));
    assert!(!is_activatable(&tree, id, "CheckBox"));
}

#[test]
fn non_widgets_are_not_activatable() {
    let (mut tree, id) = tree_of(widget("Column", 0.0, 0.0, 400.0, 400.0));
    assert!(!is_activatable(&tree, id, "Column"));
    let mut engine = Engine::new();
    assert!(!activate(&mut engine, &mut tree, id));
}

#[test]
fn selecting_one_radio_clears_its_group() {
    let mut tree = ElementTree::new();
    let mut ids = Vec::new();
    for (index, selected) in [(0, true), (1, false)] {
        let mut element = widget("RadioButton", 0.0, index as f32 * 30.0, 200.0, 22.0);
        element.set("group", Value::Enum("plan".to_string()));
        element.set("selected", Value::Bool(selected));
        let id = tree.insert(element);
        tree.push_root(id);
        ids.push(id);
    }
    let mut engine = Engine::new();
    assert!(activate(&mut engine, &mut tree, ids[1]));
    assert_eq!(bool_of(&tree, ids[1], "selected"), Some(true));
    assert_eq!(bool_of(&tree, ids[0], "selected"), Some(false));
}

#[test]
fn re_clicking_a_selected_radio_keeps_it_selected() {
    let (mut tree, id) = tree_of(widget("RadioButton", 0.0, 0.0, 200.0, 22.0));
    tree.arena[id].set("group", Value::Enum("plan".to_string()));
    tree.arena[id].set("selected", Value::Bool(true));
    let mut engine = Engine::new();
    // A radio *selects*; there is no "none of the above", so clicking the
    // chosen one again is a no-op rather than a clear.
    assert!(!activate(&mut engine, &mut tree, id));
    assert_eq!(bool_of(&tree, id, "selected"), Some(true));
}

#[test]
fn radios_in_another_group_are_untouched() {
    let mut tree = ElementTree::new();
    let mut ids = Vec::new();
    for (group, selected) in [("plan", true), ("size", true), ("plan", false)] {
        let mut element = widget("RadioButton", 0.0, 0.0, 200.0, 22.0);
        element.set("group", Value::Enum(group.to_string()));
        element.set("selected", Value::Bool(selected));
        let id = tree.insert(element);
        tree.push_root(id);
        ids.push(id);
    }
    let mut engine = Engine::new();
    activate(&mut engine, &mut tree, ids[2]);
    assert_eq!(bool_of(&tree, ids[2], "selected"), Some(true));
    assert_eq!(bool_of(&tree, ids[0], "selected"), Some(false));
    assert_eq!(
        bool_of(&tree, ids[1], "selected"),
        Some(true),
        "a different group is a different radio set"
    );
}

#[test]
fn radios_without_a_group_do_not_interfere() {
    let mut tree = ElementTree::new();
    let mut loose = widget("RadioButton", 0.0, 0.0, 200.0, 22.0);
    loose.set("selected", Value::Bool(true));
    let loose_id = tree.insert(loose);
    tree.push_root(loose_id);
    let mut grouped = widget("RadioButton", 0.0, 30.0, 200.0, 22.0);
    grouped.set("group", Value::Enum("plan".to_string()));
    let grouped_id = tree.insert(grouped);
    tree.push_root(grouped_id);

    let mut engine = Engine::new();
    activate(&mut engine, &mut tree, grouped_id);
    // A missing `group` matches nothing, so the unnamed radio is neither
    // cleared nor treated as a member of anything.
    assert_eq!(bool_of(&tree, loose_id, "selected"), Some(true));
}

#[test]
fn checkbox_toggle_is_unaffected_by_group_logic() {
    let mut tree = ElementTree::new();
    let mut ids = Vec::new();
    for (index, checked) in [(0, true), (1, false)] {
        let mut element = widget("CheckBox", 0.0, index as f32 * 30.0, 200.0, 22.0);
        element.set("group", Value::Enum("plan".to_string()));
        element.set("checked", Value::Bool(checked));
        let id = tree.insert(element);
        tree.push_root(id);
        ids.push(id);
    }
    let mut engine = Engine::new();
    activate(&mut engine, &mut tree, ids[1]);
    assert_eq!(bool_of(&tree, ids[1], "checked"), Some(true));
    // Group clearing is a RadioButton rule; two CheckBoxes sharing a
    // `group` name stay independent.
    assert_eq!(bool_of(&tree, ids[0], "checked"), Some(true));
}

#[test]
fn dragging_a_slider_maps_the_pointer_onto_its_range() {
    let (mut tree, id) = tree_of(slider(0.0, 0.0, 200.0, 20.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    // 20dp thumb on a 200-wide track: its centre sweeps [10, 190], which
    // is exactly what makes both ends reachable.
    state_update(&mut states, &mut engine, &mut tree, 10.0, 10.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(0.0));
    state_update(&mut states, &mut engine, &mut tree, 190.0, 10.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(100.0));
    state_update(&mut states, &mut engine, &mut tree, 100.0, 10.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(50.0));
}

#[test]
fn a_vertical_slider_drags_along_y() {
    let mut element = widget("Slider", 0.0, 0.0, 20.0, 200.0);
    element.set("orientation", Value::Enum("vertical".to_string()));
    element.set("min", Value::Float(0.0));
    element.set("max", Value::Float(100.0));
    element.set("value", Value::Float(0.0));
    let (mut tree, id) = tree_of(element);
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    // The thumb is square and rides the *cross* axis, so a vertical
    // slider's thumb is as tall as the element is wide (20) and travel is
    // [10, 190] along y. Reading `height` here would make the 200dp track
    // double as the thumb size and collapse travel to zero.
    state_update(&mut states, &mut engine, &mut tree, 10.0, 190.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(100.0));
    state_update(&mut states, &mut engine, &mut tree, 10.0, 10.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(0.0));
    state_update(&mut states, &mut engine, &mut tree, 10.0, 100.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(50.0));
}

#[test]
fn a_slider_keeps_tracking_outside_its_box() {
    let (mut tree, id) = tree_of(slider(0.0, 0.0, 200.0, 20.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    // The whole reason for the capture: the drag keeps updating after the
    // pointer leaves the box. The value clamps rather than running away.
    state_update(&mut states, &mut engine, &mut tree, 400.0, 200.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(100.0));
    assert_eq!(bool_of(&tree, id, "pressed"), Some(true));
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
    // Far to the left clamps at `min`.
    state_update(&mut states, &mut engine, &mut tree, -400.0, 200.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(0.0));
}

#[test]
fn a_slider_does_not_drag_without_a_capture() {
    let (mut tree, id) = tree_of(slider(0.0, 0.0, 200.0, 20.0));
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    // Pointer down over the slider, but the gesture belongs to nothing
    // (the press landed elsewhere), so the value must not move.
    state_update(&mut states, &mut engine, &mut tree, 190.0, 10.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(0.0));
    assert_eq!(bool_of(&tree, id, "pressed"), Some(false));
    assert_eq!(bool_of(&tree, id, "hovered"), Some(true));
}

#[test]
fn a_slider_quantises_by_step() {
    let mut element = slider(0.0, 0.0, 200.0, 20.0);
    element.set("step", Value::Float(25.0));
    let (mut tree, id) = tree_of(element);
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    // x=150 → (150 - 10) / 180 = 0.778 → 77.8 → nearest 25 is 75.
    state_update(&mut states, &mut engine, &mut tree, 150.0, 10.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(75.0));
    // The direct call reports whether it moved, which is how the caller
    // decides to redraw. x=10 is the far end, so this does move.
    assert!(drag_value(
        &mut engine,
        &mut tree,
        id,
        nui_core::Point::new(10.0, 10.0)
    ));
    assert_eq!(read_number(&tree, id, "value"), Some(0.0));
    assert!(!drag_value(
        &mut engine,
        &mut tree,
        id,
        nui_core::Point::new(10.0, 10.0)
    ));
}

#[test]
fn a_zero_span_slider_parks_at_min() {
    let mut element = widget("Slider", 0.0, 0.0, 200.0, 20.0);
    element.set("min", Value::Float(5.0));
    element.set("max", Value::Float(5.0));
    let (mut tree, id) = tree_of(element);
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    // A degenerate range would divide by zero; park at `min` instead.
    state_update(&mut states, &mut engine, &mut tree, 150.0, 10.0, true);
    assert_eq!(read_number(&tree, id, "value"), Some(5.0));
}

#[test]
fn an_integer_slider_stays_integer() {
    let mut element = widget("Slider", 0.0, 0.0, 200.0, 20.0);
    element.set("min", Value::Int(0));
    element.set("max", Value::Int(100));
    element.set("value", Value::Int(0));
    let (mut tree, id) = tree_of(element);
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    states.capture(Some(id));
    state_update(&mut states, &mut engine, &mut tree, 100.0, 10.0, true);
    // Byte-exact: an `Int` slot must not decay into a `Float` halfway
    // through a drag, or every read of `value` changes type.
    assert_eq!(tree.arena[id].get("value"), Some(&Value::Int(50)));
}

#[test]
fn dialog_is_an_overlay_by_type_and_others_opt_in() {
    assert!(is_overlay(&Element::new("Dialog", None)));
    assert!(!is_overlay(&Element::new("Column", None)));
    let mut popup = Element::new("Column", None);
    popup.set("overlay", Value::Bool(true));
    assert!(is_overlay(&popup));
}

#[test]
fn an_overlay_without_open_is_always_shown() {
    // Toast / Tooltip have no `open`: they are shown whenever present.
    assert!(is_open(&Element::new("Dialog", None)));
    let mut closed = Element::new("Dialog", None);
    closed.set("open", Value::Bool(false));
    assert!(!is_open(&closed));
    let mut opened = Element::new("Dialog", None);
    opened.set("open", Value::Bool(true));
    assert!(is_open(&opened));
}

#[test]
fn an_overlay_without_open_is_always_live() {
    let mut tree = ElementTree::new();
    let dialog_id = tree.insert(Element::new("Dialog", None));
    tree.push_root(dialog_id);
    let button_id = tree.insert(widget("Button", 0.0, 0.0, 100.0, 40.0));
    tree.append_child(dialog_id, button_id);
    assert!(!is_inside_a_closed_overlay(&tree, button_id));
}

#[test]
fn a_closed_overlay_hides_its_subtree_from_interaction() {
    let mut tree = ElementTree::new();
    let mut dialog = Element::new("Dialog", None);
    dialog.set("open", Value::Bool(false));
    let dialog_id = tree.insert(dialog);
    tree.push_root(dialog_id);
    let button_id = tree.insert(widget("Button", 0.0, 0.0, 100.0, 40.0));
    tree.append_child(dialog_id, button_id);
    assert!(is_inside_a_closed_overlay(&tree, button_id));

    let mut engine = Engine::new();
    let mut states = WidgetStates::new();
    // The scene walk drops the whole subtree, so interaction must not
    // report `hovered` for something the user cannot see.
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, false);
    assert_eq!(bool_of(&tree, button_id, "hovered"), Some(false));
    // Inert for a different reason than `enabled = false`, and the two
    // stay distinguishable so a binding can tell them apart.
    assert_eq!(bool_of(&tree, button_id, "disabled"), Some(false));

    tree.arena[dialog_id].set("open", Value::Bool(true));
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, false);
    assert_eq!(bool_of(&tree, button_id, "hovered"), Some(true));
}

#[test]
fn active_modal_finds_the_topmost_open_modal_dialog() {
    let mut tree = ElementTree::new();
    let mut closed = Element::new("Dialog", None);
    closed.set("open", Value::Bool(false));
    let closed_id = tree.insert(closed);
    tree.push_root(closed_id);
    // A non-modal dialog is a popover, not a modal: it does not take the
    // input away from the content behind it.
    let mut optional = Element::new("Dialog", None);
    optional.set("modal", Value::Bool(false));
    let optional_id = tree.insert(optional);
    tree.push_root(optional_id);
    assert_eq!(active_modal(&tree), None);

    let mut shown = Element::new("Dialog", None);
    shown.set("open", Value::Bool(true));
    let shown_id = tree.insert(shown);
    tree.push_root(shown_id);
    assert_eq!(active_modal(&tree), Some(shown_id));

    // Document order decides: the later dialog paints later, so it is the
    // one on top and the one that owns the input.
    let mut top = Element::new("Dialog", None);
    top.set("open", Value::Bool(true));
    let top_id = tree.insert(top);
    tree.push_root(top_id);
    assert_eq!(active_modal(&tree), Some(top_id));
}

#[test]
fn is_behind_modal_separates_focus_inside_from_behind() {
    let mut tree = ElementTree::new();
    let dialog_id = tree.insert(Element::new("Dialog", None));
    tree.push_root(dialog_id);
    let inside_id = tree.insert(widget("Button", 0.0, 0.0, 100.0, 40.0));
    tree.append_child(dialog_id, inside_id);
    let behind_id = tree.insert(widget("Button", 0.0, 100.0, 100.0, 40.0));
    tree.push_root(behind_id);

    assert!(!is_behind_modal(&tree, dialog_id, dialog_id));
    assert!(!is_behind_modal(&tree, dialog_id, inside_id));
    assert!(is_behind_modal(&tree, dialog_id, behind_id));
}

#[test]
fn dismiss_on_backdrop_defaults_to_true() {
    assert!(dismisses_on_backdrop(&Element::new("Dialog", None)));
    let mut pinned = Element::new("Dialog", None);
    pinned.set("dismiss_on_backdrop", Value::Bool(false));
    assert!(!dismisses_on_backdrop(&pinned));
}

/// Drives the tracker at one point and discards the change list.
fn state_update(
    states: &mut WidgetStates,
    engine: &mut Engine,
    tree: &mut ElementTree,
    x: f32,
    y: f32,
    down: bool,
) {
    states.update(engine, tree, input(x, y, down));
}

/// A widget inside an invisible subtree keeps its stale box (layout skips
/// it, `write_back` never clears it), so hover must consult `visible` —
/// an ancestor's flag counts, since the flag lives on the page root the
/// widget sits in.
#[test]
fn widgets_under_an_invisible_ancestor_never_hover() {
    let mut tree = ElementTree::new();
    let mut page = Element::new("Column", None);
    page.set("visible", Value::Bool(false));
    let page_id = tree.insert(page);
    tree.push_root(page_id);
    let button_id = tree.insert(widget("Button", 50.0, 50.0, 100.0, 40.0));
    tree.append_child(page_id, button_id);
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();

    // The pointer sits inside the stale box; without the ancestor check
    // this lights `hovered` for a widget that is not on screen.
    state_update(&mut states, &mut engine, &mut tree, 60.0, 60.0, false);
    assert_eq!(bool_of(&tree, button_id, "hovered"), Some(false));
    assert_eq!(bool_of(&tree, button_id, "pressed"), Some(false));

    // `visible = false` on the widget itself counts the same way.
    let (mut tree, id) = tree_of(widget("Button", 0.0, 0.0, 100.0, 40.0));
    tree.arena[id].set("visible", Value::Bool(false));
    state_update(&mut states, &mut engine, &mut tree, 50.0, 20.0, false);
    assert_eq!(bool_of(&tree, id, "hovered"), Some(false));
}

/// A closed overlay is off-screen, subtree included — hover must not
/// reach a control inside it through its stale box.
#[test]
fn widgets_inside_a_closed_overlay_hover_as_invisible() {
    let mut tree = ElementTree::new();
    let mut dialog = Element::new("Dialog", None);
    dialog.set("open", Value::Bool(false));
    let dialog_id = tree.insert(dialog);
    tree.push_root(dialog_id);
    let button_id = tree.insert(widget("Button", 40.0, 40.0, 100.0, 40.0));
    tree.append_child(dialog_id, button_id);
    let mut engine = Engine::new();
    let mut states = WidgetStates::new();

    state_update(&mut states, &mut engine, &mut tree, 50.0, 50.0, false);
    assert_eq!(bool_of(&tree, button_id, "hovered"), Some(false));
    // An open overlay is on screen and hovers normally.
    tree.arena[dialog_id].set("open", Value::Bool(true));
    state_update(&mut states, &mut engine, &mut tree, 50.0, 50.0, false);
    assert_eq!(bool_of(&tree, button_id, "hovered"), Some(true));
}
