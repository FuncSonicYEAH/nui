//! A host-registered component that declares an
//! [`Interaction`] is a control: the widget tracker writes its states, a
//! click activates it, and Tab reaches it.
//!
//! The point of these tests is that nothing here knows the component's
//! *name*. `RippleButton` and `Rectangle` are the same thing to layout and
//! to the renderer, so the only way a hover highlight can appear is if the
//! interaction layer was told about the component — which is exactly the
//! seam being tested.
#![allow(clippy::unwrap_used)]

use nui_compiler::compile;
use nui_core::Value;
use nui_runtime::{
    BehaviorContext, ComponentDesc, ElementBehavior, ElementId, ElementTree, Engine, Interaction,
    PointerInput, Registry, WidgetStates,
};

/// Test error type for `Result`-returning tests.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Compiles and instantiates a document with a registry, asserting the
/// document is clean.
fn build(source: &str, registry: Registry) -> (ElementTree, Engine) {
    let outcome = compile(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "test source must compile cleanly: {:?}",
        outcome.diagnostics
    );
    let instance = nui_runtime::instantiate_with(&outcome.document, registry);
    return (instance.tree, instance.engine);
}

/// A registry declaring one interactive component.
fn registry_named(name: &str, interaction: Interaction) -> Registry {
    let mut registry = Registry::new();
    registry.register_component(
        ComponentDesc::new(name).with_interaction(interaction),
        Some(Box::new(|| return Box::new(Noop))),
    );
    return registry;
}

/// Records the signals its element received, so activation is observable.
#[derive(Debug, Default)]
struct SignalLog(std::rc::Rc<std::cell::RefCell<Vec<String>>>);

impl ElementBehavior for SignalLog {
    fn on_signal(&mut self, _context: &mut BehaviorContext<'_>, _element: ElementId, signal: &str) {
        self.0.borrow_mut().push(signal.to_string());
    }
}

/// A behavior that does nothing, for components whose behavior is not the
/// subject of the test.
#[derive(Debug)]
struct Noop;

impl ElementBehavior for Noop {
    fn on_signal(
        &mut self,
        _context: &mut BehaviorContext<'_>,
        _element: ElementId,
        _signal: &str,
    ) {
    }
}

/// A pointer at `(x, y)`, inside the window, primary button `down` or not.
fn input(x: f32, y: f32, down: bool) -> PointerInput {
    return PointerInput {
        position: nui_core::Point::new(x, y),
        inside: true,
        down,
    };
}

/// A document with one component instance, laid out by hand at the origin.
///
/// Hand-setting the geometry is what the widget layer reads (`x`/`y`/
/// `width`/`height` are exactly what `nui-layout` writes), which keeps
/// these tests free of a layout dependency.
fn one_instance(interaction: Interaction) -> (ElementTree, Engine, ElementId) {
    let (mut tree, mut engine) = build(
        r#"
        component Chip {
            Rectangle(id = chip_box, width = 100dp, height = 40dp)
        }
        component App {
            Window(id = root) { Chip(id = chip) }
        }
    "#,
        registry_named("Chip", interaction),
    );
    let chip = tree.lookup_id("chip").expect("the instance exists");
    tree.arena[chip].set("x", Value::Float(0.0));
    tree.arena[chip].set("y", Value::Float(0.0));
    tree.arena[chip].set("width", Value::Float(100.0));
    tree.arena[chip].set("height", Value::Float(40.0));
    let _ = engine.propagate(&mut tree);
    return (tree, engine, chip);
}

#[test]
fn a_declared_interaction_makes_a_component_a_control() {
    let (tree, engine, chip) = one_instance(Interaction::momentary());
    let interaction = engine.interaction(&tree, chip);
    assert_eq!(interaction.kind, Some(nui_runtime::WidgetKind::Momentary));
    assert!(interaction.is_control());
    assert!(interaction.focusable);
}

