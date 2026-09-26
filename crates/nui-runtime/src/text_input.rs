//! TextInput editing core (M7): cursor, selection, and text edits over
//! char indices (plan §5 输入). Pure logic — no engine, no rendering — so
//! editing behavior is headless-testable; IME commits land here too as
//! plain insertions.
//!
//! Invariants: `cursor` and selection indices are **char** indices into
//! `text` (byte offsets would corrupt multi-byte input); `cursor` always
//! sits at `selection_start..=selection_end`'s edge after edits.
//!
//! The multi-line shape (FUTURE 批次 6) adds two things without changing
//! that invariant: an insertion that drops line breaks and respects
//! `max_length` when the field is single-line, and vertical movement over
//! [`VisualLine`]s — the line table the shaping layer hands in, because
//! "one line down" is a geometry question and this crate owns no fonts.

/// One visual line of a multi-line field, as the editing core sees it.
///
/// The editing core owns *cursor arithmetic*, not *shaping*: vertical
/// movement needs to know where the lines break and how wide each caret
/// position is, and both come from the shaping layer. Rather than depend on
/// it — and drag a font stack into this crate — the runtime states the
/// shape it wants and the caller, which already owns a `TextSystem`, fills
/// it in.
#[derive(Debug, Clone, PartialEq)]
pub struct VisualLine {
    /// First char index of the line.
    pub start: usize,
    /// One past the last char index of the line.
    pub end: usize,
    /// Caret x (line-relative, dp) for every char index in `start..=end`:
    /// `end - start + 1` entries. The last entry is the line's end, which
    /// is *not* the next line's start (`caret_x` at a wrap belongs to the
    /// next line).
    pub caret_x: Vec<f32>,
}

/// The index of the visual line containing `index`.
fn line_of(lines: &[VisualLine], index: usize) -> usize {
    for (position, line) in lines.iter().enumerate() {
        if index < line.end {
            return position;
        }
    }
    return lines.len().saturating_sub(1);
}

/// Caret x of char `index` inside `lines[line]`, in line-local dp.
fn caret_x_of(lines: &[VisualLine], line: usize, index: usize) -> f32 {
    let Some(metrics) = lines.get(line) else {
        return 0.0;
    };
    // At or past the line's last char — and for an empty line — the caret
    // sits at the line's own end.
    if index >= metrics.end {
        return metrics.caret_x.last().copied().unwrap_or(0.0);
    }
    return metrics
        .caret_x
        .get(index.saturating_sub(metrics.start))
        .copied()
        .unwrap_or(0.0);
}

/// Editing state of one `TextInput` element.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextInputState {
    /// The committed text.
    pub text: String,
    /// Cursor position as a char index (`0..=char_count`).
    pub cursor: usize,
    /// Selection anchor (char index); `None` = collapsed selection.
    pub anchor: Option<usize>,
    /// IME composition text shown after the cursor (not yet committed).
    pub preedit: String,
    /// The visual column a run of vertical moves is aiming for, in dp;
    /// `None` outside such a run.
    ///
    /// Walking down past a short line would otherwise erase the column: the
    /// caret clamps to that line's end, and the next move would aim from
    /// *there*. Remembering the goal for the length of the run is what makes
    /// Up-then-Down come back to where it started. Any horizontal move or
    /// edit ends the run — the caret has been placed deliberately.
    vertical_goal: Option<f32>,
}

impl TextInputState {
    /// Creates state with `text` and the cursor at the end.
    pub fn from_text(text: impl Into<String>) -> TextInputState {
        let text = text.into();
        let cursor = text.chars().count();
        return TextInputState {
            text,
            cursor,
            anchor: None,
            preedit: String::new(),
            vertical_goal: None,
        };
    }

    /// The selected char range `(start, end)` (empty when collapsed).
    pub fn selection(&self) -> (usize, usize) {
        let Some(anchor) = self.anchor else {
            return (self.cursor, self.cursor);
        };
        return (anchor.min(self.cursor), anchor.max(self.cursor));
    }

    /// Whether characters are selected.
    pub fn has_selection(&self) -> bool {
        let (start, end) = self.selection();
        return start != end;
    }

    /// The selected text slice.
    pub fn selected_text(&self) -> &str {
        let (start, end) = self.selection();
        let text: Vec<char> = self.text.chars().collect();
        let start_byte = char_to_byte(&text, start);
        let end_byte = char_to_byte(&text, end);
        return &self.text[start_byte..end_byte];
    }

    /// Inserts `input` at the cursor, replacing any selection.
    pub fn insert(&mut self, input: &str) {
        self.insert_input(input, None, true);
    }

    /// Ends a run of vertical moves: the caret is about to be placed by
    /// something other than Up/Down, so the remembered column is stale.
    fn end_vertical_run(&mut self) {
        self.vertical_goal = None;
    }

