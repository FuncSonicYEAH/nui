//! nui-tools: dependency-free helpers shared across the nui crates.
//!
//! Nothing here knows about nui. Every function takes and returns plain
//! `f32`/`f64`/`&str`/`&[u8]`, with **no** domain type in any signature —
//! that is the crate's whole admission rule, and it is what keeps this
//! crate at zero dependencies and usable from any layer.
//!
//! Why it exists: the same small pieces of arithmetic were being written
//! more than once while they lived next to the code that first needed
//! them — three spellings of a float lerp, two of "where is `value`
//! between `min` and `max`", two of "snap to a step grid". Having one
//! implementation is not just tidier; it is the only way the edge cases
//! (`NaN`, zero span, a step that is not a power of ten) get decided
//! *once* and tested once.
//!
//! What deliberately stayed behind: anything whose signature carries a nui
//! type. `Point`-based geometry (`nui-core/src/path.rs`,
//! `nui-core/src/earcut.rs`), `Color`-based palette work
//! (`nui-render/src/widget/state.rs`), `Value` interpolation
//! (`nui-runtime/src/animation.rs`), and dp/percent resolution
//! (`nui-core/src/length.rs`) are all mathematically generic but are
//! *documented in terms of* their domain type, and moving them would mean
//! either a generic signature rewrite or a second geometry type. Each is
//! listed in the migration report as a candidate for a later pass.

pub mod color;
pub mod curve;
pub mod hash;
pub mod numeric;
pub mod text;

pub use color::{blend_u8, component_to_u8, hex_value};
pub use curve::{apply_bezier, bezier_axis, bezier_x, bezier_x_slope, bezier_y, solve_segment_x};
pub use hash::{cache_key, content_hash};
pub use numeric::{
    decimal_places, inverse_lerp, lerp_f32, lerp_f64, progress, round_to_grid, step_grid,
};
pub use text::{byte_to_char, char_to_byte, edit_distance, is_email, is_float, is_integer};
