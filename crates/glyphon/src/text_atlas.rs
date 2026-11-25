use crate::{
    text_render::GlyphonCacheKey, ContentType, FontSystem, GlyphDetails, GpuCacheStatus,
    RasterizeCustomGlyphRequest, RasterizedCustomGlyph, SwashCache,
};
use etagere::{size2, Allocation, BucketedAtlasAllocator};
use lru::LruCache;
use rustc_hash::FxHasher;
use std::{
    collections::{HashMap, HashSet},
    hash::BuildHasherDefault,
};

type Hasher = BuildHasherDefault<FxHasher>;

/// Termurion patch 9: one change to an atlas texture. A backend applies them in order, so its
/// textures hold exactly the texels the atlas placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AtlasOp {
    /// The `kind` texture is made again, `size` pixels per side and empty. An `Upload` for every
    /// glyph it keeps follows.
    Grow { kind: ContentType, size: u32 },
    /// `bytes` go to the `w` by `h` rectangle at `x, y`: rows of `w` pixels, packed, of
    /// `kind.bytes_per_pixel()` bytes each.
    Upload { kind: ContentType, x: u32, y: u32, w: u32, h: u32, bytes: Vec<u8> },
}

#[allow(dead_code)]
pub(crate) struct InnerAtlas {
    pub kind: Kind,
    pub packer: BucketedAtlasAllocator,
    pub size: u32,
    pub glyph_cache: LruCache<GlyphonCacheKey, GlyphDetails, Hasher>,
    pub glyphs_in_use: HashSet<GlyphonCacheKey, Hasher>,
    /// Termurion patch 4: use counts of the held keys (every count is above 0).
    pub held: HashMap<GlyphonCacheKey, u32, Hasher>,
    pub max_texture_dimension_2d: u32,
    /// Termurion patch 5: entries evicted so far (tests check a flood really evicted).
    pub evicted: u64,
}

impl InnerAtlas {
    const INITIAL_SIZE: u32 = 256;

    // Termurion patch 9: the texture is the backend's; the core keeps only its size.
    fn new(max_texture_dimension_2d: u32, kind: Kind) -> Self {
        let size = Self::INITIAL_SIZE.min(max_texture_dimension_2d);

        let packer = BucketedAtlasAllocator::new(size2(size as i32, size as i32));

        let glyph_cache = LruCache::unbounded_with_hasher(Hasher::default());
        let glyphs_in_use = HashSet::with_hasher(Hasher::default());

        Self {
            kind,
            packer,
            size,
            glyph_cache,
            glyphs_in_use,
            held: HashMap::with_hasher(Hasher::default()),
            max_texture_dimension_2d,
            evicted: 0,
        }
    }

    pub(crate) fn try_allocate(&mut self, width: usize, height: usize) -> Option<Allocation> {
        let size = size2(width as i32, height as i32);

        loop {
            let allocation = self.packer.allocate(size);

            if allocation.is_some() {
                return allocation;
            }

            // Termurion patch 4: evict the oldest entry that is neither in use nor held. A busy
            // entry at the oldest end no longer stops the search; when none qualifies the caller
            // grows the atlas. A zero-size entry holds no space, so popping it is free.
            let victim = self
                .glyph_cache
                .iter()
                .rev()
                .find(|(key, _)| !self.glyphs_in_use.contains(*key) && !self.held.contains_key(*key))
                .map(|(key, _)| *key)?;

            let value = self.glyph_cache.pop(&victim).unwrap();
            self.evicted += 1;
            if let Some(id) = value.atlas_id {
                self.packer.deallocate(id);
            }
        }
    }

