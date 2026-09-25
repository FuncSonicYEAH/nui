//! nui-winit: the platform layer (winit windows, input translation).
//!
//! Sole `cfg(target_os)`-free boundary per plan D13: winit itself abstracts
//! the OS; this crate maps winit's event model onto nui-core's normalized
//! [`Event`] stream (all coordinates dp, y-down, post-IME text).
//!
//! winit 0.30 runs on the `ApplicationHandler` trait; the
//! [`EventTranslator`] is the stateful piece the facade drives.

use nui_core::{Event, Key, Modifiers, Point, PointerButton, Size, WheelDelta};

/// Tracks keyboard modifiers (winit delivers modifier events separately).
#[derive(Debug, Default, Clone, Copy)]
pub struct EventTranslator {
    /// dpi scale factor of the tracked window (physical / logical px).
    scale_factor: f32,
    /// Current modifier state.
    modifiers: Modifiers,
    /// Last seen cursor position in dp (winit's `MouseInput` carries no
    /// position; presses/releases must reuse the preceding `CursorMoved`).
    cursor: Point,
}

impl EventTranslator {
    /// Creates a translator for a window with `scale_factor`.
    pub fn new(scale_factor: f32) -> EventTranslator {
        return EventTranslator {
            scale_factor,
            modifiers: Modifiers::NONE,
            cursor: Point::ZERO,
        };
    }

    /// Updates the dpi scale (per-monitor DPI v2, plan §7).
    pub fn set_scale_factor(&mut self, scale_factor: f32) {
        self.scale_factor = scale_factor;
    }

    /// Current dpi scale factor.
    pub fn scale_factor(&self) -> f32 {
        return self.scale_factor;
    }

    /// Converts a physical pixel position to dp.
    fn to_dp(self, x: f64, y: f64) -> Point {
        return Point::new(
            (x as f32) / self.scale_factor,
            (y as f32) / self.scale_factor,
        );
    }

