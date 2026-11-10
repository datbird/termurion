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

/// Where the card cannot view one texture in two formats (OpenGL): the frame's sRGB texture is
/// read back to linear by the sampler, so this pass stores it sRGB-encoded in a plain texture,
/// the same bytes the raw view would have shown the shaders.
const DECODE: &str = "
@group(0) @binding(0) var src: texture_2d<f32>;
fn enc(c: f32) -> f32 {
    if c <= 0.0031308 { return c * 12.92; }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}
@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(p.xy), 0);
    return vec4<f32>(enc(c.r), enc(c.g), enc(c.b), c.a);
}
";

/// The other way, into the window's sRGB view: the stored values are decoded to linear here and
/// the card encodes them again on the write, so the window gets the bytes the chain made.
const ENCODE: &str = "
@group(0) @binding(0) var src: texture_2d<f32>;
fn dec(c: f32) -> f32 {
    if c <= 0.04045 { return c / 12.92; }
    return pow((c + 0.055) / 1.055, 2.4);
}
@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(src, vec2<i32>(p.xy), 0);
    return vec4<f32>(dec(c.r), dec(c.g), dec(c.b), c.a);
}
";

/// Whether the card can view a texture (and the window) in a second format. OpenGL cannot; the
/// effects then run with `Chain::without_view_formats`.
pub fn view_formats_supported(adapter: &wgpu::Adapter) -> bool {
    let f = adapter.get_downlevel_capabilities().flags;
    f.contains(wgpu::DownlevelFlags::VIEW_FORMATS) && f.contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS)
}

/// The two copy passes of a chain that has no second view (`Chain::without_view_formats`).
struct Blits {
    layout: wgpu::BindGroupLayout,
    decode: wgpu::RenderPipeline,
    encode: wgpu::RenderPipeline,
}

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

/// Without view formats: the frame in the window's format, and two plain textures the passes
/// read and write (the first holds the decoded frame).
struct Plain {
    _frame_tex: wgpu::Texture,
    frame: wgpu::TextureView,
    _textures: [wgpu::Texture; 2],
    raw: [wgpu::TextureView; 2],
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
    /// Set when the card has no second view: the copy passes and their textures.
    blits: Option<Blits>,
    plain: Option<Plain>,
    keep_alpha: bool,
}

/// The format the passes write: the window's, without the sRGB suffix.
pub fn raw_format(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    format.remove_srgb_suffix()
}

