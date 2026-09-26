//! Widget interaction foundation (FUTURE 控件扩展 批次 0).
//!
//! nui ships **zero built-in widgets**: `Button`, `CheckBox`, `Slider` and
//! friends are ordinary element type names that the renderer's fallback
//! branch paints as a plain rect. What they lack is not geometry but
//! *interaction* — hover tracking, press/arm state, pointer capture, and
//! keyboard activation.
//!
//! This module is that missing layer, and it deliberately lives in
//! `nui-runtime` rather than in the `.nui` language:
//!
//! - Widget state (`hovered` / `pressed` / `armed` / `focused`) is
//!   **transient** — it lasts exactly as long as a pointer gesture. A
//!   nui `component` property is **persistent** (declared once, own
//!   storage, survives every frame), so `.nui`-only widgets would pay a
//!   property slot per state per control and still have no way to observe
//!   a pointer that left the element mid-drag.
//! - Hit testing, focus, and keyboard routing already live in the host.
//!   A widget layer that sat anywhere else would need a second copy of
//!   all three.
//!
//! # Mechanism
//!
//! The host drives [`WidgetStates::update`] from its event loop. The
//! tracker walks the tree for widget-class elements (see
//! [`WidgetStates::is_widget`]), resolves each one's state from the raw
//! pointer input plus the `enabled` property, and writes the result back
//! into the *element properties* — `hovered`, `pressed`, `armed`,
//! `focused`, `disabled`. Writing them as properties (rather than keeping
//! them in a side table) means `.nui` code can bind to them directly:
//!
//! ```text
//! Rectangle(fill <- primary.hovered ? #3f6fd8 : #4a7bf7)
//! ```
//!
//! Writes go through [`Engine::set_direct`](crate::binding::Engine::set_direct)
//! so they carry [`ChangeSource::Host`](crate::notify::ChangeSource::Host)
//! and never clobber a user's `<-` binding on the same property.
//!
//! # Transitions
//!
//! A pointing device gesture is modelled as a small state machine
//! (W3C UI Events / Qt Quick `Button` semantics):
//!
//! | input | transition |
//! |---|---|
//! | pointer moves over element | `hovered = true` |
//! | pointer moves off element | `hovered = false`, `armed = false` |
//! | press inside element | `pressed = true`, `armed = true`, capture the pointer |
//! | release inside the captured element | `click` fires on that element |
//! | release outside | `armed = false`, no `click` |
//! | pointer leaves while captured | `hovered = false`, `armed = false` (still captured) |
//! | pointer returns while captured | `hovered = true`, `armed = true` again |
//! | Space/Enter with focus on the element | `click` fires |
//!
//! `pressed` means "the button that started this gesture is still down on
//! this element"; `armed` means "a release right now would fire `click`".
//! They differ in exactly one case: a drag out of the element and back.
//! `hovered` is purely geometric and follows the pointer even mid-drag, so
//! a hover highlight does not stay lit while the pointer is elsewhere.
//!
//! Pointer capture keeps `PointerMoved` flowing to the element that was
//! pressed even after the pointer leaves its box, which is what makes a
//! Slider drag work at all.
//!
//! # Layout
//!
//! | module | contents |
//! |---|---|
//! | [`bounds`] | where a widget actually is on screen |
//! | [`props`] | numeric property read/write |
//! | [`activate`] | what a click does (toggle, signal, radio groups) |
//! | [`drag`] | `Slider` dragging: pointer position to value |
//! | [`overlay`] | overlays and modal dialogs |
//!
//! The state machine itself lives here; each module above is one
//! interaction concern it composes.

mod activate;
mod bounds;
pub mod chrome;
mod drag;
mod label;
mod overlay;
mod props;
mod scroll;
mod spin;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;

pub use activate::{activate, is_activatable, toggle_property};
pub use chrome::{
    TEXT_INSET_DP, text_inset, text_inset_extra, text_size, title_inset, title_rule_y, title_size,
    title_text,
};
pub use label::{ASTERISK_GAP_DP, REQUIRED_MARK, is_required, label_target};
pub use overlay::{
    active_modal, dismisses_on_backdrop, is_behind_modal, is_inside_a_closed_overlay, is_open,
    is_overlay,
};
pub use scroll::{DEFAULT_ROW_HEIGHT, RowSlot, max_scroll_y, row_slot};
pub use spin::{
    SpinRange, direction_at, display_value, format_value, step, step_focused, strip_width,
};