    /// Inserts `input` shaped like the field: a single-line field drops
    /// line breaks (a pasted paragraph must not become one long line, and
    /// a one-line field has nothing to submit on), and `limit` caps the
    /// field's length in **chars** — replacing a selection frees room, so
    /// the cap counts what the field holds after the edit, not the typed
    /// span.
    pub fn insert_input(&mut self, input: &str, limit: Option<usize>, multiline: bool) {
        if input.is_empty() {
            return;
        }
        self.end_vertical_run();
        let stripped;
        let input = if multiline {
            input
        } else {
            stripped = input
                .chars()
                .filter(|character| return *character != '\n' && *character != '\r')
                .collect::<String>();
            if stripped.is_empty() {
                return;
            }
            stripped.as_str()
        };
        self.delete_selection();
        let room = match limit {
            Some(limit) => limit.saturating_sub(self.text.chars().count()),
            None => usize::MAX,
        };
        if room == 0 {
            return;
        }
        let taken = input.chars().count().min(room);
        if taken == 0 {
            return;
        }
        // Cut on a char boundary: `nth` gives the byte offset of the first
        // char to drop.
        let bytes = input
            .char_indices()
            .nth(taken)
            .map_or(input.len(), |(offset, _)| return offset);
        let byte = self.cursor_byte();
        self.text.insert_str(byte, &input[..bytes]);
        self.cursor += taken;
        self.anchor = None;
    }

    /// Backspace: deletes the selection or the char left of the cursor.
    pub fn backspace(&mut self) {
        self.end_vertical_run();
        if self.delete_selection() {
            return;
        }
        if self.cursor == 0 {
            return;
        }
        let start = self.cursor - 1;
        self.remove_char_range(start, self.cursor);
        self.cursor = start;
        self.anchor = None;
    }

    /// Forward delete: deletes the selection or the char right of cursor.
    pub fn delete_forward(&mut self) {
        self.end_vertical_run();
        if self.delete_selection() {
            return;
        }
        let count = self.text.chars().count();
        if self.cursor >= count {
            return;
        }
        self.remove_char_range(self.cursor, self.cursor + 1);
        self.anchor = None;
    }

    /// Moves the cursor by one char; `extend` grows the selection.
    pub fn move_horizontal(&mut self, right: bool, extend: bool) {
        let count = self.text.chars().count();
        let target = if right {
            (self.cursor + 1).min(count)
        } else {
            self.cursor.saturating_sub(1)
        };
        self.move_to(target, extend);
    }

    /// Moves the cursor to the start/end; `extend` grows the selection.
    pub fn move_edge(&mut self, end: bool, extend: bool) {
        let target = if end { self.text.chars().count() } else { 0 };
        self.move_to(target, extend);
    }

    /// Moves the cursor to the visual line above/below, as described by
    /// `lines`.
    ///
    /// The column is preserved by **x**, not by char offset: "the 6th
    /// character" means nothing on a shorter line, while "80dp in" is what
    /// the user is aiming at. The landing char is the one whose caret is
    /// *nearest* that x (ties going left), and a target line too short to
    /// reach it clamps to the line's end — the standard editor behaviour.
    ///
    /// Consecutive vertical moves keep aiming for the column the run
    /// started in (see [`TextInputState::vertical_goal`]), so walking
    /// through a short line and back returns to where it started instead of
    /// to the short line's end. Any other movement or edit ends the run.
    ///
    /// Returns whether the caret moved: on the first line `up` and on the
    /// last line `down` there is nowhere to go, and the caller is expected
    /// to leave the key unhandled so an enclosing control can use it.
    pub fn move_vertical(&mut self, lines: &[VisualLine], up: bool, extend: bool) -> bool {
        if lines.is_empty() {
            return false;
        }
        let line = line_of(lines, self.cursor);
        let target = if up {
            let Some(previous) = line.checked_sub(1) else {
                return false;
            };
            previous
        } else {
            let next = line + 1;
            if next >= lines.len() {
                return false;
            }
            next
        };
        let goal = self
            .vertical_goal
            .unwrap_or_else(|| return caret_x_of(lines, line, self.cursor));
        let target_line = &lines[target];
        // A line's caret positions run up to (but not including) its end:
        // that index is the *next* line's start, so landing on it would put
        // the caret on the line below — which would make a one-char line
        // unusable from above. The text's own end is the single exception:
        // it belongs to the last line, and to nothing else.
        let highest = if target + 1 == lines.len() {
            target_line.end
        } else {
            target_line.end - 1
        };
        let mut landing = target_line.start;
        let mut distance = f32::MAX;
        for index in target_line.start..=highest {
            let candidate = (caret_x_of(lines, target, index) - goal).abs();
            if candidate < distance {
                distance = candidate;
                landing = index;
            }
        }
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
            self.cursor = landing;
        } else {
            self.cursor = landing;
            self.anchor = None;
        }
        // The run continues: the *goal* is what carries over, not the
        // clamped landing.
        self.vertical_goal = Some(goal);
        return true;
    }

    /// Moves the cursor to `target` (clamped); `extend` grows the
    /// selection, anchoring at the pre-move position when collapsed.
    pub fn move_to(&mut self, target: usize, extend: bool) {
        self.end_vertical_run();
        let count = self.text.chars().count();
        let target = target.min(count);
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.cursor);
            }
            self.cursor = target;
            return;
        }
        self.cursor = target;
        self.anchor = None;
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        self.end_vertical_run();
        self.anchor = Some(0);
        self.cursor = self.text.chars().count();
    }

    /// Collapses the selection at the cursor.
    pub fn collapse(&mut self) {
        self.end_vertical_run();
        self.anchor = None;
    }

    /// Cut: returns the selected text and deletes it; `None` when the
    /// selection is collapsed.
    pub fn cut(&mut self) -> Option<String> {
        if !self.has_selection() {
            return None;
        }
        let selected = self.selected_text().to_string();
        self.backspace();
        return Some(selected);
    }

    /// Reconciles with an external `text` write (two-way binding, model
    /// reset): replaces the text and clamps the cursor to the end.
    pub fn set_text(&mut self, text: String) {
        if text == self.text {
            return;
        }
        self.end_vertical_run();
        self.cursor = text.chars().count();
        self.anchor = None;
        self.text = text;
    }

    /// Deletes the selected range; returns whether anything was deleted.
    fn delete_selection(&mut self) -> bool {
        if !self.has_selection() {
            return false;
        }
        let (start, end) = self.selection();
        self.remove_char_range(start, end);
        self.cursor = start;
        self.anchor = None;
        return true;
    }

    /// Removes the char range `[start, end)` (char indices).
    fn remove_char_range(&mut self, start: usize, end: usize) {
        let chars: Vec<char> = self.text.chars().collect();
        let start_byte = char_to_byte(&chars, start);
        let end_byte = char_to_byte(&chars, end);
        self.text.replace_range(start_byte..end_byte, "");
    }

    /// The byte offset of the cursor.
    fn cursor_byte(&self) -> usize {
        let chars: Vec<char> = self.text.chars().collect();
        return char_to_byte(&chars, self.cursor);
    }
}

