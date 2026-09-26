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
    /// Char index (into the whole text) of this glyph's first character.
    ///
    /// Caret placement reads this: the caret for char index `i` sits at the
    /// x of the first glyph whose `index >= i`. Carrying it on the glyph
    /// means caret maths never needs a second shaping pass.
    pub index: usize,
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

/// One visual line of a wrapped (multi-line) layout.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WrappedLine {
    /// First char index of the line in the whole text.
    pub start: usize,
    /// One past the last char index of the line.
    pub end: usize,
    /// Top of the line box (pixels from the text origin).
    pub top: f32,
    /// Height of the line box (pixels).
    pub height: f32,
    /// Width of the line's content (pixels).
    pub width: f32,
}

/// A wrapped layout: the visual lines plus every placed glyph.
///
/// This is what a multi-line text field needs and a single-paragraph
/// [`ShapedText`] cannot answer: which visual line the caret is on, and
/// where the caret sits inside it.
#[derive(Debug, Clone)]
pub struct WrappedLayout {
    /// Width of the widest line (pixels).
    pub width: f32,
    /// Total height, `lines * line_height` (pixels).
    pub height: f32,
    /// The visual lines, top to bottom; the ranges are contiguous and
    /// cover the whole text, so `lines[i].end == lines[i + 1].start`.
    pub lines: Vec<WrappedLine>,
    /// Placed glyphs in reading order; `x` restarts at each line, `y` is
    /// measured from the text origin.
    pub glyphs: Vec<ShapedGlyph>,
}

impl WrappedLayout {
    /// The index of the visual line containing `char_index`.
    pub fn line_of(&self, char_index: usize) -> usize {
        for (index, line) in self.lines.iter().enumerate() {
            if char_index < line.end {
                return index;
            }
        }
        return self.lines.len().saturating_sub(1);
    }

    /// The caret's x offset inside its line for `char_index`: the x of the
    /// first glyph at or after it, or the line's end.
    pub fn caret_x(&self, char_index: usize) -> f32 {
        let line_index = self.line_of(char_index);
        let Some(line) = self.lines.get(line_index) else {
            return 0.0;
        };
        for glyph in &self.glyphs {
            if glyph.index >= line.end {
                break; // past this line: no glyph of it is at or after the caret
            }
            if glyph.index >= char_index {
                return glyph.x;
            }
        }
        return line.width;
    }

    /// The caret box `(x, top, height)` for `char_index`.
    pub fn caret_box(&self, char_index: usize) -> (f32, f32, f32) {
        let line_index = self.line_of(char_index);
        let Some(line) = self.lines.get(line_index) else {
            return (0.0, 0.0, 0.0);
        };
        return (self.caret_x(char_index), line.top, line.height);
    }
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
    /// Measure cache for *wrapped* text, keyed by
    /// `(text, font size bits, width bits)`: a multi-line field asks for
    /// the same measurement every frame, and the wrap width changes only
    /// on resize.
    wrapped_measure_cache: HashMap<(String, u32, u32), (f32, f32)>,
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
            wrapped_measure_cache: HashMap::new(),
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

    /// Measures `text` wrapped at `max_width` (px); returns
    /// `(widest line, height)`. Cached per `(text, size, width)`.
    ///
    /// Separate from [`TextSystem::measure`] rather than a mode switch on
    /// it: the single-line path is on the hot path of every `Text` element
    /// in the document and must not grow a wrap parameter.
    pub fn measure_wrapped(&mut self, text: &str, font_size: f32, max_width: f32) -> (f32, f32) {
        if max_width <= 0.0 {
            return self.measure(text, font_size);
        }
        let key = (text.to_string(), font_size.to_bits(), max_width.to_bits());
        if let Some(cached) = self.wrapped_measure_cache.get(&key) {
            return *cached;
        }
        let layout = self.layout_wrapped(text, font_size, max_width);
        let measured = (layout.width, layout.height);
        self.wrapped_measure_cache.insert(key, measured);
        return measured;
    }

    /// Shapes `text` (single paragraph, no wrap) and returns placed glyphs.
    pub fn shape(&mut self, text: &str, font_size: f32) -> ShapedText {
        let (width, height, _lines, glyphs) = self.place(text, font_size, None);
        return ShapedText {
            width,
            height,
            glyphs,
        };
    }

    /// Shapes `text` wrapped at `max_width`, returning the visual lines
    /// (with their char ranges) plus every placed glyph.
    pub fn layout_wrapped(&mut self, text: &str, font_size: f32, max_width: f32) -> WrappedLayout {
        let (width, height, lines, glyphs) = self.place(text, font_size, Some(max_width));
        return WrappedLayout {
            width,
            height,
            lines,
            glyphs,
        };
    }

