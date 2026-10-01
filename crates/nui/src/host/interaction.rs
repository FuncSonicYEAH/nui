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

use nui_core::{Key, Modifiers, PointerButton};
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

/// An in-flight scrollbar drag.
///
/// A scrollbar is not an element (D36), so it cannot ride the
/// [`PointerGesture`] capture that a `Slider` uses — there is no `ElementId`
/// for the thing under the pointer. This is the parallel bookkeeping for the
/// one gesture the element-based path cannot express.
///
/// `grab_offset` is what keeps the thumb from jumping: it is how far below
/// the thumb's top edge the press landed, so a drag resumes from wherever
/// the user actually grabbed instead of snapping the thumb's top to the
/// cursor. It is measured in lane space (dp from the container's top edge),
/// so the arithmetic stays valid when the container is itself inside a
/// scrolled ancestor.
#[derive(Debug, Default)]
pub(super) struct ScrollbarDrag {
    /// The scroll container whose bar is being dragged.
    container: Option<ElementId>,
    /// Pointer offset from the thumb's top edge at press time, in dp.
    grab_offset: f32,
}

impl ScrollbarDrag {
    /// The container being dragged, if a drag is live.
    pub(super) fn container(&self) -> Option<ElementId> {
        return self.container;
    }
}

/// A pointer hit on a scrollbar: which container, what part of the bar, and
/// the geometry that decided it.
///
/// The metrics travel with the hit because the caller needs them to turn a
/// pointer position into a `scroll_y`, and recomputing them would be a
/// second reading of state that could disagree with the one that decided
/// the hit.
struct ScrollbarTarget {
    container: ElementId,
    hit: nui_render::widget::ScrollbarHit,
    metrics: nui_render::widget::ScrollbarMetrics,
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
        // A live scrollbar drag owns the pointer: it follows to wherever
        // the cursor goes, including out of the container's box, which is
        // what the gesture is for. Returning early also stops the widget
        // tracker from re-resolving hover mid-drag, so a drag across a
        // button does not light it up.
        if self.scrollbar_drag.container().is_some() {
            return self.drag_scrollbar_to(position);
        }
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
        // A scrollbar claims the press before any element does. It has to
        // come first: the bar sits *over* the content, so an element-based
        // hit test would hand the press to whatever row is underneath and
        // arm a button the user was aiming past. Returns early for the same
        // reason — nothing else about this press should happen.
        if self.begin_scrollbar_drag(position) {
            return true;
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
        // A scrollbar drag ends here and consumes the release: it never
        // fires a `click` on whatever element the pointer happens to be
        // over, or a drag that ends outside the container would click the
        // page behind it.
        if self.end_scrollbar_drag() {
            return true;
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
    ///
    /// Both spellings of Space are accepted. The winit translation reports
    /// the key as [`Key::Space`] — `NamedKey::Space` is matched before the
    /// character branch, so [`Key::Character(' ')`] never arrives from a
    /// real keyboard. Matching only the character spelling meant Space
    /// activated nothing, and no test said so, because a test can build
    /// either value by hand.
    pub(super) fn activate_focused(&mut self, key: Key) -> bool {
        let Some(focused) = self.engine.focused() else {
            return false;
        };
        if self.tree.arena[focused].is_text_input()
            || !matches!(key, Key::Space | Key::Character(' ') | Key::Enter)
            || !nui_runtime::widget::is_activatable(&self.engine, &self.tree, focused)
        {
            return false;
        }
        nui_runtime::widget::activate(&mut self.engine, &mut self.tree, focused);
        self.update_widget_states();
        self.run_frame_pipeline(nui_core::Duration::ZERO);
        return true;
    }

    /// Hands a key the widget layer declined to the document.
    ///
    /// The routing is [`Engine::dispatch_key`]'s — who the key is for, the
    /// three properties it writes, the signal it bubbles. It lives in
    /// `nui-runtime` because that is where it can be tested: this host
    /// cannot be built headless, so logic kept here would only be reachable
    /// by a test that re-implemented it.
    pub(super) fn dispatch_key_signal(&mut self, key: Key, modifiers: Modifiers) -> bool {
        return self.engine.dispatch_key(&mut self.tree, key, modifiers);
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

    /// Starts a scrollbar drag if `position` is on one. Returns whether it
    /// did, in which case the press is consumed.
    ///
    /// Walks the ancestor chain from the hit element because the bar is
    /// painted at the container's right edge, which is *inside every
    /// descendant row's* box: the element under the pointer is whatever
    /// content is there, and only its ancestors can be the container. This
    /// is the same climb the wheel handler does to find its target, and for
    /// the same reason.
    ///
    /// The nearest container with a bar wins, so a list nested in another
    /// list drags the inner one — matching what the pointer is visually on
    /// top of.
    fn begin_scrollbar_drag(&mut self, position: nui_core::Point) -> bool {
        let Some(target) = self.scrollbar_at(position) else {
            return false;
        };
        let ScrollbarTarget {
            container,
            hit,
            metrics,
        } = target;
        match hit {
            nui_render::widget::ScrollbarHit::Thumb => {
                // Resume from where the user grabbed, not from the thumb's
                // top edge: otherwise the thumb leaps to centre itself under
                // the cursor on the first pixel of movement.
                self.scrollbar_drag.container = Some(container);
                self.scrollbar_drag.grab_offset = position.y - metrics.thumb.origin.y;
                return true;
            }
            nui_render::widget::ScrollbarHit::Track => {
                // A click on the lane jumps the thumb's top to the pointer
                // — the standard "jump" reading of a click on a scrollbar.
                // Not a paged animation: with no scroll animator in the
                // framework, a jump is the only thing that lands where the
                // user pointed.
                //
                // The press is consumed whether or not the jump *changed*
                // anything. Returning the write's result here was wrong: a
                // click on the lane that happened to map to the current
                // offset would report "unhandled" and fall through to the
                // content, arming a button behind the bar. The gesture is
                // the bar's either way.
                self.scrollbar_drag.container = None;
                self.write_scroll_for_thumb_top(container, position.y, metrics);
                return true;
            }
        }
    }

    /// Moves a live drag to follow the pointer. Returns whether the frame
    /// needs redrawing.
    fn drag_scrollbar_to(&mut self, position: nui_core::Point) -> bool {
        let Some(container) = self.scrollbar_drag.container() else {
            return false;
        };
        let grab = self.scrollbar_drag.grab_offset;
        let Some(metrics) = self.scrollbar_metrics_of(container) else {
            // The container lost its scrollbar mid-drag (the model emptied,
            // say). End the gesture rather than write an offset nowhere.
            self.scrollbar_drag.container = None;
            return false;
        };
        return self.write_scroll_for_thumb_top(container, position.y - grab, metrics);
    }

    /// Ends a live scrollbar drag. Returns whether one was live.
    fn end_scrollbar_drag(&mut self) -> bool {
        return self.scrollbar_drag.container.take().is_some();
    }

    /// The innermost scrollable ancestor of the pointer that shows a bar at
    /// `position`, with its resolved metrics.
    fn scrollbar_at(&self, position: nui_core::Point) -> Option<ScrollbarTarget> {
        let mut current = hit_test(&self.tree, position).map(|hit| return hit.element);
        while let Some(id) = current {
            // Only these two types scroll, so only they can have a bar.
            // Read into a local so the borrow of the tree ends before
            // `scrollbar_in` takes `&self`.
            let is_container = matches!(self.tree.arena[id].ty.as_str(), "Scroll" | "ListView");
            if is_container && let Some(target) = self.scrollbar_in(id, position) {
                return Some(target);
            }
            current = self.tree.arena[id].parent;
        }
        return None;
    }

    /// The bar hit on one specific container, if the pointer is on it.
    fn scrollbar_in(
        &self,
        container: ElementId,
        position: nui_core::Point,
    ) -> Option<ScrollbarTarget> {
        let metrics = self.scrollbar_metrics_of(container)?;
        let hit = nui_render::widget::scrollbar_hit(
            metrics.track.origin,
            metrics.track.size,
            self.scroll_y_of(container),
            self.scroll_limit_of(container),
            position,
        )?;
        return Some(ScrollbarTarget {
            container,
            hit,
            metrics,
        });
    }

    /// A container's scrollbar geometry, or `None` when it has none to draw.
    ///
    /// The three inputs are all read through the same helpers the painter
    /// and the wheel use, so the bar the user grabs is the bar on screen.
    fn scrollbar_metrics_of(
        &self,
        container: ElementId,
    ) -> Option<nui_render::widget::ScrollbarMetrics> {
        let bounds = crate::app::element_bounds(&self.tree, container)?;
        return nui_render::widget::scrollbar_metrics(
            bounds.origin,
            bounds.size,
            self.scroll_y_of(container),
            self.scroll_limit_of(container),
        );
    }

    /// A container's current `scroll_y`.
    fn scroll_y_of(&self, container: ElementId) -> f32 {
        return match self.tree.arena[container].get("scroll_y") {
            Some(nui_core::Value::Float(value)) => *value as f32,
            _ => 0.0,
        };
    }

    /// A container's travel limit — the same number the wheel clamps
    /// against and the painter sizes the thumb from.
    fn scroll_limit_of(&self, container: ElementId) -> f32 {
        return nui_runtime::widget::max_scroll_y(&self.engine, &self.tree, container);
    }

    /// Writes the `scroll_y` a thumb top edge stands for, and settles the
    /// frame. Returns whether anything changed.
    fn write_scroll_for_thumb_top(
        &mut self,
        container: ElementId,
        thumb_top: f32,
        metrics: nui_render::widget::ScrollbarMetrics,
    ) -> bool {
        // Convert the window-space thumb top into lane space: the inverse
        // mapping works in dp from the container's own top edge, so a drag
        // inside a scrolled ancestor stays correct.
        let lane_offset = thumb_top - metrics.track.origin.y;
        let Some(next) = nui_render::widget::scroll_y_from_thumb_top(
            metrics.track.size,
            lane_offset,
            self.scroll_limit_of(container),
        ) else {
            return false;
        };
        return self.set_scroll_y(container, next);
    }

    /// Writes `scroll_y` directly and redraws if it moved.
    ///
    /// Settles through [`WindowHost::run_scroll_pipeline`], not the full
    /// pipeline: a scrollbar drag writes this once per pointer move and
    /// `scroll_y` is not a layout input, so paying for a taffy relayout and
    /// a re-shape of every string on each of those moves is what made the
    /// drag stutter (D38).
    fn set_scroll_y(&mut self, container: ElementId, next: f32) -> bool {
        return self.set_direct_with_scroll_pipeline(container, next);
    }

    /// Writes a container's `scroll_y` and settles through the scroll-only
    /// pipeline (D38). The one entry point both the drag and the wheel use,
    /// so the two paths cannot drift.
    pub(super) fn set_direct_with_scroll_pipeline(
        &mut self,
        container: ElementId,
        next: f32,
    ) -> bool {
        if !self.engine.set_direct(
            &mut self.tree,
            container,
            "scroll_y",
            nui_core::Value::Float(f64::from(next)),
        ) {
            return false;
        }
        self.run_scroll_pipeline();
        return true;
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
        // Read before `&mut self.text`: the caret table has to be broken in the
        // same face the field is painted in, or the caret drifts from the glyph
        // it belongs to as soon as the field is anything but regular.
        let typeface = nui_layout::typeface_of(element);
        return Some(nui_layout::visual_lines(
            &mut self.text,
            &content,
            font_size,
            wrap_width,
            &typeface,
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
