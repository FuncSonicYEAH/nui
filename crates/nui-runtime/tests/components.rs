//! Component instantiation: a declared `component` used as a node type
//! expands into its own subtree, with each instance isolated from the
//! others.
//!
//! The hard part is not building the tree — it is that the runtime resolves
//! names through one flat id table and sends bare property reads to the
//! document's first root. Both are single-instance assumptions, so every
//! test here is really asking the same question from a different angle:
//! does a second instance stay independent of the first?
#![allow(clippy::unwrap_used)]

use nui_compiler::compile;
use nui_core::Value;
use nui_runtime::{ElementTree, Engine, instantiate};

/// Test error type for `Result`-returning tests.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Compiles a document, asserting it is clean, and instantiates it.
fn build(source: &str) -> (ElementTree, Engine) {
    let outcome = compile(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "test source must compile cleanly: {:?}",
        outcome.diagnostics
    );
    let instance = instantiate(&outcome.document);
    return (instance.tree, instance.engine);
}

/// Compiles a document expected to fail, returning the rendered diagnostics.
fn diagnostics_of(source: &str) -> String {
    let outcome = compile(source);
    assert!(
        !outcome.diagnostics.is_empty(),
        "this source is expected to produce a diagnostic"
    );
    return outcome
        .diagnostics
        .iter()
        .map(|diagnostic| return nui_syntax::render_diagnostic(source, "test.nui", diagnostic))
        .collect::<Vec<_>>()
        .join("");
}

/// A library component plus an entry component that instantiates it twice.
const TWO_CHIPS: &str = r#"
    component Chip {
        property label: String = "unnamed"
        property tint: Color = #336699
        signal picked

        Rectangle(id = chip_box, width = 80dp, height = 30dp, fill <- tint) {
            Text(id = chip_label, content <- label, width = 80dp, height = 30dp)
            on click => emit picked
        }
    }

    component App {
        property picks: Int = 0
        property tint: Color = #ff8800

        Window(id = root) {
            Column {
                Chip(id = first, label = "one", tint <- root.tint) {
                    on picked => root.picks += 1
                }
                Chip(id = second, label = "two") {
                    on picked => root.picks += 10
                }
            }
        }
    }
"#;

#[test]
fn a_component_reference_expands_into_its_subtree() -> TestResult {
    let (tree, _) = build(TWO_CHIPS);
    let first = tree
        .lookup_id("first")
        .expect("the call site's id resolves");
    // The instance element *is* the component's root, not a wrapper.
    assert_eq!(tree.arena[first].ty, "Rectangle");
    assert_eq!(tree.arena[first].component.as_deref(), Some("Chip"));
    // ...so the component's own child is a real child of it.
    assert_eq!(tree.arena[first].children.len(), 1);
    let label = tree.arena[first].children[0];
    assert_eq!(tree.arena[label].ty, "Text");
    return Ok(());
}

#[test]
fn a_library_component_produces_no_root_of_its_own() {
    let (tree, _) = build(TWO_CHIPS);
    // Exactly one root: the entry component's `Window`. Before component
    // instantiation, a declared component was instantiated too and painted
    // itself at the origin.
    assert_eq!(tree.roots.len(), 1);
    assert_eq!(tree.arena[tree.roots[0]].ty, "Window");
}

#[test]
fn two_instances_keep_separate_ids() {
    let (tree, _) = build(TWO_CHIPS);
    let first = tree.lookup_id("first").expect("first instance");
    let second = tree.lookup_id("second").expect("second instance");
    // Both declare `id = chip_label` inside the component; the two must not
    // land on one element.
    let first_label = tree.arena[first].children[0];
    let second_label = tree.arena[second].children[0];
    assert_ne!(first_label, second_label);
    // Each is reachable under its own prefixed name, and the component's
    // internal references were rewritten to match.
    assert_ne!(
        tree.arena[first_label].id.as_deref(),
        tree.arena[second_label].id.as_deref(),
    );
}

