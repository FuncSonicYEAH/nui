//! Chrome geometry shared by layout and painting.
//!
//! A `Panel` / `Card` / `Dialog` reserves room at the top of its content
//! box for a title bar. Layout has to know that height (children start
//! below the title), and painting has to know it too (the title text and
//! the hairline under it are placed from the same numbers). Two copies of
//! "font size + 8 + 8" in two crates would drift the first time either is
//! tuned, so the formula lives here once and both sides read it.
//!
//! This is the one place where a *control's look* is load-bearing for
//! layout, and it is deliberately confined to two formulas: the title bar
//! and the inset a text field keeps around its text. Everything else about
//! a control's appearance is painted inside a box that layout has already
//! decided.

use crate::element::Element;

use super::props::{dp_or, number_of};

/// Title font size (dp) when `font.size` is unset.
pub const DEFAULT_TITLE_SIZE_DP: f32 = 16.0;

/// Gap between the title text, the hairline under it, and the body.
pub const TITLE_GAP_DP: f32 = 8.0;

/// Gap (dp) a text field keeps between its box and its text when it
/// declares no `padding` of its own.
pub const TEXT_INSET_DP: f32 = 8.0;

/// Font size (dp) a text field's content draws at when it declares no
/// `font.size`.
pub const DEFAULT_TEXT_SIZE_DP: f32 = 16.0;

/// Default content padding (dp) for the chrome'd containers.
///
/// Shared with the painting side so a `Dialog`'s declared padding and the
/// inset its title is drawn at cannot disagree — they did before this
/// module existed (`20` in the dialog painter, `0` in layout).
pub fn default_padding(ty: &str) -> f32 {
    return match ty {
        "Dialog" => 20.0,
        "Panel" | "Card" => 16.0,
        _ => 0.0,
    };
}

/// The element's `title`, or `None` when it is absent or empty.
pub fn title_text(element: &Element) -> Option<&str> {
    let value = element.get("title")?.as_str().ok()?;
    if value.is_empty() {
        return None;
    }
    return Some(value);
}

/// The title's font size in dp.
pub fn title_size(element: &Element) -> f32 {
    return element
        .get("font.size")
        .and_then(|value| return number_of(value))
        .unwrap_or(DEFAULT_TITLE_SIZE_DP);
}

/// Height the title bar adds *below* the element's own top padding; `0.0`
/// when the element declares no title.
///
/// Layout folds this into `padding-top`, so a titled panel behaves exactly
/// like an untitled one from a child's point of view — it just starts
/// lower.
pub fn title_inset(element: &Element) -> f32 {
    if title_text(element).is_none() {
        return 0.0;
    }
    return title_size(element) + TITLE_GAP_DP * 2.0;
}

/// Vertical centre of the title's line box, measured from the element's
/// top edge (i.e. *including* the element's own padding).
pub fn title_center_y(element: &Element) -> f32 {
    return dp_or(element, "padding", default_padding(&element.ty)) + title_size(element) * 0.5;
}

/// The y offset (from the element's top edge) of the hairline separating
/// the title from the body.
pub fn title_rule_y(element: &Element) -> f32 {
    return dp_or(element, "padding", default_padding(&element.ty))
        + title_size(element)
        + TITLE_GAP_DP;
}

/// The gap a text field keeps between its box and its text: its own
/// `padding`, or [`TEXT_INSET_DP`].
///
/// Painting reads this to place the text, the selection and the caret;
/// layout's wrapping measure reads it to decide where the line breaks.
/// They have to agree: a multi-line field that wraps at a different width
/// than it draws at either clips its last word or leaves a ragged gap.
pub fn text_inset(element: &Element) -> f32 {
    return dp_or(element, "padding", TEXT_INSET_DP);
}

/// The part of [`text_inset`] that taffy's style padding does **not**
/// already account for.
///
/// A text field's inset is a *control default*, not a declared style: an
/// element with no `padding` property has no taffy padding (so the measure
/// callback receives the field's full width) yet still draws its text inset
/// by [`TEXT_INSET_DP`]. A wrapping measure must therefore inset the width
/// it wraps at by this remainder — and add it back to the height — for the
/// visual breaks to match the drawn ones. When the document declares
/// `padding` explicitly the two agree and this is zero.
pub fn text_inset_extra(element: &Element) -> f32 {
    return (text_inset(element) - dp_or(element, "padding", 0.0)).max(0.0);
}

/// The font size a text field's *content* draws at.
///
/// The third number layout, painting and the caret all have to agree on: a
/// multi-line field wraps at the size it draws at, and Up/Down walks the
/// lines that wrap produced. Three crates, one formula.
pub fn text_size(element: &Element) -> f32 {
    return element
        .get("font.size")
        .and_then(|value| return number_of(value))
        .unwrap_or(DEFAULT_TEXT_SIZE_DP);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::Value;

    #[test]
    fn an_untitled_element_reserves_nothing() {
        let plain = Element::new("Panel", None);
        assert_eq!(title_inset(&plain), 0.0);
        let mut empty = Element::new("Panel", None);
        empty.set("title", Value::String(String::new()));
        assert_eq!(title_inset(&empty), 0.0, "an empty title is no title");
    }

    #[test]
    fn a_title_reserves_its_line_plus_two_gaps() {
        let mut panel = Element::new("Panel", None);
        panel.set("title", Value::String("Details".to_string()));
        assert_eq!(
            title_inset(&panel),
            DEFAULT_TITLE_SIZE_DP + TITLE_GAP_DP * 2.0
        );
        // The three numbers live in two frames: `title_inset` is measured
        // from the *content* box (layout adds it to `padding-top`), while
        // the rule and the title's centre are measured from the element's
        // top edge. In the shared frame the rule sits below the title and
        // above the body.
        let top = default_padding("Panel");
        assert!(title_rule_y(&panel) > title_center_y(&panel));
        assert!(
            title_rule_y(&panel) < top + title_inset(&panel),
            "the rule must not land under the first body line"
        );
    }

    #[test]
    fn font_size_moves_the_inset_with_it() {
        let mut panel = Element::new("Panel", None);
        panel.set("title", Value::String("Details".to_string()));
        panel.set("font.size", Value::Float(24.0));
        assert_eq!(title_inset(&panel), 24.0 + TITLE_GAP_DP * 2.0);
    }

    #[test]
    fn padding_defaults_follow_the_container() {
        assert_eq!(default_padding("Panel"), 16.0);
        assert_eq!(default_padding("Dialog"), 20.0);
        assert_eq!(default_padding("Column"), 0.0);
        // The title's centre starts from the element's padding, so a
        // dialog's title is not drawn in its very first pixel row.
        let mut dialog = Element::new("Dialog", None);
        dialog.set("title", Value::String("Confirm".to_string()));
        assert_eq!(title_center_y(&dialog), 20.0 + DEFAULT_TITLE_SIZE_DP * 0.5);
    }
}
