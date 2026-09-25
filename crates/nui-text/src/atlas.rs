//! Glyph atlas: CPU-side shelf packing (etagere) of rasterized alpha masks.
//!
//! Pages are 1024×1024 u8 alpha buffers. Glyphs rasterize once per cache
//! key (`SwashCache` dedupes rasterization; the atlas dedupes placement);
//! the renderer polls [`GlyphAtlas::take_dirty_pages`] and uploads each
//! dirty page wholesale once per change (warmup cost only).

use std::collections::HashMap;

use cosmic_text::CacheKey;
use etagere::{AtlasAllocator, size2};

/// Atlas page edge length in texels.
const PAGE_SIZE: u32 = 1024;

/// The atlas page edge length (the renderer sizes its textures to match).
pub const fn atlas_page_size() -> u32 {
    return PAGE_SIZE;
}

/// One rasterized glyph alpha mask (8 bits per pixel, row-major).
#[derive(Debug, Clone)]
pub struct GlyphMask {
    /// Mask width in pixels.
    pub width: u32,
    /// Mask height in pixels.
    pub height: u32,
    /// Alpha bytes, `width * height` entries.
    pub data: Vec<u8>,
}

/// Where one glyph mask lives inside the atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphSlot {
    /// Page index.
    pub page: u32,
    /// Texel rect of the mask inside the page.
    pub x: u32,
    /// Texel rect of the mask inside the page.
    pub y: u32,
    /// Mask width in texels.
    pub width: u32,
    /// Mask height in texels.
    pub height: u32,
}

/// A quad ready for the text pipeline: atlas placement plus the offset of
/// the mask's top-left relative to the glyph's baseline origin.
#[derive(Debug, Clone, Copy)]
pub struct GlyphQuad {
    /// Atlas placement of the mask.
    pub slot: GlyphSlot,
    /// Mask top-left offset from the glyph baseline origin (typically
    /// `left <= 0`, `top < 0` — the mask hangs below the baseline).
    pub left: i32,
    /// Mask top-left offset from the glyph baseline origin.
    pub top: i32,
}

/// One atlas page: packed alpha pixels plus its allocator.
struct AtlasPage {
    allocator: AtlasAllocator,
    pixels: Vec<u8>,
    dirty: bool,
}

impl AtlasPage {
    fn new() -> AtlasPage {
        return AtlasPage {
            allocator: AtlasAllocator::new(size2(PAGE_SIZE as i32, PAGE_SIZE as i32)),
            pixels: vec![0; (PAGE_SIZE * PAGE_SIZE) as usize],
            dirty: false,
        };
    }

    fn insert(&mut self, mask: &GlyphMask) -> Option<(u32, u32)> {
        let allocation = self
            .allocator
            .allocate(size2(mask.width as i32, mask.height as i32))?;
        let rect = allocation.rectangle;
        let (x, y) = (rect.min.x as u32, rect.min.y as u32);
        for row in 0..mask.height as usize {
            let source = row * mask.width as usize;
            let target = ((y as usize + row) * PAGE_SIZE as usize) + x as usize;
            self.pixels[target..target + mask.width as usize]
                .copy_from_slice(&mask.data[source..source + mask.width as usize]);
        }
        self.dirty = true;
        return Some((x, y));
    }
}

/// A dirtied page: index plus a snapshot of its pixels (for upload).
#[derive(Debug)]
pub struct DirtyPage {
    /// Page index.
    pub index: u32,
    /// Page edge length in texels.
    pub size: u32,
    /// Page pixels (alpha), `size * size` entries.
    pub data: Vec<u8>,
}

/// The glyph atlas: placement bookkeeping and pixel pages.
#[derive(Default)]
pub struct GlyphAtlas {
    /// Cache-key → placement (rasterize-once semantics).
    slots: HashMap<CacheKey, GlyphSlot>,
    /// Alpha pages, allocated on demand.
    pages: Vec<AtlasPage>,
}

impl std::fmt::Debug for GlyphAtlas {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return formatter
            .debug_struct("GlyphAtlas")
            .field("slots", &self.slots.len())
            .field("pages", &self.pages.len())
            .finish();
    }
}

impl GlyphAtlas {
    /// Creates an empty atlas.
    pub fn new() -> GlyphAtlas {
        return GlyphAtlas::default();
    }

    /// The recorded placement for `key`, if already rasterized.
    pub fn lookup(&self, key: &CacheKey) -> Option<GlyphSlot> {
        return self.slots.get(key).copied();
    }

