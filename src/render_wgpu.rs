//! The wgpu backend: runs a `DrawList` with the quad and image pipelines and glyphon.

use glyphon::{Buffer, Color, ContentType, FontSystem, RasterizeCustomGlyphRequest, RasterizedCustomGlyph, Resolution, RowArea, RowRenderer, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport};
use wgpu::util::DeviceExt;

use crate::draw_list::{Bounds, BufferWrite, DrawList, ImageDraw, QuadBuf, Shadow, Uploads, IMAGE_STRIDE, QUAD_STRIDE};

/// Rectangles: two triangles each, position and color per vertex.
struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    /// Scales what is already in the target by the blend constant (the fade pass).
    fader: wgpu::RenderPipeline,
    /// The fade pass for a target with no non-sRGB view (GL): the target's own format. Built on
    /// the first such fade, so other backends never compile it.
    fader_view: Option<wgpu::RenderPipeline>,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl QuadPipeline {
    /// The fade pass on the target's own (sRGB) view, scaling in linear space.
    fn fade_for_view(&self, device: &wgpu::Device) -> wgpu::RenderPipeline {
        let scale = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Constant, operation: wgpu::BlendOperation::Add };
        Self::make(device, &self.shader, &self.pipeline_layout, "quad fade view", self.format, wgpu::BlendState { color: scale, alpha: scale })
    }

    /// One quad pipeline writing `format` with `blend`.
    fn make(device: &wgpu::Device, shader: &wgpu::ShaderModule, pipeline_layout: &wgpu::PipelineLayout, label: &'static str, format: wgpu::TextureFormat, blend: wgpu::BlendState) -> wgpu::RenderPipeline {
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState {
                module: shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: QUAD_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(blend),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("quad"),
            source: wgpu::ShaderSource::Wgsl(include_str!("quad.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quad screen"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            ..Default::default()
        });
        let make = |label: &'static str, format: wgpu::TextureFormat, blend: wgpu::BlendState| Self::make(device, &shader, &pipeline_layout, label, format, blend);
        let pipeline = make("quad", format, wgpu::BlendState::ALPHA_BLENDING);
        // result = destination * constant, in every channel: a premultiplied target scaled whole.
        let scale = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Zero, dst_factor: wgpu::BlendFactor::Constant, operation: wgpu::BlendOperation::Add };
        // Run on the target's non-sRGB view: the stored values are what DirectComposition reads as
        // premultiplied, so they are scaled as stored (an sRGB view would scale in linear space and
        // leave the color above the alpha).
        let fade_blend = wgpu::BlendState { color: scale, alpha: scale };
        let fader = make("quad fade", format.remove_srgb_suffix(), fade_blend);
        QuadPipeline { pipeline, fader, fader_view: None, shader, pipeline_layout, format, uniform, bind_group }
    }
}

/// Inline images: textured rectangles, one texture per picture.
struct ImagePipeline {
    pipeline: wgpu::RenderPipeline,
    screen_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Tiled background images repeat.
    repeat_sampler: wgpu::Sampler,
    /// Textures by picture (and whether it repeats), with the frame count they were last
    /// drawn at.
    cache: std::collections::HashMap<((u64, usize), bool), (wgpu::BindGroup, u64)>,
    uploads: usize,
    frames: u64,
}

/// A texture no frame drew for this many frames is dropped.
const TEXTURE_KEEP: u64 = 120;

