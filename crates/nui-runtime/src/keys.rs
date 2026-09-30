//! `on key`: handing a key to the document.
//!
//! A key arrives at the host with no element attached. A click has the
//! pointer to hit-test with; a key has nothing, so two questions have no
//! obvious answer — *what* was pressed, and *who* is it for.
//!
//! # What it was: the window's
//!
//! `hovered` and `armed` are facts about the pointer's position relative to
//! *an element*, which is why the widget layer writes them on the element
//! that earned them. A keystroke is not about an element: it is a fact
//! about the window. So the key is written on the window root — `key`,
//! `character`, `modifiers` — and nowhere else.
//!
//! Writing it in exactly one place is the point. A document-level shortcut
//! reads `root.key`, and it gets the same answer whether or not some
//! control happens to hold focus; a host that also wrote the properties
//! onto the focus target would make `root.key` stale in precisely the case
//! a window-wide shortcut exists for.
//!
//! # Who it is for: focus decides, and only focus
//!
//! The signal bubbles from the focused element when there is one, so a
//! container that wants to see keys while its child is focused is asked
//! first; with nothing focused it bubbles from the window root; either way
//! the window sees it on the way up. The route does not move the
//! properties, so a handler never has to work out which element the host
//! chose before it can read the key.
//!
//! One consequence is worth stating plainly, because it decides where a
//! shortcut is written: bubbling travels *upwards*, so a whole-window
//! shortcut belongs on the `Window`. A page or panel that is a descendant
//! of the root is never on the path from the root, and never asked — which
//! is the right answer anyway for a subtree that `visible = false` has
//! taken out of the frame.
//!
//! Note that the focus target here is not a *consumer*. [`Engine::dispatch_key`]
//! is called after the widget layer has declined the key — a focused
//! `TextInput` that took it has already done so, and a focused `Button`
//! that activated has already fired its `click`. What is left is a
//! document shortcut, and the document owns the window root.
//!
//! # Reading it
//!
//! A document reads these the way it reads any other property, so it has to
//! declare them:
//!
//! ```text
//! component App {
//!     property key: String = ""
//!
//!     Window(id = root) {
//!         on key => { if key == "s" then save() }
//!     }
//! }
//! ```
//!
//! The declaration is load-bearing for two reasons. `root.key` is checked
//! against the component's property table, so without it the read is a
//! compile error — that one is a diagnosis, not a trap. The trap is a
//! *bare* `key`: the checker resolves an identifier that matches nothing to
//! an enum variant literal (that is how `bold`, `ease-out` and friends are
//! spelled), so a handler that forgot the declaration would quietly assign
//! the word `key` rather than the key that was pressed.
//!
//! # Ordering
//!
//! A key the widget layer consumed never reaches here. That matters: letting
//! a consumed key also arrive as a document shortcut is how an application
//! ends up typing "s" and saving a file on the same keystroke.
//!
//! # Prohibited shape
//!
//! Writing the properties unconditionally means a document that ignores
//! `key` still pays three host writes per unhandled keystroke. That is the
//! honest cost of not making the document declare what it listens to; it
//! buys the property that adding a shortcut needs no registration.

use nui_core::{Key, Modifiers, Value};

use crate::binding::Engine;
use crate::element::{ElementId, ElementTree};

/// Property holding [`Key::name`] — `"7"`, `"Enter"`, `"ArrowUp"`.
pub const KEY_PROPERTY: &str = "key";
/// Property holding [`Key::character`] — what the key types, or `""`.
pub const CHARACTER_PROPERTY: &str = "character";
/// Property holding [`Modifiers::name`] — `"ctrl+shift"`, or `""`.
pub const MODIFIERS_PROPERTY: &str = "modifiers";

/// The signal a key bubbles.
pub const KEY_SIGNAL: &str = "key";

impl Engine {
    /// Where a key's signal starts: the focused element while it is still
    /// in the tree, the window root otherwise.
    ///
    /// `None` only for an empty tree, which has nothing to tell.
    pub fn key_route(&self, tree: &ElementTree) -> Option<ElementId> {
        if let Some(element) = self.focused()
            && tree.arena.contains_key(element)
        {
            return Some(element);
        }
        return tree.roots.first().copied();
    }

