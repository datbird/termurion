//! Text on Metal: glyphon's glyph and row pipelines (`shader.wgsl`, `row_shader.wgsl`) over the
//! atlas textures and row buffers, kept equal to the CPU atlas and row renderer by applying their
//! ops and writes in order. `glyphon::wgpu_backend` is the reference: the same formats, sampler,
//! slots and draws.

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCullMode, MTLDevice, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLSamplerAddressMode,
    MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLSamplerState, MTLSize, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLTriangleFillMode, MTLWinding,
};

use glyphon::{AtlasOp, ColorMode, ContentType, GlyphDraws, RowDraws, RowRenderer, RowWrite, TextAtlas, GLYPH_SIZE, ROW_TABLE_CHUNK, ROW_UNIFORM_SIZE};

use super::quads::{Attr, Blend, Layout, Shader};
use super::shaders::{self, Entry, Kind};

type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;

const INSTANCE_SIZE: u64 = GLYPH_SIZE as u64;
const ROW_ID_SIZE: u64 = std::mem::size_of::<u32>() as u64;

/// Group 0: the color and mask atlases (both stages read their size) and the sampler.
const ATLAS: [Entry; 3] = [Entry::new(0, Kind::Texture, true, true), Entry::new(1, Kind::Texture, true, true), Entry::new(2, Kind::Sampler, false, true)];
/// Group 1: the viewport uniform. Group 2 (rows only): one chunk of the row table.
const UNIFORM: [Entry; 1] = [Entry::new(0, Kind::Buffer, true, false)];

/// The slots the groups above get in the vertex stage (`shaders::stage_resources`).
const PARAMS_SLOT: usize = 0;
const TABLE_SLOT: usize = 1;
/// The sizes buffer comes after the stage's other buffers.
const GLYPH_SIZES_SLOT: usize = 1;
const ROW_SIZES_SLOT: usize = 2;

/// `GlyphToRender`, as glyphon's `Cache` lays it out.
const GLYPH_ATTRS: [Attr; 6] = [
    Attr { location: 0, offset: 0, format: wgpu::VertexFormat::Sint32x2 },
    Attr { location: 1, offset: 8, format: wgpu::VertexFormat::Uint32 },
    Attr { location: 2, offset: 12, format: wgpu::VertexFormat::Uint32 },
    Attr { location: 3, offset: 16, format: wgpu::VertexFormat::Uint32 },
    Attr { location: 4, offset: 20, format: wgpu::VertexFormat::Uint32 },
    Attr { location: 5, offset: 24, format: wgpu::VertexFormat::Float32 },
];
const ROW_ID_ATTRS: [Attr; 1] = [Attr { location: 6, offset: 0, format: wgpu::VertexFormat::Uint32 }];
const GLYPH_BUFFERS: [Layout; 1] = [Layout { stride: GLYPH_SIZE as u32, instance: true, attrs: &GLYPH_ATTRS }];
/// The instances, then their row ids.
const ROW_BUFFERS: [Layout; 2] = [Layout { stride: GLYPH_SIZE as u32, instance: true, attrs: &GLYPH_ATTRS }, Layout { stride: ROW_ID_SIZE as u32, instance: true, attrs: &ROW_ID_ATTRS }];

/// glyphon's `next_copy_buffer_size`: the next power of two, a multiple of 4.
fn copy_size(size: u64) -> u64 {
    ((size.next_power_of_two() + 3) & !3).max(4)
}

/// A packed row of `w` texels as it sits in staging: 4-byte aligned, as Metal's copies want.
fn pitch(w: u32, kind: ContentType) -> usize {
    (w as usize * kind.bytes_per_pixel()).next_multiple_of(4)
}

/// Sizes a backend starts the text state at: a new atlas and row renderer's.
pub(super) struct TextStart {
    color_mode: ColorMode,
    color_size: u32,
    mask_size: u32,
    capacity: u32,
    table_rows: u32,
}

