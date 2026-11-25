//! Glyphon renders 2D text with [cosmic-text] and [etagere].
//!
//! Termurion patch 9: everything but the backend module is a GPU-free core. The atlas packs and
//! rasterizes glyphs and records each texture change as an [`AtlasOp`]; the text and row
//! renderers turn text into [`GlyphToRender`] instances ([`GlyphDraws`], [`RowDraws`]). A backend
//! applies those and draws; the default feature brings the one this crate has always had.
//!
//! [cosmic-text]: https://github.com/pop-os/cosmic-text
//! [etagere]: https://github.com/nical/etagere

mod custom_glyph;
mod error;
mod row_render;
mod text_atlas;
mod text_render;

// Termurion patch 9: checks the core files name no GPU API.
#[cfg(test)]
mod core_tests;
#[cfg(feature = "wgpu")]
mod cache;
#[cfg(all(test, feature = "wgpu"))]
mod test_gpu;
#[cfg(feature = "wgpu")]
mod upload;
#[cfg(feature = "wgpu")]
mod viewport;
#[cfg(feature = "wgpu")]
pub mod wgpu_backend;

pub use custom_glyph::{
    ContentType, CustomGlyph, CustomGlyphId, RasterizeCustomGlyphRequest, RasterizedCustomGlyph,
};
pub use error::{PrepareError, RenderError};
// Termurion patch 4: GlyphKey names a cached glyph for hold and release.
// Termurion patch 9: AtlasOp is what a backend applies to its atlas textures.
pub use text_atlas::{AtlasOp, ColorMode, GlyphKey, TextAtlas};
// Termurion patch 6: the row renderer. Termurion patch 9: what it hands a backend.
pub use row_render::{RowArea, RowBatch, RowDraws, RowRenderer, RowWrite, ROW_TABLE_CHUNK, ROW_UNIFORM_SIZE};
pub use text_render::{GlyphDraws, TextRenderer};
#[cfg(feature = "wgpu")]
pub use cache::Cache;
// Termurion patch 7: one reused upload path for buffer writes.
#[cfg(feature = "wgpu")]
pub use upload::UploadBelt;
#[cfg(feature = "wgpu")]
pub use viewport::Viewport;

// Re-export all top-level types from `cosmic-text` for convenience.
#[doc(no_inline)]
pub use cosmic_text::{
    self, fontdb, Action, Affinity, Attrs, AttrsList, AttrsOwned, Buffer, BufferLine, CacheKey,
    Color, Command, Cursor, Edit, Editor, Family, FamilyOwned, Font, FontSystem, LayoutCursor,
    LayoutGlyph, LayoutLine, LayoutRun, LayoutRunIter, Metrics, ShapeGlyph, ShapeLine, ShapeSpan,
    ShapeWord, Shaping, Stretch, Style, SubpixelBin, SwashCache, SwashContent, SwashImage, Weight,
    Wrap,
};

/// Termurion patch 1: the flag bits that ask for a filtered text mask (bit 8 aliased, bit 9 thin
/// strokes). cosmic-text keeps bits it does not know (`CacheKeyFlags::from_bits_retain`).
const MASK_FLAG_BITS: u32 = (1 << 8) | (1 << 9);

/// A mask filter: the glyph's `CacheKey` flag bits and its alpha mask, rewritten in place.
pub type MaskFilter = fn(u32, &mut [u8]);

static MASK_FILTER: std::sync::OnceLock<MaskFilter> = std::sync::OnceLock::new();

/// Termurion patch 1: sets the process-wide mask filter. A second call is ignored (every window's
/// renderer sets the same function).
pub fn set_mask_filter(filter: MaskFilter) {
    let _ = MASK_FILTER.set(filter);
}

/// Termurion patch 1: runs the filter over a text glyph's alpha mask when its flags ask for one.
/// Every place that rasterizes a text glyph calls this (the first upload and the re-rasterization
/// when an atlas grows); color glyphs never come here.
pub(crate) fn filter_text_mask(flags: u32, alpha: &mut [u8]) {
    if flags & MASK_FLAG_BITS != 0 {
        if let Some(filter) = MASK_FILTER.get() {
            filter(flags, alpha);
        }
    }
}

use etagere::AllocId;

pub(crate) enum GpuCacheStatus {
    InAtlas {
        x: u16,
        y: u16,
        content_type: ContentType,
    },
    SkipRasterization,
}

pub(crate) struct GlyphDetails {
    width: u16,
    height: u16,
    gpu_cache: GpuCacheStatus,
    atlas_id: Option<AllocId>,
    top: i16,
    left: i16,
}

/// One glyph quad, as both shaders read it (`shader.wgsl`, `row_shader.wgsl`). Termurion patch 9:
/// public, so every backend uploads the same bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlyphToRender {
    /// The top left corner in target pixels.
    pub pos: [i32; 2],
    /// Width and height in pixels.
    pub dim: [u16; 2],
    /// The top left corner in the atlas.
    pub uv: [u16; 2],
    /// RGBA, 8 bits each (`cosmic_text::Color`).
    pub color: u32,
    /// The `ContentType` (0 color, 1 mask), then 1 when the color converts to linear.
    pub content_type_with_srgb: [u16; 2],
    /// The quad's depth for a depth test; always 0 here (no caller draws with depth).
    pub depth: f32,
}

/// Bytes per `GlyphToRender` (no padding: every field is 4-byte sized and aligned).
pub const GLYPH_SIZE: usize = std::mem::size_of::<GlyphToRender>();
const _: () = assert!(GLYPH_SIZE == 28);

/// The instances as the bytes a GPU buffer holds.
pub fn glyph_bytes(instances: &[GlyphToRender]) -> &[u8] {
    // Sound: `repr(C)`, no padding, and u8 has no alignment.
    unsafe { std::slice::from_raw_parts(instances.as_ptr() as *const u8, std::mem::size_of_val(instances)) }
}

/// The screen resolution to use when rendering text.
#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resolution {
    /// The width of the screen in pixels.
    pub width: u32,
    /// The height of the screen in pixels.
    pub height: u32,
}

/// Controls the visible area of the text. Any text outside of the visible area will be clipped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TextBounds {
    /// The position of the left edge of the visible area.
    pub left: i32,
    /// The position of the top edge of the visible area.
    pub top: i32,
    /// The position of the right edge of the visible area.
    pub right: i32,
    /// The position of the bottom edge of the visible area.
    pub bottom: i32,
}

/// The default visible area doesn't clip any text.
impl Default for TextBounds {
    fn default() -> Self {
        Self {
            left: i32::MIN,
            top: i32::MIN,
            right: i32::MAX,
            bottom: i32::MAX,
        }
    }
}

/// A text area containing text to be rendered along with its overflow behavior.
#[derive(Clone)]
pub struct TextArea<'a> {
    /// The buffer containing the text to be rendered.
    pub buffer: &'a Buffer,
    /// The left edge of the buffer.
    pub left: f32,
    /// The top edge of the buffer.
    pub top: f32,
    /// The scaling to apply to the buffer.
    pub scale: f32,
    /// The visible bounds of the text area. This is used to clip the text and doesn't have to
    /// match the `left` and `top` values.
    pub bounds: TextBounds,
    /// The default color of the text area.
    pub default_color: Color,
    /// Additional custom glyphs to render.
    pub custom_glyphs: &'a [CustomGlyph],
}