#[test]
fn a_component_without_an_interaction_stays_inert() {
    // The default is deliberately "not a control": a component that only
    // arranges other things must not start reporting hover.
    let (mut tree, mut engine, chip) = one_instance(Interaction::default());
    assert!(!engine.interaction(&tree, chip).is_control());
    let mut states_tracker = WidgetStates::new();
    states_tracker.update(&mut engine, &mut tree, input(10.0, 10.0, false));
    assert_eq!(
        tree.arena[chip].get("hovered"),
        None,
        "an inert component gets no state properties at all"
    );
}

#[test]
fn hovering_a_component_writes_its_state_properties() -> TestResult {
    let (mut tree, mut engine, chip) = one_instance(Interaction::momentary());
    let mut tracker = WidgetStates::new();
    // Pointer outside: every state is written once, all false.
    tracker.update(&mut engine, &mut tree, input(500.0, 500.0, false));
    assert_eq!(tree.arena[chip].get("hovered"), Some(&Value::Bool(false)));
    // Pointer inside.
    tracker.update(&mut engine, &mut tree, input(50.0, 20.0, false));
    assert_eq!(tree.arena[chip].get("hovered"), Some(&Value::Bool(true)));
    // Press inside: `pressed` and `armed` both light up.
    tracker.capture(Some(chip));
    tracker.update(&mut engine, &mut tree, input(50.0, 20.0, true));
    assert_eq!(tree.arena[chip].get("pressed"), Some(&Value::Bool(true)));
    assert_eq!(tree.arena[chip].get("armed"), Some(&Value::Bool(true)));
    // Drag out of the box while held: the press stays, the arming drops —
    // the distinction a release-outside needs.
    tracker.update(&mut engine, &mut tree, input(500.0, 500.0, true));
    assert_eq!(tree.arena[chip].get("pressed"), Some(&Value::Bool(true)));
    assert_eq!(tree.arena[chip].get("armed"), Some(&Value::Bool(false)));
    return Ok(());
}

#[test]
fn a_disabled_component_reports_disabled_and_no_hover() -> TestResult {
    let (mut tree, mut engine, chip) = one_instance(Interaction::momentary());
    tree.arena[chip].set("enabled", Value::Bool(false));
    let mut tracker = WidgetStates::new();
    tracker.update(&mut engine, &mut tree, input(50.0, 20.0, false));
    assert_eq!(tree.arena[chip].get("disabled"), Some(&Value::Bool(true)));
    assert_eq!(tree.arena[chip].get("hovered"), Some(&Value::Bool(false)));
    return Ok(());
}

#[test]
fn a_toggle_component_flips_its_declared_property() -> TestResult {
    let (mut tree, mut engine, chip) = one_instance(Interaction::toggle("checked"));
    assert_eq!(
        engine.interaction(&tree, chip).toggle_property,
        Some("checked")
    );
    assert!(
        nui_runtime::widget::activate(&mut engine, &mut tree, chip),
        "a toggle writes, so activate reports the write"
    );
    let _ = engine.propagate(&mut tree);
    assert_eq!(tree.arena[chip].get("checked"), Some(&Value::Bool(true)));
    // A second click flips it back: `checked` toggles.
    let _ = nui_runtime::widget::activate(&mut engine, &mut tree, chip);
    let _ = engine.propagate(&mut tree);
    assert_eq!(tree.arena[chip].get("checked"), Some(&Value::Bool(false)));
    return Ok(());
}