#[test]
fn each_instance_resolves_its_own_properties() -> TestResult {
    let (mut tree, mut engine) = build(TWO_CHIPS);
    engine.propagate(&mut tree);
    let first = tree.lookup_id("first").expect("first instance");
    let second = tree.lookup_id("second").expect("second instance");
    let first_label = tree.arena[first].children[0];
    let second_label = tree.arena[second].children[0];
    // The argument wins on one, the declaration's default on the other.
    assert_eq!(
        tree.arena[first_label].get("content"),
        Some(&Value::String("one".to_string()))
    );
    assert_eq!(
        tree.arena[second_label].get("content"),
        Some(&Value::String("two".to_string()))
    );
    // The `<-` argument re-drives only the instance it was written on.
    assert_eq!(
        tree.arena[first].get("tint"),
        Some(&Value::Color(nui_core::Color::from_hex("#ff8800").unwrap()))
    );
    assert_eq!(
        tree.arena[second].get("tint"),
        Some(&Value::Color(nui_core::Color::from_hex("#336699").unwrap()))
    );
    return Ok(());
}

#[test]
fn a_reactive_argument_follows_the_callers_property() -> TestResult {
    let (mut tree, mut engine) = build(TWO_CHIPS);
    engine.propagate(&mut tree);
    let first = tree.lookup_id("first").expect("first instance");
    let root = tree.lookup_id("root").expect("root");
    assert_eq!(
        tree.arena[first].get("tint"),
        Some(&Value::Color(nui_core::Color::from_hex("#ff8800").unwrap())),
        "the binding runs before the manual write below"
    );
    // Change the caller's property; only the instance that bound to it moves.
    engine.set_direct(
        &mut tree,
        root,
        "tint",
        Value::Color(nui_core::Color::from_hex("#00ff00").unwrap()),
    );
    engine.propagate(&mut tree);
    let second = tree.lookup_id("second").expect("second instance");
    assert_eq!(
        tree.arena[first].get("tint"),
        Some(&Value::Color(nui_core::Color::from_hex("#00ff00").unwrap()))
    );
    assert_eq!(
        tree.arena[second].get("tint"),
        Some(&Value::Color(nui_core::Color::from_hex("#336699").unwrap())),
        "the untouched instance keeps its own default"
    );
    return Ok(());
}

#[test]
fn a_components_signal_reaches_its_own_call_site() -> TestResult {
    let (mut tree, mut engine) = build(TWO_CHIPS);
    engine.propagate(&mut tree);
    let root = tree.lookup_id("root").expect("root");
    let first = tree.lookup_id("first").expect("first instance");
    let second = tree.lookup_id("second").expect("second instance");
    // A click on the *label inside* the component bubbles to the component's
    // own `on click`, which emits the component's signal — and that has to
    // land on the instance's call site, not on the label.
    let first_label = tree.arena[first].children[0];
    engine.emit_bubble(&mut tree, first_label, "click")?;
    engine.propagate(&mut tree);
    assert_eq!(tree.arena[root].get("picks"), Some(&Value::Int(1)));
    let second_label = tree.arena[second].children[0];
    engine.emit_bubble(&mut tree, second_label, "click")?;
    engine.propagate(&mut tree);
    assert_eq!(tree.arena[root].get("picks"), Some(&Value::Int(11)));
    return Ok(());
}

#[test]
fn an_instances_inner_references_read_its_own_state() -> TestResult {
    // The component's `count <- ...` and `Text(content <- count)` are both
    // implicit references to "this component's property". With one instance
    // that resolves to the first root; with two it has to resolve per
    // instance, or both labels show the same number.
    let (mut tree, mut engine) = build(
        r#"
        component Counter {
            property count: Int = 0
            Text(id = readout, content <- "{count}", width = 40dp, height = 20dp)
        }
        component App {
            Window(id = root) {
                Column {
                    Counter(id = a)
                    Counter(id = b, count = 100)
                }
            }
        }
    "#,
    );
    engine.propagate(&mut tree);
    let a = tree.lookup_id("a").expect("instance a");
    let b = tree.lookup_id("b").expect("instance b");
    // The instance element *is* the component's `Text` root, so its own
    // `content` is what the test reads.
    assert_eq!(
        tree.arena[a].get("content"),
        Some(&Value::String("0".to_string()))
    );
    assert_eq!(
        tree.arena[b].get("content"),
        Some(&Value::String("100".to_string())),
        "b's own argument, not a's default"
    );
    // Writing one instance's property must not touch the other.
    engine.set_direct(&mut tree, a, "count", Value::Int(7));
    engine.propagate(&mut tree);
    assert_eq!(
        tree.arena[a].get("content"),
        Some(&Value::String("7".to_string()))
    );
    assert_eq!(
        tree.arena[b].get("content"),
        Some(&Value::String("100".to_string()))
    );
    return Ok(());
}