use nui_core::Value;

use crate::binding::Engine;
use crate::element::{Element, ElementId, ElementTree};

use bounds::absolute_bounds;
use drag::drag_value;

/// Element type names the widget layer treats as interactive controls.
///
/// Kept as a flat list rather than a registry trait because the set is
/// small, fixed, and shared by the host (hit testing, keyboard) and the
/// renderer (which paints each one). A host *custom* component built from
/// [`ElementBehavior`](crate::registry::ElementBehavior) gets the same
/// state by declaring the matching type name.
const WIDGET_TYPES: &[&str] = &[
    "Button",
    "CheckBox",
    "Switch",
    "Slider",
    "RadioButton",
    "Dialog",
    "SpinBox",
];

/// Which widget behaviour an element wants from the pointer. Several
/// element types share one behaviour (CheckBox/Switch/RadioButton are all
/// "click toggles"), so the tracker selects on this rather than on the raw
/// type name at every call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    /// Activates on click; no drag, no toggle. `Button`, `Dialog`.
    Momentary,
    /// Click sets a Boolean (`checked` / `selected`) rather than emitting
    /// only. `CheckBox`, `Switch`, `RadioButton`.
    Toggle,
    /// Click sets a position; a captured drag updates `value`. `Slider`.
    Drag,
    /// Click, wheel and arrow keys step a number. `SpinBox`.
    ///
    /// Its own kind rather than a `Momentary`: a stepper's click has a
    /// *direction* (the half of the box that was pressed), and it acts on
    /// press rather than on release, so the generic click path would both
    /// lose the direction and step twice.
    Spin,
}

impl WidgetKind {
    /// The behaviour for an element type name, or `None` when the type is
    /// not a widget. CheckBox/Switch/RadioButton are click toggles: the
    /// renderer distinguishes them, the interaction does not.
    pub fn of(ty: &str) -> Option<WidgetKind> {
        return match ty {
            "Button" | "Dialog" => Some(WidgetKind::Momentary),
            "CheckBox" | "Switch" | "RadioButton" => Some(WidgetKind::Toggle),
            "Slider" => Some(WidgetKind::Drag),
            "SpinBox" => Some(WidgetKind::Spin),
            _ => None,
        };
    }
}

/// Whether an element is a widget type (host hit testing / keyboard).
pub fn is_widget_type(ty: &str) -> bool {
    return WIDGET_TYPES.contains(&ty);
}

/// Widget types that join the keyboard focus order (Tab, Space/Enter).
///
/// Deliberately *not* [`WIDGET_TYPES`]: a `Dialog` is interactive but is
/// not a tab stop (it is a container that owns focus *inside* itself), so
/// a control the user can actually operate is a different list from "types
/// the widget layer knows". Text fields are the third entry in the focus
/// order and are marked where they are instantiated (`TextInput` is not in
/// `WIDGET_TYPES` — it has no pointer behaviour of its own).
///
/// This is what makes keyboard activation reach a `Button` at all, and
/// what a form label's `for` needs: clicking a label focuses its control.
const FOCUSABLE_TYPES: &[&str] = &[
    "Button",
    "CheckBox",
    "Switch",
    "Slider",
    "RadioButton",
    "SpinBox",
];

/// Whether an element type takes keyboard focus.
pub fn is_focusable_type(ty: &str) -> bool {
    return FOCUSABLE_TYPES.contains(&ty);
}

/// Reads `element.enabled`, defaulting to `true` (an element has to opt
/// *out* of interaction; there is no opt-in flag to forget).
pub fn is_enabled(element: &Element) -> bool {
    return element
        .get("enabled")
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(true);
}

/// Reads `element.visible`, defaulting to `true`.
///
/// The one flag all three layers read, which is why it lives here rather
/// than in any of them:
///
/// | layer | what `visible = false` means |
/// |---|---|
/// | layout | `display: none` — no box, no space, subtree not measured |
/// | scene | the walk returns before painting anything, subtree included |
/// | hit test | the walk prunes the subtree, *same predicate as the scene* |
///
/// The hit-test row used to say "covered by the missing box" — it is not:
/// `write_back` skips a hidden subtree, so its **last laid-out boxes stay
/// in the element slots** and a hidden page's stale geometry will steal
/// clicks from whatever is visible underneath. Geometry consumers must
/// prune on this flag, not on box emptiness. See [`is_offscreen`] for the
/// ancestor-walking form.
///
/// Distinct from [`overlay::is_open`], which is a *subtype's* own switch
/// (`Dialog.open`) rather than a general one: an element can be visible
/// and not yet open, or open and hidden by an ancestor.
pub fn is_visible(element: &Element) -> bool {
    return element
        .get("visible")
        .and_then(|value| return value.as_bool().ok())
        .unwrap_or(true);
}

