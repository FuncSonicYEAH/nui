//! Component references inside a `For` / `ListView` body.
//!
//! Rows are built by the *engine*, on its own, once per frame — long after
//! the instantiation pass that holds the document has returned. Expanding a
//! component reference there needs the catalog plus a prefix counter whose
//! numbering survives across frames, neither of which used to outlive
//! instantiation. The checker rejected the shape, and the runtime panicked
//! as a backstop for the case the checker missed.
//!
//! This file records both halves of that contract. The work opened with the
//! two pins below, in the form that *failed* at the time: a `#[should_panic]`
//! on the runtime backstop and an assertion that the checker emits the
//! rejection. Later steps flipped them to describe the supported behavior —
//! which is what a pin is for, provided each test keeps saying what it
//! guarantees rather than merely passing. See **D35** in `plan.md`.
#![allow(clippy::unwrap_used)]

use nui_compiler::compile;
use nui_core::Value;
use nui_runtime::{ElementTree, Engine};

/// Compiles, asserting the source is clean, and returns the document.
fn compile_clean(source: &str) -> nui_compiler::DocumentIr {
    let outcome = compile(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "test source must compile cleanly: {:?}",
        outcome.diagnostics
    );
    return outcome.document;
}

/// Compiles a clean document and instantiates it.
fn build(source: &str) -> (ElementTree, Engine) {
    let instance = nui_runtime::instantiate(&compile_clean(source));
    return (instance.tree, instance.engine);
}

/// The full M4 frame ordering: propagate (fills `@for`), row sync, then
/// propagate again (fills the fresh rows' bindings).
fn model_frame(tree: &mut ElementTree, engine: &mut Engine) {
    engine.propagate(tree);
    engine.sync_for_nodes(tree);
    engine.propagate(tree);
}

/// Drives one `Model`-typed property with a vector of single-field rows.
fn drive(
    tree: &mut ElementTree,
    engine: &mut Engine,
    root_property: &str,
    rows: Vec<Vec<(&str, Value)>>,
) -> nui_runtime::ModelId {
    let model = engine.add_model(Box::new(nui_runtime::VecModel::from_rows(
        rows.into_iter()
            .map(|row| {
                return row
                    .into_iter()
                    .map(|(name, value)| return (name.to_string(), value))
                    .collect();
            })
            .collect(),
    )));
    let root = tree.lookup_id("root").expect("the entry component exists");
    engine.set_direct(tree, root, root_property, Value::Model(model.0));
    return model;
}

/// Finds the first element of the given node type.
fn find_by_type(tree: &ElementTree, ty: &str) -> nui_runtime::ElementId {
    let mut found = None;
    tree.visit_pre_order(|id, element| {
        if element.ty == ty {
            found = Some(id);
        }
    });
    return found.expect("an element of that type exists");
}

/// Renders the diagnostics of a source expected to fail.
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

const CHIP_IN_FOR: &str = r#"
    component Chip {
        property label: String = "unnamed"
        Rectangle(id = box, width = 80dp, height = 30dp) {
            Text(id = chip_label, content <- label)
        }
    }
    component App {
        property items: Model
        Window(id = root) {
            For(item in root.items) {
                Chip(label <- item.label)
            }
        }
    }
"#;

/// **Pin (checker half).** A component reference inside a `For` body is no
/// longer rejected.
///
/// This test is the flip side of the diagnostic that used to fire here:
/// `` `Chip` cannot be instantiated inside a `For` body: rows are built
/// without the document, so there is nothing to expand it from ``. Its
/// absence is the whole point, so assert on the *text* — an empty set of
/// diagnostics would also be produced by a checker that silently stopped
/// running.
#[test]
fn a_component_in_a_for_body_is_not_rejected() {
    let outcome = compile(CHIP_IN_FOR);
    let rendered = outcome
        .diagnostics
        .iter()
        .map(|diagnostic| return nui_syntax::render_diagnostic(CHIP_IN_FOR, "test.nui", diagnostic))
        .collect::<Vec<_>>()
        .join("");
    assert!(
        !rendered.contains("cannot be instantiated inside"),
        "the `For`-body component rejection must be gone: {rendered}"
    );
    assert!(rendered.is_empty(), "expected no diagnostics: {rendered}");
}