/// Byte offset of char index `index` in a collected char slice.
fn char_to_byte(chars: &[char], index: usize) -> usize {
    return chars
        .iter()
        .take(index)
        .map(|character| return character.len_utf8())
        .sum();
}

// -- engine routing (M7): focus cycling, key dispatch, input writes --------

use crate::binding::Engine;
use crate::element::{Element, ElementId, ElementTree};
use crate::notify::ChangeSource;
use nui_core::{Key, Modifiers, Value};

impl Engine {
    /// Moves keyboard focus to `element`, applying the field's focus
    /// policy: a text field with `select_all_on_focus` selects its whole
    /// content, so the next keystroke replaces it.
    ///
    /// A focus *reconciles* first — the field's editing state is caught up
    /// with its `text` property and its validator runs — because focus is
    /// where a field's validity first becomes visible: a prefilled but
    /// wrong seed must show as invalid before the user touches it. (Note
    /// the difference from [`Engine::focus`], the plain setter, which
    /// reconciles nothing.)
    ///
    /// Returns whether the caret state changed (the caller may redraw).
    pub fn focus_element(&mut self, tree: &mut ElementTree, element: ElementId) -> bool {
        self.focused = Some(element);
        let changed = self.reconcile_text_input(tree, element);
        if !tree.arena[element].selects_all_on_focus() {
            return changed;
        }
        let Some(state) = tree.arena[element].text_input.as_mut() else {
            return changed;
        };
        state.select_all();
        self.sync_text_property(tree, element);
        return true;
    }

    /// Moves keyboard focus to `element` without a focus policy. The plain
    /// setter; [`Engine::focus_element`] is the one that also runs
    /// `select_all_on_focus`.
    pub fn focus(&mut self, element: ElementId) {
        self.focused = Some(element);
    }

    /// The focused element, if any.
    pub fn focused(&self) -> Option<ElementId> {
        return self.focused;
    }

    /// Clears keyboard focus.
    pub fn blur(&mut self) {
        self.focused = None;
    }

    /// Cycles focus to the next focusable element (pre-order; wraps).
    pub fn focus_next(&mut self, tree: &mut ElementTree) {
        let mut focusable: Vec<ElementId> = Vec::new();
        tree.visit_pre_order(|id, element| {
            if element.is_focusable() {
                focusable.push(id);
            }
        });
        if focusable.is_empty() {
            return;
        }
        let next = match self.focused {
            Some(current) => {
                let index = focusable
                    .iter()
                    .position(|id| return *id == current)
                    .map_or(0, |position| return position + 1);
                focusable[index % focusable.len()]
            }
            None => focusable[0],
        };
        // Tab stops at a field the same way a click does, focus policy
        // included: tabbing into a pre-filled field and typing must replace
        // its content, not append to it.
        let _ = self.focus_element(tree, next);
    }

