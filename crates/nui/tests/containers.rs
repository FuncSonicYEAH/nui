//! End-to-end layout tests for the containers added in FUTURE 批次 5,
//! driven by the gallery's `containers` page.
//!
//! `nui-layout`'s own tests build trees by hand, which is the right level
//! for the adapter's rules. These go through the whole chain instead —
//! `.nui` source → compiler → instantiation → layout — so a container
//! that compiles but cannot actually be laid out (a `Stack` that collapses,
//! a `Wrap` that never wraps) is caught. Using the shipped page as the
//! fixture keeps the demo and the test from drifting apart.
#![allow(clippy::unwrap_used)]

use nui_core::{Size, Value};
use nui_runtime::{ElementId, ElementTree};

/// The gallery's `containers` page, in a document of its own (the page
/// binds nothing outside itself, so a bare window is enough to host it).
fn page_source() -> String {
    return format!(
        "component ContainersPage {{\n    Window(id = root) {{\n{}\n    }}\n}}\n",
        include_str!("../examples/gallery/pages/containers.nui")
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
    // Bindings settle on the first pass, geometry on the next.
    for _ in 0..3 {
        let mut text = nui_text::TextSystem::with_embedded_font();
        nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
    }
    return tree;
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

#[test]
fn the_page_compiles_and_instantiates() {
    let tree = example_tree();
    for id in [
        "containers_page",
        "containers_settings",
        "containers_stats",
        "containers_rule",
        "containers_swatches",
        "containers_chips",
        "containers_avatar",
        "containers_filler",
    ] {
        let element = find(&tree, id);
        assert!(
            number(&tree, element, "width") > 0.0,
            "`{id}` must have been laid out"
        );
    }
}

#[test]
fn a_panel_starts_its_body_below_the_title_bar() {
    let tree = example_tree();
    let panel = find(&tree, "containers_settings");
    let body = tree.arena[panel].children[0];
    let panel_y = number(&tree, panel, "y");
    // 16dp default padding + (16dp title + 8 + 8 gaps).
    assert_eq!(number(&tree, body, "y"), panel_y + 48.0);
    assert_eq!(
        number(&tree, body, "x"),
        number(&tree, panel, "x") + 16.0,
        "the body is inset by the padding"
    );
}

#[test]
fn a_card_and_a_panel_fill_their_row() {
    let tree = example_tree();
    for id in ["containers_settings", "containers_stats"] {
        let element = find(&tree, id);
        assert_eq!(
            number(&tree, element, "height"),
            96.0,
            "`{id}` stretches to the 96dp row"
        );
    }
}

#[test]
fn a_grid_places_six_children_in_two_rows_of_three() {
    let tree = example_tree();
    let grid = find(&tree, "containers_swatches");
    let cells = tree.arena[grid].children.clone();
    assert_eq!(cells.len(), 6);
    let grid_x = number(&tree, grid, "x");
    let grid_y = number(&tree, grid, "y");
    let row_height = number(&tree, cells[0], "height");
    assert_eq!(number(&tree, cells[0], "x"), grid_x);
    assert_eq!(number(&tree, cells[0], "y"), grid_y);
    // Columns advance, rows stack.
    assert!(number(&tree, cells[1], "x") > number(&tree, cells[0], "x"));
    assert!(number(&tree, cells[2], "x") > number(&tree, cells[1], "x"));
    assert_eq!(
        number(&tree, cells[3], "x"),
        grid_x,
        "cell 3 starts row two"
    );
    assert_eq!(
        number(&tree, cells[3], "y"),
        grid_y + row_height + 8.0,
        "row two clears the 8dp row gap"
    );
    // Auto-width children stretch to their track.
    assert!(number(&tree, cells[0], "width") > 150.0);
}

#[test]
fn a_wrap_flows_onto_several_lines() {
    let tree = example_tree();
    let wrap = find(&tree, "containers_chips");
    let chips = tree.arena[wrap].children.clone();
    assert_eq!(chips.len(), 5);
    // The wrap declares a 476dp width: three 140dp chips fit a line
    // (3*140 + 2*8 = 436), the fourth does not (436 + 8 + 140 > 476), so
    // the flow breaks — a property of the page, not of the viewport.
    let first_y = number(&tree, chips[0], "y");
    let lines: Vec<f32> = chips
        .iter()
        .map(|id| return number(&tree, *id, "y"))
        .filter(|y| return *y != first_y)
        .collect();
    assert!(
        !lines.is_empty(),
        "five 140dp chips cannot fit the 476dp wrap on one line"
    );
    let wrap_right = number(&tree, wrap, "x") + number(&tree, wrap, "width");
    for chip in chips {
        assert!(
            number(&tree, chip, "x") + number(&tree, chip, "width") <= wrap_right,
            "no chip may overflow the wrap"
        );
    }
}

#[test]
fn a_stack_overlaps_its_children_and_the_badge_takes_the_corner() {
    let tree = example_tree();
    let stack = find(&tree, "containers_avatar");
    let children = tree.arena[stack].children.clone();
    assert_eq!(children.len(), 2);
    let (disc, badge) = (children[0], children[1]);
    let stack_x = number(&tree, stack, "x");
    let stack_y = number(&tree, stack, "y");
    let stack_width = number(&tree, stack, "width");
    let stack_height = number(&tree, stack, "height");

    // The disc has no size of its own: it fills the 72dp stack.
    assert_eq!(number(&tree, disc, "width"), stack_width);
    assert_eq!(number(&tree, disc, "height"), stack_height);
    // The badge keeps its 20dp and is pinned bottom-right by its own
    // `align_self` / `justify_self`, which override the stack's default
    // stretch for that one child.
    assert_eq!(number(&tree, badge, "width"), 20.0);
    assert_eq!(number(&tree, badge, "x"), stack_x + stack_width - 20.0);
    assert_eq!(number(&tree, badge, "y"), stack_y + stack_height - 20.0);
}

#[test]
fn a_separator_spans_the_page_content_width() {
    let tree = example_tree();
    let page = find(&tree, "containers_page");
    let rule = find(&tree, "containers_rule");
    assert_eq!(number(&tree, rule, "height"), 1.0);
    assert_eq!(
        number(&tree, rule, "width"),
        number(&tree, page, "width") - 48.0,
        "the page's 24dp padding is on both sides"
    );
}

#[test]
fn a_spacer_pushes_the_last_line_to_the_bottom() {
    let tree = example_tree();
    let page = find(&tree, "containers_page");
    let spacer = find(&tree, "containers_filler");
    let footer = *tree.arena[page]
        .children
        .last()
        .expect("the page has a footer line");
    let spacer_height = number(&tree, spacer, "height");
    assert!(
        spacer_height > 0.0,
        "a height-100% page leaves the spacer the remaining space"
    );
    assert!(
        number(&tree, footer, "y") > number(&tree, spacer, "y"),
        "the footer sits after the spacer"
    );
    // The page is 760dp tall with 24dp padding: the footer ends there.
    let page_bottom = number(&tree, page, "y") + number(&tree, page, "height");
    assert_eq!(
        number(&tree, footer, "y") + number(&tree, footer, "height"),
        page_bottom - 24.0,
        "the spacer took exactly the free space"
    );
}
