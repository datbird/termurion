//! Termurion patch 9: the ops a core atlas records rebuild exactly the texels its textures hold.

use super::*;
use crate::{
    text_render::GlyphonCacheKey, Attrs, Buffer, Color, CustomGlyph, Family, FontSystem,
    GlyphDraws, GpuCacheStatus, Metrics, RasterizeCustomGlyphRequest, RasterizedCustomGlyph,
    Resolution, Shaping, SwashCache, TextArea, TextBounds, TextRenderer,
};

/// An atlas texture replayed on the CPU from the ops alone.
struct CpuTexture {
    side: u32,
    bpp: usize,
    texels: Vec<u8>,
}

impl CpuTexture {
    fn new(side: u32, bpp: usize) -> Self {
        Self { side, bpp, texels: vec![0; side as usize * side as usize * bpp] }
    }

    fn apply(&mut self, op: &AtlasOp) {
        match op {
            AtlasOp::Grow { size, .. } => *self = Self::new(*size, self.bpp),
            AtlasOp::Upload { x, y, w, h, bytes, .. } => {
                let (row, side) = (*w as usize * self.bpp, self.side as usize);
                for line in 0..*h as usize {
                    let at = ((*y as usize + line) * side + *x as usize) * self.bpp;
                    self.texels[at..at + row].copy_from_slice(&bytes[line * row..(line + 1) * row]);
                }
            }
        }
    }

    /// The `w` by `h` rectangle at `x, y`, rows packed.
    fn rect(&self, x: u32, y: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::new();
        for line in 0..h as usize {
            let at = ((y as usize + line) * self.side as usize + x as usize) * self.bpp;
            out.extend_from_slice(&self.texels[at..at + w as usize * self.bpp]);
        }
        out
    }
}

/// The texels of a square texture, read back from the GPU. A side of at least 256 keeps each
/// row a multiple of the copy alignment.
fn read_back(device: &Device, queue: &Queue, texture: &Texture, side: u32, bpp: u32) -> Vec<u8> {
    let out = device.create_buffer(&BufferDescriptor {
        label: None,
        size: (side * side * bpp) as u64,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &out,
            layout: TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(side * bpp), rows_per_image: Some(side) },
        },
        Extent3d { width: side, height: side, depth_or_array_layers: 1 },
    );
    queue.submit(Some(enc.finish()));
    out.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let bytes = out.slice(..).get_mapped_range().unwrap().to_vec();
    bytes
}

/// A custom glyph's texels, different for every id and size, so a misplaced glyph shows.
fn pattern(id: u16, w: u16, h: u16, bpp: usize) -> Vec<u8> {
    (0..w as usize * h as usize * bpp).map(|i| (i * 31 + id as usize * 7 + 1) as u8).collect()
}

fn is_color(id: u16) -> bool {
    id % 3 == 0
}

fn rasterize(req: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
    let content_type = if is_color(req.id) { ContentType::Color } else { ContentType::Mask };
    Some(RasterizedCustomGlyph { data: pattern(req.id, req.width, req.height, content_type.bytes_per_pixel()), content_type })
}

fn first_difference(a: &[u8], b: &[u8]) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

#[test]
fn atlas_ops_rebuild_the_same_texels() {
    let (device, queue) = crate::test_gpu::shared();
    let cache = Cache::new(&device);
    let mut atlas = TextAtlas::new(device.limits().max_texture_dimension_2d);
    // Growth from 256 to 512, then eviction.
    atlas.set_max_size(512);
    let mut gpu = GpuAtlas::new(&device, &cache, &atlas);
    let mut cpu_mask = CpuTexture::new(atlas.mask_size(), 1);
    let mut cpu_color = CpuTexture::new(atlas.color_size(), 4);

    let mut font_system = FontSystem::new_with_fonts([]);
    font_system
        .db_mut()
        .load_font_data(include_bytes!("../../../../tests/fixtures/fonts/TermurionWideBoldTest-Regular.ttf").to_vec());
    let mut text = Buffer::new(&mut font_system, Metrics::new(18.0, 22.0));
    text.set_text("Text glyphs re-raster on growth gjy", &Attrs::new().family(Family::Name("Termurion Wide Bold Test")), Shaping::Advanced, None);
    text.shape_until_scroll(&mut font_system, false);
    let mut swash = SwashCache::new();
    let mut renderer = TextRenderer::new();
    let mut out = GlyphDraws::default();

    let mut ops = Vec::new();
    for frame in 0..300u16 {
        // The first frames keep every glyph in use, so the atlas must grow; after that only a
        // frame's own glyphs are, so older ones are evicted.
        if frame >= 60 {
            atlas.trim();
        }
        let glyphs: Vec<CustomGlyph> = (0..4u16)
            .map(|k| CustomGlyph {
                id: frame * 4 + k,
                left: k as f32 * 30.0,
                top: 40.0,
                width: 24.0,
                height: 20.0 + k as f32,
                color: Some(Color::rgb(200, 100, 50)),
                snap_to_physical_pixel: true,
                ..Default::default()
            })
            .collect();
        let area = TextArea {
            buffer: &text,
            left: 0.0,
            top: 0.0,
            scale: 1.0,
            bounds: TextBounds::default(),
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: &glyphs,
        };
        renderer
            .prepare_cpu(&mut font_system, &mut atlas, Resolution { width: 1024, height: 1024 }, [area], &mut swash, rasterize, &mut out)
            .unwrap();
        ops.clear();
        atlas.take_ops(&mut ops);
        gpu.apply(&device, &queue, &ops);
        for op in &ops {
            let (AtlasOp::Grow { kind, .. } | AtlasOp::Upload { kind, .. }) = op;
            match kind {
                ContentType::Mask => cpu_mask.apply(op),
                ContentType::Color => cpu_color.apply(op),
            }
        }
    }
    assert!(atlas.generation() > 0, "the atlas never grew");
    assert!(atlas.evictions() > 0, "the atlas never evicted");
    assert_eq!((cpu_mask.side, cpu_color.side), (atlas.mask_size(), atlas.color_size()));

    for (what, cpu, texture) in [("mask", &cpu_mask, &gpu.mask.texture), ("color", &cpu_color, &gpu.color.texture)] {
        let read = read_back(&device, &queue, texture, cpu.side, cpu.bpp as u32);
        if let Some(i) = first_difference(&cpu.texels, &read) {
            let px = i / cpu.bpp;
            panic!("{what}: the ops and the texture differ first at x {} y {}", px % cpu.side as usize, px / cpu.side as usize);
        }
    }

    // And what the ops built is right: every cached custom glyph sits where the atlas says.
    let mut checked = 0;
    for (inner, cpu) in [(&atlas.mask_atlas, &cpu_mask), (&atlas.color_atlas, &cpu_color)] {
        for (key, details) in inner.glyph_cache.iter() {
            let (GlyphonCacheKey::Custom(key), GpuCacheStatus::InAtlas { x, y, .. }) = (key, &details.gpu_cache) else { continue };
            assert_eq!(is_color(key.glyph_id), cpu.bpp == 4, "glyph {} is in the wrong atlas", key.glyph_id);
            let got = cpu.rect(*x as u32, *y as u32, key.width as u32, key.height as u32);
            assert!(got == pattern(key.glyph_id, key.width, key.height, cpu.bpp), "glyph {} is not at {x}, {y}", key.glyph_id);
            checked += 1;
        }
    }
    assert!(checked > 20, "only {checked} glyphs were checked");
}