    /// Translates a winit window event into zero or more normalized events.
    /// `window_size` is the inner size in physical pixels.
    pub fn translate(&mut self, event: winit::event::WindowEvent, window_size: Size) -> Vec<Event> {
        return match event {
            winit::event::WindowEvent::RedrawRequested => Vec::new(),
            winit::event::WindowEvent::Resized(physical) => {
                vec![Event::WindowResized {
                    size: Size::new(
                        physical.width as f32 / self.scale_factor,
                        physical.height as f32 / self.scale_factor,
                    ),
                }]
            }
            winit::event::WindowEvent::Focused(focused) => {
                vec![Event::WindowFocused { focused }]
            }
            winit::event::WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers_changed(modifiers)
            }
            winit::event::WindowEvent::CloseRequested => vec![Event::CloseRequested],
            winit::event::WindowEvent::CursorMoved { position, .. } => self.cursor_moved(position),
            winit::event::WindowEvent::CursorEntered { .. } => {
                vec![Event::CursorEnteredWindow]
            }
            winit::event::WindowEvent::CursorLeft { .. } => vec![Event::CursorLeftWindow],
            winit::event::WindowEvent::MouseInput { state, button, .. } => {
                self.mouse_input(state, button)
            }
            winit::event::WindowEvent::MouseWheel { delta, .. } => {
                let scaled = match delta {
                    winit::event::MouseScrollDelta::PixelDelta(logical) => WheelDelta::Pixels {
                        x: logical.x as f32 / self.scale_factor,
                        y: logical.y as f32 / self.scale_factor,
                    },
                    winit::event::MouseScrollDelta::LineDelta(x, y) => WheelDelta::Lines { x, y },
                };
                vec![Event::WheelScrolled {
                    position: Point::ZERO,
                    delta: scaled,
                }]
            }
            winit::event::WindowEvent::KeyboardInput { event, .. } => {
                return self.translate_keyboard(event);
            }
            winit::event::WindowEvent::Ime(ime) => {
                return match ime {
                    winit::event::Ime::Commit(text) => {
                        vec![Event::TextInput { text }]
                    }
                    winit::event::Ime::Preedit(text, _) => {
                        vec![Event::ImePreedit { text }]
                    }
                    _ => Vec::new(),
                };
            }
            winit::event::WindowEvent::ScaleFactorChanged {
                scale_factor,
                inner_size_writer,
            } => {
                self.scale_factor = scale_factor as f32;
                // Keep the window's physical size; only the dpi scale
                // changes (plan §7 per-monitor DPI).
                drop(inner_size_writer);
                Vec::new()
            }
            winit::event::WindowEvent::Destroyed => Vec::new(),
            // The remaining winit variants (moved/destroyed files, theme,
            // occlusion...) carry no nui behavior in M3; `window_size` is
            // used by the facade for hit testing, not translation.
            _ => {
                let _ = window_size;
                Vec::new()
            }
        };
    }

    /// Cursor moved: converts to dp and remembers the position — winit's
    /// `MouseInput` carries no position, so presses/releases reuse this.
    pub(crate) fn cursor_moved(
        &mut self,
        position: winit::dpi::PhysicalPosition<f64>,
    ) -> Vec<Event> {
        let position = self.to_dp(position.x, position.y);
        self.cursor = position;
        return vec![Event::PointerMoved { position }];
    }

    /// Mouse button state change at the tracked cursor position.
    pub(crate) fn mouse_input(
        &mut self,
        state: winit::event::ElementState,
        button: winit::event::MouseButton,
    ) -> Vec<Event> {
        let button = map_button(button);
        return match state {
            winit::event::ElementState::Pressed => {
                vec![Event::PointerPressed {
                    button,
                    position: self.cursor,
                    modifiers: self.modifiers,
                }]
            }
            winit::event::ElementState::Released => {
                vec![Event::PointerReleased {
                    button,
                    position: self.cursor,
                    modifiers: self.modifiers,
                }]
            }
        };
    }

    /// Updates the tracked modifier state (shift-selection, ctrl shortcuts).
    pub(crate) fn modifiers_changed(&mut self, modifiers: winit::event::Modifiers) -> Vec<Event> {
        let state = modifiers.state();
        self.modifiers = Modifiers {
            ctrl: state.control_key(),
            shift: state.shift_key(),
            alt: state.alt_key(),
            meta: state.super_key(),
        };
        return Vec::new();
    }

    fn translate_keyboard(&mut self, event: winit::event::KeyEvent) -> Vec<Event> {
        let state = event.state;
        let key = map_key(&event);
        let Some(key) = key else {
            return Vec::new();
        };
        return match state {
            winit::event::ElementState::Pressed => {
                vec![Event::KeyPressed {
                    key,
                    modifiers: self.modifiers,
                }]
            }
            winit::event::ElementState::Released => {
                vec![Event::KeyReleased {
                    key,
                    modifiers: self.modifiers,
                }]
            }
        };
    }
}

/// Maps a winit button. `Back`/`Forward` map to `Other(4)`/`Other(5)`
/// (the historical X11 button numbers).
fn map_button(button: winit::event::MouseButton) -> PointerButton {
    return match button {
        winit::event::MouseButton::Left => PointerButton::Left,
        winit::event::MouseButton::Right => PointerButton::Right,
        winit::event::MouseButton::Middle => PointerButton::Middle,
        winit::event::MouseButton::Back => PointerButton::Other(4),
        winit::event::MouseButton::Forward => PointerButton::Other(5),
        winit::event::MouseButton::Other(index) => PointerButton::Other(index as u8),
    };
}

