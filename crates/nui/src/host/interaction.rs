//! How the host drives the widget layer (FUTURE 控件扩展 批次 0–4).
//!
//! `nui-runtime` owns the widget *state machine* and `nui-render` owns the
//! widget *painting*; this module is the third side of that triangle — the
//! event wiring. It answers three questions the event loop would otherwise
//! inline into a 200-line `match`:
//!
//! - **Who gets the gesture?** `hit_test` finds the element under the
//!   pointer; [`WindowHost::hit_interactive`] then skips past disabled ones
//!   so a disabled control passes the click through instead of swallowing
//!   it.
//! - **Where does the pointer belong?** [`PointerGesture`] records the
//!   capture target, which is what lets a press-and-drag keep updating
//!   after the pointer leaves the control's box (a Slider drag).
//! - **Who owns the input right now?** An open modal dialog does. The gates
//!   here ([`WindowHost::modal_covers`], [`WindowHost::modal_key_gate`])
//!   run *before* anything else in their arm, because a modal that can be
//!   bypassed is not modal.
//! - **What does Up/Down mean?** A multi-line text field walks its shaped
//!   lines; a `SpinBox` takes a step. Both need the *host* — the caret's
//!   line table comes from a real font, and a step needs the pointer's
//!   position — so [`WindowHost::move_vertical`] and
//!   [`WindowHost::step_spin_at`] take the direction the widget layer
//!   cannot know (批次 6).
//!
//! # Layout
//!
//! | piece | role |
//! |---|---|
//! | [`PointerGesture`] | the gesture the host is currently routing |
//! | `on_pointer_*` | one entry point per pointer phase |
//! | `modal_*` | the modal gates, shared by pointer, wheel and keyboard |
//! | `step_spin*` / `move_vertical` | the two direction-carrying gestures (批次 6) |
//! | `hit_interactive` / `cursor_in` / `focus_hit` / `close_dialog` | routing helpers |
//!
//! Everything here is a method on [`WindowHost`] rather than a free
//! function because it all mutates the same four things — the tree, the
//! engine, the cursor position, and the gesture — and threading those
//! through a parameters list would obscure more than the `impl` block does.

use nui_core::{Key, PointerButton};
use nui_runtime::element::ElementId;

use crate::app::hit_test;

use super::WindowHost;

/// Pointer gesture bookkeeping for the current drag (M3 + FUTURE 批次 0).
///
/// `captured` is the element the press landed on, and doubles as the
/// pointer-capture target: `PointerMoved` keeps flowing to it even when the
/// pointer leaves its box, which is what makes a Slider drag work. The
/// *widget* state machine ([`nui_runtime::WidgetStates`]) owns the
/// per-element `hovered`/`armed`/`pressed` properties; this struct owns
/// only the gesture lifecycle the host needs to route events.
#[derive(Debug, Default)]
pub(super) struct PointerGesture {
    /// Element captured by the press (if any). Visible to `host` because
    /// the frame pipeline needs it to tell the tracker a gesture is live.
    pub(super) captured: Option<ElementId>,
    /// Position of the press, for drag deltas (Slider).
    origin: nui_core::Point,
}

impl WindowHost {
    /// Pointer moved: record where, then let the tracker re-resolve hover.
    ///
    /// There is no modal gate here. Hover is *geometric* — the tracker
    /// answers "is the pointer inside this element's box" and nothing else
    /// — so a widget behind an open modal does report `hovered` while the
    /// pointer crosses it. The modal gates below stop a *press*, a wheel
    /// and a key from reaching that layer, which is what "modal" has to
    /// mean; suppressing hover too would need the tracker to know about
    /// modals, and no shipped control binds a visual to it.
    pub(super) fn on_pointer_moved(&mut self, position: nui_core::Point) -> bool {
        self.cursor = position;
        return self.update_widget_states();
    }

    /// Pointer pressed: open a gesture, unless a modal claims it.
    pub(super) fn on_pointer_pressed(
        &mut self,
        button: PointerButton,
        position: nui_core::Point,
    ) -> bool {
        if button != PointerButton::Left {
            return false;
        }
        // Modal gate: a press outside the open dialog's subtree either
        // dismisses it (a backdrop click, when the dialog allows it) or is
        // swallowed. It never reaches the layer underneath.
        if let Some(modal) = self.modal()
            && self.modal_covers(position)
        {
            if nui_runtime::widget::dismisses_on_backdrop(&self.tree.arena[modal]) {
                return self.close_dialog(modal);
            }
            return false;
        }
        let hit = self.hit_interactive(position);
        self.click.captured = hit;
        self.click.origin = position;
        self.widgets.capture(hit);
        self.focus_hit(hit);
        let mut redraw = self.update_widget_states();
        // A stepper's press *is* its step (批次 6): which half of the strip
        // was hit is the direction. The release still fires `click`.
        if let Some(id) = hit
            && self.tree.arena[id].ty == "SpinBox"
        {
            redraw |= self.step_spin_at(id, position);
        }
        return redraw;
    }