/// **Pin (runtime half).** The runtime backstop panic is gone: instantiation
/// of a document whose entry component holds a `For` with a component body
/// now succeeds on its own.
///
/// At the time this file was written this could not even be *reached* —
/// `compile` produced a diagnostic first. It is kept as a separate test
/// because the two halves are genuinely independent: the checker is the
/// friendly error, the panic is the guarantee that a checker gap cannot
/// degrade into a silently-empty row.
#[test]
fn instantiating_a_document_with_a_component_in_a_for_body_does_not_panic() {
    let document = compile_clean(CHIP_IN_FOR);
    let _instance = nui_runtime::instantiate(&document);
}

/// The `ListView` half of the same shape. Kept distinct from the `For` case
/// because the two share the row path but not the sync algorithm — and
/// "`For` works, `ListView` does not" would be a strange asymmetry to
/// ship.
#[test]
fn a_component_in_a_list_view_body_is_not_rejected() {
    let outcome = compile(
        r#"
        component Chip {
            property label: String = "unnamed"
            Text(id = chip_label, content <- label)
        }
        component App {
            property items: Model
            Window(id = root) {
                ListView(item in root.items, row_height = 40dp) {
                    Chip(label <- item.label)
                }
            }
        }
    "#,
    );
    let rendered = outcome
        .diagnostics
        .iter()
        .map(|diagnostic| return nui_syntax::render_diagnostic("", "test.nui", diagnostic))
        .collect::<Vec<_>>()
        .join("");
    assert!(rendered.is_empty(), "expected no diagnostics: {rendered}");
}

/// A guard against the fix being implemented by simply *deleting* the
/// checker rule and letting anything through: an unknown node type inside a
/// `For` body is still an error. The rejection that was removed was about
/// component references specifically; the general "unknown type" check must
/// survive it.
#[test]
fn an_unknown_node_type_in_a_for_body_is_still_an_error() {
    let rendered = diagnostics_of(
        r#"
        component App {
            property items: Model
            Window(id = root) {
                For(item in root.items) {
                    DefinitelyNotAType(id = a)
                }
            }
        }
    "#,
    );
    assert!(
        rendered.contains("DefinitelyNotAType"),
        "the unknown type must still be named in the diagnostic: {rendered}"
    );
}

// ---------------------------------------------------------------------------
// End-to-end: a component inside a row actually works
// ---------------------------------------------------------------------------

/// A row component that reads `item.field` through a binding on its own
/// declared property, and writes the field back from a click handler.
///
/// The handler is the interesting half: `Chip` cannot name the model row
/// directly — it only sees the *expression* `item.label` its call site
/// passed. Writing back goes through `model_set_field`, whose invalidation
/// key is `(row root, "@field.label")`, and the row root is found by
/// walking up from the element that *read* the field. That walk passes
/// through the component instance element, which is exactly the path this
/// file exists to pin.
const ROW_CHIP: &str = r#"
    component Chip {
        property label: String = "unnamed"
        property picks: Int = 0

        Rectangle(id = box, width = 80dp, height = 30dp) {
            Text(id = chip_label, content <- label)
            on click => picks += 1
        }
    }

    component App {
        property items: Model

        Window(id = root) {
            For(item in root.items) {
                Chip(id = row, label <- item.label, picks <- item.picks)
            }
        }
    }
"#;

