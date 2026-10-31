//! Render plugins on the GPU: the frame is drawn into an offscreen texture, then each plugin
//! shader runs as a full-screen triangle that reads the previous result as `iChannel0`. The last
//! pass writes the window. wgpu only, so the same on every platform.

use std::borrow::Cow;

use crate::render_fx::{Compiled, Layout, Uniforms, uniform_layout};

/// A full-screen triangle; the fragment shaders read `@builtin(position)` (gl_FragCoord).
const VERTEX: &str = "
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}
";

struct Pass {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    layout: Option<Layout>,
}

/// One offscreen texture with its two views: `srgb` for the renderer to draw into and `raw`
/// (the same bytes, no sRGB conversion) for the shaders to read and write, so sampled colors
/// stay in the same sRGB-encoded space as the color uniforms.
struct Target {
    _texture: wgpu::Texture,
    srgb: wgpu::TextureView,
    raw: wgpu::TextureView,
}

pub struct Chain {
    passes: Vec<Pass>,
    bind_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The window's format; the offscreen textures share it so the renderer's pipelines fit.
    format: wgpu::TextureFormat,
    size: (u32, u32),
    /// [0] is the frame; [1] is the ping-pong partner for chains of more than one shader.
    targets: Vec<Target>,
}

/// The format the passes write: the window's, without the sRGB suffix.
pub fn raw_format(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    format.remove_srgb_suffix()
}

impl Chain {
    /// Builds a pipeline per shader. Fails with the first shader the device rejects.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, shaders: Vec<Compiled>) -> Result<Chain, String> {
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render_fx"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("render_fx"), bind_group_layouts: &[Some(&bind_layout)], ..Default::default() });
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("render_fx vertex"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(VERTEX)) });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("render_fx"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut passes = Vec::new();
        for shader in shaders {
            let layout = uniform_layout(&shader.module);
            let size = layout.as_ref().map_or(16, |l| l.size.max(16)) as u64;
            let entry = shader.entry.clone();
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("render_fx plugin"), source: wgpu::ShaderSource::Naga(Cow::Owned(shader.module)) });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("render_fx"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &vertex, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(&entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: raw_format(format), blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
            if let Some(e) = ready(device, scope.pop()) {
                return Err(e.to_string());
            }
            let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("render_fx uniforms"), size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            passes.push(Pass { pipeline, uniforms, layout });
        }
        Ok(Chain { passes, bind_layout, sampler, format, size: (0, 0), targets: Vec::new() })
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    fn target(device: &wgpu::Device, format: wgpu::TextureFormat, (w, h): (u32, u32)) -> Target {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render_fx frame"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[raw_format(format)],
        });
        let srgb = texture.create_view(&Default::default());
        let raw = texture.create_view(&wgpu::TextureViewDescriptor { format: Some(raw_format(format)), ..Default::default() });
        Target { _texture: texture, srgb, raw }
    }

    /// The views the renderer draws the frame into this frame (made again on a resize):
    /// (the window-format view, its non-sRGB twin for fades).
    pub fn frame_views(&mut self, device: &wgpu::Device, size: (u32, u32)) -> (&wgpu::TextureView, &wgpu::TextureView) {
        let wanted = if self.passes.len() > 1 { 2 } else { 1 };
        if self.size != size || self.targets.len() != wanted {
            self.targets = (0..wanted).map(|_| Self::target(device, self.format, size)).collect();
            self.size = size;
        }
        (&self.targets[0].srgb, &self.targets[0].raw)
    }

    /// Runs every pass; the last writes `out`, a non-sRGB view of the window texture.
    pub fn run(&self, device: &wgpu::Device, queue: &wgpu::Queue, uniforms: &Uniforms, out: &wgpu::TextureView) {
        if self.targets.is_empty() {
            return;
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render_fx") });
        let last = self.passes.len().saturating_sub(1);
        for (i, pass) in self.passes.iter().enumerate() {
            if let Some(layout) = &pass.layout {
                queue.write_buffer(&pass.uniforms, 0, &uniforms.pack(layout));
            }
            // Ping-pong: pass i reads target i % 2 and writes the other (or the window).
            let input = &self.targets[i % 2].raw;
            let output = if i == last { out } else { &self.targets[(i + 1) % 2].raw };
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("render_fx"),
                layout: &self.bind_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) },
                    wgpu::BindGroupEntry { binding: 1, resource: pass.uniforms.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            });
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("render_fx"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&pass.pipeline);
            rp.set_bind_group(0, &bind, &[]);
            rp.draw(0..3, 0..1);
        }
        queue.submit([encoder.finish()]);
    }
}

/// The result of an error scope. On native wgpu it is ready at once; a few polls cover a
/// backend that answers late, and a scope that never answers counts as no error.
fn ready<F: std::future::Future<Output = Option<wgpu::Error>>>(device: &wgpu::Device, f: F) -> Option<wgpu::Error> {
    use std::task::{Context, Poll, Wake, Waker};
    struct Noop;
    impl Wake for Noop {
        fn wake(self: std::sync::Arc<Self>) {}
    }
    let waker = Waker::from(std::sync::Arc::new(Noop));
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    for _ in 0..100 {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        let _ = device.poll(wgpu::PollType::Poll);
    }
    None
}