    // Termurion patch 9: the new texture and its re-uploads go out as `AtlasOp`s.
    pub(crate) fn grow(
        &mut self,
        font_system: &mut FontSystem,
        cache: &mut SwashCache,
        scale_factor: f32,
        mut rasterize_custom_glyph: impl FnMut(
            RasterizeCustomGlyphRequest,
        ) -> Option<RasterizedCustomGlyph>,
        ops: &mut Vec<AtlasOp>,
    ) -> bool {
        if self.size >= self.max_texture_dimension_2d {
            return false;
        }

        // Grow each dimension by a factor of 2. The growth factor was chosen to match the growth
        // factor of `Vec`.`
        const GROWTH_FACTOR: u32 = 2;
        let new_size = (self.size * GROWTH_FACTOR).min(self.max_texture_dimension_2d);

        self.packer.grow(size2(new_size as i32, new_size as i32));

        let kind = self.kind.as_content_type();
        ops.push(AtlasOp::Grow { kind, size: new_size });

        // Re-upload glyphs
        for (&cache_key, glyph) in &self.glyph_cache {
            let (x, y) = match glyph.gpu_cache {
                GpuCacheStatus::InAtlas { x, y, .. } => (x, y),
                GpuCacheStatus::SkipRasterization => continue,
            };

            let (image_data, width, height) = match cache_key {
                GlyphonCacheKey::Text(cache_key) => {
                    // Termurion patch 2: a glyph that no longer rasterizes is skipped (the
                    // original unwraps and panics); the others are filtered like the first upload.
                    let Some(mut image) = cache.get_image_uncached(font_system, cache_key) else {
                        continue;
                    };
                    // Termurion patch 1: filter the mask of an aliased or thin-stroke glyph.
                    if matches!(image.content, cosmic_text::SwashContent::Mask) {
                        crate::filter_text_mask(cache_key.flags.bits(), &mut image.data);
                    }
                    let width = image.placement.width as usize;
                    let height = image.placement.height as usize;

                    (image.data, width, height)
                }
                GlyphonCacheKey::Custom(cache_key) => {
                    let input = RasterizeCustomGlyphRequest {
                        id: cache_key.glyph_id,
                        width: cache_key.width,
                        height: cache_key.height,
                        x_bin: cache_key.x_bin,
                        y_bin: cache_key.y_bin,
                        scale: scale_factor,
                    };

                    let Some(rasterized_glyph) = (rasterize_custom_glyph)(input) else {
                        panic!("Custom glyph rasterizer returned `None` when it previously returned `Some` for the same input {:?}", &input);
                    };

                    // Sanity checks on the rasterizer output
                    rasterized_glyph.validate(&input, Some(self.kind.as_content_type()));

                    (
                        rasterized_glyph.data,
                        cache_key.width as usize,
                        cache_key.height as usize,
                    )
                }
            };

            ops.push(AtlasOp::Upload {
                kind,
                x: x as u32,
                y: y as u32,
                w: width as u32,
                h: height as u32,
                bytes: image_data,
            });
        }

        self.size = new_size;

        true
    }

    fn trim(&mut self) {
        self.glyphs_in_use.clear();
    }
}

/// Termurion patch 4: names a cached glyph for [`TextAtlas::hold`] and [`TextAtlas::release`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey(pub(crate) GlyphonCacheKey);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Mask,
    Color { srgb: bool },
}

impl Kind {
    fn as_content_type(&self) -> ContentType {
        match self {
            Self::Mask => ContentType::Mask,
            Self::Color { .. } => ContentType::Color,
        }
    }
}

/// The color mode of a [`TextAtlas`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorMode {
    /// Accurate color management.
    ///
    /// This mode will use a proper sRGB texture for colored glyphs. This will
    /// produce physically accurate color blending when rendering.
    Accurate,

    /// Web color management.
    ///
    /// This mode reproduces the color management strategy used in the Web and
    /// implemented by browsers.
    ///
    /// This entails storing glyphs colored using the sRGB color space in a
    /// linear RGB texture. Blending will not be physically accurate, but will
    /// produce the same results as most UI toolkits.
    ///
    /// This mode should be used to render to a linear RGB texture containing
    /// sRGB colors.
    Web,
}

/// An atlas containing a cache of rasterized glyphs that can be rendered.
///
/// Termurion patch 9: the CPU half only. It packs and rasterizes; what the textures must hold
/// goes out through [`TextAtlas::take_ops`].
pub struct TextAtlas {
    pub(crate) color_atlas: InnerAtlas,
    pub(crate) mask_atlas: InnerAtlas,
    pub(crate) color_mode: ColorMode,
    /// Termurion patch 4: raised by every successful grow.
    generation: u64,
    /// Termurion patch 9: texture changes no backend has taken yet.
    pub(crate) ops: Vec<AtlasOp>,
}

