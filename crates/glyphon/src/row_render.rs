//! Termurion patch 6: a text renderer that keeps each row's prepared glyph instances on the GPU,
//! so a row that did not change costs no `prepare_glyph` work in the next frame. Clipping to the
//! row's bounds happens in the vertex shader (`row_shader.wgsl`), with the same integer steps as
//! `prepare_glyph`, so the pixels match `TextRenderer` byte for byte.

use crate::{
    cache::ROW_UNIFORM_SIZE,
    text_render::{
        next_copy_buffer_size, prepare_glyph, text_glyph_image, zero_depth, GlyphBounds,
        GlyphMetadata, GlyphSystem, GlyphonCacheKey,
    },
    Buffer, Color, FontSystem, GlyphKey, GlyphToRender, PrepareError, RasterizeCustomGlyphRequest,
    RasterizedCustomGlyph, RenderError, State, SwashCache, TextAtlas, TextBounds, UploadBelt, Viewport,
};
use std::{
    collections::{HashMap, HashSet},
    mem, slice,
};
use wgpu::{
    BindGroup, Buffer as GpuBuffer, BufferDescriptor, BufferUsages, Device, MultisampleState,
    Queue, RenderPass, RenderPipeline,
};

/// One row of text for [`RowRenderer::prepare`].
#[derive(Clone)]
pub struct RowArea<'a> {
    /// Names the buffer's content: give a new serial whenever the buffer is shaped again.
    pub serial: u64,
    /// The shaped text of the row.
    pub buffer: &'a Buffer,
    /// The left edge of the buffer.
    pub left: f32,
    /// The top edge of the buffer.
    pub top: f32,
    /// The visible bounds of the row.
    pub bounds: TextBounds,
    /// The color of glyphs that carry none.
    pub default_color: Color,
}

/// The smallest slot, in instances.
const MIN_SLOT: u32 = 64;
/// The instance buffer's first size, in instances.
const FIRST_CAPACITY: u32 = 4096;
/// The row table's first size, in rows.
const FIRST_TABLE_ROWS: u32 = 64;
const INSTANCE_SIZE: u64 = mem::size_of::<GlyphToRender>() as u64;

/// `RowUniform` in `row_shader.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RowUniform {
    offset: [i32; 2],
    _pad: [i32; 2],
    clip: [i32; 4],
}

const _: () = assert!(mem::size_of::<RowUniform>() as u64 == ROW_UNIFORM_SIZE);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RowKey {
    serial: u64,
    left: u32,
    color: u32,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    start: u32,
    /// The capacity is `MIN_SLOT << class`.
    class: u32,
}

struct Entry {
    /// The top the instances were made at.
    top: f32,
    /// The distinct y terms of the glyphs when every one is whole, else `None`.
    y_terms: Option<Vec<f32>>,
    instances: Vec<GlyphToRender>,
    /// Where each layout run's instances end, so a run TextRenderer would cull is left out.
    run_ends: Vec<u32>,
    keys: Vec<GlyphKey>,
    slot: Option<Slot>,
    frame: u64,
}

impl Entry {
    /// The `dy` that draws this entry at `top`, or `None` when it must be prepared again.
    fn shift_to(&self, top: f32) -> Option<i32> {
        if top.to_bits() == self.top.to_bits() {
            return Some(0);
        }
        let terms = self.y_terms.as_ref()?;
        if !(self.top >= 0.0 && top >= 0.0) {
            return None;
        }
        // physical() truncates gy + top; check each term with the same float steps so an
        // addition that rounds across a whole number never shifts wrongly.
        let dy = top.trunc() as i32 - self.top.trunc() as i32;
        let moves = |gy: f32| gy.mul_add(1.0, top).trunc() as i32 - gy.mul_add(1.0, self.top).trunc() as i32;
        terms.iter().all(|&gy| moves(gy) == dy).then_some(dy)
    }

    /// The slot range holding the instances of `runs`.
    fn drawn(&self, slot: Slot, runs: std::ops::Range<usize>) -> (u32, u32) {
        let from = if runs.start == 0 { 0 } else { self.run_ends[runs.start - 1] };
        let to = self.run_ends[runs.end - 1];
        (slot.start + from, to - from)
    }
}

fn row_key(row: &RowArea) -> RowKey {
    RowKey {
        serial: row.serial,
        left: row.left.to_bits(),
        color: row.default_color.0,
    }
}

struct Draw {
    start: u32,
    len: u32,
}

/// Draws rows of text, keeping each row's glyph instances on the GPU between frames.
pub struct RowRenderer {
    pipeline: RenderPipeline,
    instances: GpuBuffer,
    /// In instances.
    capacity: u32,
    /// The first instance no slot has used yet.
    end: u32,
    /// Free slot starts, one list per class.
    free: Vec<Vec<u32>>,
    entries: HashMap<RowKey, Vec<Entry>>,
    draws: Vec<Draw>,
    table: GpuBuffer,
    table_rows: u32,
    /// What the table holds now, one entry per drawn row.
    table_cpu: Vec<RowUniform>,
    rows_bind_group: BindGroup,
    stride: u64,
    generation: u64,
    frame: u64,
    prepared_rows: u64,
    /// Keys of entries dropped by `clear`, released at the next `prepare`.
    unreleased: Vec<GlyphKey>,
}

