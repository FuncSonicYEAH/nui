//! A form label: a `Text` that points at the control it names.
//!
//! There is no `Label` element. A label *is* a `Text` — so every text
//! property, the wrapping, the styling and the `content <-` bindings keep
//! working — that may declare two extra things:
//!
//! - `for = <element id>`: the control a click on the label focuses. The
//!   association lives on the label, not on the field, because a label
//!   knows what it names and a field has no idea it is being named.
//! - `required = true`: draws the asterisk a form needs. It is the
//!   *label's* decoration because that is where the marker is read from —
//!   a reader looks at the label, not at the box, to find out whether the
//!   field is mandatory.
//!
//! Both live here rather than in the painter because two readers need
//! them: the host routes the click that `for` implies, and the scene
//! builder draws both the target and the marker.

use crate::element::Element;

/// The gap (dp) between a label's text and its required marker.
pub const ASTERISK_GAP_DP: f32 = 2.0;

/// The glyph a `required` label marks itself with.
pub const REQUIRED_MARK: &str = "*";

/// The element id a label points at — `None` for anything that is not a
/// `Text`, or names nothing.
///
/// Restricted to `Text` on purpose: `for` on a `Button` would be a
/// document bug that silently "works" (a click would focus something
/// unrelated), and the type is the cheapest place to refuse it.
pub fn label_target(element: &Element) -> Option<&str> {
    if element.ty != "Text" {
        return None;
    }
    let name = element.get("for")?.as_enum().ok()?;
    if name.is_empty() {
        return None;
    }
    return Some(name);
}

/// Whether a label draws the required marker.
pub fn is_required(element: &Element) -> bool {
    if label_target(element).is_none() {
        return false;
    }
    return element
        .get("required")
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(false);
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_core::Value;

    fn label(name: Option<&str>) -> Element {
        let mut element = Element::new("Text", None);
        element.set("content", Value::String("Name".to_string()));
        if let Some(name) = name {
            element.set("for", Value::String(name.to_string()));
        }
        return element;
    }

    #[test]
    fn a_text_naming_a_control_is_a_label() {
        assert_eq!(label_target(&label(Some("user"))), Some("user"));
        assert_eq!(label_target(&label(None)), None, "no `for`, no label");
    }

    #[test]
    fn only_a_text_can_be_a_label() {
        let mut button = Element::new("Button", None);
        button.set("for", Value::String("user".to_string()));
        assert_eq!(label_target(&button), None, "`for` on a Button is not one");
    }

    #[test]
    fn required_needs_a_target_and_a_true() {
        let mut required = label(Some("user"));
        required.set("required", Value::Bool(true));
        assert!(is_required(&required));
        required.set("required", Value::Bool(false));
        assert!(!is_required(&required));
        // A decorative asterisk on a plain Text is not a required marker.
        let mut orphan = label(None);
        orphan.set("required", Value::Bool(true));
        assert!(!is_required(&orphan));
    }
}