/// Whether `id` sits inside a subtree the scene does not draw: the
/// element itself or an ancestor is `visible = false`, or an ancestor is
/// a closed overlay.
///
/// The scene walk prunes those subtrees, and layout gives them no box —
/// but their *stale* boxes (from when they were last visible) stay in
/// the element slots. Anything that consults geometry per element —
/// hover, press routing — must apply the same predicate, or an invisible
/// widget lights a `hovered` binding from a box it no longer occupies.
/// The hit test's top-down walk prunes with the same two checks inline,
/// where visiting the ancestor first makes the walk form unnecessary.
pub fn is_offscreen(tree: &ElementTree, id: ElementId) -> bool {
    let mut current = Some(id);
    while let Some(handle) = current {
        let element = &tree.arena[handle];
        if !is_visible(element) {
            return true;
        }
        if is_overlay(element) && !is_open(element) {
            return true;
        }
        current = element.parent;
    }
    return false;
}

/// The interaction state of one widget, as resolved from the raw pointer
/// input for the current frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WidgetState {
    /// Pointer is over the element (or over it while captured).
    pub hovered: bool,
    /// The press that started the current gesture landed on this element
    /// and the pointer is still down.
    pub pressed: bool,
    /// A release right now would activate the element (pressed *and* the
    /// pointer is currently inside the box).
    pub armed: bool,
    /// The engine's focus is on this element.
    pub focused: bool,
    /// `enabled = false`: no hover, no press, no keyboard.
    pub disabled: bool,
}

impl WidgetState {
    /// Only the visual states participate in change detection; `pressed`
    /// and `armed` are gesture bookkeeping.
    fn visual(self) -> (bool, bool, bool, bool) {
        return (self.hovered, self.armed, self.focused, self.disabled);
    }
}

/// Raw pointer input for one update pass: what the host knows before the
/// tracker resolves it per element.
#[derive(Debug, Clone, Copy, Default)]
pub struct PointerInput {
    /// Pointer position in dp.
    pub position: nui_core::Point,
    /// Cursor is inside the window.
    pub inside: bool,
    /// Primary button is down.
    pub down: bool,
}

/// The tracker's cross-frame memory: which element owns the current
/// gesture. Everything else derives from the pointer input each pass.
#[derive(Debug, Default)]
pub struct WidgetStates {
    /// Element that received the press (pointer capture target).
    pub captured: Option<ElementId>,
    /// Latched element states from the previous pass, for change
    /// detection (avoids dirtying the tree every frame).
    previous: std::collections::HashMap<ElementId, WidgetState>,
}

impl WidgetStates {
    /// Creates an empty tracker.
    pub fn new() -> WidgetStates {
        return WidgetStates::default();
    }

    /// Whether `ty` participates in widget interaction.
    pub fn is_widget(&self, ty: &str) -> bool {
        return is_widget_type(ty);
    }

    /// Records a press: the element under the pointer captures it.
    pub fn capture(&mut self, element: Option<ElementId>) {
        self.captured = element;
    }

    /// Drops the capture (after a release, or when the document rebuilds).
    pub fn release_capture(&mut self) {
        self.captured = None;
    }

    /// Forgets all latched state (hot reload replaces every handle).
    pub fn reset(&mut self) {
        self.captured = None;
        self.previous.clear();
    }

    /// The element the gesture is locked to, if any.
    pub fn captured(&self) -> Option<ElementId> {
        return self.captured;
    }