impl RowRenderer {
    /// Creates a new `RowRenderer`.
    pub fn new(atlas: &mut TextAtlas, device: &Device, multisample: MultisampleState) -> Self {
        let pipeline = atlas
            .cache
            .get_or_create_row_pipeline(device, atlas.format, multisample);
        let stride = (device.limits().min_uniform_buffer_offset_alignment as u64).max(ROW_UNIFORM_SIZE);
        let instances = instance_buffer(device, FIRST_CAPACITY);
        let table = table_buffer(device, FIRST_TABLE_ROWS, stride);
        let rows_bind_group = atlas.cache.create_rows_bind_group(device, &table);
        Self {
            pipeline,
            instances,
            capacity: FIRST_CAPACITY,
            end: 0,
            free: Vec::new(),
            entries: HashMap::new(),
            draws: Vec::new(),
            table,
            table_rows: FIRST_TABLE_ROWS,
            table_cpu: Vec::new(),
            rows_bind_group,
            stride,
            generation: atlas.generation(),
            frame: 0,
            prepared_rows: 0,
            unreleased: Vec::new(),
        }
    }

    /// Rows that ran `prepare_glyph` since this renderer was made (for tests).
    pub fn prepared_rows(&self) -> u64 {
        self.prepared_rows
    }

    /// Forgets every prepared row; the next frame prepares all of them again.
    pub fn clear(&mut self) {
        for (_, list) in self.entries.drain() {
            for entry in list {
                self.unreleased.extend(entry.keys);
            }
        }
        self.free.clear();
        self.end = 0;
        self.draws.clear();
    }