    /// Pointer released: close the gesture, and fire `click` when the
    /// release lands back inside the captured element.
    pub(super) fn on_pointer_released(
        &mut self,
        button: PointerButton,
        position: nui_core::Point,
    ) -> bool {
        if button != PointerButton::Left {
            return false;
        }
        // The gesture belongs to the captured element: a release *inside*
        // it fires `click`; a release anywhere else just ends the gesture
        // (a drag out and back re-arms, so the final position decides).
        let captured = self.click.captured.take();
        self.widgets.release_capture();
        let inside = captured.is_some_and(|id| return self.cursor_in(id, position));
        let mut activated = false;
        if let Some(target) = captured
            && inside
        {
            let _ = self.engine.emit_bubble(&mut self.tree, target, "click");
            // Widget activation also maintains the toggle property
            // (CheckBox/Switch/RadioButton) and emits `changed`.
            nui_runtime::widget::activate(&mut self.engine, &mut self.tree, target);
            activated = true;
        }
        let redraw = self.update_widget_states();
        if activated {
            self.run_frame_pipeline(nui_core::Duration::ZERO);
        }
        return activated || redraw;
    }

    /// Space or Enter on the focused widget: fire the same `click` a
    /// pointer release inside the element would. Returns whether it acted.
    ///
    /// Text inputs keep their Space (a literal character) and submit on
    /// Enter through `handle_key`, so they are excluded.
    pub(super) fn activate_focused(&mut self, key: Key) -> bool {
        let Some(focused) = self.engine.focused() else {
            return false;
        };
        if self.tree.arena[focused].is_text_input()
            || !matches!(key, Key::Character(' ') | Key::Enter)
            || !nui_runtime::widget::is_activatable(
                &self.tree,
                focused,
                &self.tree.arena[focused].ty,
            )
        {
            return false;
        }
        nui_runtime::widget::activate(&mut self.engine, &mut self.tree, focused);
        self.update_widget_states();
        self.run_frame_pipeline(nui_core::Duration::ZERO);
        return true;
    }

    /// The keyboard half of the modal gate: `Some(handled)` when the key
    /// was consumed (or the dialog dismissed), `None` when it may proceed.
    pub(super) fn modal_key_gate(&mut self, key: Key) -> Option<bool> {
        let modal = self.modal()?;
        if key == Key::Escape {
            if nui_runtime::widget::dismisses_on_backdrop(&self.tree.arena[modal]) {
                return Some(self.close_dialog(modal));
            }
            return Some(false);
        }
        // Any other key is swallowed unless focus is already inside the
        // dialog, so Tab cannot wander into the inert layer.
        let outside = self
            .engine
            .focused()
            .is_none_or(|id| return nui_runtime::widget::is_behind_modal(&self.tree, modal, id));
        return if outside { Some(false) } else { None };
    }

    /// Whether the modal layer intercepts a pointer at `position`: a hit
    /// outside the dialog's subtree, or empty space. Shared by the press
    /// and the wheel gate.
    pub(super) fn modal_covers(&self, position: nui_core::Point) -> bool {
        let Some(modal) = self.modal() else {
            return false;
        };
        let hit = hit_test(&self.tree, position).map(|hit| return hit.element);
        return hit.is_none_or(|id| {
            return nui_runtime::widget::is_behind_modal(&self.tree, modal, id);
        });
    }

    /// The topmost open modal dialog, if any.
    fn modal(&self) -> Option<ElementId> {
        return nui_runtime::widget::active_modal(&self.tree);
    }

    /// Focuses the nearest focusable ancestor of the clicked element
    /// (clicking empty space blurs). A form label is not focusable itself —
    /// it *names* the control a click should focus (批次 6).
    fn focus_hit(&mut self, hit: Option<ElementId>) {
        let mut current = hit;
        while let Some(id) = current {
            // `for = <id>` first: a click anywhere on the label moves focus
            // to the field it names, which may be anywhere in the document
            // (so this is a lookup, not an ancestor walk).
            let target = nui_runtime::widget::label_target(&self.tree.arena[id])
                .and_then(|name| return self.tree.lookup_id(name));
            if let Some(target) = target {
                let _ = self.engine.focus_element(&mut self.tree, target);
                return;
            }
            if self.tree.arena[id].is_focusable() {
                let _ = self.engine.focus_element(&mut self.tree, id);
                return;
            }
            current = self.tree.arena[id].parent;
        }
        self.engine.blur();
    }

    /// Hit test for interaction: the topmost element under the pointer,
    /// unless it is disabled (`enabled = false`) — a disabled widget must
    /// not swallow the click, it passes through to whatever is behind it.
    fn hit_interactive(&self, position: nui_core::Point) -> Option<ElementId> {
        let mut current = hit_test(&self.tree, position).map(|hit| return hit.element);
        while let Some(id) = current {
            let element = &self.tree.arena[id];
            if nui_runtime::widget::is_enabled(element) {
                return Some(id);
            }
            // Disabled: keep looking behind it, by walking up to the
            // parent's rect. Falling back to the parent is approximate
            // (it does not test siblings), but the common case is a
            // disabled group whose parent is the clickable surface.
            current = element.parent;
        }
        return None;
    }

