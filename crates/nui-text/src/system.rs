//! TextSystem: cosmic-text shaping + swash rasterization + the glyph atlas.
//!
//! All positions are pixel-space: `shape` returns glyph baseline origins
//! (already subpixel-binned by cosmic-text's cache keys), and `glyph_quad`
//! returns the atlas placement plus the mask offset from the baseline
//! origin — the composition rule is
//! `quad_top_left = glyph_origin + (left, top)`, matching cosmic-text's own
//! renderer.

use std::collections::HashMap;

use cosmic_text::{Attrs, Buffer, FontSystem, Metrics, Shaping, SwashCache, SwashContent};

use crate::atlas::{GlyphAtlas, GlyphMask, GlyphQuad};

/// Default line-height factor over font size (baseline rules unified; QML
/// uses ~1.2 for single-line text).
const LINE_HEIGHT_FACTOR: f32 = 1.2;

/// One shaped glyph: baseline origin in pixels plus its atlas cache key.
#[derive(Debug, Clone, Copy)]
pub struct ShapedGlyph {
    /// Baseline-origin x (pixels from the line start).
    pub x: f32,
    /// Baseline-origin y (pixels from the shaped text's top).
    pub y: f32,
    /// Atlas cache key (font, glyph id, size, subpixel bins).
    pub key: cosmic_text::CacheKey,
}

/// A shaped single-paragraph text: measured size plus placed glyphs.
#[derive(Debug, Clone)]
pub struct ShapedText {
    /// Total width (pixels).
    pub width: f32,
    /// Total height (pixels, `lines * line_height`).
    pub height: f32,
    /// Placed glyphs in order.
    pub glyphs: Vec<ShapedGlyph>,
}

/// Owns shaping, rasterization, and the glyph atlas.
pub struct TextSystem {
    font_system: FontSystem,
    swash_cache: SwashCache,
    atlas: GlyphAtlas,
    /// Baseline offsets per rasterized glyph (placement is only known at
    /// rasterize time, so it is stored beside the atlas slot).
    placements: HashMap<cosmic_text::CacheKey, (i32, i32)>,
    /// Measure cache keyed by `(text, font size bits)`: taffy's measure
    /// callback runs many times per frame for the same strings.
    measure_cache: HashMap<(String, u32), (f32, f32)>,
}

impl std::fmt::Debug for TextSystem {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return formatter
            .debug_struct("TextSystem")
            .field("atlas_pages", &self.atlas.page_count())
            .field("measure_cache", &self.measure_cache.len())
            .finish();
    }
}

impl TextSystem {
    /// Creates a system with only the bundled DejaVu Sans: deterministic
    /// and offline-safe (golden tests; guaranteed fallback family).
    pub fn with_embedded_font() -> TextSystem {
        let source = fontdb::Source::Binary(std::sync::Arc::new(crate::EMBEDDED_FONT));
        return TextSystem::from_font_system(FontSystem::new_with_fonts([source]));
    }

    /// Creates the app-default system: platform fonts plus the embedded
    /// fallback family.
    pub fn with_system_fonts() -> TextSystem {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        database.load_font_source(fontdb::Source::Binary(std::sync::Arc::new(
            crate::EMBEDDED_FONT,
        )));
        let locale = std::env::var("LANG").unwrap_or_else(|_| return "en".to_string());
        return TextSystem::from_font_system(FontSystem::new_with_locale_and_db(locale, database));
    }

    fn from_font_system(font_system: FontSystem) -> TextSystem {
        return TextSystem {
            font_system,
            swash_cache: SwashCache::new(),
            atlas: GlyphAtlas::new(),
            placements: HashMap::new(),
            measure_cache: HashMap::new(),
        };
    }