    /// Prepares the rows to draw, in the order they draw. Rows not given are dropped. Buffer
    /// writes go through `uploads`, whose `submit` must send them before the rows draw.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare<'a>(
        &mut self,
        device: &Device,
        queue: &Queue,
        uploads: &mut UploadBelt,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        rows: impl IntoIterator<Item = RowArea<'a>>,
        cache: &mut SwashCache,
        mut rasterize_custom_glyph: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) -> Result<(), PrepareError> {
        let rows: Vec<RowArea<'a>> = rows.into_iter().collect();
        for key in self.unreleased.drain(..) {
            atlas.release(key);
        }

        // Rows not in this frame let go of their glyphs first, so they never take atlas space the
        // new rows need (the atlas never has to hold two screens at once).
        let wanted: HashSet<RowKey> = rows.iter().map(row_key).collect();
        self.drop_entries(atlas, |key, _| !wanted.contains(key));

        let mut uniforms = Vec::with_capacity(rows.len());
        let mut retried = false;
        loop {
            // A grown atlas may have moved what the kept instances point at.
            if atlas.generation() != self.generation {
                self.drop_entries(atlas, |_, _| true);
                self.generation = atlas.generation();
            }
            self.frame += 1;
            self.draws.clear();
            uniforms.clear();
            let prepared = self.prepare_rows(
                device,
                queue,
                uploads,
                font_system,
                atlas,
                viewport,
                &rows,
                cache,
                &mut uniforms,
                &mut rasterize_custom_glyph,
            );
            if let Err(e) = prepared {
                self.draws.clear();
                // A full atlas: let go of every kept row this pass did not reach and try once more.
                if !retried {
                    retried = true;
                    let frame = self.frame;
                    self.drop_entries(atlas, |_, e| e.frame != frame);
                    continue;
                }
                return Err(e);
            }
            if atlas.generation() == self.generation {
                break;
            }
        }

        let frame = self.frame;
        self.drop_entries(atlas, |_, e| e.frame != frame);
        self.write_table(device, uploads, atlas, uniforms);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_rows(
        &mut self,
        device: &Device,
        queue: &Queue,
        uploads: &mut UploadBelt,
        font_system: &mut FontSystem,
        atlas: &mut TextAtlas,
        viewport: &Viewport,
        rows: &[RowArea],
        cache: &mut SwashCache,
        uniforms: &mut Vec<RowUniform>,
        rasterize_custom_glyph: &mut impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
    ) -> Result<(), PrepareError> {
        let state = State { device, queue };
        let resolution = viewport.resolution();

        for row in rows {
            // The layout runs TextRenderer would draw: it skips runs whose line box misses the
            // unclamped bounds, then draws while they meet them.
            let visible = |run: &cosmic_text::LayoutRun| {
                let start = (row.top + run.line_top) as i32;
                let end = start + run.line_height as i32;
                start <= row.bounds.bottom && row.bounds.top <= end
            };
            let first = row.buffer.layout_runs().take_while(|r| !visible(r)).count();
            let shown = row.buffer.layout_runs().skip(first).take_while(visible).count();
            if shown == 0 {
                continue;
            }
            let runs = first..first + shown;

            let bounds = GlyphBounds::clamped(row.bounds, resolution);
            let clip = [bounds.x.min, bounds.y.min, bounds.x.max, bounds.y.max];
            let key = row_key(row);

            let frame = self.frame;
            let found = self.entries.get_mut(&key).and_then(|list| {
                let exact = list.iter().position(|e| e.top.to_bits() == row.top.to_bits());
                let i = exact.or_else(|| list.iter().position(|e| e.shift_to(row.top).is_some()))?;
                let entry = &mut list[i];
                entry.frame = frame;
                let dy = entry.shift_to(row.top)?;
                Some((dy, entry.slot.map(|s| entry.drawn(s, runs.clone()))))
            });

            let (dy, drawn) = match found {
                Some(hit) => hit,
                None => {
                    let mut system = GlyphSystem {
                        atlas: &mut *atlas,
                        cache: &mut *cache,
                        font_system: &mut *font_system,
                    };
                    let mut entry = prepare_row(&state, &mut system, row, &mut *rasterize_custom_glyph)?;
                    entry.frame = frame;
                    self.prepared_rows += 1;
                    for &k in &entry.keys {
                        atlas.hold(k);
                    }
                    if !entry.instances.is_empty() {
                        let slot = self.alloc(device, uploads, entry.instances.len() as u32);
                        uploads.write(
                            device,
                            &self.instances,
                            slot.start as u64 * INSTANCE_SIZE,
                            instance_bytes(&entry.instances),
                        );
                        entry.slot = Some(slot);
                    }
                    let drawn = entry.slot.map(|s| entry.drawn(s, runs.clone()));
                    self.entries.entry(key).or_default().push(entry);
                    (0, drawn)
                }
            };

            if let Some((start, len)) = drawn.filter(|&(_, len)| len > 0) {
                self.draws.push(Draw { start, len });
                uniforms.push(RowUniform {
                    offset: [0, dy],
                    _pad: [0, 0],
                    clip,
                });
            }
        }
        Ok(())
    }

    /// Takes a slot for `len` instances, doubling the buffer when none is free.
    fn alloc(&mut self, device: &Device, uploads: &mut UploadBelt, len: u32) -> Slot {
        let class = (len.max(MIN_SLOT).next_power_of_two() / MIN_SLOT).trailing_zeros();
        if let Some(start) = self.free.get_mut(class as usize).and_then(Vec::pop) {
            return Slot { start, class };
        }
        let size = MIN_SLOT << class;
        if self.end + size > self.capacity {
            let mut capacity = self.capacity * 2;
            while self.end + size > capacity {
                capacity *= 2;
            }
            // Dropped, not destroyed: copies recorded this frame may still target it.
            self.instances = instance_buffer(device, capacity);
            self.capacity = capacity;
            // The new buffer starts empty: write every live row from its CPU copy.
            for entry in self.entries.values().flatten() {
                if let Some(slot) = entry.slot {
                    uploads.write(
                        device,
                        &self.instances,
                        slot.start as u64 * INSTANCE_SIZE,
                        instance_bytes(&entry.instances),
                    );
                }
            }
        }
        let start = self.end;
        self.end += size;
        Slot { start, class }
    }

    /// Drops the entries `drop` picks, releasing their glyphs and freeing their slots.
    fn drop_entries(&mut self, atlas: &mut TextAtlas, drop: impl Fn(&RowKey, &Entry) -> bool) {
        let free = &mut self.free;
        self.entries.retain(|key, list| {
            list.retain_mut(|entry| {
                if !drop(key, entry) {
                    return true;
                }
                for &k in &entry.keys {
                    atlas.release(k);
                }
                if let Some(slot) = entry.slot {
                    let class = slot.class as usize;
                    if free.len() <= class {
                        free.resize_with(class + 1, Vec::new);
                    }
                    free[class].push(slot.start);
                }
                false
            });
            !list.is_empty()
        });
    }

    /// Writes the row table where an entry changed, growing it when it is too small.
    fn write_table(
        &mut self,
        device: &Device,
        uploads: &mut UploadBelt,
        atlas: &TextAtlas,
        uniforms: Vec<RowUniform>,
    ) {
        if uniforms.len() as u32 > self.table_rows {
            let rows = (uniforms.len() as u32).next_power_of_two();
            self.table = table_buffer(device, rows, self.stride);
            self.table_rows = rows;
            self.rows_bind_group = atlas.cache.create_rows_bind_group(device, &self.table);
            self.table_cpu.clear();
        }

        // Write each run of changed entries with one call.
        let stride = self.stride as usize;
        let mut i = 0;
        while i < uniforms.len() {
            if self.table_cpu.get(i) == Some(&uniforms[i]) {
                i += 1;
                continue;
            }
            let first = i;
            while i < uniforms.len() && self.table_cpu.get(i) != Some(&uniforms[i]) {
                i += 1;
            }
            let mut bytes = vec![0u8; (i - first - 1) * stride + ROW_UNIFORM_SIZE as usize];
            for (n, u) in uniforms[first..i].iter().enumerate() {
                let raw = unsafe {
                    slice::from_raw_parts(u as *const RowUniform as *const u8, ROW_UNIFORM_SIZE as usize)
                };
                bytes[n * stride..n * stride + raw.len()].copy_from_slice(raw);
            }
            uploads.write(device, &self.table, (first * stride) as u64, &bytes);
        }
        self.table_cpu = uniforms;
    }

    /// Draws the rows given to the last `prepare`, in that order.
    pub fn render(
        &self,
        atlas: &TextAtlas,
        viewport: &Viewport,
        pass: &mut RenderPass<'_>,
    ) -> Result<(), RenderError> {
        if self.draws.is_empty() {
            return Ok(());
        }
        debug_assert_eq!(
            atlas.generation(),
            self.generation,
            "the atlas grew after the rows were prepared"
        );
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &atlas.bind_group, &[]);
        pass.set_bind_group(1, &viewport.bind_group, &[]);
        for (i, draw) in self.draws.iter().enumerate() {
            pass.set_bind_group(2, &self.rows_bind_group, &[(i as u64 * self.stride) as u32]);
            let from = draw.start as u64 * INSTANCE_SIZE;
            let to = from + draw.len as u64 * INSTANCE_SIZE;
            pass.set_vertex_buffer(0, self.instances.slice(from..to));
            pass.draw(0..4, 0..draw.len);
        }
        Ok(())
    }
}

