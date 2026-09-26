//! Hit-testing with scroll offsets (M9): content scrolled inside a `Scroll`
//! element must hit-test at its *translated* position.
#![allow(clippy::unwrap_used)]

use nui::hit_test;
use nui_core::{Point, Value};
use nui_runtime::Element;

fn build_scroll_tree(scroll_y: f64) -> nui_runtime::ElementTree {
    let mut tree = nui_runtime::ElementTree::new();
    let mut scroll = Element::new("Scroll", None);
    scroll.set("width", Value::Float(100.0));
    scroll.set("height", Value::Float(50.0));
    scroll.set("scroll_y", Value::Float(scroll_y));
    let scroll_id = tree.insert(scroll);
    tree.push_root(scroll_id);
    let mut row = Element::new("Rectangle", None);
    row.set("width", Value::Float(100.0));
    row.set("height", Value::Float(20.0));
    let row_id = tree.insert(row);
    tree.append_child(scroll_id, row_id);
    return tree;
}

#[test]
fn hit_test_accounts_for_scroll_offset() {
    // Unscrolled: the row occupies y 0..20 and is the innermost hit.
    let tree = build_scroll_tree(0.0);
    let row = tree.arena[tree.roots[0]].children[0];
    let hit = hit_test(&tree, Point::new(50.0, 10.0));
    assert_eq!(hit.map(|target| return target.element), Some(row));

    // Scrolled by 20: the row now spans y -20..0; y=10 hits the scroll
    // viewport itself, not the row.
    let tree = build_scroll_tree(20.0);
    let scroll = tree.roots[0];
    let hit = hit_test(&tree, Point::new(50.0, 10.0));
    assert_eq!(hit.map(|target| return target.element), Some(scroll));
    // The row is found 20px higher, at its translated position.
    let hit = hit_test(&tree, Point::new(50.0, -10.0));
    assert_eq!(hit.map(|target| return target.element), Some(row));
}

/// A hidden page keeps the *stale* boxes of its last layout (`write_back`
/// skips an invisible subtree), so "no box, no hit" is false — the boxes
/// are very much there. The walk must prune on `visible` itself, or a
/// hidden page's geometry wins the smallest-area contest and steals the
/// click from whatever is visible underneath (the gallery bug where a
/// second text field could not be clicked into).
#[test]
fn an_invisible_page_cannot_steal_a_hit_with_its_stale_boxes() {
    let mut tree = nui_runtime::ElementTree::new();
    let mut hidden = Element::new("Column", None);
    hidden.set("visible", Value::Bool(false));
    let hidden_id = tree.insert(hidden);
    tree.push_root(hidden_id);
    // A small element at the stale position the click will land on. Its
    // area is far smaller than the visible field's, so unpruned it wins.
    let mut stale = Element::new("Rectangle", None);
    stale.set("x", Value::Float(200.0));
    stale.set("y", Value::Float(160.0));
    stale.set("width", Value::Float(60.0));
    stale.set("height", Value::Float(20.0));
    let stale_id = tree.insert(stale);
    tree.append_child(hidden_id, stale_id);

    // The visible page: its field covers the same point.
    let shown = Element::new("Column", None);
    let shown_id = tree.insert(shown);
    tree.push_root(shown_id);
    let mut field = Element::new("TextInput", None);
    field.set("x", Value::Float(200.0));
    field.set("y", Value::Float(160.0));
    field.set("width", Value::Float(360.0));
    field.set("height", Value::Float(40.0));
    let field_id = tree.insert(field);
    tree.append_child(shown_id, field_id);

    let hit = hit_test(&tree, Point::new(210.0, 170.0));
    assert_eq!(
        hit.map(|target| return target.element),
        Some(field_id),
        "the visible field wins; the hidden page's stale box must not"
    );
}

/// Same rule for a closed overlay: the scene walk drops the subtree, so
/// its stale box must not catch clicks either.
#[test]
fn a_closed_overlay_cannot_steal_a_hit_with_its_stale_boxes() {
    let mut tree = nui_runtime::ElementTree::new();
    let mut dialog = Element::new("Dialog", None);
    dialog.set("open", Value::Bool(false));
    let dialog_id = tree.insert(dialog);
    tree.push_root(dialog_id);
    let mut stale = Element::new("Button", None);
    stale.set("x", Value::Float(40.0));
    stale.set("y", Value::Float(40.0));
    stale.set("width", Value::Float(80.0));
    stale.set("height", Value::Float(24.0));
    let stale_id = tree.insert(stale);
    tree.append_child(dialog_id, stale_id);

    let shown = Element::new("Column", None);
    let shown_id = tree.insert(shown);
    tree.push_root(shown_id);
    let mut field = Element::new("TextInput", None);
    field.set("x", Value::Float(40.0));
    field.set("y", Value::Float(40.0));
    field.set("width", Value::Float(200.0));
    field.set("height", Value::Float(40.0));
    let field_id = tree.insert(field);
    tree.append_child(shown_id, field_id);

    let hit = hit_test(&tree, Point::new(50.0, 50.0));
    assert_eq!(
        hit.map(|target| return target.element),
        Some(field_id),
        "the closed dialog's stale box must not catch the click"
    );
}