/// A binding inside a row component reads the row's model field: the two
/// rows show their own `label`, not the first row's, and not the default.
#[test]
fn a_row_component_reads_its_own_model_row() {
    let (mut tree, mut engine) = build(ROW_CHIP);
    let _model = drive(
        &mut tree,
        &mut engine,
        "items",
        vec![
            vec![
                ("label", Value::String("alpha".to_string())),
                ("picks", Value::Int(0)),
            ],
            vec![
                ("label", Value::String("beta".to_string())),
                ("picks", Value::Int(0)),
            ],
        ],
    );
    model_frame(&mut tree, &mut engine);

    let for_element = find_by_type(&tree, "For");
    let rows = tree.arena[for_element].children.clone();
    assert_eq!(rows.len(), 2, "one row element per model entry");
    for (row, expected) in rows.iter().zip(["alpha", "beta"]) {
        // The row element is the component instance; its declared property
        // carries the model value.
        assert_eq!(
            tree.arena[*row].get("label"),
            Some(&Value::String(expected.to_string())),
            "row {row:?} reads its own field"
        );
        assert_eq!(
            tree.arena[*row].component.as_deref(),
            Some("Chip"),
            "the row element is the component instance"
        );
    }
    // And the value reaches the component's *internal* text element, which
    // is the point of the binding: it is not merely stored on the instance.
    let texts: Vec<Option<&Value>> = rows
        .iter()
        .map(|row| {
            let box_id = find_by_type_under(&tree, *row, "Text");
            return tree.arena[box_id].get("content");
        })
        .collect();
    assert_eq!(
        texts,
        vec![
            Some(&Value::String("alpha".to_string())),
            Some(&Value::String("beta".to_string())),
        ]
    );
}

/// Finds the first element of a given type within a subtree.
fn find_by_type_under(
    tree: &ElementTree,
    root: nui_runtime::ElementId,
    ty: &str,
) -> nui_runtime::ElementId {
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if tree.arena[id].ty == ty {
            return id;
        }
        for child in &tree.arena[id].children {
            stack.push(*child);
        }
    }
    panic!("no `{ty}` under {root:?}");
}

/// **The invalidation-key test.** `model_set_field` invalidates one row's
/// field by keying on the row *root*, which it finds by walking up from the
/// element that recorded the dependency. A component instance sits between
/// the component's internals and the row root, so the question is whether
/// that key still lands on the right element.
///
/// # What this test established (and why it is not the hoist test the plan
/// expected)
///
/// The plan flagged the upward walk in `find_row_scope` as the top risk:
/// if it stopped at the instance element instead of climbing to the row
/// root, the field write would update the model and nothing would redraw.
/// Writing this test settled the question the other way, and the reason is
/// a language rule worth stating:
///
/// **The row variable is not visible inside a component.** `Chip`'s body
/// cannot name `item` at all (`unknown name \`item\``), because the
/// component is compiled on its own, before any `For` exists. So the only
/// place `item.field` can be written is the *call site*, and the call
/// site's argument binding is registered on the instance element — which
/// **is** the row root.
///
/// The reader and the row root are therefore the same element, and the
/// upward walk is a no-op. Disabling it leaves this test passing; that was
/// verified by hand rather than assumed. The walk still earns its keep for
/// the case it was written for — a row whose *own* elements (not a
/// component's) read `item.field` at depth, where reader and row root
/// genuinely differ.
#[test]
fn model_field_write_redraws_through_a_row_component() {
    let (mut tree, mut engine) = build(ROW_CHIP);
    let model = drive(
        &mut tree,
        &mut engine,
        "items",
        vec![
            vec![
                ("label", Value::String("alpha".to_string())),
                ("picks", Value::Int(0)),
            ],
            vec![
                ("label", Value::String("beta".to_string())),
                ("picks", Value::Int(0)),
            ],
        ],
    );
    model_frame(&mut tree, &mut engine);

    let for_element = find_by_type(&tree, "For");
    let rows = tree.arena[for_element].children.clone();
    // The call-site argument lands on the instance element and flows into
    // the component's own `Text` through its declared property.
    let text_of = |tree: &ElementTree, row: nui_runtime::ElementId| {
        return find_by_type_under(tree, row, "Text");
    };
    assert_eq!(
        tree.arena[text_of(&tree, rows[1])].get("content"),
        Some(&Value::String("beta".to_string())),
        "the argument reached the component's internal binding"
    );

    let changed = engine.model_set_field(
        &mut tree,
        model,
        1,
        "label",
        Value::String("gamma".to_string()),
    );
    assert!(changed, "the model accepts the write");
    engine.propagate(&mut tree);

    assert_eq!(
        tree.arena[text_of(&tree, rows[0])].get("content"),
        Some(&Value::String("alpha".to_string())),
        "row 0 is untouched"
    );
    assert_eq!(
        tree.arena[text_of(&tree, rows[1])].get("content"),
        Some(&Value::String("gamma".to_string())),
        "row 1's component re-read the field after the write"
    );
}