    /// Whether the pointer at `position` is inside the element's box.
    fn cursor_in(&self, id: ElementId, position: nui_core::Point) -> bool {
        let Some(rect) = crate::app::element_bounds(&self.tree, id) else {
            return false;
        };
        return rect.contains(position);
    }

    /// A press (or wheel notch) on a stepper: `position` in window space,
    /// the step direction read off where in the box it landed.
    ///
    /// The strip's geometry lives in `nui-runtime` next to the stepping
    /// arithmetic and the painter reads the same function, so the glyph and
    /// the press zone cannot drift apart.
    pub(super) fn step_spin_at(&mut self, id: ElementId, position: nui_core::Point) -> bool {
        let Some(bounds) = crate::app::element_bounds(&self.tree, id) else {
            return false;
        };
        let steps = nui_runtime::widget::direction_at(
            bounds.size.width,
            bounds.size.height,
            position.x - bounds.origin.x,
            position.y - bounds.origin.y,
        );
        if steps == 0.0 {
            return false;
        }
        return self.step_spin(id, steps);
    }

    /// Steps a stepper's `value` and settles the frame. Shared by the
    /// press, the wheel and the arrow keys.
    pub(super) fn step_spin(&mut self, id: ElementId, steps: f64) -> bool {
        if !nui_runtime::widget::step(&mut self.engine, &mut self.tree, id, steps) {
            return false;
        }
        self.run_frame_pipeline(nui_core::Duration::ZERO);
        return true;
    }

    /// Up/Down with focus: a `SpinBox` takes a step, a multi-line
    /// `TextInput` walks its shaped lines. Returns whether the key was
    /// consumed (a `false` leaves it to whatever else wants it).
    ///
    /// The stepper half is asked first and lives in `nui-runtime`
    /// ([`nui_runtime::widget::step_focused`]) because "which element does
    /// this key reach" is a focus question, not a pointer one. The caret
    /// half has to stay here: "one line down" needs a font.
    pub(super) fn move_vertical(&mut self, up: bool, extend: bool) -> bool {
        let steps = if up { 1.0 } else { -1.0 };
        if nui_runtime::widget::step_focused(&mut self.engine, &mut self.tree, steps) {
            self.run_frame_pipeline(nui_core::Duration::ZERO);
            return true;
        }
        let Some(focused) = self.engine.focused() else {
            return false;
        };
        if !self.tree.arena[focused].is_multiline() {
            return false;
        }
        let Some(lines) = self.caret_lines(focused) else {
            return false;
        };
        if !self
            .engine
            .move_focused_vertical(&mut self.tree, &lines, up, extend)
        {
            // The first/last line: the key is free for anything enclosing.
            return false;
        }
        self.run_frame_pipeline(nui_core::Duration::ZERO);
        return true;
    }

    /// The visual line table for a multi-line field, wrapped at exactly the
    /// width the painter wraps it at (`box - 2 × inset`).
    ///
    /// Built on demand — a caret move is a keystroke, not a frame — from
    /// the host's own text system, so the breaks the caret walks are the
    /// breaks on screen.
    fn caret_lines(&mut self, id: ElementId) -> Option<Vec<nui_runtime::text_input::VisualLine>> {
        let bounds = crate::app::element_bounds(&self.tree, id)?;
        let element = &self.tree.arena[id];
        let inset = nui_runtime::widget::chrome::text_inset(element);
        let font_size = nui_runtime::widget::chrome::text_size(element);
        let content = match element.get("text") {
            Some(value) => value.as_str().ok()?.to_string(),
            None => String::new(),
        };
        let wrap_width = (bounds.size.width - inset * 2.0).max(1.0);
        return Some(nui_layout::visual_lines(
            &mut self.text,
            &content,
            font_size,
            wrap_width,
        ));
    }

    /// Closes a dialog: `open = false` plus the `closed` signal, so a
    /// document can react (Escape, or a click on the backdrop).
    ///
    /// Setting `open` through `set_direct` rather than emitting a signal
    /// that a handler would have to catch is deliberate: a dialog must
    /// close even when the document wrote no `on closed` handler.
    fn close_dialog(&mut self, id: ElementId) -> bool {
        if !self
            .engine
            .set_direct(&mut self.tree, id, "open", nui_core::Value::Bool(false))
        {
            return false;
        }
        let _ = self.engine.emit_signal(&mut self.tree, id, "closed");
        self.run_frame_pipeline(nui_core::Duration::ZERO);
        return true;
    }

    /// Recomputes widget interaction state against the current pointer and
    /// writes changed properties back. Returns whether a redraw is needed.
    fn update_widget_states(&mut self) -> bool {
        let input = nui_runtime::PointerInput {
            position: self.cursor,
            inside: true,
            down: self.click.captured.is_some(),
        };
        let changed = self.widgets.update(&mut self.engine, &mut self.tree, input);
        if !changed.is_empty() {
            // Widget state feeds `.nui` bindings; settle them and relayout.
            self.run_frame_pipeline(nui_core::Duration::ZERO);
            return true;
        }
        return false;
    }
}
