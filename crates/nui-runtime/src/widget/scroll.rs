//! How far a scroll container can travel, and how a `ListView` row fills
//! its slot.
//!
//! Both answers are *judgements*, not wiring: "is there more content
//! below?" needs the content height and the viewport height, and neither
//! is a property the host can read off the element. They live in
//! `nui-runtime` for the same reason every other decision does — the host
//! has no tests (it needs a real window and a GPU), so anything with a
//! right answer has to sit where a test can reach it.
//!
//! `Scroll` and `ListView` measure their content differently:
//!
//! - A `ListView` knows exactly: `row_count * row_height`. That is what a
//!   declared uniform row height is *for*.
//! - A `Scroll` has to look. Its children keep their natural size
//!   (`nui-layout` sets `flex_shrink = 0.0` — the Qt-Quick rule that an
//!   element is never shrunk to fit), so the content is as tall as its
//!   lowest child's bottom edge.

use nui_core::Value;

use crate::binding::Engine;
use crate::element::{Element, ElementId, ElementTree, FOR_VALUE_PROPERTY};
use crate::model::ModelId;

use super::props::{dp_or, number};

/// Row height a `ListView` falls back to when it declares none.
pub const DEFAULT_ROW_HEIGHT: f32 = 40.0;

/// The furthest `scroll_y` may go: where the content's bottom edge meets
/// the viewport's bottom edge.
///
/// Never negative — content shorter than the viewport does not scroll.
/// The caller may rely on that; a bare `max(0.0)` in the wheel handler is
/// what let a list be scrolled clean off its own content, leaving a blank
/// strip under the last row and no way to notice, because the deep-scroll
/// tests set `scroll_y` directly and never went through the wheel.
pub fn max_scroll_y(engine: &Engine, tree: &ElementTree, id: ElementId) -> f32 {
    let Some(element) = tree.arena.get(id) else {
        return 0.0;
    };
    let viewport = number(element, "height").unwrap_or(0.0);
    let content = content_height(engine, tree, element);
    return (content - viewport).max(0.0);
}

/// Total height of a scroll container's content, in dp.
fn content_height(engine: &Engine, tree: &ElementTree, element: &Element) -> f32 {
    if element.ty == "ListView" {
        let row_height = dp_or(element, "row_height", DEFAULT_ROW_HEIGHT).max(1.0);
        return row_count(engine, element) as f32 * row_height;
    }
    return element
        .children
        .iter()
        .filter_map(|child| {
            let child = tree.arena.get(*child)?;
            return Some(number(child, "y")? + number(child, "height")?);
        })
        .fold(0.0, f32::max);
}

/// Rows a `ListView` iterates; `0` when its `@for` is unset.
fn row_count(engine: &Engine, element: &Element) -> usize {
    let Some(Value::Model(index)) = element.get(FOR_VALUE_PROPERTY) else {
        return 0;
    };
    if *index == Value::UNSET_MODEL {
        return 0;
    }
    return engine.model_row_count(ModelId(*index));
}

/// How a `ListView` row fills its slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowSlot {
    /// Height to force on the row root; `None` keeps the row's own.
    pub height: Option<f32>,
    /// Bottom margin that tops a declared-height row up to the slot.
    pub margin_bottom: f32,
}

/// Resolves how a row fills a `row_height` slot.
///
/// `row_height` is a **slot**, not a hint. The virtualizer derives both
/// the visible window and the pre-window `Spacer` from it, while taffy
/// stacks the window's rows by their own height — so a row that does not
/// occupy exactly `row_height` drifts away from its declared position, a
/// little more with every row. `layout_probe` shows it: `row_height =
/// 30dp` with 24dp rows puts the second visible row at y = 30024 where
/// the slot says 30030. A window holds ~10 rows, so the drift stays under
/// a hundred dp — which is exactly why it read as "slightly tight rows"
/// rather than as a bug.
///
/// A row that declares its own height keeps it and takes the difference
/// as a bottom margin: the gallery's list page draws a 32dp card in a
/// 36dp slot, and that inset is the design. A row that declares none —
/// or declares one that does not fit — is stretched to the slot, because
/// the virtualizer's arithmetic is load-bearing and the row's `height`
/// is not.
pub fn row_slot(row_height: f32, declared_height: Option<f32>) -> RowSlot {
    let slot = row_height.max(1.0);
    return match declared_height {
        Some(declared) if declared > 0.0 && declared < slot => RowSlot {
            height: None,
            margin_bottom: slot - declared,
        },
        _ => RowSlot {
            height: Some(slot),
            margin_bottom: 0.0,
        },
    };
}

#[cfg(test)]
mod tests {
    use nui_core::Length;

    use super::*;
    use crate::model::VecModel;