impl TextStart {
    /// The sizes of a new atlas that may grow to `max_side` and a new row renderer over it, as
    /// `Renderer::restart_for_backend` makes them.
    pub fn fresh(max_side: u32) -> TextStart {
        let atlas = TextAtlas::new(max_side);
        let rows = RowRenderer::new(&atlas);
        TextStart { color_mode: atlas.color_mode(), color_size: atlas.color_size(), mask_size: atlas.mask_size(), capacity: rows.capacity(), table_rows: rows.table_rows() }
    }
}

/// What a frame's text changes need made before anything is encoded, in the order they are used.
#[derive(Default)]
pub(super) struct Made {
    /// One per `AtlasOp::Grow`; each is cleared before its uploads, as wgpu zeroes a new texture.
    textures: Vec<Texture>,
    /// One instance and id buffer pair per `RowWrite::Grow`.
    rows: Vec<(Buffer, Buffer)>,
    /// One per `RowWrite::GrowTable`.
    tables: Vec<Buffer>,
    /// A bigger glyph instance buffer and its size.
    vertices: Option<(Buffer, u64)>,
}

/// The text half of the Metal draw.
pub(super) struct MetalText {
    glyphs: Pipeline,
    rows: Pipeline,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    color_format: MTLPixelFormat,
    color: Texture,
    mask: Texture,
    vertices: Buffer,
    vertices_size: u64,
    /// Glyph instances in `vertices` for this frame.
    count: u32,
    instances: Buffer,
    ids: Buffer,
    table: Buffer,
    /// Buffers and textures made after the first ones (tests).
    pub made: u64,
}

fn private_buffer(device: &ProtocolObject<dyn MTLDevice>, size: u64) -> Result<Buffer, String> {
    device.newBufferWithLength_options(size as usize, MTLResourceOptions::StorageModePrivate).ok_or_else(|| format!("no Metal buffer of {size} bytes"))
}

/// An atlas texture: private, sampled, and a render target only so a new one can be cleared.
fn atlas_texture(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat, size: u32) -> Result<Texture, String> {
    // SAFETY: a plain 2D descriptor; the atlas never grows past `MAX_TEXTURE_SIDE`.
    let desc = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(format, size as usize, size as usize, false) };
    desc.setUsage(MTLTextureUsage::ShaderRead | MTLTextureUsage::RenderTarget);
    desc.setStorageMode(MTLStorageMode::Private);
    device.newTextureWithDescriptor(&desc).ok_or_else(|| format!("no Metal atlas texture of {size} px"))
}

/// glyphon's sampler (Nearest filters, LOD clamped to 0, edges clamped), as wgpu-hal
/// `create_sampler` builds it: a Nearest mip filter over a 0..0 LOD range is no mipmapping.
fn sampler(device: &ProtocolObject<dyn MTLDevice>) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>, String> {
    let d = MTLSamplerDescriptor::new();
    d.setMinFilter(MTLSamplerMinMagFilter::Nearest);
    d.setMagFilter(MTLSamplerMinMagFilter::Nearest);
    d.setMipFilter(MTLSamplerMipFilter::NotMipmapped);
    d.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
    d.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
    d.setRAddressMode(MTLSamplerAddressMode::ClampToEdge);
    d.setMaxAnisotropy(1);
    d.setLodMinClamp(0.0);
    d.setLodMaxClamp(0.0);
    device.newSamplerStateWithDescriptor(&d).ok_or_else(|| "no Metal sampler".to_string())
}

