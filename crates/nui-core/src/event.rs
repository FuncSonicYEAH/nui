//! Normalized input and window events.
//!
//! Produced by the platform layer (nui-winit, M3) by translating winit
//! events, and dispatched by the runtime (hit testing, focus routing, DSL
//! signals). All coordinates are in dp. Keyboard and text input coexist:
//! `KeyPressed` drives shortcuts/navigation, while `TextInput` carries
//! post-IME-composition text.

use crate::geometry::{Point, Size};

/// Pointer (mouse/touch) button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    /// Primary button (left click / single-finger touch).
    Left,
    /// Secondary button (right click).
    Right,
    /// Middle button (wheel press).
    Middle,
    /// Platform-specific button, carrying the raw button number.
    Other(u8),
}

/// Modifier key state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Modifiers {
    /// Ctrl (on macOS the Control key, not Command).
    pub ctrl: bool,
    /// Shift.
    pub shift: bool,
    /// Alt / Option.
    pub alt: bool,
    /// Meta / Command / Win.
    pub meta: bool,
}

impl Modifiers {
    /// No modifier keys pressed.
    pub const NONE: Modifiers = Modifiers {
        ctrl: false,
        shift: false,
        alt: false,
        meta: false,
    };

    /// Whether no modifier keys are pressed.
    pub fn is_none(&self) -> bool {
        return *self == Modifiers::NONE;
    }
}

/// Keyboard key.
///
/// Covers the set needed for shortcuts and navigation; extended when winit
/// lands in M3. No wildcard variant, so adding a key forces the compiler to
/// handle every match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Printable character (after keyboard-layout mapping, not a scan code).
    Character(char),
    /// Enter.
    Enter,
    /// Escape / cancel.
    Escape,
    /// Backspace.
    Backspace,
    /// Forward delete.
    Delete,
    /// Tab.
    Tab,
    /// Space.
    Space,
    /// Up arrow.
    ArrowUp,
    /// Down arrow.
    ArrowDown,
    /// Left arrow.
    ArrowLeft,
    /// Right arrow.
    ArrowRight,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
}

/// Wheel scroll delta.
///
/// Pixel mode: x positive is right, y positive is down (matching screen
/// coordinates); line mode follows the same directions, with the height of
/// one line interpreted by the consumer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WheelDelta {
    /// Pixel delta.
    Pixels {
        /// Horizontal delta (positive is right).
        x: f32,
        /// Vertical delta (positive is down).
        y: f32,
    },
    /// Logical line delta.
    Lines {
        /// Horizontal lines (positive is right).
        x: f32,
        /// Vertical lines (positive is down).
        y: f32,
    },
}

/// Normalized event (platform layer -> runtime).
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Pointer moved.
    PointerMoved {
        /// Pointer position (dp, window-relative).
        position: Point,
    },
    /// Pointer pressed.
    PointerPressed {
        /// Pressed button.
        button: PointerButton,
        /// Press position.
        position: Point,
        /// Modifier state.
        modifiers: Modifiers,
    },
    /// Pointer released.
    PointerReleased {
        /// Released button.
        button: PointerButton,
        /// Release position.
        position: Point,
        /// Modifier state.
        modifiers: Modifiers,
    },
    /// Pointer entered the window.
    CursorEnteredWindow,
    /// Pointer left the window.
    CursorLeftWindow,
    /// Wheel scrolled.
    WheelScrolled {
        /// Pointer position.
        position: Point,
        /// Scroll delta.
        delta: WheelDelta,
    },
    /// Key pressed.
    KeyPressed {
        /// Key.
        key: Key,
        /// Modifier state.
        modifiers: Modifiers,
    },
    /// Key released.
    KeyReleased {
        /// Key.
        key: Key,
        /// Modifier state.
        modifiers: Modifiers,
    },
    /// Text input (including IME-composed text), appended to the focused
    /// text component.
    TextInput {
        /// Input text.
        text: String,
    },
    /// IME composition in progress (fcitx5/ibus/XIM): shown inline in the
    /// focused text component until the next commit replaces it.
    ImePreedit {
        /// Composition text (empty = composition ended).
        text: String,
    },
    /// Window size changed (dp logical size).
    WindowResized {
        /// New logical size.
        size: Size,
    },
    /// Window focus gained or lost.
    WindowFocused {
        /// true = focus gained.
        focused: bool,
    },
    /// Window close requested.
    CloseRequested,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn none_modifiers_equals_default() {
        assert_eq!(Modifiers::NONE, Modifiers::default());
    }

    #[test]
    fn is_none_reports_modifier_presence() {
        let shifted = Modifiers {
            shift: true,
            ..Modifiers::NONE
        };
        assert!(Modifiers::NONE.is_none());
        assert!(!shifted.is_none());
    }

    #[test]
    fn events_compare_by_value() {
        let pressed = Event::PointerPressed {
            button: PointerButton::Left,
            position: Point::new(10.0, 20.0),
            modifiers: Modifiers::NONE,
        };
        assert_eq!(pressed, pressed.clone());
    }

    #[test]
    fn wheel_delta_variants_carry_direction() {
        let delta = WheelDelta::Pixels { x: 0.0, y: -120.0 };
        let WheelDelta::Pixels { x, y } = delta else {
            panic!("expected pixel delta");
        };
        assert_eq!((x, y), (0.0, -120.0));
    }
}