/// Makes a row's instances as `TextRenderer` does, at scale 1, with no clipping and no run culling.
fn prepare_row(
    state: &State,
    system: &mut GlyphSystem,
    row: &RowArea,
    rasterize_custom_glyph: &mut impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
) -> Result<Entry, PrepareError> {
    let mut instances = Vec::new();
    let mut run_ends = Vec::new();
    let mut keys = Vec::new();
    let mut y_terms: Option<Vec<f32>> = Some(Vec::new());

    for run in row.buffer.layout_runs() {
        for glyph in run.glyphs.iter() {
            // The y term physical() truncates after adding top.
            let gy = glyph.y - glyph.font_size * glyph.y_offset;
            if let Some(terms) = &mut y_terms {
                if gy.fract() != 0.0 {
                    y_terms = None;
                } else if !terms.iter().any(|t| t.to_bits() == gy.to_bits()) {
                    terms.push(gy);
                }
            }

            let physical = glyph.physical((row.left, row.top), 1.0);
            let color = glyph.color_opt.unwrap_or(row.default_color);
            let cache_key = GlyphonCacheKey::Text(physical.cache_key);

            if let Some(instance) = prepare_glyph(
                state,
                system,
                GlyphMetadata {
                    x: physical.x,
                    y: physical.y,
                    line_y: run.line_y,
                    color,
                    metadata: glyph.metadata,
                    cache_key,
                    scale_factor: 1.0,
                },
                GlyphBounds::UNBOUNDED,
                |system, _| text_glyph_image(system, physical.cache_key),
                zero_depth,
                &mut *rasterize_custom_glyph,
            )? {
                instances.push(instance);
                keys.push(GlyphKey(cache_key));
            }
        }
        run_ends.push(instances.len() as u32);
    }

    Ok(Entry {
        top: row.top,
        y_terms,
        instances,
        run_ends,
        keys,
        slot: None,
        frame: 0,
    })
}

fn instance_bytes(instances: &[GlyphToRender]) -> &[u8] {
    unsafe { slice::from_raw_parts(instances.as_ptr() as *const u8, mem::size_of_val(instances)) }
}