    /// Routes a key press: Tab cycles focus, the rest edit the focused
    /// `TextInput`. Returns whether the event was consumed.
    ///
    /// Vertical movement over a multi-line field is *not* handled here:
    /// "one line down" needs the shaped line table, which only the host can
    /// build. The host intercepts Up/Down and calls
    /// [`Engine::move_focused_vertical`].
    pub fn handle_key(&mut self, tree: &mut ElementTree, key: Key, modifiers: Modifiers) -> bool {
        if key == Key::Tab {
            self.focus_next(tree);
            return true;
        }
        let Some(element) = self.focused else {
            return false;
        };
        if !tree.arena.contains_key(element) {
            self.focused = None;
            return false;
        }
        if !self.reconcile_text_input(tree, element) {
            return false;
        }
        let read_only = tree.arena[element].is_read_only();
        let multiline = tree.arena[element].is_multiline();
        let limit = tree.arena[element].max_length();
        if modifiers.ctrl && key == Key::Character('a') {
            tree.arena[element]
                .text_input
                .as_mut()
                .expect("reconciled text input")
                .select_all();
            self.sync_text_property(tree, element);
            return true;
        }
        if key == Key::Enter && (!multiline || modifiers.ctrl) {
            // A one-line field submits on Enter; a multi-line one takes
            // Enter as a newline and submits on Ctrl+Enter (Qt's split), so
            // a text area can be typed into at all.
            let _ = self.emit_signal(tree, element, "accepted");
            return true;
        }
        if key == Key::Escape {
            self.blur();
            return true;
        }
        let handled = {
            let Some(state) = tree.arena[element].text_input.as_mut() else {
                return false;
            };
            match key {
                // Plain typing (no IME active) arrives as character keys;
                // IME commits arrive through `handle_text_input`.
                Key::Character(character) if !read_only => {
                    state.insert_input(&character.to_string(), limit, multiline);
                    true
                }
                Key::Space if !read_only => {
                    state.insert_input(" ", limit, multiline);
                    true
                }
                // Only reachable for a multi-line field: the one-line case
                // submitted above.
                Key::Enter if !read_only => {
                    state.insert_input("\n", limit, true);
                    true
                }
                Key::Backspace if !read_only => {
                    state.backspace();
                    true
                }
                Key::Delete if !read_only => {
                    state.delete_forward();
                    true
                }
                Key::ArrowLeft => {
                    state.move_horizontal(false, modifiers.shift);
                    true
                }
                Key::ArrowRight => {
                    state.move_horizontal(true, modifiers.shift);
                    true
                }
                Key::Home => {
                    state.move_edge(false, modifiers.shift);
                    true
                }
                Key::End => {
                    state.move_edge(true, modifiers.shift);
                    true
                }
                _ => false,
            }
        };
        if handled {
            self.sync_text_property(tree, element);
        }
        return handled;
    }

    /// Routes text input (keyboard characters and IME commits) into the
    /// focused `TextInput`. Returns whether the event was consumed.
    pub fn handle_text_input(&mut self, tree: &mut ElementTree, input: &str) -> bool {
        let Some(element) = self.focused else {
            return false;
        };
        if !tree.arena.contains_key(element) || !self.reconcile_text_input(tree, element) {
            return false;
        }
        // A read-only field consumes nothing, so the key can still reach an
        // enclosing control.
        if tree.arena[element].is_read_only() {
            return false;
        }
        let multiline = tree.arena[element].is_multiline();
        let limit = tree.arena[element].max_length();
        if let Some(state) = tree.arena[element].text_input.as_mut() {
            state.insert_input(input, limit, multiline);
            state.preedit.clear();
        }
        self.sync_text_property(tree, element);
        return true;
    }

    /// Moves the focused multi-line field's caret to the visual line above
    /// or below, given the line table the host shaped for the current text.
    ///
    /// Returns whether the caret moved. A `false` leaves the key free for
    /// an enclosing control (a list, a stepper) on the first/last line.
    pub fn move_focused_vertical(
        &mut self,
        tree: &mut ElementTree,
        lines: &[VisualLine],
        up: bool,
        extend: bool,
    ) -> bool {
        let Some(element) = self.focused else {
            return false;
        };
        let Some(state) = tree.arena[element].text_input.as_mut() else {
            return false;
        };
        if !state.move_vertical(lines, up, extend) {
            return false;
        }
        self.sync_text_property(tree, element);
        return true;
    }

    /// The focused `TextInput`'s selection (copy source); `None` when
    /// nothing selected or nothing focused.
    pub fn copy_focused(&self, tree: &ElementTree) -> Option<String> {
        let element = self.focused?;
        let state = tree.arena[element].text_input.as_ref()?;
        let selected = state.selected_text();
        return if selected.is_empty() {
            None
        } else {
            Some(selected.to_string())
        };
    }

    /// Cut: copies and deletes the focused `TextInput`'s selection.
    ///
    /// `None` for a read-only field: a cut is an *edit*, and a read-only
    /// field has none. Copy stays available, so the selection can still be
    /// taken out of it.
    pub fn cut_focused(&mut self, tree: &mut ElementTree) -> Option<String> {
        let element = self.focused?;
        if tree.arena[element].is_read_only() {
            return None;
        }
        let state = tree.arena[element].text_input.as_mut()?;
        let copied = state.cut()?;
        self.sync_text_property(tree, element);
        return Some(copied);
    }

