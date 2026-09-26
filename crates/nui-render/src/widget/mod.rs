//! Built-in control painting (FUTURE 控件扩展 批次 1).
//!
//! Every control is assembled from *parts* — a surface rect, a stroke
//! outline, a label, an indicator glyph — rather than owning a bespoke
//! renderer. This module holds the part vocabulary plus the resolution of
//! a widget's visual state into concrete colors, so a control's look is
//! one table lookup instead of a scattering of `if hovered { ... }`
//! branches across the scene builder.
//!
//! # Why parts and not a `Widget` trait
//!
//! The scene builder walks the element tree and produces a flat draw list.
//! A trait would force the walk to hand control to each widget mid-walk
//! (and then need a way to add child draws back into the parent's list in
//! the right order). Parts keep the walk in charge: a widget resolves to a
//! `Vec<WidgetPart>`, the builder appends them, and painting order stays a
//! property of the tree rather than of a trait's internals.
//!
//! # State resolution
//!
//! [`VisualState`] is the *four* states a control can be styled for
//! (`disabled` > `pressed` > `hovered` > `normal`, checked separately).
//! [`state`] picks one from the `hovered`/`armed`/`pressed`/`focused`/
//! `disabled` properties the widget tracker writes
//! ([`nui_runtime::widget`]), so the renderer never reads raw pointer
//! state.
//!
//! # Layout
//!
//! | module | contents |
//! |---|---|
//! | [`state`] | [`VisualState`], [`Palette`], the color helpers |
//! | [`part`] | [`WidgetPart`] and the shared `label` used by several controls |
//! | [`button`] [`checkbox`] [`radio`] [`switch`] [`slider`] [`dialog`] | one control each |
//!
//! Each control module is a pure `element + bounds -> Vec<WidgetPart>`
//! function plus the tests for its geometry. Nothing in here touches the
//! [`crate::scene::SceneBuilder`] — [`parts_for`] is the single seam the
//! scene walk uses, and [`crate::scene`] owns turning parts into draws.

mod button;
mod checkbox;
mod dialog;
mod panel;
mod part;
mod radio;
mod separator;
mod slider;
mod spinbox;
mod state;
mod switch;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

pub use part::{DropShadow, WidgetPart, surface};
pub use state::{Palette, VisualState, darken, lighten, variant_palette, with_alpha_scale};

use nui_core::{Rect, Size};
use nui_runtime::Element;

/// Resolves one built-in control into the parts that paint it.
///
/// Returns an empty list for a type this module does not own — the caller
/// has already established that `ty` is a control, so an empty list means
/// a control that genuinely paints nothing this frame.
///
/// `viewport` is the window box in dp. Only [`dialog`] needs it, to size
/// its modal backdrop; every other control is bounded by its own box.
pub fn parts_for(
    ty: &str,
    element: &Element,
    bounds: Rect,
    viewport: Size,
    state: VisualState,
    palette: Palette,
) -> Vec<WidgetPart> {
    return match ty {
        "Button" => button::parts(element, bounds, state, palette),
        "CheckBox" => checkbox::parts(element, bounds, state, palette),
        "RadioButton" => radio::parts(element, bounds, state, palette),
        "Switch" => switch::parts(element, bounds, state, palette),
        "Slider" => slider::parts(element, bounds, state, palette),
        "SpinBox" => spinbox::parts(element, bounds, state, palette),
        "Dialog" => dialog::parts(element, bounds, viewport, state, palette),
        // Containers with chrome (FUTURE 批次 5). Their *layout* is a
        // column (see `nui-layout`); this is the panel drawn behind it.
        "Panel" | "Card" => panel::parts(element, bounds, state, palette),
        "Separator" => separator::parts(element, bounds, state, palette),
        _ => Vec::new(),
    };
}
