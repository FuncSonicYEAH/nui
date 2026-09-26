//! Fixtures shared by the scene and control unit tests.
//!
//! Only compiled for the crate's own test harness: an integration test
//! builds its own tree because it cannot see `#[cfg(test)]` items.

use std::collections::HashMap;

use nui_core::Value;
use nui_runtime::{Element, ElementTree};

use crate::scene::{Scene, SceneBuilder, SceneContext};

/// Builds a scene from `tree` with no focus and no image registry — the
/// shape every draw-list assertion needs.
pub fn build(tree: &ElementTree) -> Scene {
    return SceneBuilder::build_with_context(
        tree,
        &mut nui_text::TextSystem::with_embedded_font(),
        SceneContext {
            focused: None,
            image_keys: &HashMap::new(),
        },
    );
}

/// An element of `ty` sized `width` x `height` dp, with nothing else set.
pub fn widget(ty: &str, width: f32, height: f32) -> Element {
    let mut element = Element::new(ty, None);
    element.set("width", Value::Length(nui_core::Length::Dp(width)));
    element.set("height", Value::Length(nui_core::Length::Dp(height)));
    return element;
}

/// A `400`x`300` root holding `children`, used by the overlay tests: the
/// first root's box is what the builder reads as the viewport.
pub fn window_tree(children: Vec<Element>) -> ElementTree {
    let mut tree = ElementTree::new();
    let mut root = Element::new("Column", None);
    root.set("width", Value::Float(400.0));
    root.set("height", Value::Float(300.0));
    let root_id = tree.insert(root);
    tree.push_root(root_id);
    for child in children {
        let id = tree.insert(child);
        tree.append_child(root_id, id);
    }
    return tree;
}
