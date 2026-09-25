//! TextInput editing core (M7): cursor, selection, and text edits over
//! char indices (plan §5 输入). Pure logic — no engine, no rendering — so
//! editing behavior is headless-testable; IME commits land here too as
//! plain insertions.
//!
//! Invariants: `cursor` and selection indices are **char** indices into
//! `text` (byte offsets would corrupt multi-byte input); `cursor` always
//! sits at `selection_start..=selection_end`'s edge after edits.

/// Editing state of one `TextInput` element.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextInputState {
    /// The committed text.
    pub text: String,
    /// Cursor position as a char index (`0..=char_count`).
    pub cursor: usize,
    /// Selection anchor (char index); `None` = collapsed selection.
    pub anchor: Option<usize>,
    /// IME composition text shown after the cursor (not yet committed).
    pub preedit: String,
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
        if input.is_empty() {
            return;
        }
        let input_chars = input.chars().count();
        self.delete_selection();
        let byte = self.cursor_byte();
        self.text.insert_str(byte, input);
        self.cursor += input_chars;
        self.anchor = None;
    }

    /// Backspace: deletes the selection or the char left of the cursor.
    pub fn backspace(&mut self) {
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

    /// Moves the cursor to `target` (clamped); `extend` grows the
    /// selection, anchoring at the pre-move position when collapsed.
    pub fn move_to(&mut self, target: usize, extend: bool) {
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
        self.anchor = Some(0);
        self.cursor = self.text.chars().count();
    }

    /// Collapses the selection at the cursor.
    pub fn collapse(&mut self) {
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
    /// Moves keyboard focus to `element`.
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
    pub fn focus_next(&mut self, tree: &ElementTree) {
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
        self.focused = Some(next);
    }

    /// Routes a key press: Tab cycles focus, the rest edit the focused
    /// `TextInput`. Returns whether the event was consumed.
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
        if !reconcile_text_input(tree, element) {
            return false;
        }
        if modifiers.ctrl && key == Key::Character('a') {
            tree.arena[element]
                .text_input
                .as_mut()
                .expect("reconciled text input")
                .select_all();
            self.sync_text_property(tree, element);
            return true;
        }
        if key == Key::Enter {
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
                Key::Character(character) => {
                    state.insert(&character.to_string());
                    true
                }
                Key::Space => {
                    state.insert(" ");
                    true
                }
                Key::Backspace => {
                    state.backspace();
                    true
                }
                Key::Delete => {
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
        if !tree.arena.contains_key(element) || !reconcile_text_input(tree, element) {
            return false;
        }
        if let Some(state) = tree.arena[element].text_input.as_mut() {
            state.insert(input);
            state.preedit.clear();
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
    pub fn cut_focused(&mut self, tree: &mut ElementTree) -> Option<String> {
        let element = self.focused?;
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
    }
}

/// Resyncs the editing state from the `text` property (external writes land
/// on the property first: `<=>` partners, bindings, host writes). Returns
/// whether `element` is a text input at all.
fn reconcile_text_input(tree: &mut ElementTree, element: ElementId) -> bool {
    if tree.arena[element].text_input.is_none() {
        return false;
    }
    if let Some(Value::String(text)) = tree.arena[element].get("text").cloned()
        && let Some(state) = tree.arena[element].text_input.as_mut()
    {
        state.set_text(text);
    }
    return true;
}

/// Focus bookkeeping lives on the engine (per-window state).
impl Element {
    /// Convenience: whether the element is a `TextInput`.
    pub fn is_text_input(&self) -> bool {
        return self.ty == "TextInput";
    }
}

impl Engine {
    /// Sets the preedit (IME composition) text of the focused input.
    pub fn set_preedit(&mut self, tree: &mut ElementTree, preedit: &str) -> bool {
        let Some(element) = self.focused else {
            return false;
        };
        if !reconcile_text_input(tree, element) {
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
}
