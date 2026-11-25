//! Termurion patch 9: the wgpu backend. It applies what the GPU-free core hands it (atlas ops,
//! glyph instances, row buffer writes) with the code glyphon always drew with, and draws.

use crate::{
    AtlasOp, Cache, ColorMode, ContentType, GlyphDraws, RenderError, RowDraws, RowRenderer,
    RowWrite, TextAtlas, UploadBelt, Viewport, GLYPH_SIZE, ROW_TABLE_CHUNK, ROW_UNIFORM_SIZE,
};
use wgpu::{
    BindGroup, Buffer, BufferDescriptor, BufferUsages, DepthStencilState, Device, Extent3d,
    MultisampleState, Origin3d, Queue, RenderPass, RenderPipeline, TexelCopyBufferLayout,
    TexelCopyTextureInfo, Texture, TextureAspect, TextureDescriptor, TextureDimension,
    TextureFormat, TextureUsages, TextureView, TextureViewDescriptor, COPY_BUFFER_ALIGNMENT,
};

#[cfg(test)]
mod atlas_tests;
#[cfg(test)]
mod row_tests;

const INSTANCE_SIZE: u64 = GLYPH_SIZE as u64;
const ROW_ID_SIZE: u64 = std::mem::size_of::<u32>() as u64;

pub(crate) fn next_copy_buffer_size(size: u64) -> u64 {
    let align_mask = COPY_BUFFER_ALIGNMENT - 1;
    ((size.next_power_of_two() + align_mask) & !align_mask).max(COPY_BUFFER_ALIGNMENT)
}

fn create_oversized_buffer(
    device: &Device,
    label: Option<&str>,
    contents: &[u8],
    usage: BufferUsages,
) -> (Buffer, u64) {
    let size = next_copy_buffer_size(contents.len() as u64);
    let buffer = device.create_buffer(&BufferDescriptor {
        label,
        size,
        usage,
        mapped_at_creation: true,
    });
    buffer
        .slice(..contents.len() as u64)
        .get_mapped_range_mut()
        .unwrap()
        .copy_from_slice(contents);
    buffer.unmap();
    (buffer, size)
}

/// One atlas texture and its view.
struct AtlasTexture {
    texture: Texture,
    view: TextureView,
    format: TextureFormat,
}

impl AtlasTexture {
    fn new(device: &Device, format: TextureFormat, size: u32) -> Self {
        // Create a texture to use for our atlas
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("glyphon atlas"),
            size: Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            // The atlas test reads the texels back.
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | if cfg!(test) { TextureUsages::COPY_SRC } else { TextureUsages::empty() },
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        Self { texture, view, format }
    }
}

/// The atlas textures of a [`TextAtlas`], kept equal to it by [`GpuAtlas::apply`].
pub struct GpuAtlas {
    cache: Cache,
    color: AtlasTexture,
    mask: AtlasTexture,
    pub(crate) bind_group: BindGroup,
}

impl GpuAtlas {
    /// The textures of `atlas` as it is now; it must have uploaded nothing yet (a new atlas).
    pub fn new(device: &Device, cache: &Cache, atlas: &TextAtlas) -> Self {
        // Termurion patch 9: textures made from a used atlas would miss what it already placed.
        debug_assert!(atlas.ops.is_empty() && atlas.generation() == 0, "GpuAtlas::new over an atlas that already placed glyphs");
        let color_format = match atlas.color_mode() {
            ColorMode::Accurate => TextureFormat::Rgba8UnormSrgb,
            ColorMode::Web => TextureFormat::Rgba8Unorm,
        };
        let color = AtlasTexture::new(device, color_format, atlas.color_size());
        let mask = AtlasTexture::new(device, TextureFormat::R8Unorm, atlas.mask_size());
        let bind_group = cache.create_atlas_bind_group(device, &color.view, &mask.view);
        Self { cache: cache.clone(), color, mask, bind_group }
    }

    /// Applies the ops in order: a grow makes the texture again, an upload writes its texels.
    pub fn apply(&mut self, device: &Device, queue: &Queue, ops: &[AtlasOp]) {
        let mut rebind = false;
        for op in ops {
            match op {
                AtlasOp::Grow { kind, size } => {
                    let inner = self.texture_mut(*kind);
                    *inner = AtlasTexture::new(device, inner.format, *size);
                    rebind = true;
                }
                AtlasOp::Upload { kind, x, y, w, h, bytes } => {
                    queue.write_texture(
                        TexelCopyTextureInfo {
                            texture: &self.texture_mut(*kind).texture,
                            mip_level: 0,
                            origin: Origin3d { x: *x, y: *y, z: 0 },
                            aspect: TextureAspect::All,
                        },
                        bytes,
                        TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(*w * kind.bytes_per_pixel() as u32),
                            rows_per_image: None,
                        },
                        Extent3d { width: *w, height: *h, depth_or_array_layers: 1 },
                    );
                }
            }
        }
        if rebind {
            self.bind_group = self.cache.create_atlas_bind_group(device, &self.color.view, &self.mask.view);
        }
    }

    fn texture_mut(&mut self, kind: ContentType) -> &mut AtlasTexture {
        match kind {
            ContentType::Color => &mut self.color,
            ContentType::Mask => &mut self.mask,
        }
    }
}

/// Draws the instances of a [`crate::TextRenderer`].
pub struct GpuText {
    vertex_buffer: Buffer,
    vertex_buffer_size: u64,
    pipeline: RenderPipeline,
    /// Instances in the buffer for `render`.
    count: u32,
}

