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

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    quads: QuadPipeline,
    font_system: FontSystem,
    swash: SwashCache,
    atlas: TextAtlas,
    viewport: Viewport,
    text: TextRenderer,
    /// One text buffer per row slot, with the spans it last shaped.
    buffers: Vec<(Buffer, Vec<Span>, f32)>,
    family: String,
    font_px: f32,
    pub cell: (f32, f32),
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, family: &str, font_px: f32) -> Renderer {
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let mut r = Renderer {
            device: device.clone(),
            queue: queue.clone(),
            quads: QuadPipeline::new(device, format),
            font_system: FontSystem::new(),
            swash: SwashCache::new(),
            atlas,
            viewport,
            text,
            buffers: Vec::new(),
            family: family.to_string(),
            font_px,
            cell: (0.0, 0.0),
        };
        r.set_font_px(font_px);
        r
    }

    /// Sets the font size and measures the cell: the advance of "M" by 1.2 lines.
    pub fn set_font_px(&mut self, px: f32) {
        self.font_px = px;
        let line = (px * 1.2).round();
        let mut b = Buffer::new(&mut self.font_system, Metrics::new(px, line));
        b.set_text("M", &Attrs::new().family(Family::Name(&self.family)), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.font_system, false);
        let w = b.layout_runs().next().and_then(|r| r.glyphs.first().map(|g| g.w)).unwrap_or(px * 0.6);
        self.cell = (w.round().max(1.0), line);
        self.buffers.clear();
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
            let spans = row.spans.iter().map(|s| {
                let attrs = base
                    .clone()
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
            rows: vec![TextRow {
                x: 36.0,
                y: 0.0,
                spans: vec![Span { text: "\u{2588}".into(), color: rgb(255, 255, 255), bold: false, italic: false }],
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
}
