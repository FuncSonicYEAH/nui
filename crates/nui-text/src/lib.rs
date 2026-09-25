//! nui-text: text facilities (plan §6.2): cosmic-text shaping and line
//! breaking, swash glyph rasterization, and the etagere glyph atlas.
//!
//! The [`TextSystem`] is the single entry point: it owns the
//! `FontSystem`, the rasterization cache, and the CPU-side glyph atlas.
//! Two constructors exist:
//!
//! - [`TextSystem::with_embedded_font`] — deterministic, loads only the
//!   bundled DejaVu Sans (`assets/fonts/`, offline-safe). Used by golden
//!   tests and as the guaranteed fallback family.
//! - [`TextSystem::with_system_fonts`] — system fonts plus the embedded
//!   one (the app default; script fallback follows the platform).
//!
//! Glyph quads are emitted in pixel space: shape (layout positions) →
//! [`TextSystem::glyph_quad`] (rasterize into the atlas) → upload dirty
//! pages once → draw textured quads through the nui-render text pipeline.

pub mod atlas;
pub mod system;

pub use atlas::{GlyphAtlas, GlyphMask, GlyphQuad, GlyphSlot, atlas_page_size};
pub use system::{ShapedGlyph, ShapedText, TextSystem};

/// The bundled fallback typeface (DejaVu Sans; license alongside the file).
pub const EMBEDDED_FONT: &[u8] = include_bytes!("../../../assets/fonts/DejaVuSans.ttf");
