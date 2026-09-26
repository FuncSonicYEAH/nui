//! Pipeline-level control tests.
//!
//! Per-control geometry lives next to each control; this module covers
//! what only shows up once [`crate::scene`] has taken the parts through
//! [`crate::widget::parts_for`] and painted them — the dispatcher and the
//! pruning around it.

use crate::testkit::{build, widget};
use nui_core::Value;
use nui_runtime::ElementTree;

#[test]
fn zero_sized_controls_paint_nothing() {
    let mut tree = ElementTree::new();
    let id = tree.insert(widget("Button", 0.0, 0.0));
    tree.push_root(id);
    let scene = build(&tree);
    assert!(scene.rects.is_empty());
    assert!(scene.sources.is_empty());
}

#[test]
fn control_opacity_scales_every_part() {
    let mut tree = ElementTree::new();
    let mut button = widget("Button", 120.0, 36.0);
    button.set("opacity", Value::Float(0.5));
    let id = tree.insert(button);
    tree.push_root(id);
    let scene = build(&tree);
    let base = crate::widget::Palette::default().surface;
    assert!((scene.rects[0].fill.alpha() - base.alpha() * 0.5).abs() < 0.001);
}
