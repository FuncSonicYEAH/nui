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

/// The slot: a component declares a `Slot`, a call site fills it.
///
/// These are the properties the feature is *allowed* to have, written down
/// next to the implementation so a later change is caught here rather than
/// discovered by a component that quietly renders nothing.
mod slot {
    use super::*;
    use nui_runtime::element::{ElementId, SLOT as SLOT_ID};

    /// Content lands inside the component, in the slot's place.
    ///
    /// The load-bearing case: the point of a slot is that the caller decides
    /// *what* is in the middle of someone else's layout, and the component still
    /// owns everything around it.
    #[test]
    fn content_lands_in_the_slot() {
        let (tree, _engine) = build(
            r#"
            component Frame {
                Stack(id = self, width = 200dp, height = 200dp) {
                    Rectangle(id = head, width = 200dp, height = 40dp)
                    Slot { }
                }
            }
            component App {
                Window(id = root, width = 400dp, height = 400dp) {
                    Frame { Rectangle(id = body, width = 200dp, height = 80dp) }
                }
            }
            "#,
        );
        let root = tree.lookup_id("root").expect("the window");
        let slot = tree
            .descendants_of_type(root, SLOT_ID)
            .first()
            .copied()
            .expect("a slot element");
        let body = tree.lookup_id("body").expect("the slotted content");
        assert_eq!(
            tree.arena[body].parent,
            Some(slot),
            "the content is a child of the slot, not a sibling of it"
        );
    }

