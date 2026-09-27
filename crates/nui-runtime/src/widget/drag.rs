//! Pointer position in, value(s) out.
//!
//! Two shapes share the mapping: a `Slider` writes one `value`, and a range
//! control writes whichever of two ends is nearer the pointer. The travel
//! arithmetic is identical, so it is computed once here and only the
//! *choice* of property differs.

use crate::binding::Engine;
use crate::element::{ElementId, ElementTree};

use super::bounds::absolute_bounds;
use super::props::{dp_or, number, number_of, number_of_64, write_number};

/// Applies a captured drag to a `Drag` or `Range` widget: maps the pointer's
/// position onto the element's `[min, max]` value range, quantised by
/// `step`, and writes the value — or, for a range control, whichever of its
/// two ends is nearer. Returns whether anything changed.
///
/// The caller has already checked the kind.
///
/// The mapping runs through the *thumb travel*, not the full box: the
/// thumb is square and sits on the cross axis, so its centre sweeps
/// `[thumb / 2, length - thumb / 2]`. Using the raw box would make the
/// extremes unreachable (the thumb would hang half off each end).
pub fn drag_value(
    engine: &mut Engine,
    tree: &mut ElementTree,
    id: ElementId,
    position: nui_core::Point,
    range_properties: Option<(&str, &str)>,
) -> bool {
    let Some(bounds) = absolute_bounds(tree, id) else {
        return false;
    };
    let element = &tree.arena[id];
    let vertical = matches!(
        element
            .get("orientation")
            .and_then(|value| return value.as_enum().ok()),
        Some("vertical") | Some("Vertical")
    );
    let min = number(element, "min").unwrap_or(0.0);
    let max = number(element, "max").unwrap_or(1.0);
    let step = number(element, "step").unwrap_or(0.0);
    // A degenerate range would divide by zero; park the value at `min`.
    let span = max - min;
    if span <= 0.0 {
        return write_number(engine, tree, id, "value", f64::from(min));
    }
    // Thumb travel: inset by half the thumb on both ends. The thumb is
    // square and sits on the *cross* axis, so a horizontal slider's thumb
    // is as wide as the element is tall and a vertical one's is as tall as
    // the element is wide. Using `height` unconditionally would make a
    // vertical slider read its own track length as the thumb size and
    // collapse the travel to zero.
    let cross_axis = if vertical { "width" } else { "height" };
    let thumb = element
        .get("thumb_size")
        .and_then(|value| return number_of(value))
        .unwrap_or_else(|| return dp_or(element, cross_axis, 16.0));
    let (offset, extent) = if vertical {
        (position.y - bounds.origin.y, bounds.size.height)
    } else {
        (position.x - bounds.origin.x, bounds.size.width)
    };
    let travel = (extent - thumb).max(f32::EPSILON);
    let ratio = ((offset - thumb * 0.5) / travel).clamp(0.0, 1.0);
    let mut value = f64::from(min) + f64::from(ratio) * f64::from(span);
    if step > 0.0 {
        // Quantise to the nearest step from `min`, then clamp again so
        // rounding cannot push past an end.
        let min = f64::from(min);
        let step = f64::from(step);
        value = min + ((value - min) / step).round() * step;
        value = value.clamp(min, f64::from(max));
    }
    let Some((first, second)) = range_properties else {
        return write_number(engine, tree, id, "value", value);
    };
    // Two ends, one gesture: the nearer one moves. Distance is measured in
    // *value* space rather than pixels, so the choice stays right even if
    // the ends are placed by something other than a linear map, and a tie
    // resolves to the low end so a click exactly between the two always
    // does the same thing.
    let first_value = end_value(tree, id, first);
    let second_value = end_value(tree, id, second);
    let moving = if (value - first_value).abs() <= (value - second_value).abs() {
        first
    } else {
        second
    };
    return write_number(engine, tree, id, moving, value);
}

/// One of a range control's two end values, as `f64`; `min` when unset, so
/// a control that has not been given its ends yet still picks one to move.
fn end_value(tree: &ElementTree, id: ElementId, property: &str) -> f64 {
    let element = &tree.arena[id];
    return element
        .get(property)
        .and_then(number_of_64)
        .unwrap_or_else(|| return f64::from(number(element, "min").unwrap_or(0.0)));
}
