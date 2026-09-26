//! Overlays and modal dialogs.
//!
//! An overlay is hoisted out of normal paint order and lifted out of its
//! parent's layout flow (FUTURE 批次 4). The interaction side of that is
//! here: which overlay is open, which modal owns the input, and what sits
//! behind it.
//!
//! `nui-runtime` owns the *decisions*; `nui`'s host applies them. That
//! split is why this lives here rather than in the event loop — it keeps
//! the modal rules testable without a window.

use crate::element::{Element, ElementId, ElementTree};

/// Whether an element is an overlay: hoisted out of normal paint order
/// and lifted out of its parent's layout flow (FUTURE 批次 4).
///
/// `Dialog` implies it, because a dialog that could be forgotten is a
/// dialog that silently paints under the content it should cover. Other
/// elements opt in with `overlay = true` (Popup / Toast / Tooltip).
pub fn is_overlay(element: &Element) -> bool {
    if element.ty == "Dialog" {
        return true;
    }
    return element
        .get("overlay")
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(false);
}

/// Whether an overlay is currently shown. An overlay with no `open`
/// property (Toast, Tooltip) is always shown; one that declares it is
/// hidden until `open` is true.
pub fn is_open(element: &Element) -> bool {
    return match element.get("open") {
        Some(value) => value.as_bool().unwrap_or(false),
        None => true,
    };
}

/// Whether `id` sits inside a closed overlay's subtree.
///
/// The scene walk skips a closed overlay entirely (subtree included), so
/// nothing under it is drawn. Interaction has to apply the same rule in
/// the other direction, or a control the user cannot see would still
/// report `hovered` and light up a binding.
pub fn is_inside_a_closed_overlay(tree: &ElementTree, id: ElementId) -> bool {
    let mut current = Some(id);
    while let Some(handle) = current {
        let element = &tree.arena[handle];
        if is_overlay(element) && !is_open(element) {
            return true;
        }
        current = element.parent;
    }
    return false;
}

/// The topmost open modal dialog, or `None`.
///
/// "Topmost" is the last one in document order, matching the overlay paint
/// order (later overlays composite later, hence on top). A dialog is modal
/// by default: `modal = false` keeps it out of this scan so a Popover can
/// leave the content behind it live.
pub fn active_modal(tree: &ElementTree) -> Option<ElementId> {
    let mut found = None;
    tree.visit_pre_order(|id, element| {
        if element.ty == "Dialog"
            && is_open(element)
            && element
                .get("modal")
                .and_then(|value| return value.as_bool().ok())
                .unwrap_or(true)
        {
            found = Some(id);
        }
    });
    return found;
}

/// Whether `id` lies behind the modal `modal` — i.e. outside its subtree.
/// Everything behind a modal is inert, so the host consults this before
/// dispatching a pointer or key event.
pub fn is_behind_modal(tree: &ElementTree, modal: ElementId, id: ElementId) -> bool {
    let mut current = Some(id);
    while let Some(handle) = current {
        if handle == modal {
            return false;
        }
        current = tree.arena[handle].parent;
    }
    return true;
}

/// Whether a click on the backdrop or Escape should close the dialog
/// (`dismiss_on_backdrop`, default true).
pub fn dismisses_on_backdrop(element: &Element) -> bool {
    return element
        .get("dismiss_on_backdrop")
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(true);
}