    /// The content keeps the *caller's* scope, not the component's.
    ///
    /// This is what makes a slot usable rather than merely possible: a
    /// component's internals are namespaced per instance, so a slotted node
    /// that came in already-namespaced would be renamed twice and its `id`
    /// would be unreachable from the document that wrote it.
    #[test]
    fn content_is_in_the_callers_scope() {
        let (tree, _engine) = build(
            r#"
            component Frame { Column(id = self) { Slot { } } }
            component App {
                property dark: Bool = true
                Window(id = root, width = 400dp, height = 400dp) {
                    Frame { Rectangle(id = mine, fill = #ff0000) }
                }
            }
            "#,
        );
        let mine = tree
            .lookup_id("mine")
            .expect("the content's id is reachable");
        // Not `i1::mine`: the caller's name, unprefixed.
        assert!(
            tree.lookup_id("i1::mine").is_none(),
            "the content must not be namespaced with the instance"
        );
        assert_eq!(tree.arena[mine].ty, "Rectangle");
    }

    /// Two instances of a slotted component each get their own content.
    ///
    /// The bug this catches is a slot filled from the *first* instance's
    /// subtree, which would put instance one's content in instance two.
    #[test]
    fn each_instance_gets_its_own_content() {
        let (tree, _engine) = build(
            r#"
            component Frame { Column(id = self) { Slot { } } }
            component App {
                Window(id = root, width = 400dp, height = 400dp) {
                    Row {
                        Frame { Rectangle(id = one) }
                        Frame { Rectangle(id = two) }
                    }
                }
            }
            "#,
        );
        let one = tree.lookup_id("one").expect("first content");
        let two = tree.lookup_id("two").expect("second content");
        let slot_of = |content: ElementId| -> Option<ElementId> {
            let mut parent = tree.arena[content].parent;
            while let Some(id) = parent {
                if tree.arena[id].ty == SLOT_ID {
                    return Some(id);
                }
                parent = tree.arena[id].parent;
            }
            return None;
        };
        assert_ne!(
            slot_of(one),
            slot_of(two),
            "both instances put their content in one slot"
        );
    }

    /// Content with no slot to go into is an error, not a silent drop.
    #[test]
    fn content_without_a_slot_is_reported() {
        let rendered = diagnostics_of(
            r#"
            component Chip { Rectangle(id = box) {} }
            component App {
                Window(id = root) { Chip { Text(content = "x") } }
            }
        "#,
        );
        assert!(rendered.contains("declares no `Slot`"), "{rendered}");
    }

    /// Two slots in one component is an error, because "which one?" has no
    /// good answer and guessing one is worse than refusing.
    #[test]
    fn two_slots_are_reported() {
        let rendered = diagnostics_of(
            r#"
            component Frame {
                Stack(id = self) { Slot { } Slot { } }
            }
            component App { Window(id = root) { Frame { Rectangle(id = x) } } }
        "#,
        );
        assert!(rendered.contains("more than one `Slot`"), "{rendered}");
    }

    /// A slot left empty is legal: an optional body is a real thing, and
    /// refusing it would force every caller to pass a `Rectangle(width = 1dp)`.
    #[test]
    fn an_unfilled_slot_is_legal() {
        let (tree, _engine) = build(
            r#"
            component Frame { Column(id = self) { Slot { } } }
            component App { Window(id = root) { Frame() } }
        "#,
        );
        let root = tree.lookup_id("root").expect("the window");
        assert_eq!(tree.arena[root].ty, "Window");
    }

    /// The slot is a layout box, not an invisible gap.
    ///
    /// Asserted through the tree rather than by reading positions: what matters
    /// here is that a slot holds its content, and the geometry belongs to
    /// `nui-layout`'s own tests. A slot that were not a container would give
    /// its content no box at all.
    #[test]
    fn a_slot_is_a_layout_container() {
        let (tree, _engine) = build(
            r#"
            component Frame {
                Stack(id = self, width = 200dp, height = 200dp) { Slot { } }
            }
            component App {
                Window(id = root, width = 400dp, height = 400dp) {
                    Frame { Rectangle(id = body, width = 60dp, height = 30dp) }
                }
            }
        "#,
        );
        let root = tree.lookup_id("root").expect("the window");
        let slot = tree
            .descendants_of_type(root, SLOT_ID)
            .first()
            .copied()
            .expect("the slot survived instantiation");
        assert!(
            !tree.arena[slot].children.is_empty(),
            "the slot holds its content"
        );
    }
}

/// `component Child extends Parent`: the checker flattens the parent into the
/// child at compile time, so these tests exist to answer one question — does
/// the *runtime* need to know? It does not, and that is the point: no
/// `mangle.rs` or `instantiate.rs` change was needed for inheritance to work,
/// which these tests pin down.
///
/// If the flattening ever stops happening, the failure mode is quiet: the
/// parent's ids would miss the instance namespace and the first instance's
/// `chip_box` would collide with the second's.
mod extends {
    use super::*;
    use nui_runtime::element::SLOT as SLOT_ID;

    /// The inherited body is built as if the child had written it itself.
    #[test]
    fn a_derived_component_builds_the_inherited_tree() {
        let (tree, _engine) = build(
            r#"
            component Chip {
                property label: String = "unnamed"
                Rectangle(id = chip_box, width = 80dp, height = 30dp) {
                    Text(id = chip_label, content <- label)
                }
            }
            component Badge extends Chip {
                property tone: Color = #ff0000
            }
            component App { Window(id = root) { Badge(id = a) } }
            "#,
        );
        // The ids come from the parent, but they belong to the *derived*
        // component's instance, so they carry that instance's namespace.
        assert!(
            tree.lookup_id("i1::chip_box").is_some(),
            "the parent's box is namespaced to the derived instance"
        );
        assert!(
            tree.lookup_id("i1::chip_label").is_some(),
            "the parent's label is namespaced to the derived instance"
        );
    }

    /// A derived body appends into the slot the parent left open.
    #[test]
    fn a_derived_body_lands_in_the_inherited_slot() {
        let (tree, _engine) = build(
            r#"
            component Card {
                Stack(id = self, width = 200dp, height = 200dp) {
                    Rectangle(id = head, width = 200dp, height = 40dp)
                    Slot { }
                }
            }
            component Tinted extends Card {
                Rectangle(id = tail, width = 200dp, height = 20dp)
            }
            component App { Window(id = root) { Tinted(id = a) } }
            "#,
        );
        let root = tree.lookup_id("root").expect("the window");
        let slot = tree
            .descendants_of_type(root, SLOT_ID)
            .first()
            .copied()
            .expect("the inherited slot survived");
        let tail = tree.lookup_id("i1::tail").expect("the derived body node");
        assert_eq!(
            tree.arena[tail].parent,
            Some(slot),
            "the derived body is a child of the inherited slot"
        );
        // The parent's own head is still there, at the top of the stack.
        let head = tree.lookup_id("i1::head").expect("the inherited head");
        assert_ne!(
            tree.arena[head].parent,
            Some(slot),
            "the inherited head stays where the parent put it"
        );
    }

    /// Two instances of a derived component stay independent.
    ///
    /// This is the case the compile-time flattening exists to get right: the
    /// parent's ids must be in the derived component's `own` set, or both
    /// instances would share one `chip_box`.
    #[test]
    fn two_instances_of_a_derived_component_are_independent() {
        let (mut tree, mut engine) = build(
            r#"
            component Chip {
                property label: String = "unnamed"
                Rectangle(id = chip_box, width = 80dp, height = 30dp) {
                    Text(id = chip_label, content <- label, width = 80dp, height = 30dp)
                }
            }
            component Badge extends Chip { property tone: Color = #ff0000 }
            component App {
                Window(id = root) {
                    Row {
                        Badge(id = one, label = "first")
                        Badge(id = two, label = "second")
                    }
                }
            }
            "#,
        );
        engine.propagate(&mut tree);
        let one_label = tree.lookup_id("i1::chip_label").expect("first label");
        let two_label = tree.lookup_id("i2::chip_label").expect("second label");
        assert_ne!(one_label, two_label, "each instance has its own label");
        assert_eq!(
            tree.arena[one_label].get("content"),
            Some(&Value::String("first".to_string()))
        );
        assert_eq!(
            tree.arena[two_label].get("content"),
            Some(&Value::String("second".to_string()))
        );
    }

    /// An overridden default reaches the inherited binding.
    #[test]
    fn an_overridden_property_feeds_the_inherited_tree() {
        let (mut tree, mut engine) = build(
            r#"
            component Chip {
                property label: String = "unnamed"
                Text(id = chip_label, content <- label)
            }
            component Badge extends Chip {
                property label: String = "badge"
            }
            component App { Window(id = root) { Badge(id = a) } }
            "#,
        );
        engine.propagate(&mut tree);
        let label = tree
            .lookup_id("i1::chip_label")
            .expect("the inherited label");
        assert_eq!(
            tree.arena[label].get("content"),
            Some(&Value::String("badge".to_string())),
            "the derived default wins over the parent's"
        );
    }

    /// A three-level chain flattens all the way down.
    #[test]
    fn a_three_level_chain_instantiates() {
        let (mut tree, mut engine) = build(
            r#"
            component A {
                property tone: Color = #111111
                Rectangle(id = box, fill <- tone) { }
            }
            component B extends A { property tone: Color = #222222 }
            component C extends B { property tone: Color = #333333 }
            component App { Window(id = root) { C(id = a) } }
            "#,
        );
        engine.propagate(&mut tree);
        let box_element = tree.lookup_id("i1::box").expect("the inherited box");
        assert_eq!(
            tree.arena[box_element].get("fill"),
            Some(&Value::Color(nui_core::Color::from_rgb8(0x33, 0x33, 0x33))),
            "the deepest override wins"
        );
    }

    /// A derived component's element is the built-in it extends.
    ///
    /// The whole point of `extends Button`: the instance is a `Button` to
    /// the widget layer, so hover, press and keyboard activation all
    /// arrive without the document re-declaring them.
    #[test]
    fn a_derived_builtin_instances_as_that_builtin() {
        let (tree, _engine) = build(
            r#"
            component Fancy extends Button {
                property label: String = "Save"
            }
            component App { Window(id = root) { Fancy(id = save) } }
            "#,
        );
        let save = tree.lookup_id("save").expect("the instance");
        assert_eq!(
            tree.arena[save].ty, "Button",
            "the instance element is the built-in, not a wrapper"
        );
        assert_eq!(
            tree.arena[save].component.as_deref(),
            Some("Fancy"),
            "and it still knows which component it came from"
        );
    }

    /// The inherited properties reach the element, defaults included.
    #[test]
    fn inherited_builtin_properties_are_applied() {
        let (mut tree, mut engine) = build(
            r#"
            component Fancy extends Button {
                property label: String = "Save"
                property variant: Enum = danger
            }
            component App { Window(id = root) { Fancy(id = save) } }
            "#,
        );
        engine.propagate(&mut tree);
        let save = tree.lookup_id("save").expect("the instance");
        assert_eq!(
            tree.arena[save].get("label"),
            Some(&Value::String("Save".to_string())),
            "the derived default reaches the element"
        );
        assert_eq!(
            tree.arena[save].get("variant"),
            Some(&Value::Enum("danger".to_string())),
            "an overridden enum default reaches it too"
        );
    }

    /// A call-site argument overrides the derived default.
    #[test]
    fn a_call_site_argument_overrides_the_derived_default() {
        let (mut tree, mut engine) = build(
            r#"
            component Fancy extends Button {
                property label: String = "Save"
            }
            component App {
                Window(id = root) {
                    Fancy(id = one)
                    Fancy(id = two, label = "Cancel")
                }
            }
            "#,
        );
        engine.propagate(&mut tree);
        let one = tree.lookup_id("one").expect("first");
        let two = tree.lookup_id("two").expect("second");
        assert_eq!(
            tree.arena[one].get("label"),
            Some(&Value::String("Save".to_string()))
        );
        assert_eq!(
            tree.arena[two].get("label"),
            Some(&Value::String("Cancel".to_string())),
            "the argument wins on the instance it was written on"
        );
    }

    /// Two instances of a derived built-in stay independent.
    ///
    /// The same silent-failure shape as the plain-component case: it is
    /// settled by the `iN::` namespace, which the flattened IR makes
    /// correct without the runtime knowing about `extends`.
    #[test]
    fn two_instances_of_a_derived_builtin_are_independent() {
        let (mut tree, mut engine) = build(
            r#"
            component Fancy extends Button {
                property label: String = "Save"
            }
            component App {
                Window(id = root) {
                    Row {
                        Fancy(id = one, label = "first")
                        Fancy(id = two, label = "second")
                    }
                }
            }
            "#,
        );
        engine.propagate(&mut tree);
        let one = tree.lookup_id("one").expect("first");
        let two = tree.lookup_id("two").expect("second");
        assert_eq!(
            tree.arena[one].get("label"),
            Some(&Value::String("first".to_string()))
        );
        assert_eq!(
            tree.arena[two].get("label"),
            Some(&Value::String("second".to_string()))
        );
    }

    /// A chain works when it bottoms out in a built-in.
    ///
    /// `Bold extends Fancy extends Button`: the middle link is an ordinary
    /// component, the last is a built-in, and the two mechanisms have to
    /// compose rather than each handling only its own end.
    #[test]
    fn a_chain_through_a_component_to_a_builtin_works() {
        let (mut tree, mut engine) = build(
            r#"
            component Fancy extends Button {
                property label: String = "Save"
                property variant: Enum = ghost
            }
            component Bold extends Fancy {
                property label: String = "Delete"
            }
            component App { Window(id = root) { Bold(id = a) } }
            "#,
        );
        engine.propagate(&mut tree);
        let a = tree.lookup_id("a").expect("the instance");
        assert_eq!(tree.arena[a].ty, "Button", "still a Button at the root");
        assert_eq!(
            tree.arena[a].get("label"),
            Some(&Value::String("Delete".to_string())),
            "the nearest override wins"
        );
        assert_eq!(
            tree.arena[a].get("variant"),
            Some(&Value::Enum("ghost".to_string())),
            "the middle link's override survives"
        );
    }

    /// Body nodes under a built-in parent are refused, not dropped.
    ///
    /// A built-in draws its own chrome and declares no `Slot`, so there is
    /// nowhere for content to go. Silently ignoring it would render a
    /// control missing whatever the author wrote, which is the failure D26
    /// exists to prevent.
    #[test]
    fn a_body_under_a_builtin_parent_is_reported() {
        let rendered = diagnostics_of(
            r#"
            component Fancy extends Button {
                property label: String = "Save"
                Rectangle(id = extra)
            }
            component App { Window(id = root) { Fancy(id = a) } }
        "#,
        );
        assert!(rendered.contains("takes no content"), "{rendered}");
    }
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
fn a_document_can_ask_the_host_to_close_the_window() -> TestResult {
    // The titlebar close button. `root.close()` is the one method a
    // component may call on itself, because it acts on the host rather than
    // on the element.
    let (mut tree, mut engine) = build(
        r#"
        component App {
            property closes: Int = 0
            Window(id = root) {
                Rectangle(id = button, width = 30dp, height = 30dp) {
                    on click => { root.close() closes += 1 }
                }
            }
        }
    "#,
    );
    engine.propagate(&mut tree);
    assert!(!engine.close_requested(), "nothing asked before the click");
    let button = tree.lookup_id("button").expect("the button");
    engine.emit_bubble(&mut tree, button, "click")?;
    engine.propagate(&mut tree);
    assert!(
        engine.close_requested(),
        "the click asked the host to close"
    );
    // The rest of the effect still ran: a close request is not an abort.
    let root = tree.lookup_id("root").expect("root");
    assert_eq!(tree.arena[root].get("closes"), Some(&Value::Int(1)));
    return Ok(());
}

#[test]
fn a_close_request_is_taken_once() -> TestResult {
    let (mut tree, mut engine) = build(
        r#"
        component App {
            Window(id = root) {
                Rectangle(id = button, width = 30dp, height = 30dp) {
                    on click => root.close()
                }
            }
        }
    "#,
    );
    engine.propagate(&mut tree);
    let button = tree.lookup_id("button").expect("the button");
    engine.emit_bubble(&mut tree, button, "click")?;
    assert!(engine.close_requested());
    assert!(engine.take_close_request(), "the host consumes it");
    assert!(
        !engine.close_requested(),
        "a host that polls every frame must act once, not every frame"
    );
    assert!(!engine.take_close_request());
    return Ok(());
}

#[test]
fn a_close_inside_a_component_reaches_the_host() -> TestResult {
    // The rewrite matters here: `root` inside a component means the
    // component's own root, so the call is namespaced with the instance
    // rather than aimed at the document's window.
    let (mut tree, mut engine) = build(
        r#"
        component Closer {
            Rectangle(id = box, width = 30dp, height = 30dp) {
                on click => root.close()
            }
        }
        component App {
            Window(id = root) { Column { Closer(id = a) Closer(id = b) } }
        }
    "#,
    );
    engine.propagate(&mut tree);
    let a = tree.lookup_id("a").expect("first closer");
    let b = tree.lookup_id("b").expect("second closer");
    engine.emit_bubble(&mut tree, a, "click")?;
    assert!(
        engine.close_requested(),
        "a component's close still reaches the host"
    );
    let _ = engine.take_close_request();
    // The second instance is a separate element, so its click is a separate
    // request rather than a replay of the first.
    engine.emit_bubble(&mut tree, b, "click")?;
    assert!(engine.take_close_request());
    return Ok(());
}

#[test]
fn close_is_the_only_method_a_component_may_call() {
    // Everything else stays rejected, so a typo cannot slip through to fail
    // at runtime instead of here.
    let rendered = diagnostics_of(
        r#"
        component App {
            property n: Int = 0
            Window(id = root) {
                Rectangle(id = box, width = 10dp, height = 10dp) {
                    on click => root.explode()
                }
            }
        }
    "#,
    );
    assert!(
        rendered.contains("components have no methods"),
        "{rendered}"
    );
}

#[test]
fn a_component_can_be_instantiated_inside_a_for_body() {
    // The companion of `tests/for_components.rs`: the shape used to be
    // rejected here, and the rejection is gone. This case is kept local to
    // the component suite because what it checks is the checker's *reading*
    // of a component in a loop body; the runtime half lives in the other
    // file.
    let (mut tree, mut engine) = build(
        r#"
        component Chip { Rectangle(id = box) {} }
        component App {
            property rows: Model
            Window(id = root) { For(item in root.rows) { Chip(id = a) } }
        }
    "#,
    );
    // Nothing expands until a model drives the loop: a `For` body is a
    // prototype, not a child list.
    assert!(
        tree.lookup_id("a").is_none(),
        "no rows before a model exists"
    );
    let root = tree.lookup_id("root").expect("the entry component exists");
    let model = engine.add_model(Box::new(nui_runtime::VecModel::from_rows(vec![vec![(
        "label".to_string(),
        Value::String("x".to_string()),
    )]])));
    engine.set_direct(&mut tree, root, "rows", Value::Model(model.0));
    engine.propagate(&mut tree);
    engine.sync_for_nodes(&mut tree);
    engine.propagate(&mut tree);
    // The call-site id is namespaced with the instance prefix (D16), the
    // same as any other component reference; the bare `a` names the
    // prototype, which has no element of its own.
    let a = tree
        .lookup_id("a")
        .expect("the call-site id resolves to the instance element");
    assert_eq!(tree.arena[a].component.as_deref(), Some("Chip"));
    // The *call site's* id is not namespaced — it belongs to the row, which
    // is the only place it appears. What is namespaced is the component's
    // own internals (its `Rectangle(id = box)` becomes `i1::box`), so two
    // rows' instances cannot collide.
    assert!(
        tree.lookup_id("i1::box").is_some(),
        "the component's internal id is namespaced per instance: {:?}",
        tree.ids.keys().collect::<Vec<_>>()
    );
    let row_scope = tree.arena[a]
        .for_scope
        .clone()
        .expect("the instance element carries the row scope");
    assert_eq!(row_scope.variable, "item");
    assert_eq!(row_scope.row, 0);
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