    /// Shapes `text` — wrapped at `max_width` when given — and returns
    /// `(widest line, total height, visual lines, glyphs)`.
    ///
    /// One implementation for both entry points: the only differences are
    /// the buffer's wrap width and whether the line ranges are reported.
    fn place(
        &mut self,
        text: &str,
        font_size: f32,
        max_width: Option<f32>,
    ) -> (f32, f32, Vec<WrappedLine>, Vec<ShapedGlyph>) {
        let metrics = Metrics::new(font_size, font_size * LINE_HEIGHT_FACTOR);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        buffer.set_size(max_width, None);
        buffer.set_text(text, &Attrs::new(), Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        // Paragraph starts, so a run's paragraph-relative byte offsets can
        // be turned into char indices into the whole text.
        let mut paragraph_starts = vec![0_usize];
        for (index, character) in text.chars().enumerate() {
            if character == '\n' {
                paragraph_starts.push(index + 1);
            }
        }
        let char_count = text.chars().count();

        let mut lines: Vec<WrappedLine> = Vec::new();
        let mut glyphs = Vec::new();
        let (mut width, mut height) = (0.0_f32, 0.0_f32);
        for run in buffer.layout_runs() {
            let line_y = run.line_y;
            width = width.max(run.line_w);
            height += run.line_height;
            let paragraph = paragraph_starts
                .get(run.line_i)
                .copied()
                .unwrap_or(char_count);
            let (start_char, end_char) = cluster_char_range(run.text, run.glyphs, paragraph);
            for glyph in run.glyphs {
                let physical = glyph.physical((0.0, 0.0), 1.0);
                glyphs.push(ShapedGlyph {
                    x: physical.x as f32,
                    y: line_y + physical.y as f32,
                    key: physical.cache_key,
                    index: paragraph + byte_to_char(run.text, glyph.start),
                });
            }
            lines.push(WrappedLine {
                start: start_char,
                end: end_char,
                top: run.line_top,
                height: run.line_height,
                width: run.line_w,
            });
        }
        close_line_ranges(&mut lines, char_count);
        return (width, height, lines, glyphs);
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

/// Char range `(start, end)` of one layout run, in whole-text char indices.
///
/// Reads the extent of the run's glyphs rather than the buffer's own line
/// bookkeeping: cluster indices are what the caret maths needs, and taking
/// `min`/`max` instead of first/last keeps it correct for RTL runs, where
/// glyphs are laid out in reverse.
fn cluster_char_range(
    paragraph: &str,
    glyphs: &[cosmic_text::LayoutGlyph],
    paragraph_start: usize,
) -> (usize, usize) {
    let mut start = None;
    let mut end = 0;
    for glyph in glyphs {
        start = Some(start.map_or(glyph.start, |current: usize| {
            return current.min(glyph.start);
        }));
        end = end.max(glyph.end);
    }
    let Some(start_byte) = start else {
        // A run with no glyphs is an empty line: a zero-width range at the
        // paragraph start (the closing pass makes it contiguous).
        return (paragraph_start, paragraph_start);
    };
    return (
        paragraph_start + byte_to_char(paragraph, start_byte),
        paragraph_start + byte_to_char(paragraph, end),
    );
}

/// Char index of a byte offset that cosmic-text reported (always on a char
/// boundary, but clamped so a malformed offset cannot panic).
fn byte_to_char(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    return text[..byte].chars().count();
}

/// Makes the visual line ranges a contiguous partition of the text.
///
/// A line runs up to *where the next one begins*: the chars a wrap drops
/// (breaking whitespace on a soft wrap, the newline itself on a hard
/// break) stay with the line they terminate. Putting them on the next
/// line instead would send the caret at the end of a line to the start of
/// the following one — the end-of-line caret has to keep sitting on its
/// own line.
fn close_line_ranges(lines: &mut [WrappedLine], char_count: usize) {
    let starts: Vec<usize> = lines.iter().map(|line| return line.start).collect();
    for (index, line) in lines.iter_mut().enumerate() {
        let next = starts.get(index + 1).copied().unwrap_or(char_count);
        line.end = next.max(line.start);
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

    #[test]
    fn wrapping_breaks_at_the_given_width() {
        let mut system = TextSystem::with_embedded_font();
        let text = "the quick brown fox jumps over the lazy dog";
        let (one_line, _) = system.measure(text, 16.0);
        // A width well past the first word but far short of the sentence
        // must wrap into several lines.
        let layout = system.layout_wrapped(text, 16.0, one_line / 4.0);
        assert!(
            layout.lines.len() >= 3,
            "expected several lines, got {}",
            layout.lines.len()
        );
        // Every line stays inside the wrap width (a single unbreakable word
        // may overflow, so allow the widest *word*, not the whole line).
        let widest = layout
            .lines
            .iter()
            .map(|line| return line.width)
            .fold(0.0_f32, f32::max);
        assert!(widest <= one_line / 4.0 + 15.0, "widest line {widest}");
        // Wrapping does not change the text's total extent when there is
        // room: the width is capped by the wrap width, the height grows.
        let (_, tall) = system.measure_wrapped(text, 16.0, one_line / 4.0);
        assert!(tall > 3.0 * 16.0);
        assert!(width_of(&layout) <= one_line / 4.0 + 15.0);
    }

    #[test]
    fn wrapped_lines_are_a_contiguous_partition_of_the_text() {
        let mut system = TextSystem::with_embedded_font();
        let text = "alpha beta gamma delta epsilon";
        let layout = system.layout_wrapped(text, 16.0, 60.0);
        assert!(layout.lines.len() >= 2);
        assert_eq!(layout.lines[0].start, 0, "the first line starts the text");
        assert_eq!(
            layout.lines.last().map(|line| return line.end),
            Some(text.chars().count()),
            "the last line ends the text"
        );
        for pair in layout.lines.windows(2) {
            assert_eq!(
                pair[0].end, pair[1].start,
                "ranges are contiguous with no gaps"
            );
        }
    }

    #[test]
    fn a_hard_newline_starts_a_line() {
        let mut system = TextSystem::with_embedded_font();
        let layout = system.layout_wrapped("first\nsecond\nthird", 16.0, 500.0);
        // Three paragraphs, each short enough not to wrap: three lines.
        assert_eq!(layout.lines.len(), 3);
        assert_eq!(layout.lines[0].start, 0);
        assert_eq!(layout.lines[1].start, "first\n".chars().count());
        assert_eq!(
            layout.lines[2].start,
            "first\nsecond\n".chars().count(),
            "a paragraph starts just past its preceding newline"
        );
        // The newline terminates the line it ends, so the line above owns
        // it: the caret after "first" still reads as end-of-line 0.
        assert_eq!(layout.lines[0].end, layout.lines[1].start);
        assert_eq!(layout.line_of(5), 0, "caret after 'first' is on line 0");
        assert!(layout.caret_x(5) > layout.caret_x(4), "end-of-line caret");
        assert_eq!(layout.line_of(6), 1, "caret after the newline is on line 1");
        assert!(layout.caret_x(6) <= 2.0, "caret at the second line's start");
        // A trailing newline still opens an empty line (like editors).
        let trailing = system.layout_wrapped("one\n", 16.0, 500.0);
        assert_eq!(trailing.lines.len(), 2);
        assert_eq!(trailing.lines[1].start, trailing.lines[1].end);
    }

    #[test]
    fn the_caret_walks_across_wrapped_lines() {
        let mut system = TextSystem::with_embedded_font();
        let text = "alpha beta gamma delta";
        let layout = system.layout_wrapped(text, 16.0, 60.0);
        assert!(layout.lines.len() >= 2);

        // The caret x is measured inside its own line: the first char of
        // the second line sits at (or very near) zero, never at the end of
        // the first line's width.
        let second_start = layout.lines[1].start;
        assert_eq!(layout.line_of(second_start), 1);
        assert!(
            layout.caret_x(second_start) <= 2.0,
            "caret at the second line's start, got {}",
            layout.caret_x(second_start)
        );
        let (_, top, height) = layout.caret_box(second_start);
        assert!(top > 0.0, "the second line's box sits below the first");
        assert!(height > 0.0);

        // Advancing within a line moves the caret right, and the caret for
        // a char past the end lands on the last line.
        let first_line_mid = layout.caret_x(1);
        assert!(first_line_mid > layout.caret_x(0));
        let end = text.chars().count();
        assert_eq!(layout.line_of(end), layout.lines.len() - 1);
        assert!(
            layout.caret_x(end) > 0.0,
            "end caret sits at the line's end"
        );
    }

    #[test]
    fn wrap_measure_is_cached_and_agrees_with_the_layout() {
        let mut system = TextSystem::with_embedded_font();
        let (width, height) = system.measure_wrapped("hello world", 20.0, 50.0);
        let layout = system.layout_wrapped("hello world", 20.0, 50.0);
        assert!((width - layout.width).abs() < 0.01);
        assert!((height - layout.height).abs() < 0.01);
        assert_eq!(
            system.measure_wrapped("hello world", 20.0, 50.0),
            (width, height)
        );
    }

    /// The widest line of a layout, for assertions on wrapped text.
    fn width_of(layout: &WrappedLayout) -> f32 {
        return layout
            .lines
            .iter()
            .map(|line| return line.width)
            .fold(0.0_f32, f32::max);
    }
}