    /// Writes the editing state back to the `text` property: user input is
    /// an effect-like write, so it clears a `<-` binding (D10) and syncs a
    /// `<=>` partner through the value channel.
    fn sync_text_property(&mut self, tree: &mut ElementTree, element: ElementId) {
        let Some(value) = tree.arena[element]
            .text_input
            .as_ref()
            .map(|state| return Value::String(state.text.clone()))
        else {
            return;
        };
        tree.arena[element].clear_binding("text");
        if tree.arena[element].set("text", value.clone()) {
            self.invalidate(element, "text");
            self.record_change(element, "text", value, ChangeSource::Input);
            self.sync_two_way_partner(tree, element, "text");
        }
        self.sync_validity(tree, element);
    }

    /// Writes the `invalid` read-only property of a field that declares a
    /// `validator`, so a document can style the field
    /// (`border.color <- invalid ? #e24b4a : #3a4150`).
    ///
    /// Host-authored, like `hovered`/`focused`: it goes through
    /// [`Engine::set_direct`], which keeps a `.nui` `<-` binding on
    /// `invalid` intact and marks the change as the host's rather than the
    /// user's. A field with no validator — or one this build does not
    /// implement — never gets an `invalid` slot at all.
    fn sync_validity(&mut self, tree: &mut ElementTree, element: ElementId) {
        let Some(kind) = tree.arena[element].validator().map(str::to_string) else {
            return;
        };
        let text = tree.arena[element]
            .text_input
            .as_ref()
            .map(|state| return state.text.clone())
            .or_else(|| {
                return match tree.arena[element].get("text") {
                    Some(Value::String(text)) => Some(text.clone()),
                    _ => None,
                };
            });
        let Some(text) = text else {
            return;
        };
        let Some(valid) = validates(&kind, &text) else {
            return;
        };
        let invalid = !valid;
        if tree.arena[element]
            .get("invalid")
            .and_then(|v| return v.as_bool().ok())
            == Some(invalid)
        {
            return;
        }
        self.set_direct(tree, element, "invalid", Value::Bool(invalid));
    }

    /// Resyncs the editing state from the `text` property (external writes
    /// land on the property first: `<=>` partners, bindings, host writes)
    /// and re-runs the validator, so `invalid` follows the *text* rather
    /// than the last keystroke. Returns whether `element` is a text input
    /// at all.
    fn reconcile_text_input(&mut self, tree: &mut ElementTree, element: ElementId) -> bool {
        if tree.arena[element].text_input.is_none() {
            return false;
        }
        if let Some(Value::String(text)) = tree.arena[element].get("text").cloned()
            && let Some(state) = tree.arena[element].text_input.as_mut()
        {
            state.set_text(text);
        }
        self.sync_validity(tree, element);
        return true;
    }
}

/// Focus bookkeeping lives on the engine (per-window state).
///
/// The field's shape and policies are read from its properties here rather
/// than from a per-type table: every one of them changes how an edit is
/// *applied* (a newline is a character in one field and a submit in
/// another; a cap silently drops what would overflow), and the renderer
/// reads the same flags to decide how to draw the field.
impl Element {
    /// Whether the element is an editable text field.
    pub fn is_text_input(&self) -> bool {
        return self.ty == "TextInput";
    }

    /// Whether the field is multi-line: Enter inserts a newline, and the
    /// text wraps inside the field's width instead of running off the end.
    pub fn is_multiline(&self) -> bool {
        return self.bool_property("multiline");
    }

    /// Whether the field refuses edits. Movement, selection and copy stay
    /// available: a read-only field is still readable and selectable.
    pub fn is_read_only(&self) -> bool {
        return self.bool_property("read_only");
    }

    /// Whether the field draws its content as a password. `reveal = true`
    /// is the escape hatch a "show password" toggle binds to, so the two
    /// properties meet here instead of at every call site.
    pub fn is_password(&self) -> bool {
        return self.bool_property("password") && !self.bool_property("reveal");
    }

    /// The field's maximum length in chars, if it declares one. A
    /// non-positive cap reads as "no cap" — `max_length = 0` is a typo, not
    /// a locked field.
    pub fn max_length(&self) -> Option<usize> {
        let limit = self.get("max_length")?.as_f64().ok()?;
        if !limit.is_finite() || limit <= 0.0 {
            return None;
        }
        return usize::try_from(limit.trunc() as u64).ok();
    }

    /// Whether focusing the field selects its whole content, so the next
    /// keystroke replaces it (the URL-bar behaviour).
    pub fn selects_all_on_focus(&self) -> bool {
        return self.bool_property("select_all_on_focus");
    }

    /// The declared validator kind (`"int"`, `"float"`, `"email"`), or
    /// `None` when the field declares none.
    pub fn validator(&self) -> Option<&str> {
        let kind = self.get("validator")?.as_enum().ok()?;
        return if kind.is_empty() { None } else { Some(kind) };
    }

    /// Reads a Boolean property, defaulting to `false`.
    fn bool_property(&self, name: &str) -> bool {
        return self
            .get(name)
            .and_then(|value| return value.as_bool().ok())
            .unwrap_or(false);
    }
}