impl GpuText {
    /// The pipeline for a target of `format`.
    pub fn new(
        device: &Device,
        cache: &Cache,
        format: TextureFormat,
        multisample: MultisampleState,
        depth_stencil: Option<DepthStencilState>,
    ) -> Self {
        let vertex_buffer_size = next_copy_buffer_size(4096);
        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("glyphon vertices"),
            size: vertex_buffer_size,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline = cache.get_or_create_pipeline(device, format, multisample, depth_stencil);

        Self { vertex_buffer, vertex_buffer_size, pipeline, count: 0 }
    }

    /// Uploads `draws` for the next `render`, through `uploads` when given (its `submit` must
    /// then send them before the render pass), else through the queue.
    pub fn prepare(&mut self, device: &Device, queue: &Queue, uploads: Option<&mut UploadBelt>, draws: &GlyphDraws) {
        self.count = draws.instances.len() as u32;
        let vertices_raw = draws.bytes();
        if vertices_raw.is_empty() {
            return;
        }

        if self.vertex_buffer_size >= vertices_raw.len() as u64 {
            match uploads {
                Some(uploads) => uploads.write(device, &self.vertex_buffer, 0, vertices_raw),
                None => queue.write_buffer(&self.vertex_buffer, 0, vertices_raw),
            }
        } else {
            // Termurion patch 7: dropped, not destroyed; a copy recorded this frame may still
            // target it.
            let (buffer, buffer_size) = create_oversized_buffer(
                device,
                Some("glyphon vertices"),
                vertices_raw,
                BufferUsages::VERTEX | BufferUsages::COPY_DST,
            );

            self.vertex_buffer = buffer;
            self.vertex_buffer_size = buffer_size;
        }
    }

    /// Draws what the last `prepare` uploaded.
    pub fn render(&self, atlas: &GpuAtlas, viewport: &Viewport, pass: &mut RenderPass<'_>) -> Result<(), RenderError> {
        if self.count == 0 {
            return Ok(());
        }

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &atlas.bind_group, &[]);
        pass.set_bind_group(1, &viewport.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..4, 0..self.count);

        Ok(())
    }
}

/// The buffers of a [`RowRenderer`]: the instances, their row ids and the row table.
pub struct GpuRows {
    cache: Cache,
    pipeline: RenderPipeline,
    instances: Buffer,
    ids: Buffer,
    table: Buffer,
    rows_bind_group: BindGroup,
}

impl GpuRows {
    /// Buffers as big as `rows` starts them, and the pipeline for a target of `format`.
    pub fn new(device: &Device, cache: &Cache, format: TextureFormat, multisample: MultisampleState, rows: &RowRenderer) -> Self {
        let pipeline = cache.get_or_create_row_pipeline(device, format, multisample);
        let (instances, ids) = instance_buffers(device, rows.capacity());
        let table = table_buffer(device, rows.table_rows());
        let rows_bind_group = cache.create_rows_bind_group(device, &table);
        Self { cache: cache.clone(), pipeline, instances, ids, table, rows_bind_group }
    }

    /// Applies the writes in order, every buffer write through `uploads`, whose `submit` must
    /// send them before the rows draw.
    pub fn apply(&mut self, device: &Device, uploads: &mut UploadBelt, draws: &RowDraws) {
        for w in &draws.writes {
            match w {
                // Dropped, not destroyed: copies recorded this frame may still target them.
                RowWrite::Grow { capacity } => (self.instances, self.ids) = instance_buffers(device, *capacity),
                RowWrite::GrowTable { rows } => {
                    self.table = table_buffer(device, *rows);
                    self.rows_bind_group = self.cache.create_rows_bind_group(device, &self.table);
                }
                RowWrite::Instances { at, bytes } => uploads.write(device, &self.instances, *at as u64 * INSTANCE_SIZE, draws.bytes_of(bytes)),
                RowWrite::Ids { at, bytes } => uploads.write(device, &self.ids, *at as u64 * ROW_ID_SIZE, draws.bytes_of(bytes)),
                RowWrite::Table { first, bytes } => uploads.write(device, &self.table, *first as u64 * ROW_UNIFORM_SIZE, draws.bytes_of(bytes)),
            }
        }
    }

    /// Draws the batches of `draws`, in order.
    pub fn render(&self, atlas: &GpuAtlas, viewport: &Viewport, pass: &mut RenderPass<'_>, draws: &RowDraws) -> Result<(), RenderError> {
        if draws.batches.is_empty() {
            return Ok(());
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &atlas.bind_group, &[]);
        pass.set_bind_group(1, &viewport.bind_group, &[]);
        let mut bound = None;
        for batch in &draws.batches {
            if bound != Some(batch.chunk) {
                bound = Some(batch.chunk);
                pass.set_bind_group(2, &self.rows_bind_group, &[(batch.chunk as u64 * ROW_TABLE_CHUNK) as u32]);
            }
            let (from, to) = (batch.start as u64, batch.end as u64);
            pass.set_vertex_buffer(0, self.instances.slice(from * INSTANCE_SIZE..to * INSTANCE_SIZE));
            pass.set_vertex_buffer(1, self.ids.slice(from * ROW_ID_SIZE..to * ROW_ID_SIZE));
            pass.draw(0..4, 0..batch.end - batch.start);
        }
        Ok(())
    }
}

/// The instance buffer and its row id buffer, for `capacity` instances.
fn instance_buffers(device: &Device, capacity: u32) -> (Buffer, Buffer) {
    let buffer = |label, size| {
        device.create_buffer(&BufferDescriptor {
            label: Some(label),
            size: next_copy_buffer_size(capacity as u64 * size),
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    (buffer("glyphon row instances", INSTANCE_SIZE), buffer("glyphon row ids", ROW_ID_SIZE))
}

fn table_buffer(device: &Device, rows: u32) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some("glyphon row table"),
        size: rows as u64 * ROW_UNIFORM_SIZE,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}
