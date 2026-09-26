//! End-to-end tests for the list family (FUTURE 批次 7), driven by the
//! gallery's `list` page.
//!
//! The virtualizer has its own tests in `nui-runtime` (`sync_list_view`
//! and its window bookkeeping), and the scroll arithmetic in
//! `nui_runtime::widget::scroll`. What is left — and what only the whole
//! chain can show — is that the rows taffy actually stacks land on the
//! boundaries the virtualizer computed. Those are the same number arrived
//! at from opposite ends, and nothing checks that they agree until here.
//!
//! That seam is where the batch's first bug lived: `row_height` fed the
//! window arithmetic and the pre-window `Spacer`, while the rows
//! themselves stacked at their own `height` — 32dp rows in a 36dp slot,
//! drifting a little further with every row. Nothing failed; the rows were
//! just slightly tight. `rows_land_on_their_slot_boundaries` is the
//! assertion that would have caught it.
//!
//! The fixture is the **shipped page file**, wrapped in the smallest
//! document that can host it (a window, plus the model property the page
//! binds to). The gallery assembles the same fragment with every other
//! page alongside it — see `examples/gallery/main.rs` — so the demo and
//! the test cannot drift apart.
#![allow(clippy::unwrap_used)]

use nui_core::{Size, Value};
use nui_runtime::widget;
use nui_runtime::{ElementId, ElementTree, Engine, ModelRow, Registry, VecModel};

/// The gallery's `list` page, in a document of its own.
fn page_source() -> String {
    return format!(
        "component ListPage {{\n    property listRows: Model\n    Window(id = root) {{\n{}\n    }}\n}}\n",
        include_str!("../examples/gallery/pages/list.nui")
    );
}

/// The viewport the gallery lays its pages out in.
const VIEWPORT: Size = Size {
    width: 1120.0,
    height: 760.0,
};

/// The page's row count and slot height, shared with the assertions.
const ROWS: usize = 10_000;
const ROW_HEIGHT: f32 = 36.0;

/// Instantiates the page, seeds its model, and settles it the way the host
/// does.
fn example() -> (ElementTree, Engine) {
    let source = page_source();
    let outcome = nui_compiler::compile(&source);
    assert!(
        outcome.diagnostics.is_empty(),
        "the shipped page must compile clean: {:?}",
        outcome.diagnostics
    );
    let registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let rows: Vec<ModelRow> = (0..ROWS)
            .map(|index| {
                return vec![
                    ("label".to_string(), Value::String(format!("row {index}"))),
                    ("hot".to_string(), Value::Bool(index % 25 == 0)),
                ];
            })
            .collect();
        let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
        let root = tree.lookup_id("root").expect("root id exists");
        engine.set_direct(tree, root, "listRows", Value::Model(model.0));
    }));
    let instance = nui_runtime::instantiate_with(&outcome.document, registry);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    settle(&mut tree, &mut engine);
    // A second frame. `write_back` has now written the measured boxes back
    // into the row elements' own `y`/`height` slots, which is the pass
    // where "content-sized" mistakes show up — and the window does not
    // move, so nothing is rebuilt to hide them.
    settle(&mut tree, &mut engine);
    return (tree, engine);
}

/// One host frame: propagate, sync `For` rows, lay out.
fn settle(tree: &mut ElementTree, engine: &mut Engine) {
    let mut text = nui_text::TextSystem::with_embedded_font();
    let _ = engine.propagate(tree);
    let rebuilt = engine.sync_for_nodes(tree);
    if rebuilt > 0 {
        let _ = engine.propagate(tree);
    }
    nui_layout::layout_with_text(tree, VIEWPORT, Some(&mut text));
}

fn find(tree: &ElementTree, id_name: &str) -> ElementId {
    return tree
        .lookup_id(id_name)
        .unwrap_or_else(|| panic!("the example declares `{id_name}`"));
}

fn number(tree: &ElementTree, id: ElementId, name: &str) -> f32 {
    return match tree.arena[id].get(name) {
        Some(Value::Float(inner)) => *inner as f32,
        Some(Value::Int(inner)) => *inner as f32,
        Some(Value::Length(nui_core::Length::Dp(inner))) => *inner,
        other => panic!("expected a numeric {name}, got {other:?}"),
    };
}

/// The window's row elements in model order — a row is an element the
/// `For` instantiated, which is exactly what `for_scope` records. The
/// leading `Spacer` (the pre-window offset) is not one.
fn window_rows(tree: &ElementTree, list: ElementId) -> Vec<ElementId> {
    return tree.arena[list]
        .children
        .iter()
        .copied()
        .filter(|child| return tree.arena[*child].for_scope.is_some())
        .collect();
}

/// The model row a window element renders.
fn row_of(tree: &ElementTree, row: ElementId) -> usize {
    return tree.arena[row]
        .for_scope
        .as_ref()
        .expect("a window row carries its scope")
        .row;
}