fn instance_buffer(device: &Device, capacity: u32) -> GpuBuffer {
    device.create_buffer(&BufferDescriptor {
        label: Some("glyphon row instances"),
        size: next_copy_buffer_size(capacity as u64 * INSTANCE_SIZE),
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn table_buffer(device: &Device, rows: u32, stride: u64) -> GpuBuffer {
    device.create_buffer(&BufferDescriptor {
        label: Some("glyphon row table"),
        size: rows as u64 * stride,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Attrs, Cache, ContentType, CustomGlyph, Family, Metrics, Resolution, Shaping, TextArea, TextRenderer};
    use wgpu::TextureFormat;

    const W: u32 = 320;
    const H: u32 = 240;
    const FORMAT: TextureFormat = TextureFormat::Rgba8UnormSrgb;
    const MONO: &str = "Termurion Wide Bold Test";
    const RTL: &str = "Termurion RTL Test";

    fn no_custom_glyph(_: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
        None
    }

    struct Rig {
        device: Device,
        queue: Queue,
        cache: Cache,
        viewport: Viewport,
        font_system: FontSystem,
        swash: SwashCache,
        atlas: TextAtlas,
        rows: RowRenderer,
        uploads: UploadBelt,
    }

    impl Rig {
        fn new() -> Self {
            let (device, queue) = crate::test_gpu::shared();
            let cache = Cache::new(&device);
            let mut viewport = Viewport::new(&device, &cache);
            viewport.update(&queue, Resolution { width: W, height: H });
            let mut font_system = FontSystem::new_with_fonts([]);
            for data in [
                &include_bytes!("../../../tests/fixtures/fonts/TermurionWideBoldTest-Regular.ttf")[..],
                &include_bytes!("../../../tests/fixtures/fonts/TermurionRtlTest-Regular.ttf")[..],
            ] {
                font_system.db_mut().load_font_data(data.to_vec());
            }
            let mut atlas = TextAtlas::new(&device, &queue, &cache, FORMAT);
            let rows = RowRenderer::new(&mut atlas, &device, MultisampleState::default());
            Self {
                device,
                queue,
                cache,
                viewport,
                font_system,
                swash: SwashCache::new(),
                atlas,
                rows,
                uploads: UploadBelt::new(),
            }
        }

        fn shape(&mut self, text: &str, family: &str) -> Buffer {
            let mut buffer = Buffer::new(&mut self.font_system, Metrics::new(16.0, 20.0));
            buffer.set_text(text, &Attrs::new().family(Family::Name(family)), Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            buffer
        }

        fn prepare_rows<'a>(&mut self, rows: impl IntoIterator<Item = RowArea<'a>>) {
            self.prepare_rows_with(rows, no_custom_glyph);
        }

        fn prepare_rows_with<'a>(
            &mut self,
            rows: impl IntoIterator<Item = RowArea<'a>>,
            rasterize: impl FnMut(RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph>,
        ) {
            self.rows
                .prepare(
                    &self.device,
                    &self.queue,
                    &mut self.uploads,
                    &mut self.font_system,
                    &mut self.atlas,
                    &self.viewport,
                    rows,
                    &mut self.swash,
                    rasterize,
                )
                .unwrap();
            self.uploads.submit(&self.queue, []);
        }

        /// Prepares and draws the rows with this rig's `RowRenderer`.
        fn draw_rows<'a>(&mut self, rows: impl IntoIterator<Item = RowArea<'a>>) -> Vec<u8> {
            self.prepare_rows(rows);
            let (rows, atlas, viewport) = (&self.rows, &self.atlas, &self.viewport);
            draw(&self.device, &self.queue, |pass| rows.render(atlas, viewport, pass).unwrap())
        }

        /// Draws the same rows with a fresh `TextRenderer` and atlas, the path rows must match.
        fn draw_text<'a>(&mut self, rows: impl IntoIterator<Item = RowArea<'a>>) -> Vec<u8> {
            let mut atlas = TextAtlas::new(&self.device, &self.queue, &self.cache, FORMAT);
            let mut text = TextRenderer::new(&mut atlas, &self.device, MultisampleState::default(), None);
            let areas = rows.into_iter().map(|r| TextArea {
                buffer: r.buffer,
                left: r.left,
                top: r.top,
                scale: 1.0,
                bounds: r.bounds,
                default_color: r.default_color,
                custom_glyphs: &[],
            });
            text.prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut atlas,
                &self.viewport,
                areas,
                &mut self.swash,
            )
            .unwrap();
            let viewport = &self.viewport;
            draw(&self.device, &self.queue, |pass| text.render(&atlas, viewport, pass).unwrap())
        }
    }

    /// Renders into a cleared offscreen target and reads it back.
    fn draw(device: &Device, queue: &Queue, record: impl FnOnce(&mut RenderPass<'_>)) -> Vec<u8> {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.05, b: 0.1, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            record(&mut pass);
        }
        let out = device.create_buffer(&BufferDescriptor {
            label: None,
            size: (W * H * 4) as u64,
            usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(W * 4),
                    rows_per_image: Some(H),
                },
            },
            wgpu::Extent3d { width: W, height: H, depth_or_array_layers: 1 },
        );
        queue.submit(Some(enc.finish()));
        out.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let bytes = out.slice(..).get_mapped_range().unwrap().to_vec();
        bytes
    }

    fn row(serial: u64, buffer: &Buffer, left: f32, top: f32, bounds: TextBounds) -> RowArea<'_> {
        RowArea {
            serial,
            buffer,
            left,
            top,
            bounds,
            default_color: Color::rgb(230, 200 - serial as u8 * 10, 120 + serial as u8 * 9),
        }
    }

    fn whole() -> TextBounds {
        TextBounds { left: 0, top: 0, right: W as i32, bottom: H as i32 }
    }

    fn b(left: i32, top: i32, right: i32, bottom: i32) -> TextBounds {
        TextBounds { left, top, right, bottom }
    }

    fn assert_same(rows: &[u8], text: &[u8], what: &str) {
        let blank = draw(&crate::test_gpu::shared().0, &crate::test_gpu::shared().1, |_| {});
        assert_ne!(text, &blank[..], "{what}: the text path drew nothing");
        if let Some(i) = rows.iter().zip(text).position(|(a, b)| a != b) {
            let px = i / 4;
            panic!("{what}: first difference at x {} y {}", px as u32 % W, px as u32 / W);
        }
    }

    #[test]
    fn rows_draw_like_the_text_renderer() {
        let mut rig = Rig::new();
        let texts = [
            ("Clipped on the left side here", MONO),
            ("Clipped on the right side here", MONO),
            ("Top cut through the glyphs", MONO),
            ("Bottom cut through gyphs jq", MONO),
            ("Bounds past every viewport edge", MONO),
            ("Starts left of the viewport edge", MONO),
            ("Hangs below the viewport bottom", MONO),
            ("\u{628}\u{64e}\u{628}\u{650} fractional tops", RTL),
            ("Box cut on all four sides gjy", MONO),
            ("Zero-size bounds draw nothing", MONO),
        ];
        let buffers: Vec<Buffer> = texts.iter().map(|(t, f)| rig.shape(t, f)).collect();
        let placed = [
            (4.0, 2.0, b(37, -5, 400, 400)),
            (4.0, 22.0, b(0, 0, 151, 400)),
            (4.0, 47.0, b(0, 53, 400, 400)),
            (9.25, 64.0, b(0, 0, 400, 77)),
            (2.0, 85.0, b(-100, -100, 1000, 1000)),
            (-31.5, 106.0, whole()),
            (6.0, 228.0, whole()),
            (40.0, 140.5, whole()),
            (12.0, 170.0, b(30, 175, 140, 184)),
            (12.0, 195.0, b(50, 200, 50, 200)),
        ];
        let areas = || {
            buffers
                .iter()
                .zip(placed)
                .enumerate()
                .map(|(i, (buf, (left, top, bounds)))| row(i as u64, buf, left, top, bounds))
        };
        let rows = rig.draw_rows(areas());
        let text = rig.draw_text(areas());
        assert_same(&rows, &text, "first frame");
        // The second frame draws every row from its kept instances.
        let rows = rig.draw_rows(areas());
        assert_eq!(rig.rows.prepared_rows(), 10);
        assert_same(&rows, &text, "kept rows");
    }

    #[test]
    fn a_moved_whole_row_is_not_prepared_again() {
        for top in [10.0, 10.25] {
            let mut rig = Rig::new();
            let buf = rig.shape("Moves down by whole pixels", MONO);
            rig.draw_rows([row(1, &buf, 3.0, top, whole())]);
            assert_eq!(rig.rows.prepared_rows(), 1);
            let moved = rig.draw_rows([row(1, &buf, 3.0, top + 20.0, whole())]);
            assert_eq!(rig.rows.prepared_rows(), 1, "top {top}: the row was prepared again");
            let fresh = rig.draw_text([row(1, &buf, 3.0, top + 20.0, whole())]);
            assert_same(&moved, &fresh, "moved row");
        }
    }

    #[test]
    fn a_row_moved_by_half_a_pixel_is_prepared_again() {
        let mut rig = Rig::new();
        // Arabic marks sit at a fractional y, so the truncation can move them apart.
        let buf = rig.shape("\u{628}\u{64e}\u{628}\u{650}\u{628}\u{64e}", RTL);
        rig.draw_rows([row(1, &buf, 3.0, 10.0, whole())]);
        let moved = rig.draw_rows([row(1, &buf, 3.0, 10.5, whole())]);
        assert_eq!(rig.rows.prepared_rows(), 2);
        let fresh = rig.draw_text([row(1, &buf, 3.0, 10.5, whole())]);
        assert_same(&moved, &fresh, "half-pixel move");
    }

    #[test]
    fn rows_not_drawn_release_their_glyphs() {
        let mut rig = Rig::new();
        let a = rig.shape("Held while drawn", MONO);
        let c = rig.shape("\u{628}\u{64e}\u{628}\u{650}", RTL);
        rig.prepare_rows([row(1, &a, 0.0, 0.0, whole()), row(2, &c, 0.0, 20.0, whole())]);
        assert!(rig.atlas.held() > 0);
        rig.prepare_rows([]);
        assert_eq!(rig.atlas.held(), 0);
        // Keys dropped by clear are released at the next prepare.
        rig.prepare_rows([row(1, &a, 0.0, 0.0, whole())]);
        assert!(rig.atlas.held() > 0);
        rig.rows.clear();
        rig.prepare_rows([]);
        assert_eq!(rig.atlas.held(), 0);
    }

    #[test]
    fn growth_prepares_every_row_again() {
        let mut rig = Rig::new();
        let first = rig.shape("The first row stays the same", MONO);
        rig.draw_rows([row(1, &first, 0.0, 0.0, whole())]);
        assert_eq!(rig.rows.prepared_rows(), 1);
        let size = rig.atlas.mask_size();
        // Large glyphs fill the 256 px atlas; every row holds them, so it must grow.
        let mut big = Buffer::new(&mut rig.font_system, Metrics::new(90.0, 100.0));
        big.set_text(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
            &Attrs::new().family(Family::Name(MONO)),
            Shaping::Advanced,
            None,
        );
        big.shape_until_scroll(&mut rig.font_system, false);
        let generation = rig.atlas.generation();
        let areas = || [row(1, &first, 0.0, 0.0, whole()), row(2, &big, 0.0, 20.0, whole())];
        let rows = rig.draw_rows(areas());
        assert!(rig.atlas.generation() > generation, "the atlas did not grow");
        assert!(rig.atlas.mask_size() > size);
        // The kept first row is reused in the pass that grew, then both rows are prepared again.
        assert_eq!(rig.rows.prepared_rows(), 1 + 1 + 2);
        let text = rig.draw_text(areas());
        assert_same(&rows, &text, "after growth");
    }

    #[test]
    fn many_rows_grow_the_buffers_and_scroll_from_kept_instances() {
        let mut rig = Rig::new();
        let buffers: Vec<Buffer> = (0..100)
            .map(|n| rig.shape(&format!("{n:03} the row text that fills forty-four cells"), MONO))
            .collect();
        let areas = |up: usize| {
            buffers.iter().enumerate().skip(up).map(move |(n, buf)| {
                row(n as u64 % 10, buf, 1.0, (n - up) as f32 * 2.4 + 0.25, whole())
            })
        };
        // Distinct serials, but the same colors as `row` makes them.
        let areas = |up: usize| {
            areas(up).enumerate().map(move |(i, mut r)| {
                r.serial = (i + up) as u64 + 100;
                r
            })
        };
        let rows = rig.draw_rows(areas(0));
        assert!(rig.rows.capacity > FIRST_CAPACITY, "the instance buffer did not double");
        assert!(rig.rows.table_rows > FIRST_TABLE_ROWS, "the row table did not grow");
        let text = rig.draw_text(areas(0));
        assert_same(&rows, &text, "100 rows");
        let rows = rig.draw_rows(areas(3));
        assert_eq!(rig.rows.prepared_rows(), 100, "scrolled rows were prepared again");
        let text = rig.draw_text(areas(3));
        assert_same(&rows, &text, "scrolled 3 up");
    }

    /// A new row every frame, the others moving up: after a warm-up the belt reuses its chunks
    /// and the rows still match the text path.
    #[test]
    fn streaming_rows_reuse_the_upload_chunks() {
        let mut rig = Rig::new();
        let buffers: Vec<Buffer> = (0..80).map(|n| rig.shape(&format!("{n:03} a streamed line"), MONO)).collect();
        let areas = |first: usize| (first..first + 8).map(|n| RowArea { serial: n as u64 + 1000, ..row(1, &buffers[n], 1.0, (n - first) as f32 * 20.0, whole()) }).collect::<Vec<_>>();
        for first in 0..20 {
            rig.draw_rows(areas(first));
        }
        let warm = rig.uploads.buffers_made();
        assert!(warm > 0, "nothing went through the belt");
        for first in 20..70 {
            rig.draw_rows(areas(first));
        }
        assert_eq!(rig.uploads.buffers_made(), warm, "streaming rows made new staging buffers");
        let rows = rig.draw_rows(areas(70));
        let text = rig.draw_text(areas(70));
        assert_same(&rows, &text, "streamed rows");
    }

    #[test]
    fn a_released_slot_is_used_again() {
        let mut rig = Rig::new();
        let a = rig.shape("First row of text", MONO);
        let c = rig.shape("Second row of text", MONO);
        let d = rig.shape("Third row of text", MONO);
        rig.prepare_rows([row(1, &a, 0.0, 0.0, whole()), row(2, &c, 0.0, 20.0, whole())]);
        let end = rig.rows.end;
        rig.prepare_rows([row(2, &c, 0.0, 20.0, whole())]);
        rig.prepare_rows([row(2, &c, 0.0, 20.0, whole()), row(3, &d, 0.0, 40.0, whole())]);
        assert_eq!(rig.rows.end, end, "the new row took a fresh slot instead of the freed one");
        let rows = rig.draw_rows([row(2, &c, 0.0, 20.0, whole()), row(3, &d, 0.0, 40.0, whole())]);
        let text = rig.draw_text([row(2, &c, 0.0, 20.0, whole()), row(3, &d, 0.0, 40.0, whole())]);
        assert_same(&rows, &text, "reused slot");
    }

    /// The last frame's rows must not pin a full atlas: TextRenderer draws each of these frames,
    /// so the row renderer must too.
    #[test]
    fn rows_from_last_frame_never_pin_a_full_atlas() {
        let mut rig = Rig::new();
        rig.atlas.set_max_size(256);
        let big = |rig: &mut Rig, text: &str| {
            let mut buf = Buffer::new(&mut rig.font_system, Metrics::new(100.0, 110.0));
            buf.set_text(text, &Attrs::new().family(Family::Name(MONO)), Shaping::Advanced, None);
            buf.shape_until_scroll(&mut rig.font_system, false);
            buf
        };
        let first = big(&mut rig, "abcdefghi");
        let second = big(&mut rig, "nopqrsuvx");
        rig.draw_rows([row(1, &first, 0.0, 0.0, whole())]);
        rig.atlas.trim();
        let held = rig.atlas.held();
        for frame in 1..4 {
            let rows = rig.draw_rows([row(2, &second, 0.0, 0.0, whole())]);
            rig.atlas.trim();
            assert_eq!(rig.atlas.held(), held, "frame {frame}: holds piled up");
            let text = rig.draw_text([row(2, &second, 0.0, 0.0, whole())]);
            assert_same(&rows, &text, "after the swap");
        }
        assert_eq!(rig.atlas.generation(), 0, "the atlas grew past its cap");
    }

    /// A row whose line box misses its bounds is skipped, like TextRenderer skips the run, even
    /// where its glyphs reach into the bounds.
    #[test]
    fn a_line_outside_its_bounds_is_culled() {
        let mut rig = Rig::new();
        // Glyphs twice the line height overhang the line box above and below.
        let mut tall = Buffer::new(&mut rig.font_system, Metrics::new(32.0, 10.0));
        tall.set_text("Tall gjy glyphs", &Attrs::new().family(Family::Name(MONO)), Shaping::Advanced, None);
        tall.shape_until_scroll(&mut rig.font_system, false);
        let shown = rig.shape("A row inside its bounds", MONO);
        let pane = b(0, 60, W as i32, 120);
        let areas = |bounds| [row(1, &shown, 4.0, 80.0, whole()), row(2, &tall, 4.0, 48.5, bounds), row(3, &tall, 4.0, 121.5, bounds)];
        // The overhang reaches into the pane when nothing clips it.
        let unclipped = rig.draw_text(areas(whole()));
        let control = rig.draw_text([row(1, &shown, 4.0, 80.0, whole())]);
        let px = |y: i32| (y * W as i32 * 4) as usize..((y + 1) * W as i32 * 4) as usize;
        assert!((60..120).any(|y| unclipped[px(y)] != control[px(y)]), "no overhang reaches the pane");
        let rows = rig.draw_rows(areas(pane));
        let text = rig.draw_text(areas(pane));
        assert_same(&rows, &text, "culled lines");
        assert_eq!(rig.rows.prepared_rows(), 1, "a culled row was prepared");
    }

    fn solid(req: RasterizeCustomGlyphRequest) -> Option<RasterizedCustomGlyph> {
        Some(RasterizedCustomGlyph {
            data: vec![255; req.width as usize * req.height as usize],
            content_type: ContentType::Mask,
        })
    }

    /// Growth from the rows re-uploads every cached glyph, custom ones too, so it needs the
    /// caller's rasterizer (a box glyph cached by TextRenderer used to panic here).
    #[test]
    fn growth_from_the_rows_keeps_custom_glyphs() {
        let mut rig = Rig::new();
        let blank = rig.shape("", MONO);
        let glyphs = [CustomGlyph {
            id: 7,
            width: 12.0,
            height: 12.0,
            snap_to_physical_pixel: true,
            ..Default::default()
        }];
        let mut text = TextRenderer::new(&mut rig.atlas, &rig.device, MultisampleState::default(), None);
        let area = TextArea {
            buffer: &blank,
            left: 0.0,
            top: 0.0,
            scale: 1.0,
            bounds: whole(),
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: &glyphs,
        };
        text.prepare_with_custom(&rig.device, &rig.queue, &mut rig.font_system, &mut rig.atlas, &rig.viewport, [area], &mut rig.swash, solid)
            .unwrap();
        let mut big = Buffer::new(&mut rig.font_system, Metrics::new(90.0, 100.0));
        big.set_text(
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz",
            &Attrs::new().family(Family::Name(MONO)),
            Shaping::Advanced,
            None,
        );
        big.shape_until_scroll(&mut rig.font_system, false);
        let generation = rig.atlas.generation();
        rig.prepare_rows_with([row(1, &big, 0.0, 0.0, whole())], solid);
        assert!(rig.atlas.generation() > generation, "the atlas did not grow");
    }

    /// A kept row that is wanted but culled still holds its glyphs; a full atlas must let them go
    /// and try again rather than fail.
    #[test]
    fn a_full_atlas_is_retried_after_dropping_unreached_rows() {
        let mut rig = Rig::new();
        rig.atlas.set_max_size(256);
        let big = |rig: &mut Rig, text: &str| {
            let mut buf = Buffer::new(&mut rig.font_system, Metrics::new(100.0, 110.0));
            buf.set_text(text, &Attrs::new().family(Family::Name(MONO)), Shaping::Advanced, None);
            buf.shape_until_scroll(&mut rig.font_system, false);
            buf
        };
        let first = big(&mut rig, "abcdefghi");
        let second = big(&mut rig, "nopqrsuvx");
        rig.draw_rows([row(1, &first, 0.0, 0.0, whole())]);
        rig.atlas.trim();
        let off = b(0, 500, W as i32, 600);
        let areas = || [row(1, &first, 0.0, 0.0, off), row(2, &second, 0.0, 0.0, whole())];
        let rows = rig.draw_rows(areas());
        let text = rig.draw_text(areas());
        assert_same(&rows, &text, "after the retry");
        assert_eq!(rig.atlas.generation(), 0, "the atlas grew past its cap");
    }
}