    /// A `ListView` over `rows` rows of `row_height` dp, in a viewport
    /// `viewport` dp tall.
    fn list_view(rows: usize, row_height: f32, viewport: f32) -> (Engine, ElementTree, ElementId) {
        let mut engine = Engine::new();
        let model = engine.add_model(Box::new(VecModel::from_rows(
            (0..rows)
                .map(|index| return vec![("i".to_string(), Value::Int(index as i64))])
                .collect(),
        )));
        let mut tree = ElementTree::new();
        let mut list = Element::new("ListView", None);
        list.set("height", Value::Float(f64::from(viewport)));
        list.set("row_height", Value::Length(Length::Dp(row_height)));
        list.set(FOR_VALUE_PROPERTY, Value::Model(model.0));
        let id = tree.insert(list);
        return (engine, tree, id);
    }

    #[test]
    fn a_list_view_scrolls_to_its_last_row() {
        // 100 rows of 10dp in a 100dp viewport: at scroll_y = 900 the
        // content's bottom edge sits on the viewport's bottom edge.
        let (engine, tree, id) = list_view(100, 10.0, 100.0);
        assert_eq!(max_scroll_y(&engine, &tree, id), 900.0);
    }

    #[test]
    fn content_shorter_than_the_viewport_does_not_scroll() {
        let (engine, tree, id) = list_view(3, 10.0, 100.0);
        assert_eq!(max_scroll_y(&engine, &tree, id), 0.0);
    }

    #[test]
    fn an_empty_list_view_does_not_scroll() {
        let (engine, tree, id) = list_view(0, 10.0, 100.0);
        assert_eq!(max_scroll_y(&engine, &tree, id), 0.0);
    }

    #[test]
    fn a_list_view_without_a_model_does_not_scroll() {
        let engine = Engine::new();
        let mut tree = ElementTree::new();
        let mut list = Element::new("ListView", None);
        list.set("height", Value::Float(100.0));
        list.set("row_height", Value::Float(10.0));
        let id = tree.insert(list);
        assert_eq!(max_scroll_y(&engine, &tree, id), 0.0);
    }

    #[test]
    fn an_unset_for_iterable_does_not_scroll() {
        // `@for` holds the `UNSET_MODEL` sentinel until a model is bound.
        let engine = Engine::new();
        let mut tree = ElementTree::new();
        let mut list = Element::new("ListView", None);
        list.set("height", Value::Float(100.0));
        list.set("row_height", Value::Float(10.0));
        list.set(FOR_VALUE_PROPERTY, Value::Model(Value::UNSET_MODEL));
        let id = tree.insert(list);
        assert_eq!(max_scroll_y(&engine, &tree, id), 0.0);
    }

    #[test]
    fn a_scroll_container_measures_its_children() {
        let engine = Engine::new();
        let mut tree = ElementTree::new();
        let mut scroll = Element::new("Scroll", None);
        scroll.set("height", Value::Float(50.0));
        let id = tree.insert(scroll);
        for (y, height) in [(0.0, 30.0), (30.0, 45.0)] {
            let mut child = Element::new("Rectangle", None);
            child.set("y", Value::Float(y));
            child.set("height", Value::Float(height));
            let child_id = tree.insert(child);
            tree.append_child(id, child_id);
        }
        // The second child ends at 75dp; the viewport is 50dp.
        assert_eq!(max_scroll_y(&engine, &tree, id), 25.0);
    }

    #[test]
    fn a_row_that_declares_its_height_is_topped_up_to_the_slot() {
        let slot = row_slot(36.0, Some(32.0));
        assert_eq!(slot.height, None, "the card keeps its 32dp");
        assert_eq!(slot.margin_bottom, 4.0, "36dp slot, 32dp card");
    }

    #[test]
    fn a_row_without_a_height_fills_the_slot() {
        let slot = row_slot(36.0, None);
        assert_eq!(slot.height, Some(36.0));
        assert_eq!(slot.margin_bottom, 0.0);
    }

    #[test]
    fn a_row_taller_than_its_slot_is_pulled_back_to_it() {
        // The virtualizer computed the window from 30dp rows; letting one
        // be 48dp would push every row after it out of position.
        let slot = row_slot(30.0, Some(48.0));
        assert_eq!(slot.height, Some(30.0));
        assert_eq!(slot.margin_bottom, 0.0);
    }

    #[test]
    fn a_zero_height_declaration_counts_as_no_declaration() {
        let slot = row_slot(30.0, Some(0.0));
        assert_eq!(slot.height, Some(30.0));
    }

    #[test]
    fn a_slot_is_never_zero_high() {
        // `row_height = 0` is a typo, not a request for invisible rows —
        // and a zero slot would divide the window into infinitely many.
        assert_eq!(row_slot(0.0, None).height, Some(1.0));
        assert_eq!(row_slot(-5.0, None).height, Some(1.0));
    }
}