#[test]
fn a_select_component_selects_and_groups() -> TestResult {
    // `selected` selects rather than toggles, and shares a `group` with
    // the built-in radio buttons — matched by behaviour, not by type name.
    let (mut tree, mut engine) = build(
        r#"
        component Choice {
            Rectangle(id = choice_box, width = 20dp, height = 20dp)
        }
        component App {
            Window(id = root) {
                Column {
                    Choice(id = mine, group = plan)
                    RadioButton(id = theirs, group = plan, selected = true)
                }
            }
        }
    "#,
        registry_named("Choice", Interaction::toggle("selected")),
    );
    let mine = tree.lookup_id("mine").expect("the component instance");
    let theirs = tree.lookup_id("theirs").expect("the built-in radio");
    let _ = nui_runtime::widget::activate(&mut engine, &mut tree, mine);
    let _ = engine.propagate(&mut tree);
    assert_eq!(tree.arena[mine].get("selected"), Some(&Value::Bool(true)));
    assert_eq!(
        tree.arena[theirs].get("selected"),
        Some(&Value::Bool(false)),
        "selecting in the group cleared the built-in radio"
    );
    // Clicking it again leaves it selected — there is no "none of the
    // above" in a radio group.
    let _ = nui_runtime::widget::activate(&mut engine, &mut tree, mine);
    let _ = engine.propagate(&mut tree);
    assert_eq!(tree.arena[mine].get("selected"), Some(&Value::Bool(true)));
    return Ok(());
}

#[test]
fn a_momentary_component_is_activatable_by_keyboard() -> TestResult {
    let (mut tree, mut engine, chip) = one_instance(Interaction::momentary());
    let _ = engine.propagate(&mut tree);
    assert!(nui_runtime::widget::is_activatable(&engine, &tree, chip));
    tree.arena[chip].set("enabled", Value::Bool(false));
    assert!(
        !nui_runtime::widget::is_activatable(&engine, &tree, chip),
        "a disabled control is not activated by Space or Enter"
    );
    return Ok(());
}

#[test]
fn a_focusable_component_instance_is_a_tab_stop() -> TestResult {
    let (mut tree, mut engine, chip) = one_instance(Interaction::momentary());
    assert!(
        tree.arena[chip].is_focusable(),
        "the focusable flag is set at instantiation, from the registration"
    );
    engine.focus_next(&mut tree);
    assert_eq!(engine.focused(), Some(chip));
    return Ok(());
}

#[test]
fn a_non_focusable_component_is_not_a_tab_stop() -> TestResult {
    let (mut tree, mut engine, chip) = one_instance(Interaction::momentary_unfocusable());
    assert!(!tree.arena[chip].is_focusable());
    engine.focus_next(&mut tree);
    assert_eq!(engine.focused(), None, "nothing to focus");
    return Ok(());
}

#[test]
fn a_behavior_still_receives_the_click() -> TestResult {
    // The registration adds interaction; it does not replace the host
    // behavior, which is how a component reacts to its own activation.
    let (mut tree, mut engine) = build(
        r#"
        component Chip { Rectangle(id = chip_box, width = 100dp, height = 40dp) }
        component App { Window(id = root) { Chip(id = chip) } }
    "#,
        registry_named("Chip", Interaction::momentary()),
    );
    let chip = tree.lookup_id("chip").expect("the instance");
    // The factory registered above is `Noop`; swap in a logging behavior
    // directly, because a `BehaviorFactory` is a `'static` closure and the
    // log is a borrowed `Rc`.
    let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> =
        std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    tree.arena[chip].behavior = Some(Box::new(SignalLog(log.clone())));
    nui_runtime::widget::activate(&mut engine, &mut tree, chip);
    assert_eq!(*log.borrow(), vec!["click".to_string()]);
    return Ok(());
}