    /// Hands `key` to the document: writes its identity onto the window
    /// root, then bubbles a [`KEY_SIGNAL`] from [`Engine::key_route`].
    ///
    /// Returns whether the properties changed, so the caller knows a frame
    /// is owed. A held key that writes the same three values reports
    /// `false`.
    pub fn dispatch_key(&mut self, tree: &mut ElementTree, key: Key, modifiers: Modifiers) -> bool {
        let Some(root) = tree.roots.first().copied() else {
            return false;
        };
        let character = match key.character() {
            Some(character) => character.to_string(),
            None => String::new(),
        };
        let mut written = self.set_direct(tree, root, KEY_PROPERTY, Value::String(key.name()));
        written |= self.set_direct(tree, root, CHARACTER_PROPERTY, Value::String(character));
        written |= self.set_direct(
            tree,
            root,
            MODIFIERS_PROPERTY,
            Value::String(modifiers.name()),
        );
        let route = self.key_route(tree).unwrap_or(root);
        let _ = self.emit_bubble(tree, route, KEY_SIGNAL);
        return written;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use nui_core::{Key, Modifiers, Value};

    use crate::element::{ElementId, ElementTree};
    use crate::instantiate::instantiate;
    use crate::keys::{CHARACTER_PROPERTY, KEY_PROPERTY, MODIFIERS_PROPERTY};

    /// A document that watches keys from both ends: the window records what
    /// it saw, and a child records that it was asked first.
    ///
    /// `key` is declared because a declared property is what makes the
    /// identifier a read — see the module docs.
    fn document() -> (ElementTree, crate::binding::Engine) {
        let outcome = nui_compiler::compile(
            r#"
            component App {
                property last: String = ""
                property trace: String = ""
                property key: String = ""

                Window(id = root) {
                    on key => {
                        last = key
                        trace = trace + "w"
                    }
                    Column(id = body, spacing = 0dp) {
                        Text(id = seen, content <- last) {
                            on key => trace = trace + "c"
                        }
                    }
                }
            }
        "#,
        );
        assert!(
            outcome.diagnostics.is_empty(),
            "the fixture must compile clean: {:?}",
            outcome.diagnostics
        );
        let instance = instantiate(&outcome.document);
        return (instance.tree, instance.engine);
    }

    /// `root` (the window) and the child a test can focus.
    fn parts(tree: &ElementTree) -> (ElementId, ElementId) {
        let root = tree.lookup_id("root").unwrap();
        let seen = tree.lookup_id("seen").unwrap();
        return (root, seen);
    }

    #[test]
    fn a_key_reaches_the_window_when_nothing_holds_focus() {
        let (mut tree, mut engine) = document();
        let (root, _) = parts(&tree);
        assert_eq!(engine.focused(), None, "nothing holds focus");
        assert!(engine.dispatch_key(&mut tree, Key::Character('7'), Modifiers::NONE));
        assert_eq!(
            tree.arena[root].get(KEY_PROPERTY),
            Some(&Value::String("7".to_string()))
        );
        assert_eq!(
            tree.arena[root].get(CHARACTER_PROPERTY),
            Some(&Value::String("7".to_string()))
        );
        // The handler ran, so the document saw it as well as the properties.
        let _ = engine.propagate(&mut tree);
        assert_eq!(
            tree.arena[root].get("last"),
            Some(&Value::String("7".to_string())),
            "`on key` fired"
        );
        assert_eq!(
            tree.arena[root].get("trace"),
            Some(&Value::String("w".to_string())),
            "with nothing focused the window is both the route and the reader"
        );
    }

    #[test]
    fn the_window_carries_the_key_even_while_a_child_holds_focus() {
        let (mut tree, mut engine) = document();
        let (root, seen) = parts(&tree);
        engine.focus(seen);
        assert!(engine.dispatch_key(&mut tree, Key::Character('q'), Modifiers::NONE));
        // The one place the identity is written is the window root — a
        // shortcut in the document reads `root.key` and is never stale.
        assert_eq!(
            tree.arena[root].get(KEY_PROPERTY),
            Some(&Value::String("q".to_string()))
        );
        assert_eq!(tree.arena[seen].get(KEY_PROPERTY), None);
        let _ = engine.propagate(&mut tree);
        assert_eq!(
            tree.arena[root].get("last"),
            Some(&Value::String("q".to_string())),
            "the window handler saw the key while a child held focus"
        );
    }

    #[test]
    fn the_focused_element_is_asked_before_the_window() {
        let (mut tree, mut engine) = document();
        let (root, seen) = parts(&tree);
        engine.focus(seen);
        let _ = engine.dispatch_key(&mut tree, Key::Character('q'), Modifiers::NONE);
        let _ = engine.propagate(&mut tree);
        assert_eq!(
            tree.arena[root].get("trace"),
            Some(&Value::String("cw".to_string())),
            "the focused element's handler runs, then the window's"
        );
        // Blur, and the window is the route outright.
        engine.blur();
        let _ = engine.dispatch_key(&mut tree, Key::Character('q'), Modifiers::NONE);
        let _ = engine.propagate(&mut tree);
        assert_eq!(
            tree.arena[root].get("trace"),
            Some(&Value::String("cww".to_string()))
        );
    }

    #[test]
    fn a_named_key_carries_a_name_and_no_character() {
        let (mut tree, mut engine) = document();
        let (root, _) = parts(&tree);
        assert!(engine.dispatch_key(&mut tree, Key::ArrowUp, Modifiers::NONE));
        assert_eq!(
            tree.arena[root].get(KEY_PROPERTY),
            Some(&Value::String("ArrowUp".to_string()))
        );
        assert_eq!(
            tree.arena[root].get(CHARACTER_PROPERTY),
            Some(&Value::String(String::new())),
            "an arrow key types nothing, so `character != \"\"` is the test for text"
        );
    }

    #[test]
    fn space_is_named_but_still_types_a_character() {
        let (mut tree, mut engine) = document();
        let (root, _) = parts(&tree);
        assert!(engine.dispatch_key(&mut tree, Key::Space, Modifiers::NONE));
        assert_eq!(
            tree.arena[root].get(KEY_PROPERTY),
            Some(&Value::String("Space".to_string()))
        );
        assert_eq!(
            tree.arena[root].get(CHARACTER_PROPERTY),
            Some(&Value::String(" ".to_string())),
            "the winit translation names the key, but it still types a space"
        );
    }

    #[test]
    fn modifiers_are_spelled_in_one_order() {
        let (mut tree, mut engine) = document();
        let (root, _) = parts(&tree);
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            alt: false,
            meta: false,
        };
        assert!(engine.dispatch_key(&mut tree, Key::Character('s'), modifiers));
        assert_eq!(
            tree.arena[root].get(MODIFIERS_PROPERTY),
            Some(&Value::String("ctrl+shift".to_string()))
        );
        // And the plain case is the empty string, not a missing property.
        assert!(engine.dispatch_key(&mut tree, Key::Character('s'), Modifiers::NONE));
        assert_eq!(
            tree.arena[root].get(MODIFIERS_PROPERTY),
            Some(&Value::String(String::new()))
        );
    }

    #[test]
    fn a_key_that_changes_nothing_is_not_dirty() {
        let (mut tree, mut engine) = document();
        assert!(engine.dispatch_key(&mut tree, Key::Character('7'), Modifiers::NONE));
        assert!(
            !engine.dispatch_key(&mut tree, Key::Character('7'), Modifiers::NONE),
            "the same key twice writes the same three values"
        );
        assert!(engine.dispatch_key(&mut tree, Key::Character('8'), Modifiers::NONE));
    }

    #[test]
    fn a_stale_focus_target_falls_back_to_the_window() {
        let (mut tree, mut engine) = document();
        let (root, seen) = parts(&tree);
        engine.focus(seen);
        // The element a focus pointed at is gone; the route must not panic
        // on the way out of the arena.
        tree.arena.remove(seen);
        assert_eq!(engine.key_route(&tree), Some(root));
        assert!(engine.dispatch_key(&mut tree, Key::Character('q'), Modifiers::NONE));
    }
}
