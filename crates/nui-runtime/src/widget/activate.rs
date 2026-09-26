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

/// The Boolean a click toggles on a `Toggle` widget. `RadioButton` names
/// it `selected` (mutual exclusion within a `group`); CheckBox/Switch use
/// `checked`.
pub fn toggle_property(ty: &str) -> Option<&'static str> {
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
    let ty = tree.arena[id].ty.clone();
    let Some(kind) = WidgetKind::of(&ty) else {
        return false;
    };
    let mut wrote = false;
    if kind == WidgetKind::Toggle
        && let Some(property) = toggle_property(&ty)
    {
        let current = tree.arena[id]
            .get(property)
            .and_then(|value| return value.as_bool().ok())
            .unwrap_or(false);
        // A RadioButton *selects*, it does not toggle: clicking the chosen
        // one again must not clear it (there is no "none of the above" in
        // a radio group). CheckBox and Switch do flip.
        let wanted = if ty == "RadioButton" { true } else { !current };
        if engine.set_direct(tree, id, property, Value::Bool(wanted)) {
            let _ = engine.emit_signal(tree, id, "changed");
            wrote = true;
        }
        // Radio buttons are mutually exclusive within a `group`: selecting
        // one clears every sibling that names the same group. There is no
        // group container element, so the scan is the selection mechanism.
        if ty == "RadioButton" && wanted {
            wrote |= clear_group(engine, tree, id);
        }
    }
    // A `Spin` control takes no toggle and no step here — see the module
    // doc — but a click on it still emits `click` below.
    let _ = engine.emit_signal(tree, id, "click");
    return wrote;
}

/// Clears `selected` on every RadioButton sharing `id`'s `group`, except
/// `id` itself. Returns whether anything was written.
///
/// A RadioButton with no `group` belongs to no group, so nothing else is
/// cleared: the scan is by name, and a missing name matches nothing.
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
            && element.ty == "RadioButton"
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

/// Whether Space/Enter activation applies: the element's type is a widget
/// and it is enabled.
pub fn is_activatable(tree: &ElementTree, id: ElementId, ty: &str) -> bool {
    return WidgetKind::of(ty).is_some() && is_enabled(&tree.arena[id]);
}