impl Chain {
    /// Builds a pipeline per shader. Fails with the first shader the device rejects. On an
    /// opaque window (`keep_alpha` false) the last pass writes color only and the window stays
    /// opaque: shaders made for Ghostty may leave any alpha, which Ghostty's opaque window ignores
    /// but a composited window would show through.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, shaders: Vec<Compiled>, keep_alpha: bool) -> Result<Chain, String> {
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
        let last = shaders.len().saturating_sub(1);
        for (i, shader) in shaders.into_iter().enumerate() {
            let write_mask = if i == last && !keep_alpha { wgpu::ColorWrites::COLOR } else { wgpu::ColorWrites::ALL };
            let layout = uniform_layout(&shader.module);
            let size = layout.as_ref().map_or(16, |l| l.size.max(16)) as u64;
            let entry = shader.entry.clone();
            // A driver compiler failure is an internal error, a bad shader a validation error.
            let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
            let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("render_fx plugin"), source: wgpu::ShaderSource::Naga(Cow::Owned(shader.module)) });
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("render_fx"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState { module: &vertex, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(&entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: raw_format(format), blend: None, write_mask })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            });
            let errors = [ready(device, validation.pop()), ready(device, internal.pop())];
            if let Some(e) = errors.into_iter().flatten().next() {
                return Err(e.to_string());
            }
            let uniforms = device.create_buffer(&wgpu::BufferDescriptor { label: Some("render_fx uniforms"), size, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            passes.push(Pass { pipeline, uniforms, layout });
        }
        Ok(Chain { passes, bind_layout, sampler, format, size: (0, 0), targets: Vec::new(), blits: None, plain: None, keep_alpha })
    }

    /// For a card that cannot view a texture in a second format (`view_formats_supported` false):
    /// the frame is drawn in the window's format and copied into plain textures for the shaders,
    /// and the result is copied into the window's own view. A window format with no sRGB suffix
    /// needs no copies.
    pub fn without_view_formats(mut self, device: &wgpu::Device) -> Chain {
        if self.format == raw_format(self.format) {
            return self;
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render_fx copy"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                count: None,
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("render_fx copy"), bind_group_layouts: &[Some(&layout)], ..Default::default() });
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("render_fx vertex"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(VERTEX)) });
        let pipe = |src: &'static str, format: wgpu::TextureFormat, write_mask: wgpu::ColorWrites| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("render_fx copy"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(src)) });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("render_fx copy"),
                layout: Some(&pl),
                vertex: wgpu::VertexState { module: &vertex, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_main"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask })] }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // The window keeps its alpha only when the last pass would have kept it.
        let out_mask = if self.keep_alpha { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::COLOR };
        let decode = pipe(DECODE, raw_format(self.format), wgpu::ColorWrites::ALL);
        let encode = pipe(ENCODE, self.format, out_mask);
        self.blits = Some(Blits { layout, decode, encode });
        self.targets.clear();
        self.size = (0, 0);
        self
    }

    /// Whether `run` writes a non-sRGB view of the window (else the window's own view).
    pub fn wants_raw_out(&self) -> bool {
        self.blits.is_none()
    }

    pub fn is_empty(&self) -> bool {
        self.passes.is_empty()
    }

    fn target(device: &wgpu::Device, format: wgpu::TextureFormat, (w, h): (u32, u32)) -> Target {
        let twin = [raw_format(format)];
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render_fx frame"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            // A format with no sRGB twin needs no second view (and no card support for one).
            view_formats: if twin[0] == format { &[] } else { &twin },
        });
        let srgb = texture.create_view(&Default::default());
        let raw = texture.create_view(&wgpu::TextureViewDescriptor { format: Some(raw_format(format)), ..Default::default() });
        Target { _texture: texture, srgb, raw }
    }

    fn plain(device: &wgpu::Device, format: wgpu::TextureFormat, (w, h): (u32, u32)) -> Plain {
        let make = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let both = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let frame_tex = make("render_fx frame", format, both);
        let a = make("render_fx plain", raw_format(format), both);
        let b = make("render_fx plain", raw_format(format), both);
        let frame = frame_tex.create_view(&Default::default());
        let raw = [a.create_view(&Default::default()), b.create_view(&Default::default())];
        Plain { _frame_tex: frame_tex, frame, _textures: [a, b], raw }
    }

    /// The views the renderer draws the frame into this frame (made again on a resize):
    /// (the window-format view, its non-sRGB twin for fades, None without view formats).
    pub fn frame_views(&mut self, device: &wgpu::Device, size: (u32, u32)) -> (&wgpu::TextureView, Option<&wgpu::TextureView>) {
        if self.blits.is_some() {
            if self.size != size || self.plain.is_none() {
                self.plain = Some(Self::plain(device, self.format, size));
                self.size = size;
            }
            let p = self.plain.as_ref().expect("made above");
            return (&p.frame, None);
        }
        let wanted = if self.passes.len() > 1 { 2 } else { 1 };
        if self.size != size || self.targets.len() != wanted {
            self.targets = (0..wanted).map(|_| Self::target(device, self.format, size)).collect();
            self.size = size;
        }
        (&self.targets[0].srgb, Some(&self.targets[0].raw))
    }

    /// One copy pass from `src` into `dst`.
    fn blit(device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, blits: &Blits, pipeline: &wgpu::RenderPipeline, src: &wgpu::TextureView, dst: &wgpu::TextureView) {
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("render_fx copy"), layout: &blits.layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) }] });
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("render_fx copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: dst, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, &bind, &[]);
        rp.draw(0..3, 0..1);
    }

    /// Runs every pass; the last writes `out`, a non-sRGB view of the window texture.
    pub fn run(&self, device: &wgpu::Device, queue: &wgpu::Queue, uniforms: &Uniforms, out: &wgpu::TextureView) {
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render_fx") });
        let last = self.passes.len().saturating_sub(1);
        // Without view formats every pass reads and writes the plain pair, and the copies wrap them.
        let plain = match (&self.blits, &self.plain) {
            (Some(b), Some(p)) => Some((b, p)),
            (Some(_), None) => return,
            (None, _) => None,
        };
        if plain.is_none() && self.targets.is_empty() {
            return;
        }
        if let Some((b, p)) = plain {
            Self::blit(device, &mut encoder, b, &b.decode, &p.frame, &p.raw[0]);
        }
        for (i, pass) in self.passes.iter().enumerate() {
            if let Some(layout) = &pass.layout {
                queue.write_buffer(&pass.uniforms, 0, &uniforms.pack(layout));
            }
            // Ping-pong: pass i reads target i % 2 and writes the other (or the window).
            let (input, output) = match plain {
                Some((_, p)) => (&p.raw[i % 2], &p.raw[(i + 1) % 2]),
                None => (&self.targets[i % 2].raw, if i == last { out } else { &self.targets[(i + 1) % 2].raw }),
            };
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
        if let Some((b, p)) = plain {
            Self::blit(device, &mut encoder, b, &b.encode, &p.raw[(last + 1) % 2], out);
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
