//! nui: the framework facade — the only crate end users depend on.
//!
//! Contents: [`Application`] (winit run loop, on-demand redraw, hot-reload
//! polling) and [`host::WindowHost`] (per-window gpu/engine/layout state
//! with hit-test click dispatch and [`WindowHost::reload`]). M5 adds the
//! [`DocumentWatcher`] seam — nui-preview implements it with notify.
//!
//! # Examples
//! ```no_run
//! use nui_core::Size;
//! let config = nui::AppConfig::new(
//!     "component Counter { Window(id = root) {} }",
//!     "counter",
//!     Size::new(420.0, 300.0),
//! );
//! // nui::Application::new(config).run(); // blocking; needs a display
//! ```

pub mod app;
pub mod host;

pub use app::{AppConfig, Application, DocumentWatcher, ReloadWaker, element_bounds, hit_test};
pub use host::{HitTarget, WindowHost};
pub use nui_runtime::{ElementTree, Engine, instantiate};