impl TextAtlas {
    /// Creates a new [`TextAtlas`] whose textures may grow to `max_side` pixels per side (the
    /// device's largest 2D texture).
    pub fn new(max_side: u32) -> Self {
        Self::with_color_mode(max_side, ColorMode::Accurate)
    }

    /// Creates a new [`TextAtlas`] with the given [`ColorMode`].
    pub fn with_color_mode(max_side: u32, color_mode: ColorMode) -> Self {
        let color_atlas = InnerAtlas::new(
            max_side,
            Kind::Color {
                srgb: match color_mode {
                    ColorMode::Accurate => true,
                    ColorMode::Web => false,
                },
            },
        );
        let mask_atlas = InnerAtlas::new(max_side, Kind::Mask);

        Self {
            color_atlas,
            mask_atlas,
            color_mode,
            generation: 0,
            ops: Vec::new(),
        }
    }

    /// Termurion patch 9: moves the texture changes made since the last call onto the end of
    /// `into`, oldest first. Each must reach the textures, in order, before they draw.
    pub fn take_ops(&mut self, into: &mut Vec<AtlasOp>) {
        into.append(&mut self.ops);
    }

    /// The color mode the atlas was made with.
    pub fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    /// Termurion patch 4: keeps a glyph in the atlas (a use count per key) until it is released
    /// as often as it was held. The key may sit in either inner atlas, so both count it.
    pub fn hold(&mut self, key: GlyphKey) {
        for inner in [&mut self.mask_atlas, &mut self.color_atlas] {
            *inner.held.entry(key.0).or_insert(0) += 1;
        }
    }

    /// Termurion patch 4: undoes one `hold`; a key with no count is left alone.
    pub fn release(&mut self, key: GlyphKey) {
        for inner in [&mut self.mask_atlas, &mut self.color_atlas] {
            if let Some(n) = inner.held.get_mut(&key.0) {
                *n -= 1;
                if *n == 0 {
                    inner.held.remove(&key.0);
                }
            }
        }
    }

    /// Termurion patch 4: how many keys are held (count above 0).
    pub fn held(&self) -> usize {
        self.mask_atlas.held.len()
    }

    /// Termurion patch 4: raised by every successful grow. Atlas positions change then, so a
    /// caller that kept positions must prepare again.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Termurion patch 5: caps both atlases at `px` per side (tests only, to force eviction).
    #[doc(hidden)]
    pub fn set_max_size(&mut self, px: u32) {
        for inner in [&mut self.mask_atlas, &mut self.color_atlas] {
            inner.max_texture_dimension_2d = inner.max_texture_dimension_2d.min(px);
        }
    }

    /// Termurion patch 5: how many entries both atlases have evicted (tests only).
    #[doc(hidden)]
    pub fn evictions(&self) -> u64 {
        self.mask_atlas.evicted + self.color_atlas.evicted
    }

    /// Termurion patch 3: the side of the mask atlas texture in pixels (it doubles when it grows).
    pub fn mask_size(&self) -> u32 {
        self.mask_atlas.size
    }

    /// Termurion patch 9: the side of the color atlas texture in pixels.
    pub fn color_size(&self) -> u32 {
        self.color_atlas.size
    }

    pub fn trim(&mut self) {
        self.mask_atlas.trim();
        self.color_atlas.trim();
    }

    pub(crate) fn grow(
        &mut self,
        font_system: &mut FontSystem,
        cache: &mut SwashCache,
        content_type: ContentType,
        scale_factor: f32,
        rasterize_custom_glyph: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) -> bool {
        let did_grow = match content_type {
            ContentType::Mask => self.mask_atlas.grow(
                font_system,
                cache,
                scale_factor,
                rasterize_custom_glyph,
                &mut self.ops,
            ),
            ContentType::Color => self.color_atlas.grow(
                font_system,
                cache,
                scale_factor,
                rasterize_custom_glyph,
                &mut self.ops,
            ),
        };

        if did_grow {
            self.generation += 1;
        }

        did_grow
    }