    /// Rasterizes `mask` into the atlas (or returns the existing slot when
    /// `lookup` already hit). Oversized masks are rejected: a glyph larger
    /// than a page cannot pack.
    pub fn insert(&mut self, key: CacheKey, mask: &GlyphMask) -> Option<GlyphSlot> {
        if mask.width == 0 || mask.height == 0 || mask.width > PAGE_SIZE || mask.height > PAGE_SIZE
        {
            return None;
        }
        if let Some(existing) = self.slots.get(&key) {
            return Some(*existing);
        }
        let page_index = self.allocate_page_for(mask)?;
        let (x, y) = self.pages[page_index].insert(mask)?;
        let slot = GlyphSlot {
            page: page_index as u32,
            x,
            y,
            width: mask.width,
            height: mask.height,
        };
        self.slots.insert(key, slot);
        return Some(slot);
    }

    /// Finds the first page with room for `mask`, growing the page list as
    /// needed (the probe allocation is undone; the real insert re-allocates).
    fn allocate_page_for(&mut self, mask: &GlyphMask) -> Option<usize> {
        let size = size2(mask.width as i32, mask.height as i32);
        for (index, page) in self.pages.iter_mut().enumerate() {
            if let Some(probe) = page.allocator.allocate(size) {
                page.allocator.deallocate(probe.id);
                return Some(index);
            }
        }
        self.pages.push(AtlasPage::new());
        return Some(self.pages.len() - 1);
    }

    /// Number of allocated pages (renderer texture count).
    pub fn page_count(&self) -> usize {
        return self.pages.len();
    }

    /// Pixel data of one page.
    pub fn page_pixels(&self, index: usize) -> Option<&[u8]> {
        return self
            .pages
            .get(index)
            .map(|page| return page.pixels.as_slice());
    }

    /// Takes dirtied pages since the last call (snapshot + clear flags).
    pub fn take_dirty_pages(&mut self) -> Vec<DirtyPage> {
        let mut dirty = Vec::new();
        for (index, page) in self.pages.iter_mut().enumerate() {
            if !page.dirty {
                continue;
            }
            page.dirty = false;
            dirty.push(DirtyPage {
                index: index as u32,
                size: PAGE_SIZE,
                data: page.pixels.clone(),
            });
        }
        return dirty;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn key(id: u16) -> CacheKey {
        return CacheKey {
            font_id: fontdb::ID::dummy(),
            glyph_id: id,
            font_size_bits: 0,
            x_bin: cosmic_text::SubpixelBin::Zero,
            y_bin: cosmic_text::SubpixelBin::Zero,
            font_weight: fontdb::Weight::NORMAL,
            flags: cosmic_text::CacheKeyFlags::empty(),
        };
    }

    fn mask(id: u16, width: u32, height: u32) -> (CacheKey, GlyphMask) {
        return (
            key(id),
            GlyphMask {
                width,
                height,
                data: vec![255; (width * height) as usize],
            },
        );
    }

    #[test]
    fn inserts_looks_up_and_marks_dirty() {
        let mut atlas = GlyphAtlas::new();
        let (key_a, mask_a) = mask(1, 8, 8);
        let slot = atlas.insert(key_a, &mask_a).unwrap();
        assert_eq!(slot.width, 8);
        assert_eq!(atlas.lookup(&key_a), Some(slot), "idempotent lookup");
        // Re-insert returns the same slot.
        assert_eq!(atlas.insert(key_a, &mask_a).unwrap(), slot);

        let dirty = atlas.take_dirty_pages();
        assert_eq!(dirty.len(), 1, "one page dirtied");
        assert!(dirty[0].data.iter().any(|alpha| return *alpha == 255));
        assert!(atlas.take_dirty_pages().is_empty(), "flags cleared");
        assert_eq!(atlas.page_pixels(0).unwrap().len(), 1024 * 1024);
    }

    #[test]
    fn oversized_masks_are_rejected() {
        let mut atlas = GlyphAtlas::new();
        let (big_key, big_mask) = mask(2, 2000, 10);
        assert!(atlas.insert(big_key, &big_mask).is_none());
        let (blank_key, blank_mask) = mask(3, 0, 5);
        assert!(atlas.insert(blank_key, &blank_mask).is_none());
    }

    #[test]
    fn fills_pages_before_allocating_new_ones() {
        let mut atlas = GlyphAtlas::new();
        let mut inserted = 0;
        for id in 10..10_000u16 {
            let (k, m) = mask(id, 32, 32);
            if atlas.insert(k, &m).is_some() {
                inserted += 1;
            }
        }
        assert!(atlas.page_count() > 1, "32×32 × 10k overflows one page");
        assert_eq!(inserted, 10_000 - 10);
    }
}