#[test]
fn a_drag_component_follows_the_pointer_while_captured() -> TestResult {
    let (mut tree, mut engine, chip) = one_instance(Interaction::drag());
    tree.arena[chip].set("min", Value::Float(0.0));
    tree.arena[chip].set("max", Value::Float(100.0));
    tree.arena[chip].set("value", Value::Float(0.0));
    let mut tracker = WidgetStates::new();
    tracker.capture(Some(chip));
    // Press near the left edge, then drag to the right half. The value has
    // to follow even though the gesture started as a click.
    tracker.update(&mut engine, &mut tree, input(10.0, 20.0, true));
    let after_press = tree.arena[chip].get("value").cloned();
    tracker.update(&mut engine, &mut tree, input(90.0, 20.0, true));
    let after_drag = tree.arena[chip].get("value").cloned();
    assert_ne!(after_press, after_drag, "the drag moved the value");
    let value = match after_drag {
        Some(Value::Float(value)) => value,
        other => panic!("expected a Float value, got {other:?}"),
    };
    assert!(
        value > 50.0,
        "the pointer is past the midpoint, got {value}"
    );
    return Ok(());
}

#[test]
fn a_range_control_moves_whichever_end_is_nearer() -> TestResult {
    // One gesture, two ends. The pointer picks the end by proximity in
    // *value* space, so pressing near the left moves the left thumb and
    // pressing near the right moves the right one — which is what a second
    // `Slider` stacked underneath cannot do, since only one element can
    // hold the capture.
    let (mut tree, mut engine) = build(
        r#"
        component Range {
            property first: Float = 25.0
            property second: Float = 75.0
            Stack(id = range_box, width = 200dp, height = 20dp) {}
        }
        component App {
            Window(id = root) { Range(id = range) }
    }
    "#,
        registry_named("Range", Interaction::range("first", "second")),
    );
    let range = tree.lookup_id("range").expect("the range control");
    tree.arena[range].set("x", Value::Float(0.0));
    tree.arena[range].set("y", Value::Float(0.0));
    tree.arena[range].set("width", Value::Float(200.0));
    tree.arena[range].set("height", Value::Float(20.0));
    tree.arena[range].set("min", Value::Float(0.0));
    tree.arena[range].set("max", Value::Float(100.0));
    let _ = engine.propagate(&mut tree);

    // Near the left end: the low thumb moves.
    let value_of = |tree: &ElementTree, name: &str| -> Option<f64> {
        return tree.arena[range]
            .get(name)
            .and_then(|value| return value.as_f64().ok());
    };
    drag_to(&mut engine, &mut tree, range, 20.0);
    assert!(
        value_of(&tree, "first").is_some_and(|value| return value < 25.0),
        "the low end moved"
    );
    assert_eq!(value_of(&tree, "second"), Some(75.0), "the far end stayed");

    // Near the right end: the high thumb moves instead, and the low one
    // keeps the position the previous drag gave it.
    let first_after_left = value_of(&tree, "first");
    drag_to(&mut engine, &mut tree, range, 180.0);
    assert_eq!(
        value_of(&tree, "first"),
        first_after_left,
        "the low end kept its new position"
    );
    assert!(
        value_of(&tree, "second").is_some_and(|value| return value > 75.0),
        "the high end moved"
    );
    return Ok(());
}

#[test]
fn a_range_control_tie_resolves_to_the_low_end() -> TestResult {
    // A press exactly between the two thumbs must always do the same thing,
    // or a control would jump unpredictably at the midpoint.
    let (mut tree, mut engine) = build(
        r#"
        component Range {
            property first: Float = 40.0
            property second: Float = 60.0
            Stack(id = range_box, width = 200dp, height = 20dp) {}
        }
        component App {
            Window(id = root) { Range(id = range) }
    }
    "#,
        registry_named("Range", Interaction::range("first", "second")),
    );
    let range = tree.lookup_id("range").expect("the range control");
    for (property, value) in [
        ("x", 0.0),
        ("y", 0.0),
        ("width", 200.0),
        ("height", 20.0),
        ("min", 0.0),
        ("max", 100.0),
    ] {
        tree.arena[range].set(property, Value::Float(value));
    }
    let _ = engine.propagate(&mut tree);
    drag_to(&mut engine, &mut tree, range, 100.0);
    let first = tree.arena[range]
        .get("first")
        .and_then(|v| return v.as_f64().ok());
    let second = tree.arena[range]
        .get("second")
        .and_then(|v| return v.as_f64().ok());
    assert_ne!(
        first, second,
        "exactly one end moved, got first={first:?} second={second:?}"
    );
    assert_eq!(
        first,
        Some(50.0),
        "the midpoint is equidistant, so the low end takes it"
    );
    assert_eq!(second, Some(60.0));
    return Ok(());
}