impl MetalText {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat, start: &TextStart) -> Result<MetalText, String> {
        let glyphs = Shader::new(device, include_str!("../../crates/glyphon/src/shader.wgsl"), ("vs_main", "fs_main"), &[&ATLAS, &UNIFORM], &GLYPH_BUFFERS)?.pipeline(device, format, Blend::ALPHA)?;
        let rows = Shader::new(device, include_str!("../../crates/glyphon/src/row_shader.wgsl"), ("vs_row", "fs_main"), &[&ATLAS, &UNIFORM, &UNIFORM], &ROW_BUFFERS)?.pipeline(device, format, Blend::ALPHA)?;
        let color_format = match start.color_mode {
            ColorMode::Accurate => MTLPixelFormat::RGBA8Unorm_sRGB,
            ColorMode::Web => MTLPixelFormat::RGBA8Unorm,
        };
        let vertices_size = copy_size(4096);
        Ok(MetalText {
            glyphs,
            rows,
            sampler: sampler(device)?,
            color_format,
            color: atlas_texture(device, color_format, start.color_size)?,
            mask: atlas_texture(device, MTLPixelFormat::R8Unorm, start.mask_size)?,
            vertices: private_buffer(device, vertices_size)?,
            vertices_size,
            count: 0,
            instances: private_buffer(device, copy_size(start.capacity as u64 * INSTANCE_SIZE))?,
            ids: private_buffer(device, copy_size(start.capacity as u64 * ROW_ID_SIZE))?,
            table: private_buffer(device, start.table_rows as u64 * ROW_UNIFORM_SIZE)?,
            made: 0,
        })
    }

    fn format(&self, kind: ContentType) -> MTLPixelFormat {
        match kind {
            ContentType::Color => self.color_format,
            ContentType::Mask => MTLPixelFormat::R8Unorm,
        }
    }

    /// Makes what this frame's changes need into `made` and returns the staging bytes they take.
    /// Nothing changes here, so a failure leaves the text as it was.
    pub fn make(&self, device: &ProtocolObject<dyn MTLDevice>, ops: &[AtlasOp], rows: &RowDraws, glyphs: &GlyphDraws, made: &mut Made) -> Result<usize, String> {
        let mut bytes = 0;
        for op in ops {
            match op {
                AtlasOp::Grow { kind, size } => made.textures.push(atlas_texture(device, self.format(*kind), *size)?),
                AtlasOp::Upload { kind, w, h, .. } => bytes += pitch(*w, *kind) * *h as usize,
            }
        }
        for w in &rows.writes {
            match w {
                RowWrite::Grow { capacity } => made.rows.push((private_buffer(device, copy_size(*capacity as u64 * INSTANCE_SIZE))?, private_buffer(device, copy_size(*capacity as u64 * ROW_ID_SIZE))?)),
                RowWrite::GrowTable { rows } => made.tables.push(private_buffer(device, *rows as u64 * ROW_UNIFORM_SIZE)?),
                RowWrite::Instances { bytes: r, .. } | RowWrite::Ids { bytes: r, .. } | RowWrite::Table { bytes: r, .. } => bytes += r.len().next_multiple_of(4),
            }
        }
        let len = glyphs.bytes().len() as u64;
        if len > self.vertices_size {
            // As `GpuText::prepare`: a buffer big enough, made anew.
            let size = copy_size(len);
            made.vertices = Some((private_buffer(device, size)?, size));
        }
        Ok(bytes + len as usize)
    }

    /// Clears every texture `made` holds, ahead of the frame's copies.
    pub fn clear_new(&self, cb: &ProtocolObject<dyn MTLCommandBuffer>, made: &Made) -> Result<(), String> {
        for t in &made.textures {
            let pass = MTLRenderPassDescriptor::new();
            // SAFETY: index 0 is the one color attachment.
            let at = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
            at.setTexture(Some(t));
            at.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 0.0 });
            at.setLoadAction(MTLLoadAction::Clear);
            at.setStoreAction(MTLStoreAction::Store);
            cb.renderCommandEncoderWithDescriptor(&pass).ok_or("no Metal render encoder")?.endEncoding();
        }
        Ok(())
    }

    /// Applies the frame's atlas ops, row writes and glyph instances in order, with the objects
    /// `made` holds. The bytes go through `copies` (the staging chunk and the blit), from byte
    /// `at`; there is none only when `make` counted no bytes.
    pub fn write(&mut self, copies: Option<(&ProtocolObject<dyn MTLBuffer>, &ProtocolObject<dyn MTLBlitCommandEncoder>)>, at: &mut usize, ops: &[AtlasOp], rows: &RowDraws, glyphs: &GlyphDraws, made: Made) {
        let copy = || copies.expect("bytes to copy come with a staging chunk");
        // Copies `bytes` to staging at `at` (4-byte aligned) and returns where they went.
        let put = |bytes: &[u8], at: &mut usize| {
            let from = *at;
            // SAFETY: `make` counted every write, so the chunk holds it; no command buffer still
            // reads the chunk (`Staging::take`).
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), copy().0.contents().as_ptr().cast::<u8>().add(from), bytes.len()) };
            *at += bytes.len().next_multiple_of(4);
            from
        };
        let mut textures = made.textures.into_iter();
        for op in ops {
            match op {
                AtlasOp::Grow { kind, .. } => {
                    let t = textures.next().expect("made above");
                    self.made += 1;
                    match kind {
                        ContentType::Color => self.color = t,
                        ContentType::Mask => self.mask = t,
                    }
                }
                AtlasOp::Upload { kind, x, y, w, h, bytes } => {
                    if *w == 0 || *h == 0 {
                        continue;
                    }
                    let (staging, blit) = copy();
                    let base = staging.contents().as_ptr().cast::<u8>();
                    let (row, stride) = (*w as usize * kind.bytes_per_pixel(), pitch(*w, *kind));
                    let from = *at;
                    for line in bytes.chunks_exact(row) {
                        // SAFETY: as in `put`; `stride` per line was counted in `make`.
                        unsafe { std::ptr::copy_nonoverlapping(line.as_ptr(), base.add(*at), row) };
                        *at += stride;
                    }
                    let texture = match kind {
                        ContentType::Color => &self.color,
                        ContentType::Mask => &self.mask,
                    };
                    // SAFETY: the rectangle lies in the texture (the atlas placed it there) and the
                    // staged rows hold it.
                    unsafe {
                        blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                            staging,
                            from,
                            stride,
                            stride * *h as usize,
                            MTLSize { width: *w as usize, height: *h as usize, depth: 1 },
                            texture,
                            0,
                            0,
                            MTLOrigin { x: *x as usize, y: *y as usize, z: 0 },
                        );
                    }
                }
            }
        }
        let (mut grown, mut tables) = (made.rows.into_iter(), made.tables.into_iter());
        for w in &rows.writes {
            // Each grown buffer is dropped from here, not destroyed: copies recorded this frame
            // keep it alive until the command buffer is done.
            let (target, offset, range) = match w {
                RowWrite::Grow { .. } => {
                    (self.instances, self.ids) = grown.next().expect("made above");
                    self.made += 2;
                    continue;
                }
                RowWrite::GrowTable { .. } => {
                    self.table = tables.next().expect("made above");
                    self.made += 1;
                    continue;
                }
                RowWrite::Instances { at: i, bytes } => (&self.instances, *i as u64 * INSTANCE_SIZE, bytes),
                RowWrite::Ids { at: i, bytes } => (&self.ids, *i as u64 * ROW_ID_SIZE, bytes),
                RowWrite::Table { first, bytes } => (&self.table, *first as u64 * ROW_UNIFORM_SIZE, bytes),
            };
            let bytes = rows.bytes_of(range);
            if bytes.is_empty() {
                continue;
            }
            let from = put(bytes, at);
            let (staging, blit) = copy();
            // SAFETY: the CPU side keeps every write inside the buffer it sized.
            unsafe { blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(staging, from, target, offset as usize, bytes.len()) };
        }
        self.count = glyphs.instances.len() as u32;
        let bytes = glyphs.bytes();
        if let Some((buffer, size)) = made.vertices {
            (self.vertices, self.vertices_size) = (buffer, size);
            self.made += 1;
        }
        if !bytes.is_empty() {
            let from = put(bytes, at);
            let (staging, blit) = copy();
            // SAFETY: `make` made the buffer big enough.
            unsafe { blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(staging, from, &self.vertices, 0, bytes.len()) };
        }
    }

    /// Sets `pipeline` with the atlas and the viewport bound, as `GpuRows::render` and
    /// `GpuText::render` bind groups 0 and 1.
    fn bind(&self, enc: &ProtocolObject<dyn MTLRenderCommandEncoder>, pipeline: &ProtocolObject<dyn MTLRenderPipelineState>, (w, h): (u32, u32)) {
        enc.setRenderPipelineState(pipeline);
        enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
        enc.setCullMode(MTLCullMode::None);
        enc.setTriangleFillMode(MTLTriangleFillMode::Fill);
        // glyphon's `Params`: the resolution and padding.
        let params: [u32; 4] = [w, h, 0, 0];
        // SAFETY: the slots are the ones the shaders were translated with (`ATLAS`, `UNIFORM`).
        unsafe {
            enc.setVertexTexture_atIndex(Some(&self.color), 0);
            enc.setVertexTexture_atIndex(Some(&self.mask), 1);
            enc.setFragmentTexture_atIndex(Some(&self.color), 0);
            enc.setFragmentTexture_atIndex(Some(&self.mask), 1);
            enc.setFragmentSamplerState_atIndex(Some(&self.sampler), 0);
            enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&params).cast(), 16, PARAMS_SLOT);
        }
    }

    /// The row batches, in order (`GpuRows::render`).
    pub fn draw_rows(&self, enc: &ProtocolObject<dyn MTLRenderCommandEncoder>, rows: &RowDraws, size: (u32, u32)) {
        if rows.batches.is_empty() {
            return;
        }
        self.bind(enc, &self.rows, size);
        let mut bound = None;
        for batch in &rows.batches {
            if bound != Some(batch.chunk) {
                bound = Some(batch.chunk);
                // SAFETY: the table holds whole chunks and the batch's chunk is in it.
                unsafe { enc.setVertexBuffer_offset_atIndex(Some(&self.table), (batch.chunk as u64 * ROW_TABLE_CHUNK) as usize, TABLE_SLOT) };
            }
            let (from, to) = (batch.start as u64, batch.end as u64);
            // The slices wgpu binds, and their lengths in the sizes buffer, in mapping order.
            let sizes = [((to - from) * INSTANCE_SIZE) as u32, ((to - from) * ROW_ID_SIZE) as u32];
            // SAFETY: the batch lies in the buffers (the CPU side sized them); the slots are the
            // shader's.
            unsafe {
                enc.setVertexBuffer_offset_atIndex(Some(&self.instances), (from * INSTANCE_SIZE) as usize, shaders::vertex_buffer_slot(0) as usize);
                enc.setVertexBuffer_offset_atIndex(Some(&self.ids), (from * ROW_ID_SIZE) as usize, shaders::vertex_buffer_slot(1) as usize);
                enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&sizes).cast(), 8, ROW_SIZES_SLOT);
                draw_quads(enc, batch.end - batch.start);
            }
        }
    }

    /// This frame's glyph instances (`GpuText::render`), from the whole buffer.
    pub fn draw_glyphs(&self, enc: &ProtocolObject<dyn MTLRenderCommandEncoder>, size: (u32, u32)) {
        if self.count == 0 {
            return;
        }
        self.bind(enc, &self.glyphs, size);
        let sizes = [u32::try_from(self.vertices_size).unwrap_or(u32::MAX)];
        // SAFETY: the instances are in the buffer (`write`); the slots are the shader's.
        unsafe {
            enc.setVertexBuffer_offset_atIndex(Some(&self.vertices), 0, shaders::vertex_buffer_slot(0) as usize);
            enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&sizes).cast(), 4, GLYPH_SIZES_SLOT);
            draw_quads(enc, self.count);
        }
    }
}

/// `count` instances of a four-vertex strip, with the call wgpu-hal picks for the count.
///
/// # Safety
/// The bound buffers hold `count` instances.
unsafe fn draw_quads(enc: &ProtocolObject<dyn MTLRenderCommandEncoder>, count: u32) {
    unsafe {
        if count == 1 {
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::TriangleStrip, 0, 4);
        } else {
            enc.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::TriangleStrip, 0, 4, count as usize);
        }
    }
}