#[test]
fn a_component_state_machine_is_per_instance() -> TestResult {
    let (mut tree, mut engine) = build(
        r#"
        component Gate {
            property open: Bool = false
            property label: String = "shut"
            signal toggle
            machine mode {
                state shut
                state open_state {
                    enter => label = "open"
                }
                on toggle from shut => open_state
                on toggle from open_state => shut
            }
            Text(id = readout, content <- label, width = 40dp, height = 20dp)
        }
        component App {
            property flips: Int = 0
            Window(id = root) {
                Column {
                    Gate(id = a) { on toggle => root.flips += 1 }
                    Gate(id = b) { on toggle => root.flips += 100 }
                }
            }
        }
    "#,
    );
    engine.propagate(&mut tree);
    let a = tree.lookup_id("a").expect("instance a");
    let b = tree.lookup_id("b").expect("instance b");
    // Firing on `a` must move only `a`'s machine, and `a`'s `enter` effect
    // must write `a`'s own `label`.
    engine.emit_signal(&mut tree, a, "toggle")?;
    engine.propagate(&mut tree);
    assert_eq!(
        tree.arena[a].get("content"),
        Some(&Value::String("open".to_string()))
    );
    assert_eq!(
        tree.arena[b].get("content"),
        Some(&Value::String("shut".to_string())),
        "b's machine never fired"
    );
    // ...and the call-site handler ran once, for `a` only.
    let root = tree.lookup_id("root").expect("root");
    assert_eq!(tree.arena[root].get("flips"), Some(&Value::Int(1)));
    // A second machine, `b`, is still in its first state and still toggles.
    engine.emit_signal(&mut tree, b, "toggle")?;
    engine.propagate(&mut tree);
    assert_eq!(tree.arena[root].get("flips"), Some(&Value::Int(101)));
    return Ok(());
}

#[test]
fn a_component_can_instantiate_another_component() -> TestResult {
    // Nesting: the inner component's ids are prefixed by the *outer*
    // instance, so two `Card`s do not share their inner labels.
    let (mut tree, mut engine) = build(
        r#"
        component Badge {
            property text: String = "0"
            Text(id = badge_text, content <- text, width = 20dp, height = 20dp)
        }
        component Card {
            property title: String = "untitled"
            Rectangle(id = card_box, width = 100dp, height = 40dp) {
                Text(id = card_title, content <- title, width = 100dp, height = 20dp)
                Badge(id = badge, text <- title)
            }
        }
        component App {
            Window(id = root) {
                Column {
                    Card(id = one, title = "first")
                    Card(id = two, title = "second")
                }
            }
        }
    "#,
    );
    engine.propagate(&mut tree);
    let one = tree.lookup_id("one").expect("first card");
    let two = tree.lookup_id("two").expect("second card");
    // The instance element is the card's `Rectangle`, whose first child is
    // the title text and whose second is the nested `Badge` instance.
    let one_title = tree.arena[one].children[0];
    let two_title = tree.arena[two].children[0];
    assert_eq!(
        tree.arena[one_title].get("content"),
        Some(&Value::String("first".to_string()))
    );
    assert_eq!(
        tree.arena[two_title].get("content"),
        Some(&Value::String("second".to_string()))
    );
    // The nested `Badge` received its text from the *enclosing* card's
    // property, which is a reference that had to be rewritten to point at
    // the right card instance.
    let one_badge = tree.arena[one].children[1];
    let two_badge = tree.arena[two].children[1];
    assert_eq!(
        tree.arena[one_badge].get("content"),
        Some(&Value::String("first".to_string()))
    );
    assert_eq!(
        tree.arena[two_badge].get("content"),
        Some(&Value::String("second".to_string()))
    );
    assert_ne!(one_badge, two_badge, "one nested badge per card");
    return Ok(());
}