#[test]
fn a_range_control_with_unset_ends_still_picks_one() -> TestResult {
    // A control that has not been given its ends yet must not divide by a
    // missing value; both read as `min` and the low end takes the press.
    let (mut tree, mut engine) = build(
        r#"
        component Range {
            Stack(id = range_box, width = 200dp, height = 20dp) {}
        }
        component App {
            Window(id = root) { Range(id = range) }
    }
    "#,
        registry_named("Range", Interaction::range("first", "second")),
    );
    let range = tree.lookup_id("range").expect("the range control");
    for (property, value) in [
        ("x", 0.0),
        ("y", 0.0),
        ("width", 200.0),
        ("height", 20.0),
        ("min", 0.0),
        ("max", 100.0),
    ] {
        tree.arena[range].set(property, Value::Float(value));
    }
    let _ = engine.propagate(&mut tree);
    drag_to(&mut engine, &mut tree, range, 150.0);
    // Not 75: the mapping runs through the *thumb travel*, which is inset by
    // half the thumb on each end (the default thumb is the element's height,
    // 20dp), so x=150 of a 200dp track is 140/180 of the way across.
    let first = tree.arena[range]
        .get("first")
        .and_then(|value| return value.as_f64().ok());
    assert!(
        first.is_some_and(|value| return (70.0..85.0).contains(&value)),
        "the low end moved to the pointer's place on the track, got {first:?}"
    );
    assert_eq!(
        tree.arena[range].get("second"),
        None,
        "the high end was never written"
    );
    return Ok(());
}

#[test]
fn a_range_control_is_a_tab_stop_and_activatable() -> TestResult {
    let (mut tree, mut engine) = build(
        r#"
        component Range {
            Stack(id = range_box, width = 200dp, height = 20dp) {}
        }
        component App {
            Window(id = root) { Range(id = range) }
    }
    "#,
        registry_named("Range", Interaction::range("first", "second")),
    );
    let range = tree.lookup_id("range").expect("the range control");
    assert!(tree.arena[range].is_focusable());
    assert!(nui_runtime::widget::is_activatable(&engine, &tree, range));
    // A range has no Boolean to flip, so activation only emits `click` —
    // which is what a document's `on click` handler needs.
    assert!(!nui_runtime::widget::activate(
        &mut engine,
        &mut tree,
        range
    ));
    return Ok(());
}

/// Drives a captured drag to `x` on a horizontal control.
fn drag_to(engine: &mut Engine, tree: &mut ElementTree, id: nui_runtime::ElementId, x: f32) {
    let mut tracker = WidgetStates::new();
    tracker.capture(Some(id));
    tracker.update(
        engine,
        tree,
        PointerInput {
            position: nui_core::Point::new(x, 10.0),
            inside: true,
            down: true,
        },
    );
}

#[test]
fn an_element_type_is_still_answered_from_its_name() -> TestResult {
    // The registry must not shadow a built-in: a `Button` in a document
    // that also registers a component called `Button` is still the built-in,
    // because only a *reference* records a component name on the element.
    let (tree, engine) = build(
        r#"
        component App { Window(id = root) { Button(id = press, width = 80dp, height = 32dp) } }
    "#,
        registry_named("Button", Interaction::default()),
    );
    let button = tree.lookup_id("press").expect("the button");
    assert_eq!(tree.arena[button].component, None);
    assert_eq!(
        engine.interaction(&tree, button).kind,
        Some(nui_runtime::WidgetKind::Momentary),
        "the built-in table still answers"
    );
    return Ok(());
}
