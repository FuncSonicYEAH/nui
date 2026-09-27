//! What a click does.
//!
//! A `Momentary` control (Button, Dialog) only fires `click`. A `Toggle`
//! flips its bound Boolean — `checked` for CheckBox/Switch, `selected` for
//! RadioButton — and emits `changed` only when it actually moved. Radio
//! buttons additionally keep their `group` mutually exclusive; clearing
//! the others is *also* a change, so the host gets one signal per control
//! that moved.
//!
//! A `Spin` control (SpinBox) is *not* stepped here. A stepper's step has a
//! direction, and "activate" does not: the pointer knows which half of the
//! box it pressed and the keyboard knows whether Up or Down was pressed, so
//! both step directly (see the host and [`crate::widget::step`]) rather than
//! coming through here and losing the one thing that matters. What a click
//! still does for a stepper is emit `click`.

use nui_core::Value;

use crate::binding::Engine;
use crate::element::{ElementId, ElementTree};

use super::{WidgetKind, is_enabled};

/// The Boolean a click toggles on a built-in `Toggle` widget.
/// `RadioButton` names it `selected` (mutual exclusion within a `group`);
/// CheckBox/Switch use `checked`.
///
/// A host component declares its own in
/// [`ComponentDesc::interaction`](crate::registry::ComponentDesc::interaction);
/// this is the built-in half, reached through [`Interaction::of_type`].
pub fn toggle_property(ty: &str) -> Option<&'static str> {
    return toggle_property_of_type(ty);
}

/// Implementation of [`toggle_property`], separated so
/// [`Interaction::of_type`](super::Interaction::of_type) can reach it
/// without going through the public name.
pub(crate) fn toggle_property_of_type(ty: &str) -> Option<&'static str> {
    return match ty {
        "CheckBox" | "Switch" => Some("checked"),
        "RadioButton" => Some("selected"),
        _ => None,
    };
}

/// Applies the click activation semantics for one widget: a click toggles
/// a `Toggle` widget's Boolean and emits `changed`; every widget emits
/// `click`. Returns whether anything was written.
///
/// Shared by the pointer path (release inside) and the keyboard path
/// (Space/Enter with focus), so both behave identically.
pub fn activate(engine: &mut Engine, tree: &mut ElementTree, id: ElementId) -> bool {
    // Resolved once, up front. A component instance's registration and a
    // built-in's type name both answer "what does a click do here", and the
    // two must not be consulted separately further down — they would then
    // be able to disagree about the same element.
    let interaction = engine.interaction(tree, id);
    let Some(kind) = interaction.kind else {
        return false;
    };
    let mut wrote = false;
    if kind == WidgetKind::Toggle
        && let Some(property) = interaction.toggle_property
    {
        let current = tree.arena[id]
            .get(property)
            .and_then(|value| return value.as_bool().ok())
            .unwrap_or(false);
        // A `selected` control *selects*, it does not toggle: clicking the
        // chosen one again must not clear it (there is no "none of the
        // above" in a radio group). `checked` controls do flip. Keying off
        // the property name rather than the type is what lets a host
        // component opt into either convention by declaring the same name
        // the built-in uses.
        let wanted = if property == "selected" {
            true
        } else {
            !current
        };
        if engine.set_direct(tree, id, property, Value::Bool(wanted)) {
            let _ = engine.emit_signal(tree, id, "changed");
            wrote = true;
        }
        // Selectors are mutually exclusive within a `group`: selecting one
        // clears every sibling that names the same group. There is no group
        // container element, so the scan is the selection mechanism.
        if property == "selected" && wanted {
            wrote |= clear_group(engine, tree, id);
        }
    }
    // A `Spin` control takes no toggle and no step here — see the module
    // doc — but a click on it still emits `click` below.
    let _ = engine.emit_signal(tree, id, "click");
    return wrote;
}

/// Clears `selected` on every selector sharing `id`'s `group`, except `id`
/// itself. Returns whether anything was written.
///
/// A selector with no `group` belongs to no group, so nothing else is
/// cleared: the scan is by name, and a missing name matches nothing. A
/// sibling is matched by its *behaviour*, not its type name, so a host
/// component that declares a `selected` toggle groups with the built-in
/// radio buttons rather than needing its own scan.
fn clear_group(engine: &mut Engine, tree: &mut ElementTree, id: ElementId) -> bool {
    let Some(group) = tree.arena[id]
        .get("group")
        .and_then(|value| return value.as_enum().ok().map(str::to_string))
    else {
        return false;
    };
    let mut others = Vec::new();
    tree.visit_pre_order(|other, element| {
        if other != id
            && engine.interaction(tree, other).toggle_property == Some("selected")
            && element
                .get("group")
                .and_then(|value| return value.as_enum().ok())
                == Some(group.as_str())
        {
            others.push(other);
        }
    });
    let mut wrote = false;
    for other in others {
        if tree.arena[other].get("selected") == Some(&Value::Bool(true))
            && engine.set_direct(tree, other, "selected", Value::Bool(false))
        {
            let _ = engine.emit_signal(tree, other, "changed");
            wrote = true;
        }
    }
    return wrote;
}

/// Whether Space/Enter activation applies: the element is a control and it
/// is enabled.
pub fn is_activatable(engine: &Engine, tree: &ElementTree, id: ElementId) -> bool {
    return engine.interaction(tree, id).is_control() && is_enabled(&tree.arena[id]);
}
