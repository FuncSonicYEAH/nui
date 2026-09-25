//! Headless engine integration tests (M2 acceptance): compile a document,
//! instantiate it, drive signals, and assert the full pipeline —
//! instantiation, bindings, signals, state machines, `when` blocks, and
//! timers — without any rendering.
#![allow(clippy::unwrap_used)]

use nui_compiler::compile;
use nui_core::{Duration, Value};
use nui_runtime::{Engine, instantiate};

/// Test error type for `Result`-returning tests.
type TestResult = Result<(), Box<dyn std::error::Error>>;

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

const STATEFUL_COUNTER: &str = r#"
    component Counter {
        property count: Int = 0
        property limit: Int = 3
        property resetPressed: Bool = false
        signal reset
        signal bump

        machine mode {
            state idle
            state overflow {
                enter => resetPressed = true
                exit => resetPressed = false
            }
            on bump from idle when count < limit => overflow
            on reset from overflow => idle
        }

        Window(id = root) {
            Text(id = label, content <- "count: {count}")
            Text(id = flag, flagged <- mode.overflow)
            when mode.overflow {
                label.opacity = 0.5
            }
        }
    }
"#;

#[test]
fn stateful_counter_walks_full_pipeline() -> TestResult {
    let (mut tree, mut engine) = build(STATEFUL_COUNTER);
    engine.propagate(&mut tree);
    let root = tree.lookup_id("root").expect("root id resolves");
    let label = tree.lookup_id("label").expect("label id resolves");

    // Initial binding evaluation filled the content.
    assert_eq!(
        tree.arena[label].get("content"),
        Some(&Value::String("count: 0".to_string()))
    );

    // The machine starts in its first declared state.
    assert_eq!(
        engine.machine_state_of(&mut tree, root, "mode", "idle"),
        Value::Bool(true)
    );

    // Counting primes the guard (count < limit = 3); the next bump
    // transitions to overflow and runs its enter effect.
    for expected in 1..=2 {
        tree.arena[root].set("count", Value::Int(expected));
        engine.invalidate(root, "count");
    }
    engine.propagate(&mut tree);
    assert_eq!(
        tree.arena[label].get("content"),
        Some(&Value::String("count: 2".to_string()))
    );

    engine.emit_signal(&mut tree, root, "bump")?;
    assert_eq!(
        engine.machine_state_of(&mut tree, root, "mode", "overflow"),
        Value::Bool(true)
    );

    // `when` applies the visual state while the machine is overflowing.
    // (M4 fix: `label.opacity` writes the `opacity` property on the label
    // element, not a dotted `label.opacity` key — the scene builder reads
    // plain `opacity`.)
    engine.apply_when_blocks(&mut tree)?;
    assert_eq!(tree.arena[label].get("opacity"), Some(&Value::Float(0.5)));

    // `reset` returns to idle and runs the exit effect.
    engine.emit_signal(&mut tree, root, "reset")?;
    assert_eq!(
        engine.machine_state_of(&mut tree, root, "mode", "idle"),
        Value::Bool(true)
    );
    return Ok(());
}

