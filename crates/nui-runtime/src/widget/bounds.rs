//! Where a widget actually is on screen.
//!
//! One implementation, shared by the state machine (hover/press) and the
//! drag handler, so a control's hitbox and its thumb travel cannot
//! disagree.

use crate::element::{ElementId, ElementTree};

use super::props::{dp_or, number};

/// The element's absolute dp rect.
///
/// Layout writes `x`/`y` **already accumulated** down the tree (see
/// `nui-layout`'s `write_back`: each child's base is its parent's absolute
/// origin), so the element's own `x`/`y` *is* its window position. Walking
/// the parent chain and summing again double-counted every ancestor — a
/// CheckBox at y=114 inside a container also at y=114 resolved to y=228,
/// which is why hitboxes sat below their controls by an amount that grew
/// with nesting depth.
///
/// `scroll_y` is the one exception: it is a viewport translation applied
/// at draw and hit-test time, not baked into the child's `y`. So a
/// scrolled subtree must be shifted up by every `Scroll`/`ListView`
/// ancestor, exactly as `nui::app::element_bounds` and the scene walk do.
///
/// `None` when the element has no size (layout has not run).
pub(super) fn absolute_bounds(tree: &ElementTree, id: ElementId) -> Option<nui_core::Rect> {
    let element = &tree.arena[id];
    let width = number(element, "width")?;
    let height = number(element, "height")?;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let x = number(element, "x")?;
    let mut y = number(element, "y")?;
    let mut current = element.parent;
    while let Some(handle) = current {
        let ancestor = &tree.arena[handle];
        if ancestor.ty == "Scroll" || ancestor.ty == "ListView" {
            y -= dp_or(ancestor, "scroll_y", 0.0);
        }
        current = ancestor.parent;
    }
    return Some(nui_core::Rect::new(
        nui_core::Point::new(x, y),
        nui_core::Size::new(width, height),
    ));
}