/// Whether `kind` accepts `text`; `None` when `kind` is not a validator
/// this build knows — an unrecognised name must not mark a field
/// permanently invalid, since a document may target a later nui.
///
/// An empty field is always valid: "nothing typed yet" is not a mistake,
/// and requiring a value is the document's business (`required` on the
/// label, or a disable on the submit button).
pub fn validates(kind: &str, text: &str) -> Option<bool> {
    return match kind {
        "int" => Some(is_integer(text)),
        "float" => Some(is_float(text)),
        "email" => Some(is_email(text)),
        _ => None,
    };
}

/// An optionally signed run of decimal digits.
fn is_integer(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let digits = text
        .strip_prefix('-')
        .or_else(|| return text.strip_prefix('+'))
        .unwrap_or(text);
    return !digits.is_empty()
        && digits
            .chars()
            .all(|character| return character.is_ascii_digit());
}

/// An optionally signed decimal with at most one point. A trailing or
/// leading point is accepted (`"1."`, `".5"`): both are states a field
/// passes through while being typed, and flagging them mid-keystroke is
/// noise, not information.
fn is_float(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    let body = text
        .strip_prefix('-')
        .or_else(|| return text.strip_prefix('+'))
        .unwrap_or(text);
    let mut seen_point = false;
    let mut digits = 0;
    for character in body.chars() {
        if character == '.' {
            if seen_point {
                return false;
            }
            seen_point = true;
            continue;
        }
        if !character.is_ascii_digit() {
            return false;
        }
        digits += 1;
    }
    return digits > 0;
}

/// A pragmatic email shape: exactly one `@`, a non-empty local part, and a
/// dotted domain with no empty labels, no whitespace anywhere.
///
/// Deliberately not RFC 5322. That grammar admits addresses no mail system
/// accepts and its complexity buys nothing for a form hint; what this does
/// catch is the mistakes people actually make — a missing `@`, a domain
/// without a dot, a trailing dot, a stray space from a paste.
fn is_email(text: &str) -> bool {
    if text.is_empty() {
        return true;
    }
    if text
        .chars()
        .any(|character| return character.is_whitespace())
    {
        return false;
    }
    let mut parts = text.split('@');
    let local = parts.next().unwrap_or_default();
    let Some(domain) = parts.next() else {
        return false;
    };
    if parts.next().is_some() || local.is_empty() || domain.is_empty() {
        return false;
    }
    return domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains("..");
}

impl Engine {
    /// Sets the preedit (IME composition) text of the focused input.
    pub fn set_preedit(&mut self, tree: &mut ElementTree, preedit: &str) -> bool {
        let Some(element) = self.focused else {
            return false;
        };
        if !self.reconcile_text_input(tree, element) {
            return false;
        }
        if let Some(state) = tree.arena[element].text_input.as_mut() {
            state.preedit = preedit.to_string();
        }
        return true;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn insert_places_cursor_after_input() {
        let mut state = TextInputState::from_text("hello");
        state.insert(" world");
        assert_eq!(state.text, "hello world");
        assert_eq!(state.cursor, 11);
        assert_eq!(state.selection(), (11, 11));
    }

    #[test]
    fn insert_replaces_selection() {
        let mut state = TextInputState::from_text("abcdef");
        state.anchor = Some(1);
        state.cursor = 4; // selects "bcd"
        assert_eq!(state.selected_text(), "bcd");
        state.insert("XY");
        assert_eq!(state.text, "aXYef");
        assert_eq!(state.cursor, 3);
    }

    #[test]
    fn backspace_and_delete_forward() {
        let mut state = TextInputState::from_text("ab");
        state.backspace();
        assert_eq!(state.text, "a");
        state.delete_forward();
        assert_eq!(state.text, "a");
        state.delete_forward();
        assert_eq!(state.text, "a", "delete at end is a no-op");
        state.move_horizontal(false, false);
        assert_eq!(state.cursor, 0);
        state.delete_forward();
        assert_eq!(
            state.text, "",
            "delete_forward at start removes the right char"
        );
        state.backspace();
        assert_eq!(state.text, "", "backspace at start is a no-op");
    }

    #[test]
    fn movement_with_and_without_selection() {
        let mut state = TextInputState::from_text("abc");
        state.move_edge(false, false);
        assert_eq!(state.cursor, 0);
        state.move_to(1, true); // extend anchors at the pre-move position
        assert_eq!(
            state.selection(),
            (0, 1),
            "extend from collapsed anchors at the cursor"
        );
        state.move_to(3, false);
        assert_eq!(state.selection(), (3, 3));
        state.anchor = Some(0);
        state.cursor = 2;
        state.move_edge(true, true);
        assert_eq!(state.selection(), (0, 3), "extend keeps the anchor");
    }

    #[test]
    fn unicode_indices_are_char_based() {
        let mut state = TextInputState::from_text("héllo"); // é is 2 bytes
        state.move_edge(true, false);
        state.backspace();
        assert_eq!(state.text, "héll");
        // Multi-byte char insertion and selection.
        state.insert("字");
        assert_eq!(state.text, "héll字");
        state.anchor = Some(4);
        state.cursor = 5;
        assert_eq!(state.selected_text(), "字");
        state.backspace();
        assert_eq!(state.text, "héll");
    }

    #[test]
    fn select_all_and_collapse() {
        let mut state = TextInputState::from_text("abc");
        state.select_all();
        assert_eq!(state.selected_text(), "abc");
        state.collapse();
        assert!(!state.has_selection());
    }

    #[test]
    fn set_text_resyncs_from_binding() {
        let mut state = TextInputState::from_text("hello");
        state.cursor = 2;
        state.set_text("hi!".to_string());
        assert_eq!(state.text, "hi!");
        assert_eq!(state.cursor, 3, "external write parks the cursor at end");
        state.set_text("hi!".to_string());
        assert_eq!(state.cursor, 3, "identical text is a no-op");
    }

    // -- field shape (FUTURE 批次 6) --------------------------------------

    /// Builds a `TextInput` with the given properties set.
    fn field(properties: &[(&str, Value)]) -> Element {
        let mut element = Element::new("TextInput", None);
        for (name, value) in properties {
            element.set(name, value.clone());
        }
        return element;
    }

    /// A two-line table: five chars at 10dp each, then five at 8dp each.
    fn two_lines() -> Vec<VisualLine> {
        return vec![
            VisualLine {
                start: 0,
                end: 5,
                caret_x: vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0],
            },
            VisualLine {
                start: 5,
                end: 10,
                caret_x: vec![0.0, 8.0, 16.0, 24.0, 32.0, 40.0],
            },
        ];
    }

