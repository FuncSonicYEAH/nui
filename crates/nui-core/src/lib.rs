//! nui-core: foundational value, geometry, color, length, duration, and
//! event types.
//!
//! Zero platform dependencies (only thiserror), shared by three layers:
//! - compiler (nui-compiler): base type set for literals and the type system;
//! - runtime (nui-runtime): property storage ([`Value`]) and event dispatch
//!   ([`Event`]);
//! - rendering (nui-render): geometry and color.
//!
//! Coordinates and lengths are uniformly in dp (density-independent pixels);
//! physical pixel conversion happens only at render submission. The module
//! checklist lives in `plan.md` at the repository root (milestone M0).

pub mod color;
pub mod duration;
pub mod earcut;
pub mod error;
pub mod event;
pub mod geometry;
pub mod length;
pub mod path;
pub mod value;

pub use color::Color;
pub use duration::Duration;
pub use error::{Error, Result};
pub use event::{Event, Key, Modifiers, PointerButton, WheelDelta};
pub use geometry::{Point, Rect, Size};
pub use length::Length;
pub use value::Value;
