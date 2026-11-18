use crate::{
    text_render::GlyphonCacheKey, Cache, ContentType, FontSystem, GlyphDetails, GpuCacheStatus,
    RasterizeCustomGlyphRequest, RasterizedCustomGlyph, State, SwashCache,
};
use etagere::{size2, Allocation, BucketedAtlasAllocator};
use lru::LruCache;
use rustc_hash::FxHasher;
use std::{
    collections::{HashMap, HashSet},
    hash::BuildHasherDefault,
};
use wgpu::{
    BindGroup, DepthStencilState, Device, Extent3d, MultisampleState, Origin3d, Queue,
    RenderPipeline, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect,
    TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
};

type Hasher = BuildHasherDefault<FxHasher>;

#[allow(dead_code)]
pub(crate) struct InnerAtlas {
    pub kind: Kind,
    pub texture: Texture,
    pub texture_view: TextureView,
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

    fn new(state: &State, kind: Kind) -> Self {
        let max_texture_dimension_2d = state.device.limits().max_texture_dimension_2d;
        let size = Self::INITIAL_SIZE.min(max_texture_dimension_2d);

        let packer = BucketedAtlasAllocator::new(size2(size as i32, size as i32));

        // Create a texture to use for our atlas
        let texture = state.device.create_texture(&TextureDescriptor {
            label: Some("glyphon atlas"),
            size: Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: kind.texture_format(),
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&TextureViewDescriptor::default());

        let glyph_cache = LruCache::unbounded_with_hasher(Hasher::default());
        let glyphs_in_use = HashSet::with_hasher(Hasher::default());

        Self {
            kind,
            texture,
            texture_view,
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

    pub fn num_channels(&self) -> usize {
        self.kind.num_channels()
    }

    pub(crate) fn grow(
        &mut self,
        state: &State,
        font_system: &mut FontSystem,
        cache: &mut SwashCache,
        scale_factor: f32,
        mut rasterize_custom_glyph: impl FnMut(
            RasterizeCustomGlyphRequest,
        ) -> Option<RasterizedCustomGlyph>,
    ) -> bool {
        if self.size >= self.max_texture_dimension_2d {
            return false;
        }

        // Grow each dimension by a factor of 2. The growth factor was chosen to match the growth
        // factor of `Vec`.`
        const GROWTH_FACTOR: u32 = 2;
        let new_size = (self.size * GROWTH_FACTOR).min(self.max_texture_dimension_2d);

        self.packer.grow(size2(new_size as i32, new_size as i32));

        // Create a texture to use for our atlas
        self.texture = state.device.create_texture(&TextureDescriptor {
            label: Some("glyphon atlas"),
            size: Extent3d {
                width: new_size,
                height: new_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: self.kind.texture_format(),
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

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

            state.queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: x as u32,
                        y: y as u32,
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image_data,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width as u32 * self.kind.num_channels() as u32),
                    rows_per_image: None,
                },
                Extent3d {
                    width: width as u32,
                    height: height as u32,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.texture_view = self.texture.create_view(&TextureViewDescriptor::default());
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
    fn num_channels(self) -> usize {
        match self {
            Kind::Mask => 1,
            Kind::Color { .. } => 4,
        }
    }

    fn texture_format(self) -> wgpu::TextureFormat {
        match self {
            Kind::Mask => TextureFormat::R8Unorm,
            Kind::Color { srgb } => {
                if srgb {
                    TextureFormat::Rgba8UnormSrgb
                } else {
                    TextureFormat::Rgba8Unorm
                }
            }
        }
    }

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
pub struct TextAtlas {
    pub(crate) cache: Cache,
    pub(crate) bind_group: BindGroup,
    pub(crate) color_atlas: InnerAtlas,
    pub(crate) mask_atlas: InnerAtlas,
    pub(crate) format: TextureFormat,
    pub(crate) color_mode: ColorMode,
    /// Termurion patch 4: raised by every successful grow.
    generation: u64,
}

impl TextAtlas {
    /// Creates a new [`TextAtlas`].
    pub fn new(device: &Device, queue: &Queue, cache: &Cache, format: TextureFormat) -> Self {
        Self::with_color_mode(device, queue, cache, format, ColorMode::Accurate)
    }

    /// Creates a new [`TextAtlas`] with the given [`ColorMode`].
    pub fn with_color_mode(
        device: &Device,
        queue: &Queue,
        cache: &Cache,
        format: TextureFormat,
        color_mode: ColorMode,
    ) -> Self {
        let state = State { device, queue };
        let color_atlas = InnerAtlas::new(
            &state,
            Kind::Color {
                srgb: match color_mode {
                    ColorMode::Accurate => true,
                    ColorMode::Web => false,
                },
            },
        );
        let mask_atlas = InnerAtlas::new(&state, Kind::Mask);

        let bind_group = cache.create_atlas_bind_group(
            device,
            &color_atlas.texture_view,
            &mask_atlas.texture_view,
        );

        Self {
            cache: cache.clone(),
            bind_group,
            color_atlas,
            mask_atlas,
            format,
            color_mode,
            generation: 0,
        }
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

    pub fn trim(&mut self) {
        self.mask_atlas.trim();
        self.color_atlas.trim();
    }

    pub(crate) fn grow(
        &mut self,
        state: &State,
        font_system: &mut FontSystem,
        cache: &mut SwashCache,
        content_type: ContentType,
        scale_factor: f32,
        rasterize_custom_glyph: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) -> bool {
        let did_grow = match content_type {
            ContentType::Mask => self.mask_atlas.grow(
                state,
                font_system,
                cache,
                scale_factor,
                rasterize_custom_glyph,
            ),
            ContentType::Color => self.color_atlas.grow(
                state,
                font_system,
                cache,
                scale_factor,
                rasterize_custom_glyph,
            ),
        };

        if did_grow {
            self.generation += 1;
            self.rebind(state.device);
        }

        did_grow
    }

    pub(crate) fn inner_for_content_mut(&mut self, content_type: ContentType) -> &mut InnerAtlas {
        match content_type {
            ContentType::Color => &mut self.color_atlas,
            ContentType::Mask => &mut self.mask_atlas,
        }
    }

    pub(crate) fn get_or_create_pipeline(
        &self,
        device: &Device,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
    ) -> RenderPipeline {
        self.cache
            .get_or_create_pipeline(device, self.format, multisample, depth_stencil)
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        self.bind_group = self.cache.create_atlas_bind_group(
            device,
            &self.color_atlas.texture_view,
            &self.mask_atlas.texture_view,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        custom_glyph::CustomGlyphCacheKey, Buffer, Cache, CustomGlyph, Metrics, PrepareError,
        Resolution, SubpixelBin, TextArea, TextBounds, TextRenderer, Viewport,
    };

    const SIDE: u16 = 32;

    struct Rig {
        device: wgpu::Device,
        queue: wgpu::Queue,
        atlas: TextAtlas,
        renderer: TextRenderer,
        viewport: Viewport,
        font_system: FontSystem,
        swash: SwashCache,
        buffer: Buffer,
        /// Custom glyphs rasterize as color (into the color atlas) when set.
        color: bool,
    }

    impl Rig {
        fn new() -> Self {
            Self::with_cap(Some(256))
        }

        fn with_cap(cap: Option<u32>) -> Self {
            let (device, queue) = crate::test_gpu::shared();
            let cache = Cache::new(&device);
            let mut atlas = TextAtlas::new(&device, &queue, &cache, TextureFormat::Rgba8Unorm);
            if let Some(px) = cap {
                atlas.set_max_size(px);
            }
            let renderer = TextRenderer::new(&mut atlas, &device, MultisampleState::default(), None);
            let mut viewport = Viewport::new(&device, &cache);
            viewport.update(&queue, Resolution { width: 1024, height: 1024 });
            let mut font_system = FontSystem::new_with_fonts([]);
            let buffer = Buffer::new(&mut font_system, Metrics::new(10.0, 12.0));
            Self {
                device,
                queue,
                atlas,
                renderer,
                viewport,
                font_system,
                swash: SwashCache::new(),
                buffer,
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
            self.renderer.prepare_with_custom(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.atlas,
                &self.viewport,
                [area],
                &mut self.swash,
                |req| {
                    Some(RasterizedCustomGlyph {
                        data: vec![255; req.width as usize * req.height as usize * channels],
                        content_type,
                    })
                },
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