#[test]
fn a_component_usable_as_an_entry_point_still_works() {
    // A document that declares one component and never references it is
    // exactly the pre-existing single-component case.
    let (tree, _) = build("component Solo { Window(id = root) { Text(content = \"hi\") } }");
    assert_eq!(tree.roots.len(), 1);
    let root = tree.lookup_id("root").expect("root");
    assert_eq!(tree.arena[root].children.len(), 1);
}

#[test]
fn an_element_property_argument_reaches_the_instances_root() -> TestResult {
    // A name the component does not declare is an ordinary element property
    // of the instance's root element.
    let (mut tree, mut engine) = build(
        r#"
        component Chip {
            Rectangle(id = box, width = 40dp, height = 20dp)
        }
        component App {
            Window(id = root) {
                Column {
                    Chip(id = a, fill = #112233, radius = 4dp)
                    Chip(id = b)
                }
            }
        }
    "#,
    );
    engine.propagate(&mut tree);
    let a = tree.lookup_id("a").expect("a");
    let b = tree.lookup_id("b").expect("b");
    assert_eq!(
        tree.arena[a].get("fill"),
        Some(&Value::Color(nui_core::Color::from_hex("#112233").unwrap()))
    );
    assert_eq!(tree.arena[b].get("fill"), None, "b was not given one");
    assert_eq!(
        tree.arena[a].get("radius"),
        Some(&Value::Length(nui_core::Length::Dp(4.0)))
    );
    return Ok(());
}

#[test]
fn a_static_argument_replaces_the_components_own_binding() -> TestResult {
    // The component binds `fill` to its own property. A call-site `fill =`
    // is a final value, so the binding must go — otherwise the next
    // propagation pass writes the component's value back over it.
    let (mut tree, mut engine) = build(
        r#"
        component Chip {
            property tint: Color = #336699
            Rectangle(id = box, fill <- tint, width = 40dp, height = 20dp)
        }
        component App {
            property tint: Color = #ff0000
            Window(id = root) {
                Column {
                    Chip(id = a, fill = #00ff00)
                    Chip(id = b)
                }
            }
        }
    "#,
    );
    engine.propagate(&mut tree);
    let a = tree.lookup_id("a").expect("a");
    let b = tree.lookup_id("b").expect("b");
    assert_eq!(
        tree.arena[a].get("fill"),
        Some(&Value::Color(nui_core::Color::from_hex("#00ff00").unwrap()))
    );
    assert_eq!(
        tree.arena[b].get("fill"),
        Some(&Value::Color(nui_core::Color::from_hex("#336699").unwrap()))
    );
    return Ok(());
}

// -- diagnostics ------------------------------------------------------------

#[test]
fn self_instantiation_is_a_compile_error() {
    let rendered = diagnostics_of("component Loop { Loop() }");
    assert!(rendered.contains("instantiates itself"), "{rendered}");
}

#[test]
fn mutual_recursion_is_a_compile_error() {
    let rendered = diagnostics_of(
        r#"
        component A { B() }
        component B { A() }
    "#,
    );
    assert!(rendered.contains("instantiates itself"), "{rendered}");
    // The chain is reported, not just the back edge.
    assert!(
        rendered.contains("A -> B -> A") || rendered.contains("B -> A -> B"),
        "{rendered}"
    );
}

#[test]
fn a_recursive_chain_that_terminates_is_fine() {
    // Depth-limited reuse is not recursion: a component may instantiate
    // another that instantiates a third, and the cycle check must not
    // complain.
    let (tree, _) = build(
        r#"
        component Leaf { Text(id = text, content = "leaf") }
        component Middle { Leaf() }
        component Top { Middle() }
        component App { Window(id = root) { Top() } }
    "#,
    );
    let top = tree.lookup_id("root").expect("root");
    assert_eq!(tree.arena[top].children.len(), 1);
}

#[test]
fn an_instantiated_component_needs_exactly_one_root() {
    let rendered = diagnostics_of(
        r#"
        component Two { Text(content = "a") Text(content = "b") }
        component App { Window(id = root) { Two() } }
    "#,
    );
    assert!(rendered.contains("exactly one root node"), "{rendered}");
    assert!(rendered.contains("found 2"), "{rendered}");
}

#[test]
fn an_unreferenced_component_may_declare_several_roots() {
    // Only *instantiated* components are constrained; a document entry
    // point with several roots still becomes several tree roots.
    let (tree, _) = build(
        r#"
        component App {
            Window(id = root) { Text(content = "a") }
            Window(id = second) { Text(content = "b") }
        }
    "#,
    );
    assert_eq!(tree.roots.len(), 2);
}

#[test]
fn an_unknown_signal_on_a_component_is_an_error() {
    let rendered = diagnostics_of(
        r#"
        component Chip { signal picked Rectangle(id = box) {} }
        component App { Window(id = root) { Chip(id = a) { on nope => x = 1 } } }
    "#,
    );
    assert!(rendered.contains("has no signal `nope`"), "{rendered}");
    // The message lists what it does have.
    assert!(rendered.contains("picked"), "{rendered}");
}

#[test]
fn a_component_property_argument_is_type_checked() {
    let rendered = diagnostics_of(
        r#"
        component Chip { property count: Int = 0 Rectangle(id = box) {} }
        component App { Window(id = root) { Chip(id = a, count = "text") } }
    "#,
    );
    assert!(rendered.contains("type mismatch"), "{rendered}");
    assert!(rendered.contains("expected Int"), "{rendered}");
}

#[test]
fn a_component_reference_takes_no_child_nodes() {
    let rendered = diagnostics_of(
        r#"
        component Chip { Rectangle(id = box) {} }
        component App { Window(id = root) { Chip(id = a) { Text(content = "x") } } }
    "#,
    );
    assert!(rendered.contains("takes no child nodes"), "{rendered}");
}

#[test]
fn a_component_reference_takes_no_body_assignments() {
    let rendered = diagnostics_of(
        r#"
        component Chip { Rectangle(id = box) {} }
        component App { Window(id = root) { Chip(id = a) { fill = #ffffff } } }
    "#,
    );
    assert!(
        rendered.contains("takes no property assignments"),
        "{rendered}"
    );
}

#[test]
fn a_component_reference_takes_no_when_blocks() {
    let rendered = diagnostics_of(
        r#"
        component Chip { Rectangle(id = box) {} }
        component App { Window(id = root) { Chip(id = a) { when true { fill = #ffffff } } } }
    "#,
    );
    assert!(rendered.contains("takes no `when` blocks"), "{rendered}");
}

#[test]
fn a_duplicate_component_declaration_is_an_error() {
    let rendered = diagnostics_of(
        r#"
        component Chip { Rectangle(id = box) {} }
        component Chip { Rectangle(id = box) {} }
    "#,
    );
    assert!(
        rendered.contains("duplicate component declaration"),
        "{rendered}"
    );
}

#[test]
fn a_component_cannot_be_instantiated_inside_a_for_body() {
    let rendered = diagnostics_of(
        r#"
        component Chip { Rectangle(id = box) {} }
        component App {
            property rows: Model
            Window(id = root) { For(item in root.rows) { Chip(id = a) } }
        }
    "#,
    );
    assert!(
        rendered.contains("cannot be instantiated inside a `For` body"),
        "{rendered}"
    );
}

#[test]
fn an_ordinary_element_type_is_not_a_component_reference() {
    // The regression this whole feature risks: a node type that merely
    // *looks* like it could be a component must still be an element.
    let (tree, _) = build(
        r#"
        component App { Window(id = root) { Rectangle(id = box, fill = #123456) } }
    "#,
    );
    let box_element = tree.lookup_id("box").expect("the rect exists");
    assert_eq!(tree.arena[box_element].ty, "Rectangle");
    assert_eq!(tree.arena[box_element].component, None);
}

#[test]
fn a_component_id_does_not_leak_into_the_callers_scope() {
    // The component declares `id = chip_label`; the caller must not see it,
    // or a caller's own `chip_label` would be reported as a duplicate.
    let (tree, _) = build(
        r#"
        component Chip { Text(id = chip_label, content = "x") }
        component App {
            Window(id = root) {
                Column {
                    Chip(id = a)
                    Text(id = chip_label, content = "mine")
                }
            }
        }
    "#,
    );
    let mine = tree
        .lookup_id("chip_label")
        .expect("the caller's own id wins");
    assert_eq!(
        tree.arena[mine].get("content"),
        Some(&Value::String("mine".to_string()))
    );
}