/// The row *variable* is not visible inside a component body — the
/// restriction that shapes every test above, pinned so a future change to
/// it is a deliberate decision rather than an accident.
///
/// If this ever starts compiling, the tests here should be revisited: it
/// would mean a component can read `item.field` directly, and then the
/// reader/row-root distinction (and the hoist in `find_row_scope`) becomes
/// load-bearing for real.
#[test]
fn a_row_variable_is_not_visible_inside_a_component_body() {
    let rendered = diagnostics_of(
        r#"
        component Chip { Text(id = chip_label, content <- item.label) }
        component App {
            property items: Model
            Window(id = root) {
                For(item in root.items) {
                    Chip(id = row)
                }
            }
        }
    "#,
    );
    assert!(
        rendered.contains("item"),
        "the row variable is expected to be out of scope in the component: {rendered}"
    );
}

/// A component inside a `For` and the same component outside it share the
/// document but not an id namespace: the two instances must not collide.
#[test]
fn a_component_inside_and_outside_a_for_do_not_share_ids() {
    let (mut tree, mut engine) = build(
        r#"
        component Chip {
            property label: String = "unnamed"
            Text(id = chip_label, content <- label)
        }
        component App {
            property items: Model
            Window(id = root) {
                Column {
                    Chip(id = outside, label = "fixed")
                    For(item in root.items) {
                        Chip(id = inside, label <- item.label)
                    }
                }
            }
        }
    "#,
    );
    drive(
        &mut tree,
        &mut engine,
        "items",
        vec![vec![("label", Value::String("row".to_string()))]],
    );
    model_frame(&mut tree, &mut engine);

    let outside = tree.lookup_id("outside").expect("the standalone instance");
    let inside = tree.lookup_id("inside").expect("the row instance");
    assert_ne!(outside, inside, "two distinct instances");
    // Each has its own copy of the component's internal id.
    let outside_text = find_by_type_under(&tree, outside, "Text");
    let inside_text = find_by_type_under(&tree, inside, "Text");
    assert_ne!(
        outside_text, inside_text,
        "each instance's internals are its own"
    );
    assert_eq!(
        tree.arena[outside_text].get("content"),
        Some(&Value::String("fixed".to_string()))
    );
    assert_eq!(
        tree.arena[inside_text].get("content"),
        Some(&Value::String("row".to_string()))
    );
}

/// Deleting a row and rebuilding reuses neither the elements nor their
/// prefixes: the rebuilt rows get numbers the old ones never held, so an
/// element that survived the rebuild cannot be written by the wrong
/// instance.
#[test]
fn rebuilt_rows_receive_fresh_instance_prefixes() {
    let (mut tree, mut engine) = build(ROW_CHIP);
    let model = drive(
        &mut tree,
        &mut engine,
        "items",
        vec![
            vec![
                ("label", Value::String("a".to_string())),
                ("picks", Value::Int(0)),
            ],
            vec![
                ("label", Value::String("b".to_string())),
                ("picks", Value::Int(0)),
            ],
        ],
    );
    model_frame(&mut tree, &mut engine);

    let for_element = find_by_type(&tree, "For");
    let first_generation = tree.arena[for_element].children.clone();
    let prefixes_of = |tree: &ElementTree, rows: &[nui_runtime::ElementId]| {
        return rows
            .iter()
            .map(|row| return tree.arena[*row].instance_id.clone())
            .collect::<Vec<_>>();
    };
    let before = prefixes_of(&tree, &first_generation);

    // Grow the model: the row count changes, so every row is rebuilt.
    engine.model_push(
        model,
        vec![("label".to_string(), Value::String("c".to_string()))],
    );
    model_frame(&mut tree, &mut engine);

    let second_generation = tree.arena[for_element].children.clone();
    assert_eq!(second_generation.len(), 3, "the new row exists");
    let after = prefixes_of(&tree, &second_generation);
    for prefix in &before {
        assert!(
            !after.contains(prefix),
            "a rebuilt row reused prefix {prefix:?}: {before:?} -> {after:?}"
        );
    }
}