/// Where a row sits in the list's own content coordinates. Layout writes
/// `y` already accumulated, so the list's own `y` comes back out.
fn row_offset(tree: &ElementTree, list: ElementId, row: ElementId) -> f32 {
    return number(tree, row, "y") - number(tree, list, "y");
}

#[test]
fn the_page_compiles_and_instantiates() {
    let (tree, _engine) = example();
    let list = find(&tree, "list_view");
    assert!(number(&tree, list, "width") > 0.0, "the list was laid out");
    assert!(number(&tree, list, "height") > 0.0);
    assert!(
        find(&tree, "list_page") != list,
        "the page kept its container"
    );
}

#[test]
fn only_the_visible_window_exists() {
    // The whole point of virtualizing: 10,000 rows, ~16 elements.
    let (tree, _engine) = example();
    let list = find(&tree, "list_view");
    let rows = window_rows(&tree, list);
    assert!(
        rows.len() <= 20,
        "a 560dp viewport holds ~15 rows of 36dp, not {ROWS} — got {}",
        rows.len()
    );
    assert!(
        tree.arena.len() < 64,
        "total element count stays bounded (a row is a card plus its label), got {}",
        tree.arena.len()
    );
}

#[test]
fn rows_land_on_their_slot_boundaries() {
    // `row_height` is a slot, not a hint. Before 批次 7 the rows stacked
    // at their own 32dp while the `Spacer` and the window arithmetic
    // assumed 36dp, so every row after the first was a little high. This
    // is the assertion that pins the two ends together.
    let (tree, _engine) = example();
    let list = find(&tree, "list_view");
    let rows = window_rows(&tree, list);
    assert!(rows.len() >= 2, "need two rows to have a pitch");
    for row in &rows {
        let index = row_of(&tree, *row);
        let expected = index as f32 * ROW_HEIGHT;
        let actual = row_offset(&tree, list, *row);
        assert!(
            (actual - expected).abs() < 0.5,
            "row {index} sits at {actual}, its slot says {expected}"
        );
    }
    let pitch = row_offset(&tree, list, rows[1]) - row_offset(&tree, list, rows[0]);
    assert!(
        (pitch - ROW_HEIGHT).abs() < 0.5,
        "consecutive rows are one slot apart, was {pitch}"
    );
}

#[test]
fn a_deep_scroll_lands_on_the_right_row_at_its_absolute_position() {
    let (mut tree, mut engine) = example();
    let list = find(&tree, "list_view");
    engine.set_direct(&mut tree, list, "scroll_y", Value::Float(30_000.0));
    settle(&mut tree, &mut engine);

    let rows = window_rows(&tree, list);
    let first = row_of(&tree, rows[0]);
    assert_eq!(
        first,
        (30_000.0 / f64::from(ROW_HEIGHT)).floor() as usize,
        "the window opens on the scrolled row"
    );
    // The row the scroll landed on is at its absolute place in the
    // content — the window shifted, the rows did not.
    let expected = first as f32 * ROW_HEIGHT;
    let actual = row_offset(&tree, list, rows[0]);
    assert!(
        (actual - expected).abs() < 0.5,
        "row {first} sits at {actual}, its slot says {expected}"
    );
}

#[test]
fn the_list_scrolls_to_the_end_of_its_content_and_no_further() {
    let (tree, engine) = example();
    let list = find(&tree, "list_view");
    let viewport = number(&tree, list, "height");
    let limit = widget::max_scroll_y(&engine, &tree, list);
    assert_eq!(limit, ROWS as f32 * ROW_HEIGHT - viewport);
}

#[test]
fn scrolling_to_the_limit_puts_the_last_row_on_screen() {
    // The limit is only right if it agrees with the window calculation:
    // at exactly `max_scroll_y`, the last row must be the last thing
    // visible, with no blank strip below it (the bug a bare `max(0.0)`
    // produced) and nothing cut off past it.
    let (mut tree, mut engine) = example();
    let list = find(&tree, "list_view");
    let limit = widget::max_scroll_y(&engine, &tree, list);
    engine.set_direct(&mut tree, list, "scroll_y", Value::Float(f64::from(limit)));
    settle(&mut tree, &mut engine);

    let rows = window_rows(&tree, list);
    let last = row_of(&tree, *rows.last().unwrap());
    assert_eq!(last, ROWS - 1, "the last row is on screen at the limit");

    // And its bottom edge is the content's bottom edge.
    let bottom = row_offset(&tree, list, *rows.last().unwrap()) + ROW_HEIGHT;
    assert!(
        (bottom - (limit + number(&tree, list, "height"))).abs() < 0.5,
        "the content ends at the viewport's bottom edge, was {bottom}"
    );
}