    #[test]
    fn a_line_break_survives_only_in_a_multiline_field() {
        let mut multiline = TextInputState::from_text("ab");
        multiline.insert_input("c\nd", None, true);
        assert_eq!(multiline.text, "abc\nd");
        assert_eq!(multiline.cursor, 5);

        let mut single = TextInputState::from_text("ab");
        single.insert_input("c\nd", None, false);
        assert_eq!(single.text, "abcd", "a one-line field has no line 2");
        assert_eq!(single.cursor, 4);
        // A paste of nothing but newlines is dropped entirely.
        single.insert_input("\n\r\n", None, false);
        assert_eq!(single.text, "abcd");
    }

    #[test]
    fn max_length_caps_what_can_be_typed() {
        let mut state = TextInputState::from_text("");
        state.insert_input("hello world", Some(5), false);
        assert_eq!(state.text, "hello");
        assert_eq!(state.cursor, 5);
        // At the cap, further input is dropped and the cursor stays put.
        state.insert_input("!", Some(5), true);
        assert_eq!(state.text, "hello");
        assert_eq!(state.cursor, 5);
        // Deleting frees room again: exactly one char fits after removing
        // one from a full field.
        state.move_edge(false, false);
        state.delete_forward();
        state.insert_input("XY", Some(5), false);
        assert_eq!(state.text, "Xello", "room for one, not two");
    }

    #[test]
    fn replacing_a_selection_frees_room_under_max_length() {
        let mut state = TextInputState::from_text("hello");
        state.anchor = Some(0);
        state.cursor = 5;
        // The cap counts what the field holds after the edit, not the
        // length of the pasted span.
        state.insert_input("hi", Some(5), false);
        assert_eq!(state.text, "hi");
    }

    #[test]
    fn vertical_movement_keeps_the_visual_column() {
        let lines = two_lines();
        let mut state = TextInputState::from_text("abcdefghij");
        state.cursor = 3; // 30dp into the first line
        assert!(state.move_vertical(&lines, false, false));
        assert_eq!(state.cursor, 9, "the nearest caret at 30dp is 32dp, char 9");
        assert!(state.move_vertical(&lines, true, false));
        assert_eq!(state.cursor, 3, "the column survives the round trip");
    }

    #[test]
    fn vertical_movement_clamps_on_a_short_line() {
        let lines = vec![
            VisualLine {
                start: 0,
                end: 8,
                caret_x: vec![0.0, 10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0],
            },
            VisualLine {
                start: 8,
                end: 9,
                caret_x: vec![0.0, 20.0],
            },
        ];
        let mut state = TextInputState::from_text("abcdefghX");
        state.cursor = 7; // 70dp in, far past the second line's end
        assert!(state.move_vertical(&lines, false, false));
        assert_eq!(state.cursor, 9, "clamped to the short line's end");
    }

    #[test]
    fn vertical_movement_stops_at_the_ends_and_can_extend() {
        let lines = two_lines();
        let mut state = TextInputState::from_text("abcdefghij");
        state.move_edge(false, false);
        assert!(!state.move_vertical(&lines, true, false), "no line above");
        state.move_edge(true, false);
        assert!(!state.move_vertical(&lines, false, false), "no line below");
        assert!(state.move_vertical(&lines, true, false));
        // Shift+Down grows the selection from the pre-move cursor.
        state.move_edge(false, false);
        assert!(state.move_vertical(&lines, false, true));
        assert_eq!(state.selection(), (0, 5));
        assert!(!TextInputState::default().move_vertical(&[], false, false));
    }