/// A component inside a row that itself instantiates another component:
/// expansion has to recurse, and each nesting level takes its own prefix.
#[test]
fn a_row_component_can_instantiate_another_component() {
    let (mut tree, mut engine) = build(
        r#"
        component Badge {
            property text: String = ""
            Text(id = badge_text, content <- text)
        }
        component Card {
            property title: String = ""
            Rectangle(id = card_box, width = 120dp, height = 40dp) {
                Badge(text <- title)
            }
        }
        component App {
            property items: Model
            Window(id = root) {
                For(item in root.items) {
                    Card(title <- item.label)
                }
            }
        }
    "#,
    );
    drive(
        &mut tree,
        &mut engine,
        "items",
        vec![
            vec![("label", Value::String("one".to_string()))],
            vec![("label", Value::String("two".to_string()))],
        ],
    );
    model_frame(&mut tree, &mut engine);

    let for_element = find_by_type(&tree, "For");
    let cards = tree.arena[for_element].children.clone();
    assert_eq!(cards.len(), 2);
    for (card, expected) in cards.iter().zip(["one", "two"]) {
        assert_eq!(tree.arena[*card].component.as_deref(), Some("Card"));
        let badge_text = find_by_type_under(&tree, *card, "Text");
        assert_eq!(
            tree.arena[badge_text].get("content"),
            Some(&Value::String(expected.to_string())),
            "the nested component resolved through both levels"
        );
    }
    // The two rows' nested instances are distinct elements.
    let first_text = find_by_type_under(&tree, cards[0], "Text");
    let second_text = find_by_type_under(&tree, cards[1], "Text");
    assert_ne!(first_text, second_text);
}

/// A `ListView` window that scrolls builds rows the same way, so a
/// component in its prototype expands and gets its own numbering.
#[test]
fn a_component_in_a_list_view_window_expands() {
    let (mut tree, mut engine) = build(
        r#"
        component Row {
            property label: String = ""
            Rectangle(id = row_box, width = 100dp, height = 40dp) {
                Text(id = row_label, content <- label)
            }
        }
        component App {
            property items: Model
            Window(id = root) {
                ListView(item in root.items, row_height = 40dp, height = 120dp) {
                    Row(label <- item.label)
                }
            }
        }
    "#,
    );
    drive(
        &mut tree,
        &mut engine,
        "items",
        (0..5)
            .map(|i| return vec![("label", Value::String(format!("r{i}")))])
            .collect(),
    );
    model_frame(&mut tree, &mut engine);

    let list = find_by_type(&tree, "ListView");
    // The window holds a spacer plus a few rows, not all five.
    let rows: Vec<_> = tree.arena[list]
        .children
        .iter()
        .copied()
        .filter(|child| return tree.arena[*child].component.as_deref() == Some("Row"))
        .collect();
    assert!(!rows.is_empty(), "the window built at least one row");
    assert!(rows.len() <= 5, "the window is virtualized");
    // Each row's instance carries a label, and the component's own text
    // element resolved to that same value — which is what proves the
    // expansion (not just the instance element) happened.
    for row in &rows {
        let Some(label) = tree.arena[*row].get("label").cloned() else {
            panic!("the row instance declares a label");
        };
        let text = find_by_type_under(&tree, *row, "Text");
        assert_eq!(
            tree.arena[text].get("content"),
            Some(&label),
            "the component's internal binding resolved to the row's value"
        );
    }
}