impl ImagePipeline {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat, uniform: &wgpu::Buffer) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("image"),
            source: wgpu::ShaderSource::Wgsl(include_str!("image.wgsl").into()),
        });
        let screen_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let screen_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &screen_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let repeat_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            ..Default::default()
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&screen_layout), Some(&texture_layout)],
            ..Default::default()
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("image"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: IMAGE_STRIDE,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attributes,
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        });
        ImagePipeline { pipeline, screen_group, texture_layout, sampler, repeat_sampler, cache: Default::default(), uploads: 0, frames: 0 }
    }

    /// Draws `list` from `vertices`; a picture with no texture (too big, bad data) is skipped.
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, vertices: wgpu::BufferSlice<'_>, list: &[ImageDraw]) {
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.screen_group, &[]);
        pass.set_vertex_buffer(0, vertices);
        for d in list {
            let Some((group, _)) = self.cache.get(&(d.key, d.repeat)) else { continue };
            pass.set_bind_group(1, group, &[]);
            pass.draw(d.vertices.clone(), 0..1);
        }
    }

    /// Makes sure every picture in `images` has a texture; drops ones long unused.
    fn prepare<'a>(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, images: impl IntoIterator<Item = &'a ImageDraw>) {
        self.frames += 1;
        for q in images {
            if let Some(entry) = self.cache.get_mut(&(q.key, q.repeat)) {
                entry.1 = self.frames;
                continue;
            }
            let (w, h) = q.size;
            let max = device.limits().max_texture_dimension_2d;
            if w == 0 || h == 0 || w > max || h > max || q.pixels.len() != (w as usize) * (h as usize) * 4 {
                continue;
            }
            let texture = device.create_texture_with_data(
                queue,
                &wgpu::TextureDescriptor {
                    label: Some("image"),
                    size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &q.pixels,
            );
            let view = texture.create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(if q.repeat { &self.repeat_sampler } else { &self.sampler }) },
                ],
            });
            self.cache.insert((q.key, q.repeat), (group, self.frames));
            self.uploads += 1;
        }
        let now = self.frames;
        self.cache.retain(|_, (_, used)| now - *used <= TEXTURE_KEEP);
    }
}

/// One persistent vertex buffer on the GPU; `Shadow` on the CPU decides what to write.
struct GpuVec {
    buf: wgpu::Buffer,
}

impl GpuVec {
    fn new(device: &wgpu::Device) -> GpuVec {
        GpuVec { buf: Self::alloc(device, Shadow::START) }
    }

    fn alloc(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor { label: None, size: cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false })
    }
}

/// What glyphon's prepare reads from the CPU side: the shaped rows (by `RowDraw::slot`), the font
/// system and swash cache, the empty buffer box glyph areas carry, and the box glyph ids.
pub struct TextInputs<'a> {
    pub font_system: &'a mut FontSystem,
    pub swash: &'a mut SwashCache,
    pub buffers: &'a [crate::render::Shaped],
    pub blank: &'a Buffer,
    pub custom: &'a crate::render::CustomIds,
}

/// The GPU half of the renderer: pipelines, the glyph atlas and text renderers, the persistent
/// vertex buffers and the upload belt every buffer write goes through.
pub struct WgpuDraw {
    quads: QuadPipeline,
    images: ImagePipeline,
    pub(crate) atlas: TextAtlas,
    viewport: Viewport,
    text: TextRenderer,
    /// Draws the text rows, keeping each row's prepared glyphs on the GPU between frames.
    rows: RowRenderer,
    /// Every buffer write of a frame goes up through this, ahead of the frame's commands.
    pub(crate) uploads: glyphon::UploadBelt,
    /// What the quad uniform holds, so it is written only when it changes.
    quad_uniform: Option<[f32; 4]>,
    bufs: [GpuVec; 6],
    format: wgpu::TextureFormat,
}

impl WgpuDraw {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> WgpuDraw {
        let cache = glyphon::Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let rows = RowRenderer::new(&mut atlas, device, wgpu::MultisampleState::default());
        let quads = QuadPipeline::new(device, format);
        let images = ImagePipeline::new(device, format, &quads.uniform);
        WgpuDraw {
            quads,
            images,
            atlas,
            viewport,
            text,
            rows,
            uploads: glyphon::UploadBelt::new(),
            quad_uniform: None,
            bufs: QuadBuf::ALL.map(|_| GpuVec::new(device)),
            format,
        }
    }

    /// The texture format the pipelines draw into.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// How many rows the row renderer prepared since it was made.
    pub fn row_prepares(&self) -> u64 {
        self.rows.prepared_rows()
    }

