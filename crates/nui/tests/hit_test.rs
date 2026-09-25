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