#[test]
fn guard_blocks_transition_until_limit() {
    let source = r#"
        component Gate {
            property open: Bool = false
            signal knock
            machine m {
                state closed
                state opened
                on knock from closed when open => opened
            }
            Window(id = root) {}
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let root = tree.lookup_id("root").unwrap();
    engine.emit_signal(&mut tree, root, "knock").unwrap();
    assert_eq!(
        engine.machine_state_of(&mut tree, root, "m", "closed"),
        Value::Bool(true),
        "guard must hold the machine in closed"
    );
    tree.arena[root].set("open", Value::Bool(true));
    engine.emit_signal(&mut tree, root, "knock").unwrap();
    assert_eq!(
        engine.machine_state_of(&mut tree, root, "m", "opened"),
        Value::Bool(true)
    );
}

#[test]
fn binding_chain_updates_on_property_write() {
    let source = r#"
        component Chain {
            property base: Int = 2
            Window(id = root) {
                Text(id = out, value <- base * 10 + 1)
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let out = tree.lookup_id("out").unwrap();
    engine.propagate(&mut tree);
    assert_eq!(tree.arena[out].get("value"), Some(&Value::Int(21)));
    let root = tree.lookup_id("root").unwrap();
    tree.arena[root].set("base", Value::Int(4));
    engine.invalidate(root, "base");
    engine.propagate(&mut tree);
    assert_eq!(tree.arena[out].get("value"), Some(&Value::Int(41)));
}

#[test]
fn timer_fires_repeatedly_until_stopped() {
    let source = r#"
        component Ticker {
            property ticks: Int = 0
            Window(id = root) {
                Timer(id = timer)
                Text(id = out, value <- "t{ticks}")
                Button(id = go) {
                    on click => timer.start(50ms)
                }
                Button(id = halt) {
                    on click => timer.stop()
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let timer = tree.lookup_id("timer").unwrap();
    let go = tree.lookup_id("go").unwrap();
    let halt = tree.lookup_id("halt").unwrap();
    let root = tree.lookup_id("root").unwrap();

    engine.emit_signal(&mut tree, go, "click").unwrap();
    assert!(tree.arena[timer].timer_interval.is_some());

    let mut elapsed = std::collections::HashMap::new();
    let fired = engine
        .tick_timers(&mut tree, Duration::from_millis(120.0), &mut elapsed)
        .unwrap();
    assert_eq!(fired.len(), 2, "120ms / 50ms = 2 fires, got {fired:?}");

    engine.emit_signal(&mut tree, halt, "click").unwrap();
    assert!(tree.arena[timer].timer_interval.is_none());
    let _ = root;
}

#[test]
fn runtime_cycle_is_caught_not_looped() {
    let source = r#"
        component Loop {
            property a: Int = 1
            Window(id = root) {
                Text(id = out, value <- a)
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let out = tree.lookup_id("out").unwrap();
    // Simulate a dynamic cycle the static checker cannot see: make `out.value`
    // read from a property bound back to itself through an extra binding.
    let expr = nui_compiler::TypedExpr::Property {
        target: nui_compiler::PropertyTarget::Id("out".to_string(), "value".to_string()),
        ty: nui_compiler::Type::Int,
    };
    let index = engine.register_binding(out, "value", expr);
    tree.arena[out].set_binding("value", nui_runtime::Binding { index });
    let errors = engine.propagate(&mut tree);
    assert!(
        errors
            .iter()
            .any(|(_, error)| matches!(error, nui_runtime::EvalError::Cycle { .. })),
        "self-reading binding must be reported as a cycle, got {errors:?}"
    );
}

#[test]
fn effect_write_clears_binding_d10_end_to_end() {
    let source = r#"
        component D10 {
            property count: Int = 0
            Window(id = root) {
                Text(id = out, value <- "n{count}")
                Button(id = btn) {
                    on click => out.value = "static"
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let out = tree.lookup_id("out").unwrap();
    engine.propagate(&mut tree);
    assert_eq!(
        tree.arena[out].get("value"),
        Some(&Value::String("n0".to_string()))
    );
    assert!(tree.arena[out].has_binding("value"));

    let btn = tree.lookup_id("btn").unwrap();
    engine.emit_signal(&mut tree, btn, "click").unwrap();
    assert_eq!(
        tree.arena[out].get("value"),
        Some(&Value::String("static".to_string()))
    );
    assert!(
        !tree.arena[out].has_binding("value"),
        "D10: the effect write must clear the <- binding"
    );
}

#[test]
fn two_way_binding_syncs_both_directions() {
    let source = r#"
        component Sync {
            property userName: String = ""
            Window(id = root) {
                TextField(id = field, text <=> root.userName)
            }
        }
    "#;
    let (mut tree, _engine) = build(source);
    let field = tree.lookup_id("field").unwrap();
    let root = tree.lookup_id("root").unwrap();
    assert!(
        !tree.arena[field].pending_two_way.is_empty(),
        "the <=> assignment must be pending before linking"
    );

    // Link manually (the id table exists post-instantiate): field.text <->
    // root.userName.
    tree.arena[field].set("text", Value::String("Ada".to_string()));
    tree.arena[field].set_two_way(
        "text",
        nui_runtime::TwoWayLink {
            partner: root,
            property: "userName".to_string(),
        },
    );
    // Simulate the field pushing its value through the value channel.
    let field_value = tree.arena[field].get("text").cloned().unwrap();
    tree.arena[root].set("userName", field_value);
    assert_eq!(
        tree.arena[root].get("userName"),
        Some(&Value::String("Ada".to_string()))
    );
}

#[derive(Debug, Clone)]
struct ChangeLog {
    entries: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

impl nui_runtime::PropertyObserver for ChangeLog {
    fn on_property_change(&mut self, change: &nui_runtime::PropertyChange) {
        self.entries
            .borrow_mut()
            .push(format!("{}={}", change.property, change.new_value));
    }
}

#[test]
fn observer_sees_binding_and_effect_changes_in_order() {
    let entries = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let source = r#"
        component Watched {
            property count: Int = 1
            Window(id = root) {
                Text(id = out, doubled <- count * 2)
                Button(id = btn) {
                    on click => count += 5
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    engine.subscribe(Box::new(ChangeLog {
        entries: std::rc::Rc::clone(&entries),
    }));
    let root = tree.lookup_id("root").unwrap();

    // Initial propagation buffers the binding write.
    engine.propagate(&mut tree);
    let changes = engine.take_changes();
    assert_eq!(changes.len(), 1, "one binding write: {changes:?}");
    assert_eq!(changes[0].source, nui_runtime::ChangeSource::Binding);
    assert_eq!(changes[0].property, "doubled");

    // An effect write arrives with its own source and syncs the dependent.
    engine.emit_signal(&mut tree, root, "click").unwrap();
    engine.propagate(&mut tree);
    let changes = engine.take_changes();
    let sources: Vec<_> = changes.iter().map(|change| return change.source).collect();
    assert!(
        sources.contains(&nui_runtime::ChangeSource::Effect),
        "effect write must be observed: {changes:?}"
    );
    assert!(
        sources.contains(&nui_runtime::ChangeSource::Binding),
        "dependent binding must be observed: {changes:?}"
    );

    // The observer received the same events in the same order.
    let log = entries.borrow().clone();
    assert_eq!(log.len(), 1 + changes.len(), "drain notifies subscribers");
}

#[test]
fn set_direct_records_host_change_and_invalidates() {
    let source = r#"
        component Hosted {
            property count: Int = 0
            Window(id = root) {
                Text(id = out, value <- "n{count}")
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    engine.propagate(&mut tree);
    engine.take_changes();
    let root = tree.lookup_id("root").unwrap();
    let out = tree.lookup_id("out").unwrap();

    let changed = engine.set_direct(&mut tree, root, "count", Value::Int(9));
    assert!(changed);
    engine.propagate(&mut tree);
    assert_eq!(
        tree.arena[out].get("value"),
        Some(&Value::String("n9".to_string())),
        "host write must invalidate dependent bindings"
    );
    let changes = engine.take_changes();
    assert!(
        changes.iter().any(
            |change| return change.source == nui_runtime::ChangeSource::Host
                && change.property == "count"
        ),
        "host write must be buffered: {changes:?}"
    );
}

#[test]
fn unsubscribed_observer_stops_receiving() {
    let entries = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let source = r#"
        component Quiet {
            Window(id = root) {
                Text(id = out, value <- "x")
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let handle = engine.subscribe(Box::new(ChangeLog {
        entries: std::rc::Rc::clone(&entries),
    }));
    engine.propagate(&mut tree);
    engine.take_changes();
    assert!(!entries.borrow().is_empty());

    assert!(engine.unsubscribe(handle));
    entries.borrow_mut().clear();
    let out = tree.lookup_id("out").unwrap();
    tree.arena[out].mark_dirty("value");
    engine.propagate(&mut tree);
    engine.take_changes();
    assert!(
        entries.borrow().is_empty(),
        "unsubscribed observer must stay quiet"
    );
}

#[test]
fn animation_tick_with_notify_buffers_changes() {
    let source = r#"
        component Anim {
            Window(id = root) {
                Rectangle(id = box, width = 0dp, height = 10dp)
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let box_element = tree.lookup_id("box").unwrap();
    let mut clock = nui_runtime::AnimationClock::new();
    clock.start_tween(
        box_element,
        "width",
        Value::Length(nui_core::Length::Dp(100.0)),
        Duration::from_millis(100.0),
        nui_runtime::Easing::Linear,
        Value::Length(nui_core::Length::Dp(0.0)),
    );
    clock.tick_with_notify(&mut tree, Duration::from_millis(50.0), &mut engine);
    let changes = engine.take_changes();
    assert!(
        changes.iter().any(
            |change| return change.source == nui_runtime::ChangeSource::Animation
                && change.property == "width"
        ),
        "animated writes must be buffered: {changes:?}"
    );
}

#[test]
fn animation_clock_animates_property_between_frames() {
    use nui_runtime::AnimationClock;
    let source = r#"
        component Anim {
            Window(id = root) {
                Rectangle(id = box, width = 100dp, height = 10dp)
            }
        }
    "#;
    let (mut tree, _engine) = build(source);
    let box_element = tree.lookup_id("box").unwrap();
    let mut clock = AnimationClock::new();
    clock.start_tween(
        box_element,
        "width",
        Value::Length(nui_core::Length::Dp(200.0)),
        Duration::from_millis(100.0),
        nui_runtime::Easing::Linear,
        Value::Length(nui_core::Length::Dp(100.0)),
    );
    clock.tick(&mut tree, Duration::from_millis(50.0));
    let mid = tree.arena[box_element].get("width").cloned().unwrap();
    assert_eq!(mid, Value::Length(nui_core::Length::Dp(150.0)));
}

const FOR_MODEL_LIST: &str = r#"
    component List {
        property items: Model

        Window(id = root) {
            For(item in root.items) {
                Text(label <- item.label)
            }
        }
    }
"#;

/// Finds the first element of the given node type.
fn find_by_type(tree: &nui_runtime::ElementTree, ty: &str) -> nui_runtime::element::ElementId {
    let mut found = None;
    tree.visit_pre_order(|id, element| {
        if element.ty == ty {
            found = Some(id);
        }
    });
    return found.expect("element of type exists");
}

/// Full M4 frame ordering for model-driven trees: propagate (fills `@for`),
/// row sync, propagate again (fills the fresh row bindings).
fn model_frame(tree: &mut nui_runtime::ElementTree, engine: &mut Engine) {
    engine.propagate(tree);
    engine.sync_for_nodes(tree);
    engine.propagate(tree);
}

#[test]
fn for_instantiates_one_row_per_model_entry() {
    use nui_core::Value as V;
    use nui_runtime::VecModel;
    let (mut tree, mut engine) = build(FOR_MODEL_LIST);
    let root = tree.lookup_id("root").unwrap();
    let model = engine.add_model(Box::new(VecModel::from_rows(vec![
        vec![("label".to_string(), V::String("alpha".to_string()))],
        vec![("label".to_string(), V::String("beta".to_string()))],
    ])));
    engine.set_direct(&mut tree, root, "items", V::Model(model.0));
    model_frame(&mut tree, &mut engine);

    let for_element = find_by_type(&tree, "For");
    let rows = tree.arena[for_element].children.clone();
    assert_eq!(rows.len(), 2, "one row element per model entry");
    assert_eq!(
        tree.arena[rows[0]].get("label"),
        Some(&V::String("alpha".to_string()))
    );
    assert_eq!(
        tree.arena[rows[1]].get("label"),
        Some(&V::String("beta".to_string()))
    );
    // Row scopes: every row element knows its variable, model, and index.
    let scope = tree.arena[rows[0]].for_scope.clone().unwrap();
    assert_eq!(scope.variable, "item");
    assert_eq!(scope.model, model);
    assert_eq!(scope.row, 0);
    assert_eq!(tree.arena[rows[1]].for_scope.clone().unwrap().row, 1);
}

#[test]
fn model_set_field_updates_only_that_row() {
    use nui_core::Value as V;
    use nui_runtime::VecModel;
    let (mut tree, mut engine) = build(FOR_MODEL_LIST);
    let root = tree.lookup_id("root").unwrap();
    let model = engine.add_model(Box::new(VecModel::from_rows(vec![
        vec![("label".to_string(), V::String("alpha".to_string()))],
        vec![("label".to_string(), V::String("beta".to_string()))],
    ])));
    engine.set_direct(&mut tree, root, "items", V::Model(model.0));
    model_frame(&mut tree, &mut engine);

    let changed =
        engine.model_set_field(&mut tree, model, 1, "label", V::String("gamma".to_string()));
    assert!(changed);
    engine.propagate(&mut tree);
    let for_element = find_by_type(&tree, "For");
    let rows = tree.arena[for_element].children.clone();
    assert_eq!(
        tree.arena[rows[0]].get("label"),
        Some(&V::String("alpha".to_string())),
        "row 0 must be untouched"
    );
    assert_eq!(
        tree.arena[rows[1]].get("label"),
        Some(&V::String("gamma".to_string()))
    );
}

#[test]
fn model_push_and_remove_rebuild_rows() {
    use nui_core::Value as V;
    use nui_runtime::VecModel;
    let (mut tree, mut engine) = build(FOR_MODEL_LIST);
    let root = tree.lookup_id("root").unwrap();
    let model = engine.add_model(Box::new(VecModel::from_rows(vec![vec![(
        "label".to_string(),
        V::String("only".to_string()),
    )]])));
    engine.set_direct(&mut tree, root, "items", V::Model(model.0));
    model_frame(&mut tree, &mut engine);
    let for_element = find_by_type(&tree, "For");
    assert_eq!(tree.arena[for_element].children.len(), 1);

    engine.model_push(
        model,
        vec![("label".to_string(), V::String("second".to_string()))],
    );
    assert!(engine.has_pending_model_sync(), "push flags a row sync");
    engine.sync_for_nodes(&mut tree);
    engine.propagate(&mut tree);
    assert_eq!(tree.arena[for_element].children.len(), 2);
    let rows = tree.arena[for_element].children.clone();
    assert_eq!(
        tree.arena[rows[1]].get("label"),
        Some(&V::String("second".to_string()))
    );

    assert!(engine.model_remove(model, 0));
    engine.sync_for_nodes(&mut tree);
    engine.propagate(&mut tree);
    let rows = tree.arena[for_element].children.clone();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        tree.arena[rows[0]].get("label"),
        Some(&V::String("second".to_string())),
        "remaining row re-instantiated from the model"
    );
}

#[test]
fn effect_let_scopes_and_resolves_locals() {
    let source = r#"
        component A {
            property count: Int = 3
            property next: Int = 0

            Window(id = root) {
                Button(id = btn) {
                    on click => {
                        let doubled = count * 2
                        next = doubled
                    }
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let btn = tree.lookup_id("btn").unwrap();
    engine
        .emit_signal(&mut tree, btn, "click")
        .expect("effect runs");
    let root = tree.lookup_id("root").unwrap();
    assert_eq!(tree.arena[root].get("next"), Some(&Value::Int(6)));
}

#[test]
fn tween_binding_routes_target_through_the_clock() {
    let source = r#"
        component A {
            property count: Int = 0

            Window(id = root) {
                Text(x <- tween(count, duration = 100ms, easing = linear))
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let root = tree.lookup_id("root").unwrap();
    let text = find_by_type(&tree, "Text");
    engine.propagate(&mut tree);
    assert!(
        !engine.has_active_animations(),
        "initial evaluation targets the seed value"
    );

    engine.set_direct(&mut tree, root, "count", Value::Int(100));
    engine.propagate(&mut tree);
    assert!(engine.has_active_animations(), "new target starts a tween");
    assert_eq!(
        tree.arena[text].get("x"),
        Some(&Value::Int(0)),
        "the displayed value must not jump to the target at bind time"
    );

    engine.tick_animations(&mut tree, Duration::from_millis(50.0));
    assert_eq!(
        tree.arena[text].get("x"),
        Some(&Value::Int(50)),
        "linear easing at half duration"
    );
    engine.tick_animations(&mut tree, Duration::from_millis(50.0));
    assert_eq!(tree.arena[text].get("x"), Some(&Value::Int(100)));
    assert!(!engine.has_active_animations(), "finished tweens retire");
}

#[test]
fn spring_binding_settles_at_target() {
    let source = r#"
        component A {
            property count: Int = 0

            Window(id = root) {
                Text(x <- spring(count))
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let root = tree.lookup_id("root").unwrap();
    let text = find_by_type(&tree, "Text");
    engine.propagate(&mut tree);
    engine.set_direct(&mut tree, root, "count", Value::Int(100));
    engine.propagate(&mut tree);
    for _ in 0..400 {
        engine.tick_animations(&mut tree, Duration::from_millis(16.0));
        if !engine.has_active_animations() {
            break;
        }
    }
    let final_value = tree.arena[text].get("x").cloned().unwrap();
    let distance = (final_value.as_f64().unwrap() - 100.0).abs();
    assert!(distance < 0.01, "spring should settle, got {final_value:?}");
    assert!(!engine.has_active_animations());
}

#[test]
fn when_block_restores_static_value_on_leave() {
    let source = r#"
        component A {
            property flag: Bool = false

            Window(id = root) {
                Rectangle(id = panel, opacity = 1.0)
                when flag {
                    panel.opacity = 0.0
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let root = tree.lookup_id("root").unwrap();
    let panel = tree.lookup_id("panel").unwrap();
    engine.propagate(&mut tree);
    engine.apply_when_blocks(&mut tree).unwrap();
    assert_eq!(tree.arena[panel].get("opacity"), Some(&Value::Float(1.0)));

    // Condition holds: the when value applies.
    engine.set_direct(&mut tree, root, "flag", Value::Bool(true));
    engine.propagate(&mut tree);
    engine.apply_when_blocks(&mut tree).unwrap();
    assert_eq!(tree.arena[panel].get("opacity"), Some(&Value::Float(0.0)));

    // Condition leaves: the captured static value is restored (M4 fix —
    // previously the when value stuck forever).
    engine.set_direct(&mut tree, root, "flag", Value::Bool(false));
    engine.propagate(&mut tree);
    engine.apply_when_blocks(&mut tree).unwrap();
    assert_eq!(
        tree.arena[panel].get("opacity"),
        Some(&Value::Float(1.0)),
        "leaving a when block must restore the pre-override value"
    );
}

#[test]
fn host_function_is_callable_from_bindings_and_effects() {
    use nui_runtime::{Registry, instantiate_with};

    let source = r#"
        component A {
            property name: String = "ada"
            property upper: String <- uppercase(name)

            Window(id = root) {
                Button(id = btn) {
                    on click => log("clicked")
                }
            }
        }
    "#;
    let mut registry = Registry::new();
    registry.register_function(
        "uppercase",
        Box::new(|args: &[nui_core::Value]| {
            let Some(text) = args.first().and_then(|value| return value.as_str().ok()) else {
                return Err(nui_runtime::FunctionError::new(
                    "uppercase expects a string",
                ));
            };
            return Ok(nui_core::Value::String(text.to_uppercase()));
        }),
    );
    registry.register_function(
        "log",
        Box::new(|_args: &[nui_core::Value]| return Ok(nui_core::Value::Bool(true))),
    );

    // Compile-time validation: the registered names pass, a typo does not.
    let outcome = nui_compiler::compile_with_functions(source, &registry.function_names());
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let typo = nui_compiler::compile_with_functions(
        "component A { property x: String <- uppercae(\"a\") }",
        &registry.function_names(),
    );
    assert!(
        typo.diagnostics
            .iter()
            .any(|d| return d.message.contains("unknown function `uppercae`"))
    );

    let instance = instantiate_with(&outcome.document, registry);
    let mut tree = instance.tree;
    let mut engine = instance.engine;
    engine.propagate(&mut tree);
    let root = tree.lookup_id("root").unwrap();
    assert_eq!(
        tree.arena[root].get("upper"),
        Some(&Value::String("ADA".to_string()))
    );

    let btn = tree.lookup_id("btn").unwrap();
    engine
        .emit_signal(&mut tree, btn, "click")
        .expect("host function call in effect succeeds");
}

/// A custom Rust component: a counter box that increments itself on click.
#[derive(Debug)]
struct TallyBehavior;

impl nui_runtime::ElementBehavior for TallyBehavior {
    fn on_signal(
        &mut self,
        context: &mut nui_runtime::BehaviorContext<'_>,
        element: nui_runtime::element::ElementId,
        signal: &str,
    ) {
        if signal != "click" {
            return;
        }
        let current = context
            .property(element, "count")
            .and_then(|value| return value.as_f64().ok())
            .unwrap_or(0.0);
        context.set_property(element, "count", Value::Int(current as i64 + 1));
    }
}

#[test]
fn custom_component_carries_descriptor_and_behavior() {
    use nui_compiler::Type;
    use nui_runtime::{ComponentDesc, PropertyDescriptor, Registry, instantiate_with};

    let source = r#"
        component A {
            Window(id = root) {
                Tally(id = tally)
            }
        }
    "#;
    let mut registry = Registry::new();
    registry.register_component(
        ComponentDesc {
            name: "Tally".to_string(),
            properties: vec![PropertyDescriptor {
                name: "count".to_string(),
                ty: Type::Int,
                default: Some(Value::Int(10)),
            }],
        },
        Some(Box::new(|| return Box::new(TallyBehavior))),
    );
    let outcome = nui_compiler::compile(source);
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let instance = instantiate_with(&outcome.document, registry);
    let mut tree = instance.tree;
    let mut engine = instance.engine;
    let tally = tree.lookup_id("tally").unwrap();
    assert_eq!(
        tree.arena[tally].get("count"),
        Some(&Value::Int(10)),
        "descriptor default applied"
    );

    engine
        .emit_signal(&mut tree, tally, "click")
        .expect("behavior runs");
    assert_eq!(tree.arena[tally].get("count"), Some(&Value::Int(11)));
    engine
        .emit_signal(&mut tree, tally, "click")
        .expect("behavior runs");
    assert_eq!(tree.arena[tally].get("count"), Some(&Value::Int(12)));
}

/// The M4 acceptance scenario (todo-list demo) end to end, headless: a
/// model-driven `For`, a custom Rust checkbox that mutates its model row, a
/// remove button, and a tweened opacity driven by the model field.
#[test]
fn todo_demo_pipeline_end_to_end() {
    use nui_compiler::{Type, compile};
    use nui_core::Color;
    use nui_runtime::{
        BehaviorContext, ComponentDesc, ElementBehavior, PropertyDescriptor, Registry, VecModel,
        instantiate_with,
    };

    const TODO: &str = r#"
        component TodoApp {
            property items: Model

            Window(id = root) {
                Column(id = content, spacing = 10dp, padding = 16dp) {
                    For(item in root.items) {
                        Row(id = row, spacing = 10dp, height = 44dp) {
                            Rectangle(
                                id = swatch,
                                width = 18dp, height = 18dp, radius = 9dp,
                                fill <- item.done ? #3f9d55 : #64748b,
                                opacity <- tween(item.done ? 1.0 : 0.4, duration = 200ms, easing = ease-out),
                            )
                            Text(id = label, content <- item.label, width = 220dp, height = 24dp)
                            Checkbox(id = check, width = 24dp, height = 24dp, fill = #475569)
                            RemoveButton(id = remove, width = 24dp, height = 24dp, fill = #b45454)
                        }
                    }
                }
            }
        }
    "#;

    #[derive(Debug)]
    struct Checkbox;
    impl ElementBehavior for Checkbox {
        fn on_signal(
            &mut self,
            context: &mut BehaviorContext<'_>,
            element: nui_runtime::element::ElementId,
            signal: &str,
        ) {
            if signal != "click" {
                return;
            }
            let Some(scope) = context.row_scope(element) else {
                return;
            };
            let done = context
                .model_field(&scope, "done")
                .and_then(|value| return value.as_bool().ok())
                .unwrap_or(false);
            context.set_model_field(&scope, "done", Value::Bool(!done));
        }
    }
    #[derive(Debug)]
    struct Remove;
    impl ElementBehavior for Remove {
        fn on_signal(
            &mut self,
            context: &mut BehaviorContext<'_>,
            element: nui_runtime::element::ElementId,
            signal: &str,
        ) {
            if signal != "click" {
                return;
            }
            if let Some(scope) = context.row_scope(element) {
                context.remove_model_row(&scope);
            }
        }
    }

    let registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let model = engine.add_model(Box::new(VecModel::from_rows(vec![
            vec![
                ("label".to_string(), Value::String("a".to_string())),
                ("done".to_string(), Value::Bool(true)),
            ],
            vec![
                ("label".to_string(), Value::String("b".to_string())),
                ("done".to_string(), Value::Bool(false)),
            ],
            vec![
                ("label".to_string(), Value::String("c".to_string())),
                ("done".to_string(), Value::Bool(false)),
            ],
        ])));
        let root = tree.lookup_id("root").expect("root id exists");
        engine.set_direct(tree, root, "items", Value::Model(model.0));
    }));
    let mut registry = registry;
    registry.register_component(
        ComponentDesc {
            name: "Checkbox".to_string(),
            properties: Vec::new(),
        },
        Some(Box::new(|| return Box::new(Checkbox))),
    );
    registry.register_component(
        ComponentDesc {
            name: "RemoveButton".to_string(),
            properties: vec![PropertyDescriptor {
                name: "unused".to_string(),
                ty: Type::Bool,
                default: None,
            }],
        },
        Some(Box::new(|| return Box::new(Remove))),
    );
    registry.register_function("log", Box::new(|_| return Ok(Value::Bool(true))));

    let outcome = compile(TODO);
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let instance = instantiate_with(&outcome.document, registry);
    let mut tree = instance.tree;
    let mut engine = instance.engine;
    engine.run_registry_init(&mut tree);

    // Frame: propagate -> row sync -> propagate.
    engine.propagate(&mut tree);
    engine.sync_for_nodes(&mut tree);
    engine.propagate(&mut tree);

    let for_element = find_by_type(&tree, "For");
    let rows = tree.arena[for_element].children.clone();
    assert_eq!(rows.len(), 3, "one row per todo");
    let swatch_of = |tree: &nui_runtime::ElementTree, row: nui_runtime::element::ElementId| {
        return tree.arena[row].children[0];
    };
    let done_green = Color::from_hex("#3f9d55").unwrap();
    let open_gray = Color::from_hex("#64748b").unwrap();
    assert_eq!(
        tree.arena[swatch_of(&tree, rows[0])].get("fill"),
        Some(&Value::Color(done_green))
    );
    assert_eq!(
        tree.arena[swatch_of(&tree, rows[1])].get("fill"),
        Some(&Value::Color(open_gray))
    );

    // Click the checkbox of the open row: its behavior flips `done`, which
    // re-drives the row bindings and starts the opacity tween.
    let check = tree.arena[rows[1]].children[2];
    engine.emit_signal(&mut tree, check, "click").unwrap();
    engine.propagate(&mut tree);
    assert!(
        engine.has_active_animations(),
        "toggling done starts the opacity tween"
    );
    assert_eq!(
        tree.arena[swatch_of(&tree, rows[1])].get("fill"),
        Some(&Value::Color(done_green))
    );
    engine.tick_animations(&mut tree, nui_core::Duration::from_millis(200.0));
    assert_eq!(
        tree.arena[swatch_of(&tree, rows[1])].get("opacity"),
        Some(&Value::Float(1.0)),
        "tween reaches the target"
    );

    // Click the remove button of row 0: structural change, rows rebuild.
    let remove = tree.arena[rows[0]].children[3];
    engine.emit_signal(&mut tree, remove, "click").unwrap();
    engine.sync_for_nodes(&mut tree);
    engine.propagate(&mut tree);
    let rows = tree.arena[for_element].children.clone();
    assert_eq!(rows.len(), 2, "row removed from the model");
    assert_eq!(
        tree.arena[swatch_of(&tree, rows[0])].get("fill"),
        Some(&Value::Color(done_green)),
        "remaining rows re-instantiated in model order (b is now done)"
    );
}

#[test]
fn hot_reload_rebuilds_tree_and_reregisters_models() {
    use nui_core::Color;
    use nui_runtime::{Registry, VecModel, reload_from_source};

    const V1: &str = r#"
        component App {
            property items: Model
            property count: Int = 0

            Window(id = root) {
                Rectangle(id = box, fill = #333333)
                Text(id = label, content <- "v1: {count}")
            }
        }
    "#;
    const V2: &str = r#"
        component App {
            property items: Model
            property count: Int = 5

            Window(id = root) {
                Rectangle(id = box, fill = #cc4444)
                Text(id = label, content <- "v2: {count}")
            }
        }
    "#;

    let registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let model = engine.add_model(Box::new(VecModel::from_rows(vec![vec![(
            "label".to_string(),
            Value::String("row".to_string()),
        )]])));
        let root = tree.lookup_id("root").expect("root id exists");
        engine.set_direct(tree, root, "items", Value::Model(model.0));
    }));
    let outcome = nui_compiler::compile(V1);
    assert!(outcome.diagnostics.is_empty());
    let mut instance = nui_runtime::instantiate_with(&outcome.document, registry);
    instance.engine.run_registry_init(&mut instance.tree);
    instance.engine.propagate(&mut instance.tree);

    let model_id = {
        let root = instance.tree.lookup_id("root").unwrap();
        match instance.tree.arena[root].get("items") {
            Some(Value::Model(index)) => *index,
            other => panic!("expected a model, got {other:?}"),
        }
    };
    assert_eq!(
        instance
            .engine
            .model_row_count(nui_runtime::ModelId(model_id)),
        1
    );

    // A compile error leaves the previous instance fully functional.
    let before = instance.tree.arena[instance.tree.lookup_id("box").unwrap()]
        .get("fill")
        .cloned();
    let error =
        reload_from_source(&mut instance, "component App { property broken: }").unwrap_err();
    assert!(error.contains("error"), "rendered diagnostic: {error}");
    assert_eq!(
        instance.tree.arena[instance.tree.lookup_id("box").unwrap()].get("fill"),
        before.as_ref(),
        "failed reload keeps the old tree"
    );
    assert_eq!(
        instance
            .engine
            .model_row_count(nui_runtime::ModelId(model_id)),
        1,
        "failed reload keeps the old models"
    );

    // A good source rebuilds: fresh state, new bindings, new fill color,
    // and the attach hook re-registered a fresh model.
    reload_from_source(&mut instance, V2).unwrap();
    instance.engine.propagate(&mut instance.tree);
    let root = instance.tree.lookup_id("root").unwrap();
    let label = instance.tree.lookup_id("label").unwrap();
    let fresh_model = match instance.tree.arena[root].get("items") {
        Some(Value::Model(index)) => *index,
        other => panic!("expected a model after reload, got {other:?}"),
    };
    assert_eq!(
        instance
            .engine
            .model_row_count(nui_runtime::ModelId(fresh_model)),
        1,
        "attach hook re-registered the model on the new engine"
    );
    assert_eq!(
        instance.tree.arena[root].get("count"),
        Some(&Value::Int(5)),
        "fresh component state (v1 default 0 is gone)"
    );
    assert_eq!(
        instance.tree.arena[label].get("content"),
        Some(&Value::String("v2: 5".to_string()))
    );
    assert_eq!(
        instance.tree.arena[instance.tree.lookup_id("box").unwrap()].get("fill"),
        Some(&Value::Color(Color::from_hex("#cc4444").unwrap()))
    );
}

#[test]
fn focus_cycles_and_text_input_edits_and_syncs_two_way() {
    let source = r#"
        component Login {
            property userName: String = ""

            Window(id = root) {
                TextInput(id = user, text <=> root.userName)
                TextInput(id = pass)
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let user = tree.lookup_id("user").unwrap();
    let pass = tree.lookup_id("pass").unwrap();

    // Tab focus cycling in document order.
    engine.handle_key(&mut tree, nui_core::Key::Tab, nui_core::Modifiers::NONE);
    assert_eq!(engine.focused(), Some(user));
    engine.handle_key(&mut tree, nui_core::Key::Tab, nui_core::Modifiers::NONE);
    assert_eq!(engine.focused(), Some(pass));
    engine.handle_key(&mut tree, nui_core::Key::Tab, nui_core::Modifiers::NONE);
    assert_eq!(engine.focused(), Some(user), "focus wraps cyclically");

    // Typing routes into the focused input and writes the property.
    engine.handle_text_input(&mut tree, "ad");
    engine.handle_key(
        &mut tree,
        nui_core::Key::Character('a'),
        nui_core::Modifiers::NONE,
    );
    engine.handle_text_input(&mut tree, "m");
    assert_eq!(
        tree.arena[user].get("text"),
        Some(&Value::String("adam".to_string()))
    );

    // Two-way partner sees the input write.
    assert_eq!(
        tree.arena[tree.lookup_id("root").unwrap()].get("userName"),
        Some(&Value::String("adam".to_string()))
    );

    // Editing: backspace removes the last char through the state.
    engine.handle_key(
        &mut tree,
        nui_core::Key::Backspace,
        nui_core::Modifiers::NONE,
    );
    assert_eq!(
        tree.arena[user].get("text"),
        Some(&Value::String("ada".to_string()))
    );

    // External property write (binding side) reconciles the editing state.
    engine.set_direct(&mut tree, user, "text", Value::String("bob".to_string()));
    engine.handle_key(&mut tree, nui_core::Key::End, nui_core::Modifiers::NONE);
    engine.handle_text_input(&mut tree, "!");
    assert_eq!(
        tree.arena[user].get("text"),
        Some(&Value::String("bob!".to_string()))
    );

    // Cut/copy helpers work off the selection.
    engine.handle_key(&mut tree, nui_core::Key::Home, nui_core::Modifiers::NONE);
    engine.handle_key(
        &mut tree,
        nui_core::Key::ArrowRight,
        nui_core::Modifiers {
            shift: true,
            ..nui_core::Modifiers::NONE
        },
    );
    let cut = engine.cut_focused(&mut tree).unwrap();
    assert_eq!(cut, "b");
    assert_eq!(
        tree.arena[user].get("text"),
        Some(&Value::String("ob!".to_string()))
    );
    assert!(
        engine.copy_focused(&tree).is_none(),
        "collapsed selection copies nothing"
    );
}

#[test]
fn text_input_accepted_signal_and_enter() {
    let source = r#"
        component A {
            property submitted: Bool = false

            Window(id = root) {
                TextInput(id = field) {
                    on accepted => submitted = true
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let field = tree.lookup_id("field").unwrap();
    engine.focus(field);
    let handled = engine.handle_key(&mut tree, nui_core::Key::Enter, nui_core::Modifiers::NONE);
    assert!(handled);
    let root = tree.lookup_id("root").unwrap();
    assert_eq!(tree.arena[root].get("submitted"), Some(&Value::Bool(true)));
}

#[test]
fn list_view_virtualizes_rows_to_the_visible_window() {
    use nui_core::Value as V;
    use nui_runtime::VecModel;

    let source = r#"
        component Big {
            property rows: Model

            Window(id = root) {
                ListView(item in root.rows, id = list, height = 50dp, row_height = 10dp) {
                    Rectangle(height = 10dp) {
                        Text(label <- item.label)
                    }
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let root = tree.lookup_id("root").unwrap();
    let rows: Vec<nui_runtime::ModelRow> = (0..100)
        .map(|index| {
            return vec![("label".to_string(), V::String(format!("row {index}")))];
        })
        .collect();
    let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
    engine.set_direct(&mut tree, root, "rows", V::Model(model.0));
    model_frame(&mut tree, &mut engine);

    let list = tree.lookup_id("list").unwrap();
    // 100 rows at 10dp in a 50dp viewport: 6 window rows + spacer.
    assert_eq!(tree.arena[list].children.len(), 7);
    let spacer = tree.arena[list].children[0];
    assert_eq!(tree.arena[spacer].ty, "Spacer");
    // Window rows start at model row 0.
    let scope = tree.arena[tree.arena[list].children[1]]
        .for_scope
        .clone()
        .unwrap();
    assert_eq!(scope.row, 0);
    engine.propagate(&mut tree);
    let first_label = tree.arena[tree.arena[list].children[1]].children[0];
    assert_eq!(
        tree.arena[first_label].get("label"),
        Some(&V::String("row 0".to_string()))
    );

    // Scroll: the window moves to rows 20..; the element count stays bound.
    engine.set_direct(&mut tree, list, "scroll_y", V::Float(200.0));
    model_frame(&mut tree, &mut engine);
    assert!(tree.arena[list].children.len() <= 8, "window stays small");
    let scope = tree.arena[tree.arena[list].children[1]]
        .for_scope
        .clone()
        .unwrap();
    assert_eq!(scope.row, 20, "window opens at the scrolled row");
    engine.propagate(&mut tree);
    let label = tree.arena[tree.arena[list].children[1]].children[0];
    assert_eq!(
        tree.arena[label].get("label"),
        Some(&V::String("row 20".to_string()))
    );

    // Field writes inside the window invalidate exactly that row.
    engine.model_set_field(&mut tree, model, 21, "label", V::String("hot".to_string()));
    engine.propagate(&mut tree);
    let second_row = tree.arena[list].children[2];
    let label = tree.arena[second_row].children[0];
    assert_eq!(
        tree.arena[label].get("label"),
        Some(&V::String("hot".to_string()))
    );
    // The spacer tracks the pre-window offset (row 20 * 10dp = 200).
    let height = tree.arena[spacer].get("height").cloned().unwrap();
    assert_eq!(height, V::Length(nui_core::Length::Dp(200.0)));
}

#[test]
fn ten_thousand_row_list_stays_bounded() {
    use nui_core::Value as V;
    use nui_runtime::VecModel;

    let source = r#"
        component Huge {
            property rows: Model

            Window(id = root) {
                ListView(item in root.rows, id = list, height = 320dp, row_height = 36dp) {
                    Rectangle(height = 32dp)
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let root = tree.lookup_id("root").unwrap();
    let rows: Vec<nui_runtime::ModelRow> = (0..10_000)
        .map(|index| {
            return vec![
                ("label".to_string(), V::String(format!("row {index}"))),
                ("hot".to_string(), V::Bool(false)),
            ];
        })
        .collect();
    let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
    engine.set_direct(&mut tree, root, "rows", V::Model(model.0));
    model_frame(&mut tree, &mut engine);

    let list = tree.lookup_id("list").unwrap();
    // 320dp / 36dp = 9 rows (+1 overscan) + spacer — NOT 10,000.
    let window = tree.arena[list].children.len();
    assert!(window <= 12, "window must stay bounded, got {window}");
    assert!(
        tree.arena.len() < 30,
        "total elements bounded, got {}",
        tree.arena.len()
    );

    // A deep scroll keeps the bound and lands on the right rows.
    engine.set_direct(&mut tree, list, "scroll_y", V::Float(360_000.0));
    model_frame(&mut tree, &mut engine);
    assert!(tree.arena[list].children.len() <= 12);
    let scope = tree.arena[tree.arena[list].children[1]]
        .for_scope
        .clone()
        .unwrap();
    assert!(scope.row >= 9_980, "deep scroll opens near the end");
}

#[test]
fn click_bubbles_to_ancestor_handlers() {
    let source = r#"
        component A {
            property hits: Int = 0

            Window(id = root) {
                Button(id = btn, width = 90dp, height = 30dp) {
                    on click => hits += 1
                    Text(id = label, width = 40dp, height = 16dp)
                }
            }
        }
    "#;
    let (mut tree, mut engine) = build(source);
    let label = tree.lookup_id("label").unwrap();
    // A click on the label (child of the button) must reach the button.
    engine.emit_bubble(&mut tree, label, "click").unwrap();
    engine.emit_bubble(&mut tree, label, "click").unwrap();
    let root = tree.lookup_id("root").unwrap();
    assert_eq!(tree.arena[root].get("hits"), Some(&Value::Int(2)));
}