    /// How many image textures were uploaded.
    pub fn image_uploads(&self) -> usize {
        self.images.uploads
    }

    /// Staging buffers the upload path made so far (tests).
    pub fn upload_buffers_made(&self) -> u64 {
        self.uploads.buffers_made()
    }

    /// Lets go of every glyph the kept rows hold (the row path turned off). With no rows nothing
    /// is allocated, so this cannot fail.
    pub fn release_rows(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, font_system: &mut FontSystem, swash: &mut SwashCache) {
        self.rows.clear();
        let released = self.rows.prepare(device, queue, &mut self.uploads, font_system, &mut self.atlas, &self.viewport, [], swash, |_| None);
        debug_assert!(released.is_ok(), "releasing the kept rows failed: {released:?}");
    }

    /// Draws `list` into `view`. `raw` is a non-sRGB view of the same target for the fade pass
    /// (see `Renderer::draw_with`).
    #[allow(clippy::too_many_arguments)]
    pub fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, list: &DrawList, up: &Uploads, text: TextInputs<'_>, view: &wgpu::TextureView, raw: Option<&wgpu::TextureView>) -> Result<(), String> {
        // The buffer writes go first: a text prepare that fails must not leave the CPU's copy of a
        // buffer ahead of the GPU's (the belt sends them with the next frame).
        if self.quad_uniform != Some(list.quad_uniform) {
            self.uploads.write(device, &self.quads.uniform, 0, bytemuck::cast_slice(&list.quad_uniform));
            self.quad_uniform = Some(list.quad_uniform);
        }
        for w in &up.buffers {
            match w {
                BufferWrite::Grow { which, size } => self.bufs[which.index()].buf = GpuVec::alloc(device, *size),
                BufferWrite::Quads { which, offset, bytes } => self.uploads.write(device, &self.bufs[which.index()].buf, *offset, bytes),
            }
        }
        for op in &up.atlas {
            match *op {}
        }

        let (width, height) = list.size;
        self.viewport.update(queue, Resolution { width, height });
        let TextInputs { font_system, swash, buffers, blank, custom } = text;
        let bounds = |b: &Bounds| TextBounds { left: b[0], top: b[1], right: b[2], bottom: b[3] };
        let white = Color::rgb(255, 255, 255);
        // A row's scale is already in its buffer's metrics, so every area draws at scale 1.
        let areas = list.rows.rows.iter().map(|r| TextArea { buffer: &buffers[r.slot].0, left: r.left, top: r.top, scale: 1.0, bounds: bounds(&r.bounds), default_color: white, custom_glyphs: &[] });
        let row_areas = || list.rows.rows.iter().map(|r| RowArea { serial: r.serial, buffer: &buffers[r.slot].0, left: r.left, top: r.top, bounds: bounds(&r.bounds), default_color: white });
        // Box and block glyphs: one more `TextArea` per clip rectangle, an empty buffer carrying the
        // glyphs as custom glyphs (rasterized by `boxdraw` on request, cached by id and size).
        let glyph_areas = list.glyphs.groups.iter().map(|(b, glyphs)| TextArea { buffer: blank, left: 0.0, top: 0.0, scale: 1.0, bounds: bounds(b), default_color: white, custom_glyphs: glyphs });
        let rasterize = |request: RasterizeCustomGlyphRequest| {
            // Total for every interned id: the same mask each time, never `None` once it was `Some`.
            let (c, scale) = custom.get(request.id)?;
            Some(RasterizedCustomGlyph { data: crate::boxdraw::rasterize(c, request.width, request.height, scale), content_type: ContentType::Mask })
        };
        if list.rows.kept {
            // The rows, then the box and block glyphs, both on one atlas: when either grows it, the
            // rows prepare again, until neither moves it.
            loop {
                self.rows.prepare(device, queue, &mut self.uploads, font_system, &mut self.atlas, &self.viewport, row_areas(), swash, &rasterize).map_err(|e| format!("text prepare: {e:?}"))?;
                let seen = self.atlas.generation();
                self.text.prepare_with_custom_uploads(device, queue, &mut self.uploads, font_system, &mut self.atlas, &self.viewport, glyph_areas.clone(), swash, &rasterize).map_err(|e| format!("text prepare: {e:?}"))?;
                if self.atlas.generation() == seen {
                    break;
                }
            }
        } else {
            self.text.prepare_with_custom_uploads(device, queue, &mut self.uploads, font_system, &mut self.atlas, &self.viewport, areas.chain(glyph_areas), swash, rasterize).map_err(|e| format!("text prepare: {e:?}"))?;
        }