    /// Measures `text` at `font_size` (px) without wrapping; returns
    /// `(width, height)`. Cached per `(text, size)`.
    pub fn measure(&mut self, text: &str, font_size: f32) -> (f32, f32) {
        let key = (text.to_string(), font_size.to_bits());
        if let Some(cached) = self.measure_cache.get(&key) {
            return *cached;
        }
        let shaped = self.shape(text, font_size);
        let measured = (shaped.width, shaped.height);
        self.measure_cache.insert(key, measured);
        return measured;
    }

    /// Shapes `text` (single paragraph, no wrap) and returns placed glyphs.
    pub fn shape(&mut self, text: &str, font_size: f32) -> ShapedText {
        let metrics = Metrics::new(font_size, font_size * LINE_HEIGHT_FACTOR);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(None, None);
        buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);
        let mut glyphs = Vec::new();
        let (mut width, mut height) = (0.0_f32, 0.0_f32);
        for run in buffer.layout_runs() {
            let line_y = run.line_y;
            width = width.max(run.line_w);
            height += run.line_height;
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                glyphs.push(ShapedGlyph {
                    x: physical.x as f32,
                    y: line_y + physical.y as f32,
                    key: physical.cache_key,
                });
            }
        }
        return ShapedText {
            width,
            height,
            glyphs,
        };
    }

    /// Returns the atlas quad for `key`, rasterizing it once on first use.
    /// `None` for blank or unsupported glyphs (color emoji in v1).
    pub fn glyph_quad(&mut self, key: &cosmic_text::CacheKey) -> Option<GlyphQuad> {
        if let Some(slot) = self.atlas.lookup(key) {
            let (left, top) = self.placements.get(key).copied().unwrap_or((0, 0));
            return Some(GlyphQuad { slot, left, top });
        }
        let image = self
            .swash_cache
            .get_image(&mut self.font_system, *key)
            .clone()?;
        // swash placement is Y-up (top = baseline -> glyph top); screen
        // coordinates are Y-down, so the sign flips.
        let (left, top) = (image.placement.left, -image.placement.top);
        let mask = alpha_mask(&image)?;
        let slot = self.atlas.insert(*key, &mask)?;
        self.placements.insert(*key, (left, top));
        return Some(GlyphQuad { slot, left, top });
    }

    /// The glyph atlas (page polling for GPU upload).
    pub fn atlas(&mut self) -> &mut GlyphAtlas {
        return &mut self.atlas;
    }
}

/// Extracts an 8-bit alpha mask from a swash image; `None` for blank or
/// non-mask (color/emoji) glyphs, which the v1 pipeline skips.
fn alpha_mask(image: &cosmic_text::SwashImage) -> Option<GlyphMask> {
    if image.content != SwashContent::Mask {
        return None;
    }
    let width = image.placement.width;
    let height = image.placement.height;
    if width == 0 || height == 0 {
        return None;
    }
    return Some(GlyphMask {
        width,
        height,
        data: image.data.to_vec(),
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn measures_embedded_font_deterministically() {
        let mut system = TextSystem::with_embedded_font();
        let (width, height) = system.measure("hello", 20.0);
        assert!(width > 20.0, "measured width {width}");
        assert!(
            (height - 24.0).abs() < 0.01,
            "line height 1.2 × 20, got {height}"
        );
        // Determinism: the same call is cached and identical.
        assert_eq!(system.measure("hello", 20.0), (width, height));
        // Wider text measures wider.
        let (wide, _) = system.measure("hello world, longer", 20.0);
        assert!(wide > width);
    }

    #[test]
    fn shapes_glyphs_with_positions() {
        let mut system = TextSystem::with_embedded_font();
        let shaped = system.shape("hi", 20.0);
        assert!(shaped.glyphs.len() >= 2, "one glyph per character minimum");
        // Glyphs advance left to right.
        assert!(shaped.glyphs[1].x > shaped.glyphs[0].x);
    }

    #[test]
    fn empty_text_shapes_to_nothing() {
        let mut system = TextSystem::with_embedded_font();
        let (width, height) = system.measure("", 20.0);
        assert_eq!(width, 0.0);
        assert!(height > 0.0, "an empty line still occupies line height");
    }
}
