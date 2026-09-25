//! Draws a scene Frame: rectangles with a small wgpu pipeline, text with glyphon.

use glyphon::{
    Attrs, Buffer, Cache, Color, Family, FontSystem, Metrics, Resolution, Shaping, Style, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use wgpu::util::DeviceExt;

use crate::scene::{Frame, Quad, Span};

/// Rectangles: two triangles each, position and color per vertex.
struct QuadPipeline {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

impl QuadPipeline {
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
        let attributes = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("quad"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 6 * 4,
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
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        QuadPipeline { pipeline, uniform, bind_group }
    }

    fn vertices(quads: &[Quad]) -> Vec<f32> {
        let mut v = Vec::with_capacity(quads.len() * 36);
        for q in quads {
            let (x0, y0, x1, y1) = (q.rect.x, q.rect.y, q.rect.x + q.rect.w, q.rect.y + q.rect.h);
            for (x, y) in [(x0, y0), (x1, y0), (x0, y1), (x0, y1), (x1, y0), (x1, y1)] {
                v.extend_from_slice(&[x, y]);
                v.extend_from_slice(&q.color);
            }
        }
        v
    }
}

/// Inline images: textured rectangles, one texture per picture.
struct ImagePipeline {
    pipeline: wgpu::RenderPipeline,
    screen_group: wgpu::BindGroup,
    texture_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Textures by picture, with the frame count they were last drawn at.
    cache: std::collections::HashMap<(u64, usize), (wgpu::BindGroup, u64)>,
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
                    array_stride: 4 * 4,
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
        ImagePipeline { pipeline, screen_group, texture_layout, sampler, cache: Default::default(), uploads: 0, frames: 0 }
    }

    /// Makes sure every picture in `images` has a texture; drops ones long unused.
    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, images: &[crate::scene::ImageQuad]) {
        self.frames += 1;
        for q in images {
            if let Some(entry) = self.cache.get_mut(&q.key) {
                entry.1 = self.frames;
                continue;
            }
            let (w, h) = q.size;
            if w == 0 || h == 0 || q.pixels.len() != (w as usize) * (h as usize) * 4 {
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
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            self.cache.insert(q.key, (group, self.frames));
            self.uploads += 1;
        }
        let now = self.frames;
        self.cache.retain(|_, (_, used)| now - *used <= TEXTURE_KEEP);
    }

    fn vertices(images: &[crate::scene::ImageQuad]) -> Vec<f32> {
        let mut v = Vec::with_capacity(images.len() * 24);
        for q in images {
            let (x0, y0, x1, y1) = (q.rect.x, q.rect.y, q.rect.x + q.rect.w, q.rect.y + q.rect.h);
            let [u0, v0, u1, v1] = q.uv;
            for (x, y, u, w) in [(x0, y0, u0, v0), (x1, y0, u1, v0), (x0, y1, u0, v1), (x0, y1, u0, v1), (x1, y0, u1, v0), (x1, y1, u1, v1)] {
                v.extend_from_slice(&[x, y, u, w]);
            }
        }
        v
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    quads: QuadPipeline,
    images: ImagePipeline,
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    text: TextRenderer,
    /// One text buffer per row slot, with the spans it last shaped.
    buffers: Vec<(Buffer, Vec<Span>, f32)>,
    family: String,
    alt_family: Option<String>,
    h_spacing: f32,
    v_spacing: f32,
    font_px: f32,
    pub cell: (f32, f32),
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, family: &str, font_px: f32) -> Renderer {
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let quads = QuadPipeline::new(device, format);
        let images = ImagePipeline::new(device, format, &quads.uniform);
        let mut r = Renderer {
            device: device.clone(),
            queue: queue.clone(),
            quads,
            images,
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
            atlas,
            viewport,
            text,
            buffers: Vec::new(),
            family: family.to_string(),
            alt_family: None,
            h_spacing: 1.0,
            v_spacing: 1.0,
            font_px,
            cell: (0.0, 0.0),
        };
        r.set_font_px(font_px);
        r
    }

    /// Sets the font size and measures the cell: the font's own advance, by 1.2 lines.
    /// The width is not rounded. Glyphs advance by the exact width, so a rounded cell
    /// would drift away from the text along a row (the cursor landed two cells short).
    pub fn set_font_px(&mut self, px: f32) {
        self.font_px = px;
        let line = (px * 1.2 * self.v_spacing).round();
        let n = 100;
        let mut b = Buffer::new(&mut self.font_system, Metrics::new(px, line));
        b.set_size(None, None);
        b.set_wrap(Wrap::None);
        b.set_text(&"M".repeat(n), &Attrs::new().family(Family::Name(&self.family)), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.font_system, false);
        let w = b.layout_runs().next().map(|r| r.line_w / n as f32).filter(|w| *w > 0.0).unwrap_or(px * 0.6);
        self.cell = (w * self.h_spacing, line);
        self.buffers.clear();
    }

    /// The installed family for an iTerm2 font name, which is usually a PostScript
    /// name ("JetBrainsMono-Regular"). Tries a family name, then the installed fonts'
    /// PostScript names, then a guess from the dashes.
    pub fn resolve_family(&self, name: &str) -> String {
        let db = self.font_system.db();
        if let Some(f) = db.faces().flat_map(|f| f.families.iter()).find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            return f.0.clone();
        }
        if let Some(face) = db.faces().find(|f| f.post_script_name.eq_ignore_ascii_case(name)) {
            if let Some((family, _)) = face.families.first() {
                return family.clone();
            }
        }
        // "CascadiaMono-Regular" or "JetBrainsMono-Bold": drop the style word, then match
        // family names with spaces removed ("Cascadia Mono", "JetBrains Mono").
        let guess = crate::import::family_from_postscript(name);
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace() && *c != '-').collect::<String>().to_lowercase();
        let want = squash(&guess);
        db.faces()
            .flat_map(|f| f.families.iter())
            .find(|(n, _)| squash(n) == want)
            .map_or(guess, |(n, _)| n.clone())
    }

    /// True when a font family with this name is installed.
    pub fn has_family(&self, name: &str) -> bool {
        self.font_system.db().faces().any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
    }

    /// Sets the fonts and size. Spacing multiplies the cell width and height, as
    /// iTerm2's horizontal and vertical spacing do.
    pub fn set_fonts(&mut self, normal: &str, non_ascii: Option<&str>, px: f32, h_spacing: f32, v_spacing: f32) {
        self.family = normal.to_string();
        self.alt_family = non_ascii.map(str::to_string);
        self.h_spacing = h_spacing;
        self.v_spacing = v_spacing;
        self.set_font_px(px);
    }

    /// How many image textures were uploaded (tests).
    pub fn uploads(&self) -> usize {
        self.images.uploads
    }

    pub fn draw(&mut self, frame: &Frame, view: &wgpu::TextureView, width: u32, height: u32) -> Result<(), String> {
        let (cw, ch) = self.cell;
        let metrics = Metrics::new(self.font_px, ch);
        while self.buffers.len() < frame.rows.len() {
            self.buffers.push((Buffer::new(&mut self.font_system, metrics), Vec::new(), 0.0));
        }
        for (slot, row) in self.buffers.iter_mut().zip(&frame.rows) {
            let (buffer, last, last_w) = slot;
            if *last == row.spans && *last_w == row.clip.w {
                continue;
            }
            buffer.set_metrics_and_size(metrics, Some(row.clip.w + cw), Some(ch));
            buffer.set_wrap(Wrap::None);
            buffer.set_monospace_width(Some(cw));
            let base = Attrs::new().family(Family::Name(&self.family));
            let alt = self.alt_family.as_deref().map(|f| Attrs::new().family(Family::Name(f)));
            let spans = row.spans.iter().map(|s| {
                let start = if s.alt_font { alt.clone().unwrap_or_else(|| base.clone()) } else { base.clone() };
                let attrs = start
                    .color(Color::rgb(s.color.r, s.color.g, s.color.b))
                    .weight(if s.bold { Weight::BOLD } else { Weight::NORMAL })
                    .style(if s.italic { Style::Italic } else { Style::Normal });
                (s.text.as_str(), attrs)
            });
            buffer.set_rich_text(spans, &base, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            *last = row.spans.clone();
            *last_w = row.clip.w;
        }

        self.viewport.update(&self.queue, Resolution { width, height });
        let areas = self.buffers.iter().zip(&frame.rows).map(|((buffer, _, _), row)| TextArea {
            buffer,
            left: row.x,
            top: row.y,
            scale: 1.0,
            bounds: TextBounds {
                left: row.clip.x as i32,
                top: row.clip.y as i32,
                right: (row.clip.x + row.clip.w) as i32,
                bottom: (row.clip.y + row.clip.h) as i32,
            },
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: &[],
        });
        self.text
            .prepare(&self.device, &self.queue, &mut self.font_system, &mut self.atlas, &self.viewport, areas, &mut self.swash)
            .map_err(|e| format!("text prepare: {e:?}"))?;

        self.queue.write_buffer(&self.quads.uniform, 0, bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]));
        let under = QuadPipeline::vertices(&frame.quads);
        let over = QuadPipeline::vertices(&frame.overlay);
        let make = |v: &[f32]| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(v),
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        let (under_buf, over_buf) = (make(&under), make(&over));
        self.images.prepare(&self.device, &self.queue, &frame.images);
        let picture = ImagePipeline::vertices(&frame.images);
        let picture_buf = make(&picture);

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let [r, g, b, a] = frame.clear;
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
            pass.set_pipeline(&self.quads.pipeline);
            pass.set_bind_group(0, &self.quads.bind_group, &[]);
            if !under.is_empty() {
                pass.set_vertex_buffer(0, under_buf.slice(..));
                pass.draw(0..(under.len() / 6) as u32, 0..1);
            }
            if !picture.is_empty() {
                pass.set_pipeline(&self.images.pipeline);
                pass.set_bind_group(0, &self.images.screen_group, &[]);
                pass.set_vertex_buffer(0, picture_buf.slice(..));
                for (i, q) in frame.images.iter().enumerate() {
                    let Some((group, _)) = self.images.cache.get(&q.key) else { continue };
                    pass.set_bind_group(1, group, &[]);
                    let first = (i * 6) as u32;
                    pass.draw(first..first + 6, 0..1);
                }
            }
            self.text.render(&self.atlas, &self.viewport, &mut pass).map_err(|e| format!("text render: {e:?}"))?;
            if !over.is_empty() {
                pass.set_pipeline(&self.quads.pipeline);
                pass.set_bind_group(0, &self.quads.bind_group, &[]);
                pass.set_vertex_buffer(0, over_buf.slice(..));
                pass.draw(0..(over.len() / 6) as u32, 0..1);
            }
        }
        self.queue.submit(Some(encoder.finish()));
        self.atlas.trim();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;
    use crate::palette::rgb;
    use crate::scene::{Quad, Span, TextRow, rgba};

    fn device() -> (wgpu::Device, wgpu::Queue) {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).unwrap();
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).unwrap()
    }

    fn read(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture, w: u32, h: u32) -> Vec<u8> {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (w * h * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit(Some(enc.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buf.slice(..).get_mapped_range().unwrap().to_vec();
        data
    }

    #[test]
    fn text_advance_matches_the_cell_width() {
        // The cursor and backgrounds use `cell`. If glyphs advance by a different width,
        // text drifts away from them along the row.
        let (device, queue) = device();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut r = Renderer::new(&device, &queue, format, "Cascadia Mono", 16.0);
        let (w, h) = (512u32, 32u32);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let n = 40;
        let frame = Frame {
            clear: rgba(rgb(0, 0, 0), 1.0),
            quads: vec![],
            images: vec![],
            rows: vec![TextRow {
                x: 0.0,
                y: 0.0,
                spans: vec![Span { text: "\u{2588}".repeat(n), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false }],
                clip: Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 },
            }],
            overlay: vec![],
        };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let lit = |x: u32| (0..h).any(|y| px[((y * w + x) * 4) as usize] > 128);
        let right = (0..w).rev().find(|&x| lit(x)).unwrap() as f32 + 1.0;
        let want = n as f32 * r.cell.0;
        assert!((right - want).abs() < r.cell.0 / 2.0, "text ends at {right} px, cells end at {want} px (cell {:?})", r.cell);
    }

    #[test]
    fn postscript_names_resolve_to_installed_families() {
        let (device, queue) = device();
        let r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        assert_eq!(r.resolve_family("CascadiaMono-Regular"), "Cascadia Mono");
        assert_eq!(r.resolve_family("Cascadia Mono"), "Cascadia Mono");
        // Not installed: the dash heuristic is the best guess.
        assert_eq!(r.resolve_family("MesloLGS-NF-Regular"), "MesloLGS NF");
    }

    #[test]
    fn draws_quads_and_text_offscreen() {
        let (device, queue) = device();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut r = Renderer::new(&device, &queue, format, "Cascadia Mono", 24.0);
        assert!(r.cell.0 > 5.0 && r.cell.1 > 10.0, "{:?}", r.cell);
        let (w, h) = (64u32, 32u32);
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let frame = Frame {
            clear: rgba(rgb(0, 0, 0), 1.0),
            quads: vec![Quad { rect: Rect { x: 0.0, y: 0.0, w: 32.0, h: 32.0 }, color: rgba(rgb(255, 0, 0), 1.0) }],
            images: vec![],
            rows: vec![TextRow {
                x: 36.0,
                y: 0.0,
                spans: vec![Span { text: "\u{2588}".into(), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false }],
                clip: Rect { x: 32.0, y: 0.0, w: 32.0, h: 32.0 },
            }],
            overlay: vec![],
        };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..((y * w + x) * 4 + 3) as usize];
        assert_eq!(at(4, 4), &[255, 0, 0]);
        let white = (32..64).flat_map(|x| (0..32).map(move |y| (x, y))).any(|(x, y)| at(x, y)[0] > 200 && at(x, y)[2] > 200);
        assert!(white, "no text pixels drawn");
    }

    fn target(device: &wgpu::Device, w: u32, h: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn picture(key: u64) -> crate::scene::ImageQuad {
        // Two pixels: red, then blue.
        let px = vec![255, 0, 0, 255, 0, 0, 255, 255];
        crate::scene::ImageQuad {
            rect: Rect { x: 0.0, y: 0.0, w: 32.0, h: 16.0 },
            uv: [0.0, 0.0, 1.0, 1.0],
            key: (key, 0),
            pixels: std::sync::Arc::new(px),
            size: (2, 1),
        }
    }

    #[test]
    fn draws_an_image_offscreen() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        // Readback rows must be a multiple of 256 bytes: 64 px wide.
        let tex = target(&device, 64, 16);
        let view = tex.create_view(&Default::default());
        let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), images: vec![picture(1)], ..Default::default() };
        r.draw(&frame, &view, 64, 16).unwrap();
        let px = read(&device, &queue, &tex, 64, 16);
        let at = |x: u32, y: u32| &px[((y * 64 + x) * 4) as usize..((y * 64 + x) * 4 + 3) as usize];
        assert!(at(2, 8)[0] > 200 && at(2, 8)[2] < 50, "{:?}", at(2, 8));
        assert!(at(29, 8)[2] > 200 && at(29, 8)[0] < 50, "{:?}", at(29, 8));
    }

    #[test]
    fn textures_upload_once() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let tex = target(&device, 32, 16);
        let view = tex.create_view(&Default::default());
        let frame = Frame { images: (0..100).map(picture).collect(), ..Default::default() };
        r.draw(&frame, &view, 32, 16).unwrap();
        r.draw(&frame, &view, 32, 16).unwrap();
        assert_eq!(r.uploads(), 100);
    }
}