        // Background images and inline images share one texture cache.
        self.images.prepare(device, queue, list.backdrop.iter().chain(&list.picture));
        // Only the list's own length: a list that shrank leaves its old tail unread.
        let quads = |which: QuadBuf, verts: u32| self.bufs[which.index()].buf.slice(..verts as u64 * QUAD_STRIDE);
        let pictures = |which: QuadBuf, list: &[ImageDraw]| self.bufs[which.index()].buf.slice(..list.len() as u64 * 6 * IMAGE_STRIDE);

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let [r, g, b, a] = list.clear;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: r as f64, g: g as f64, b: b as f64, a: a as f64 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if !list.backdrop.is_empty() {
                self.images.draw(&mut pass, pictures(QuadBuf::Backdrop, &list.backdrop), &list.backdrop);
            }
            pass.set_pipeline(&self.quads.pipeline);
            pass.set_bind_group(0, &self.quads.bind_group, &[]);
            if !list.under.is_empty() {
                pass.set_vertex_buffer(0, quads(QuadBuf::Under, list.under.end));
                pass.draw(list.under.clone(), 0..1);
            }
            if !list.picture.is_empty() {
                self.images.draw(&mut pass, pictures(QuadBuf::Picture, &list.picture), &list.picture);
            }
            // Popup boxes cover images and quads; their text and what they cut stay on top.
            if !list.pops.is_empty() {
                pass.set_pipeline(&self.quads.pipeline);
                pass.set_bind_group(0, &self.quads.bind_group, &[]);
                pass.set_vertex_buffer(0, quads(QuadBuf::Pops, list.pops.end));
                pass.draw(list.pops.clone(), 0..1);
            }
            if list.rows.kept {
                self.rows.render(&self.atlas, &self.viewport, &mut pass).map_err(|e| format!("text render: {e:?}"))?;
            }
            self.text.render(&self.atlas, &self.viewport, &mut pass).map_err(|e| format!("text render: {e:?}"))?;
            if !list.over.is_empty() {
                pass.set_pipeline(&self.quads.pipeline);
                pass.set_bind_group(0, &self.quads.bind_group, &[]);
                pass.set_vertex_buffer(0, quads(QuadBuf::Over, list.over.end));
                pass.draw(list.over.clone(), 0..1);
            }
        }
        // A fading window: the whole target, premultiplied, scaled by the fade, in a pass of its own
        // on the non-sRGB view.
        if let Some(f) = list.fade {
            let f = f as f64;
            if raw.is_none() && self.quads.format.is_srgb() && self.quads.fader_view.is_none() {
                self.quads.fader_view = Some(self.quads.fade_for_view(device));
            }
            let fader = match (raw, &self.quads.fader_view) {
                (None, Some(v)) if self.quads.format.is_srgb() => v,
                _ => &self.quads.fader,
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: raw.unwrap_or(view),
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(fader);
            pass.set_bind_group(0, &self.quads.bind_group, &[]);
            pass.set_blend_constant(wgpu::Color { r: f, g: f, b: f, a: f });
            pass.set_vertex_buffer(0, quads(QuadBuf::Cover, 6));
            pass.draw(0..6, 0..1);
        }
        self.uploads.submit(queue, Some(encoder.finish()));
        self.atlas.trim();
        Ok(())
    }
}