/// Maps a winit key to the normalized key set; unmapped keys are dropped.
fn map_key(event: &winit::event::KeyEvent) -> Option<Key> {
    use winit::keyboard::{Key as WinitKey, NamedKey};
    return match &event.logical_key {
        WinitKey::Named(NamedKey::Enter) => Some(Key::Enter),
        WinitKey::Named(NamedKey::Escape) => Some(Key::Escape),
        WinitKey::Named(NamedKey::Backspace) => Some(Key::Backspace),
        WinitKey::Named(NamedKey::Delete) => Some(Key::Delete),
        WinitKey::Named(NamedKey::Tab) => Some(Key::Tab),
        WinitKey::Named(NamedKey::Space) => Some(Key::Space),
        WinitKey::Named(NamedKey::ArrowUp) => Some(Key::ArrowUp),
        WinitKey::Named(NamedKey::ArrowDown) => Some(Key::ArrowDown),
        WinitKey::Named(NamedKey::ArrowLeft) => Some(Key::ArrowLeft),
        WinitKey::Named(NamedKey::ArrowRight) => Some(Key::ArrowRight),
        WinitKey::Named(NamedKey::Home) => Some(Key::Home),
        WinitKey::Named(NamedKey::End) => Some(Key::End),
        WinitKey::Named(NamedKey::PageUp) => Some(Key::PageUp),
        WinitKey::Named(NamedKey::PageDown) => Some(Key::PageDown),
        WinitKey::Character(text) => text.chars().next().map(Key::Character),
        _ => None,
    };
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn resize_translates_to_logical_dp() {
        let mut translator = EventTranslator::new(2.0);
        let events = translator.translate(
            winit::event::WindowEvent::Resized(winit::dpi::PhysicalSize::new(800, 600)),
            Size::new(400.0, 300.0),
        );
        assert_eq!(
            events,
            vec![Event::WindowResized {
                size: Size::new(400.0, 300.0),
            }]
        );
    }

    #[test]
    fn close_request_passes_through() {
        let mut translator = EventTranslator::new(1.0);
        let events = translator.translate(
            winit::event::WindowEvent::CloseRequested,
            Size::new(0.0, 0.0),
        );
        assert_eq!(events, vec![Event::CloseRequested]);
    }

    #[test]
    fn cursor_move_scales_by_dpi() {
        let mut translator = EventTranslator::new(2.0);
        let device_id = winit::event::DeviceId::dummy();
        let events = translator.translate(
            winit::event::WindowEvent::CursorMoved {
                device_id,
                position: winit::dpi::PhysicalPosition::new(100.0, 50.0),
            },
            Size::new(0.0, 0.0),
        );
        assert_eq!(
            events,
            vec![Event::PointerMoved {
                position: Point::new(50.0, 25.0),
            }]
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod cursor_tests {
    use super::*;
    use nui_core::Point;
    use winit::event::{ElementState, MouseButton};

    #[test]
    fn mouse_input_reuses_last_cursor_position() {
        let mut translator = EventTranslator::new(1.0);
        let moved = translator.cursor_moved(winit::dpi::PhysicalPosition::new(120.0, 60.0));
        assert_eq!(
            moved,
            vec![Event::PointerMoved {
                position: Point::new(120.0, 60.0)
            }]
        );
        let pressed = translator.mouse_input(ElementState::Pressed, MouseButton::Left);
        assert!(
            matches!(pressed.as_slice(), [Event::PointerPressed { position, .. }] if *position == Point::new(120.0, 60.0)),
            "press must reuse the tracked cursor, got {pressed:?}"
        );
        // dpi scale: positions convert to dp.
        let mut scaled = EventTranslator::new(2.0);
        let _ = scaled.cursor_moved(winit::dpi::PhysicalPosition::new(100.0, 100.0));
        let pressed = scaled.mouse_input(ElementState::Pressed, MouseButton::Left);
        assert!(
            matches!(pressed.as_slice(), [Event::PointerPressed { position, .. }] if *position == Point::new(50.0, 50.0)),
            "press positions are dp"
        );
    }

    #[test]
    fn ime_preedit_and_commit_translate() {
        let mut translator = EventTranslator::new(1.0);
        let preedit = translator.translate(
            winit::event::WindowEvent::Ime(winit::event::Ime::Preedit(
                "ni".to_string(),
                Some((2, 2)),
            )),
            nui_core::Size::new(800.0, 600.0),
        );
        assert!(matches!(preedit.as_slice(), [Event::ImePreedit { text }] if text == "ni"));
        let commit = translator.translate(
            winit::event::WindowEvent::Ime(winit::event::Ime::Commit("你".to_string())),
            nui_core::Size::new(800.0, 600.0),
        );
        assert!(matches!(commit.as_slice(), [Event::TextInput { text }] if text == "你"));
    }

    #[test]
    fn modifiers_survive_across_events() {
        let mut translator = EventTranslator::new(1.0);
        let state = winit::keyboard::ModifiersState::CONTROL;
        let _ = translator.modifiers_changed(winit::event::Modifiers::from(state));
        let pressed = translator.mouse_input(ElementState::Pressed, MouseButton::Left);
        assert!(
            matches!(pressed.as_slice(), [Event::PointerPressed { modifiers, .. }] if modifiers.ctrl),
            "ctrl must persist across events"
        );
    }
}