    pub(crate) fn inner_for_content_mut(&mut self, content_type: ContentType) -> &mut InnerAtlas {
        match content_type {
            ContentType::Color => &mut self.color_atlas,
            ContentType::Mask => &mut self.mask_atlas,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        custom_glyph::CustomGlyphCacheKey, Buffer, CustomGlyph, GlyphDraws, Metrics,
        PrepareError, Resolution, SubpixelBin, TextArea, TextBounds, TextRenderer,
    };

    const SIDE: u16 = 32;

    // Termurion patch 9: the atlas is CPU only, so these run with no device.
    struct Rig {
        atlas: TextAtlas,
        renderer: TextRenderer,
        font_system: FontSystem,
        swash: SwashCache,
        buffer: Buffer,
        out: GlyphDraws,
        /// Custom glyphs rasterize as color (into the color atlas) when set.
        color: bool,
    }

    impl Rig {
        fn new() -> Self {
            Self::with_cap(Some(256))
        }

        fn with_cap(cap: Option<u32>) -> Self {
            let mut atlas = TextAtlas::new(8192);
            if let Some(px) = cap {
                atlas.set_max_size(px);
            }
            let mut font_system = FontSystem::new_with_fonts([]);
            let buffer = Buffer::new(&mut font_system, Metrics::new(10.0, 12.0));
            Self {
                atlas,
                renderer: TextRenderer::new(),
                font_system,
                swash: SwashCache::new(),
                buffer,
                out: GlyphDraws::default(),
                color: false,
            }
        }
        /// Prepares one 32x32 solid mask glyph per id, after a trim (nothing is in use).
        fn prepare(&mut self, ids: &[u16]) -> Result<(), PrepareError> {
            self.atlas.trim();
            let glyphs: Vec<CustomGlyph> = ids
                .iter()
                .map(|&id| CustomGlyph {
                    id,
                    left: 0.0,
                    top: 0.0,
                    width: SIDE as f32,
                    height: SIDE as f32,
                    snap_to_physical_pixel: true,
                    ..Default::default()
                })
                .collect();
            self.prepare_keep(&glyphs)
        }

        /// Like `prepare` without the trim: the glyphs stay in use.
        fn prepare_keep(&mut self, glyphs: &[CustomGlyph]) -> Result<(), PrepareError> {
            let area = TextArea {
                buffer: &self.buffer,
                left: 0.0,
                top: 0.0,
                scale: 1.0,
                bounds: TextBounds { left: 0, top: 0, right: 1024, bottom: 1024 },
                default_color: crate::Color::rgb(255, 255, 255),
                custom_glyphs: glyphs,
            };
            let (channels, content_type) = match self.color {
                true => (4, ContentType::Color),
                false => (1, ContentType::Mask),
            };
            self.renderer.prepare_cpu(
                &mut self.font_system,
                &mut self.atlas,
                Resolution { width: 1024, height: 1024 },
                [area],
                &mut self.swash,
                |req| {
                    Some(RasterizedCustomGlyph {
                        data: vec![255; req.width as usize * req.height as usize * channels],
                        content_type,
                    })
                },
                &mut self.out,
            )
        }

        fn flood(&mut self, from: u16, to: u16) {
            for id in from..to {
                self.prepare(&[id]).unwrap();
            }
        }

        fn at(&self, id: u16) -> Option<(u16, u16)> {
            let inner = match self.color {
                true => &self.atlas.color_atlas,
                false => &self.atlas.mask_atlas,
            };
            match inner.glyph_cache.peek(&key(id).0)?.gpu_cache {
                GpuCacheStatus::InAtlas { x, y, .. } => Some((x, y)),
                GpuCacheStatus::SkipRasterization => None,
            }
        }
    }

    fn key(id: u16) -> GlyphKey {
        GlyphKey(GlyphonCacheKey::Custom(CustomGlyphCacheKey {
            glyph_id: id,
            width: SIDE,
            height: SIDE,
            x_bin: SubpixelBin::Zero,
            y_bin: SubpixelBin::Zero,
        }))
    }

    #[test]
    fn held_glyphs_are_never_evicted() {
        let mut rig = Rig::new();
        rig.prepare(&[0]).unwrap();
        let before = rig.at(0).unwrap();
        rig.atlas.hold(key(0));
        assert_eq!(rig.atlas.held(), 1);
        // 200 glyphs of 32x32 cannot fit in 256x256.
        rig.flood(1, 201);
        assert_eq!(rig.at(0), Some(before));
        assert_eq!(rig.atlas.mask_size(), 256);
    }

    #[test]
    fn released_glyphs_can_be_evicted() {
        let mut rig = Rig::new();
        rig.prepare(&[0]).unwrap();
        rig.atlas.hold(key(0));
        rig.atlas.hold(key(0));
        rig.atlas.release(key(0));
        assert_eq!(rig.atlas.held(), 1, "one hold is left");
        rig.atlas.release(key(0));
        rig.atlas.release(key(0));
        assert_eq!(rig.atlas.held(), 0, "a release at 0 does nothing");
        rig.flood(1, 201);
        assert_eq!(rig.at(0), None);
    }

    #[test]
    fn eviction_skips_held_entries_at_the_lru_end() {
        let mut rig = Rig::new();
        rig.prepare(&[0]).unwrap();
        rig.atlas.hold(key(0));
        // Fill until the first eviction; the oldest entry is held, so the next oldest goes.
        let mut id = 1;
        loop {
            let len = rig.atlas.mask_atlas.glyph_cache.len();
            rig.prepare(&[id]).expect("a held entry at the LRU end is not AtlasFull");
            if rig.atlas.mask_atlas.glyph_cache.len() <= len {
                break;
            }
            id += 1;
            assert!(id < 200, "the atlas never filled");
        }
        assert!(rig.at(0).is_some(), "the held glyph stays");
        assert!(rig.at(1).is_none(), "the next oldest was evicted");
    }

    #[test]
    fn growth_bumps_the_generation() {
        let mut rig = Rig::with_cap(None);
        assert_eq!(rig.atlas.generation(), 0);
        // Everything stays in use, so a full atlas can only grow.
        let glyphs: Vec<CustomGlyph> = (0..100u16)
            .map(|id| CustomGlyph {
                id,
                width: SIDE as f32,
                height: SIDE as f32,
                snap_to_physical_pixel: true,
                ..Default::default()
            })
            .collect();
        rig.atlas.trim();
        rig.prepare_keep(&glyphs).unwrap();
        assert!(rig.atlas.generation() >= 1);
        assert!(rig.atlas.mask_size() > 256);
    }

    fn glyph(id: u16) -> CustomGlyph {
        CustomGlyph {
            id,
            width: SIDE as f32,
            height: SIDE as f32,
            snap_to_physical_pixel: true,
            ..Default::default()
        }
    }

    #[test]
    fn glyphs_in_use_are_never_evicted() {
        let mut rig = Rig::new();
        rig.prepare(&[0]).unwrap();
        let before = rig.at(0).unwrap();
        // Glyph 0 stays in use (never trimmed between frames) but is not held.
        for id in 1..201 {
            rig.atlas.trim();
            rig.prepare_keep(&[glyph(0), glyph(id)]).unwrap();
        }
        assert_eq!(rig.atlas.held(), 0);
        assert_eq!(rig.at(0), Some(before));
        assert_eq!(rig.atlas.mask_size(), 256);
    }

    #[test]
    fn held_color_glyphs_stay_in_the_color_atlas() {
        let mut rig = Rig::new();
        rig.color = true;
        rig.prepare(&[0]).unwrap();
        let before = rig.at(0).unwrap();
        rig.atlas.hold(key(0));
        rig.flood(1, 201);
        assert_eq!(rig.at(0), Some(before));
        assert!(rig.atlas.mask_atlas.glyph_cache.is_empty(), "color glyphs went to the mask atlas");
        rig.atlas.release(key(0));
        rig.flood(201, 401);
        assert_eq!(rig.at(0), None, "a released color glyph can be evicted");
    }

    #[test]
    fn zero_size_entries_are_evicted_for_free() {
        let mut rig = Rig::new();
        rig.color = true;
        // A zero-size glyph (a space) is cached with no atlas space, here at the LRU's oldest end.
        let zero = key(999).0;
        rig.atlas.color_atlas.glyph_cache.put(
            zero,
            GlyphDetails {
                width: 0,
                height: 0,
                gpu_cache: GpuCacheStatus::SkipRasterization,
                atlas_id: None,
                top: 0,
                left: 0,
            },
        );
        rig.flood(1, 201);
        assert!(!rig.atlas.color_atlas.glyph_cache.contains(&zero), "the zero-size entry stayed");
    }
}