    /// Recomputes every widget's state against `input` and writes the
    /// changed ones back into the element properties.
    ///
    /// Returns the elements whose *visual* state changed (the host uses
    /// this to decide whether a redraw is needed).
    pub fn update(
        &mut self,
        engine: &mut Engine,
        tree: &mut ElementTree,
        input: PointerInput,
    ) -> Vec<ElementId> {
        // A captured element that left the tree (a `For` row rebuild, a
        // hot reload) must not keep the gesture.
        if let Some(captured) = self.captured
            && !tree.arena.contains_key(captured)
        {
            self.captured = None;
        }
        let focused = engine.focused();
        let mut ids = Vec::new();
        tree.visit_pre_order(|id, element| {
            if is_widget_type(&element.ty) {
                ids.push(id);
            }
        });
        let mut changed = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            let state = self.resolve(tree, id, input, focused);
            let visual = state.visual();
            let previous = self.previous.get(&id).map(|state| return state.visual());
            if previous != Some(visual) {
                changed.push(id);
            }
            // `pressed`/`armed` are recomputed every pass but only written
            // when they actually differ, so an idle pointer costs nothing.
            self.write(engine, tree, id, state);
            self.previous.insert(id, state);
            seen.insert(id);
        }
        // A captured `Drag` widget follows the pointer for as long as the
        // gesture lasts — including outside its own box, which is the
        // whole point of the capture.
        if input.down
            && let Some(captured) = self.captured
            && tree.arena.contains_key(captured)
            && WidgetKind::of(&tree.arena[captured].ty) == Some(WidgetKind::Drag)
            && drag_value(engine, tree, captured, input.position)
        {
            changed.push(captured);
        }
        // Forget elements that are gone.
        self.previous.retain(|id, _| return seen.contains(id));
        return changed;
    }

    /// Reads the element's box and resolves its state. Returns the state
    /// without writing anything.
    fn resolve(
        &self,
        tree: &ElementTree,
        id: ElementId,
        input: PointerInput,
        focused: Option<ElementId>,
    ) -> WidgetState {
        let element = &tree.arena[id];
        let enabled = is_enabled(element);
        let is_focused = focused == Some(id);
        // A widget the scene does not draw is not on screen at all: an
        // invisible subtree (a hidden page keeps its *stale* boxes —
        // `write_back` skips it) or a closed overlay. Reporting `hovered`
        // here would light a binding for something the user cannot see —
        // or worse, an off-screen hover would fight the visible widget at
        // the same point. Treat it exactly like a disabled one — inert,
        // but keep `disabled` false so the two causes stay distinguishable.
        if !enabled || is_offscreen(tree, id) {
            return WidgetState {
                hovered: false,
                pressed: false,
                armed: false,
                focused: is_focused,
                disabled: !enabled,
            };
        }
        // Geometry in absolute dp. Layout already writes `x`/`y` fully
        // accumulated, so `absolute_bounds` reads the element's own origin
        // (adjusting only for `Scroll` viewport translation) — see its
        // doc for the double-counting bug that lived here.
        let bounds = absolute_bounds(tree, id);
        let inside =
            input.inside && bounds.is_some_and(|rect| return rect.contains(input.position));
        let captured = self.captured == Some(id);
        // `hovered` is purely geometric: a captured drag that leaves the
        // box turns the hover highlight off (Qt Quick says the same — the
        // press stays `pressed`, the highlight does not). `pressed` /
        // `armed` carry the gesture.
        let hovered = inside;
        let pressed = captured && input.down;
        let armed = pressed && inside;
        return WidgetState {
            hovered,
            pressed,
            armed,
            focused: is_focused,
            disabled: false,
        };
    }

    /// Writes the state into the element's property slots, skipping
    /// no-op writes so an idle pointer does not dirty the tree every
    /// frame. Every state is written on the first pass, so a `.nui`
    /// binding on `hovered` has a value to read even for a widget that
    /// never sees a pointer.
    fn write(
        &mut self,
        engine: &mut Engine,
        tree: &mut ElementTree,
        id: ElementId,
        state: WidgetState,
    ) {
        const NAMES: [&str; 5] = ["hovered", "armed", "pressed", "focused", "disabled"];
        let values = [
            Value::Bool(state.hovered),
            Value::Bool(state.armed),
            Value::Bool(state.pressed),
            Value::Bool(state.focused),
            Value::Bool(state.disabled),
        ];
        for (name, value) in NAMES.iter().zip(values) {
            let current = tree.arena[id]
                .get(name)
                .and_then(|v| return v.as_bool().ok());
            if current == value.as_bool().ok() {
                continue;
            }
            engine.set_direct(tree, id, name, value);
        }
    }
}