    #[test]
    fn a_vertical_run_remembers_the_column_past_a_short_line() {
        // A wide line, a one-char line, then a wide one.
        let lines = vec![
            VisualLine {
                start: 0,
                end: 3,
                caret_x: vec![0.0, 20.0, 40.0, 60.0],
            },
            VisualLine {
                start: 3,
                end: 4,
                caret_x: vec![0.0, 10.0],
            },
            VisualLine {
                start: 4,
                end: 7,
                caret_x: vec![0.0, 20.0, 40.0, 60.0],
            },
        ];
        let mut state = TextInputState::from_text("abcdefg");
        state.move_to(2, false); // 40dp into the first line
        assert!(state.move_vertical(&lines, false, false));
        assert_eq!(
            state.cursor, 3,
            "clamped onto the short line itself, not onto the next line's start"
        );
        assert!(state.move_vertical(&lines, false, false));
        assert_eq!(state.cursor, 6, "the run still aims 40dp in");
        assert!(state.move_vertical(&lines, true, false));
        assert!(state.move_vertical(&lines, true, false));
        assert_eq!(state.cursor, 2, "and returns to the column it started in");

        // A horizontal move ends the run, so the next vertical move aims
        // from the caret's own position.
        assert!(state.move_vertical(&lines, false, false));
        state.move_horizontal(false, false);
        assert_eq!(state.cursor, 2, "left along the first line");
        assert!(state.move_vertical(&lines, false, false));
        assert_eq!(
            state.cursor, 3,
            "and down from the caret it was moved to, not the old column"
        );
    }

    #[test]
    fn field_flags_read_from_their_properties() {
        let plain = field(&[]);
        assert!(plain.is_text_input());
        assert!(!plain.is_multiline());
        assert!(!plain.is_read_only());
        assert!(!plain.is_password());
        assert!(!plain.selects_all_on_focus());
        assert_eq!(plain.max_length(), None);
        assert_eq!(plain.validator(), None);
        // Only a TextInput carries the shape; a Text does not.
        assert!(!Element::new("Text", None).is_text_input());

        let flagged = field(&[
            ("multiline", Value::Bool(true)),
            ("read_only", Value::Bool(true)),
            ("password", Value::Bool(true)),
            ("select_all_on_focus", Value::Bool(true)),
            ("max_length", Value::Int(8)),
            ("validator", Value::String("int".to_string())),
        ]);
        assert!(flagged.is_multiline());
        assert!(flagged.is_read_only());
        assert!(flagged.is_password());
        assert!(flagged.selects_all_on_focus());
        assert_eq!(flagged.max_length(), Some(8));
        assert_eq!(flagged.validator(), Some("int"));
    }

    #[test]
    fn reveal_beats_password_and_a_silly_cap_is_no_cap() {
        let revealed = field(&[
            ("password", Value::Bool(true)),
            ("reveal", Value::Bool(true)),
        ]);
        assert!(!revealed.is_password(), "the show-password toggle wins");

        for cap in [0, -3] {
            assert_eq!(
                field(&[("max_length", Value::Int(cap))]).max_length(),
                None,
                "max_length = {cap} is a typo, not a locked field"
            );
        }
        // A float cap is read, and its fraction is dropped.
        assert_eq!(
            field(&[("max_length", Value::Float(4.7))]).max_length(),
            Some(4)
        );
        // An empty validator name is no validator.
        assert_eq!(
            field(&[("validator", Value::String(String::new()))]).validator(),
            None
        );
    }

    #[test]
    fn validators_accept_what_they_promise() {
        assert_eq!(validates("int", "42"), Some(true));
        assert_eq!(validates("int", "-7"), Some(true));
        assert_eq!(validates("int", "4.2"), Some(false));
        assert_eq!(validates("int", "-"), Some(false));
        assert_eq!(validates("int", ""), Some(true), "empty is not a mistake");

        assert_eq!(validates("float", "4.2"), Some(true));
        assert_eq!(validates("float", "-0.5"), Some(true));
        assert_eq!(validates("float", "4."), Some(true), "still being typed");
        assert_eq!(validates("float", ".5"), Some(true));
        assert_eq!(validates("float", "1.2.3"), Some(false));
        assert_eq!(validates("float", "1e5"), Some(false));
        assert_eq!(validates("float", "."), Some(false));

        assert_eq!(validates("email", "a@b.co"), Some(true));
        assert_eq!(validates("email", "a@b"), Some(false), "no dotted domain");
        assert_eq!(validates("email", "a@b..co"), Some(false));
        assert_eq!(validates("email", "a@.co"), Some(false));
        assert_eq!(validates("email", "a@b.co."), Some(false));
        assert_eq!(validates("email", "a b@c.co"), Some(false));
        assert_eq!(validates("email", "a@@b.co"), Some(false));
        assert_eq!(
            validates("email", "a+b@c.co"),
            Some(true),
            "the local part is not character-checked"
        );

        // A name this build does not implement judges nothing: the field
        // must not be permanently invalid because of it.
        assert_eq!(validates("regex", "anything"), None);
        assert_eq!(validates("", "anything"), None);
    }
}
