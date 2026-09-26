//! `Slider` dragging: pointer position in, value out.
//!
//! Only `Slider` has this behaviour — the caller has already checked the
//! kind. The mapping runs through the *thumb travel* rather than the whole
//! box, because the thumb is square and centred on the cross axis: using
//! the raw box would make both extremes unreachable.

use crate::binding::Engine;
use crate::element::{ElementId, ElementTree};

use super::bounds::absolute_bounds;
use super::props::{dp_or, number, number_of, write_number};

/// Applies a captured drag to a `Drag` widget: maps the pointer's x
/// position onto the element's `[min, max]` value range, quantised by
/// `step`, and writes `value` when it moved.
///
/// Returns the elements whose value changed. Only `Slider` has this
/// behaviour, so the caller has already checked the kind.
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
    let mut value = min + ratio * span;
    if step > 0.0 {
        // Quantise to the nearest step from `min`, then clamp again so
        // rounding cannot push past an end.
        value = min + ((value - min) / step).round() * step;
        value = value.clamp(min, max);
    }
    return write_number(engine, tree, id, "value", f64::from(value));
}
