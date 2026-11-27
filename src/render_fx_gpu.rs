//! Render plugins on the GPU: the frame is drawn into an offscreen texture, then each plugin
//! shader runs as a full-screen triangle that reads the previous result as `iChannel0`. The last
//! pass writes the window. The chain is wgpu's (Windows and Linux); the full-screen triangle, the
//! copy shader, `Shade` and `upload_range` are shared with the Metal chain, so they build on macOS too.

#[cfg(not(target_os = "macos"))]
use std::borrow::Cow;

#[cfg(not(target_os = "macos"))]
use crate::render_fx::{Compiled, Layout, RegionTerm, Uniforms, uniform_layout};
use crate::render_fx::PxRect;

/// A full-screen triangle; the fragment shaders read `@builtin(position)` (gl_FragCoord).
pub(crate) const VERTEX: &str = "
@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}
";

#[cfg(not(target_os = "macos"))]
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

/// The kept output into the window, byte for byte: no filtering and no color conversion.
pub(crate) const COPY: &str = "
@group(0) @binding(0) var src: texture_2d<f32>;
@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(src, vec2<i32>(p.xy), 0);
}
";

#[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
/// Whether the card can view a texture (and the window) in a second format. OpenGL cannot; the
/// effects then run with `Chain::without_view_formats`.
pub fn view_formats_supported(adapter: &wgpu::Adapter) -> bool {
    let f = adapter.get_downlevel_capabilities().flags;
    f.contains(wgpu::DownlevelFlags::VIEW_FORMATS) && f.contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS)
}

#[cfg(not(target_os = "macos"))]
/// The two copy passes of a chain that has no second view (`Chain::without_view_formats`).
struct Blits {
    layout: wgpu::BindGroupLayout,
    decode: wgpu::RenderPipeline,
    encode: wgpu::RenderPipeline,
}

#[cfg(not(target_os = "macos"))]
struct Pass {
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    layout: Option<Layout>,
}

#[cfg(not(target_os = "macos"))]
/// One offscreen texture with its two views: `srgb` for the renderer to draw into and `raw`
/// (the same bytes, no sRGB conversion) for the shaders to read and write, so sampled colors
/// stay in the same sRGB-encoded space as the color uniforms.
struct Target {
    _texture: wgpu::Texture,
    srgb: wgpu::TextureView,
    raw: wgpu::TextureView,
}

#[cfg(not(target_os = "macos"))]
/// Without view formats: the frame in the window's format, and two plain textures the passes
/// read and write (the first holds the decoded frame).
struct Plain {
    _frame_tex: wgpu::Texture,
    frame: wgpu::TextureView,
    _textures: [wgpu::Texture; 2],
    raw: [wgpu::TextureView; 2],
}

#[cfg(not(target_os = "macos"))]
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
    /// The frame texture holds the window's last terminal picture (`draw_fx` drew it there and
    /// nothing has replaced it since), so a frame of the same scene can run the passes alone.
    frame_drawn: bool,
    /// One bind group per pass, made with the textures they read (`remake_binds`), never per frame.
    binds: Vec<wgpu::BindGroup>,
    /// Without view formats: the decode copy's and the encode copy's bind groups.
    copy_binds: Vec<wgpu::BindGroup>,
    /// What each pass's uniform buffer holds: a frame whose packed bytes are the same writes nothing.
    last_packed: Vec<Vec<u8>>,
    binds_made: u64,
    uniform_writes: u64,
    /// The last run drew the window's picture (`skip_if_still`).
    ran: bool,
    skipped: u64,
    /// What `skip_if_still` packed for this frame's uniforms: `run_with` writes these instead of packing again.
    scratch: Vec<Vec<u8>>,
    /// `scratch` holds this frame's packed bytes (`skip_if_still` made them).
    staged: bool,
    /// Where the effects can draw (`set_region_terms`); None runs every frame full screen.
    region_terms: Option<Vec<RegionTerm>>,
    /// With region terms: the last pass's output, kept so a frame can shade only its region,
    /// then copied whole into the window (whose texture changes every frame).
    kept: Option<Kept>,
    /// `kept` holds this chain's last window frame at `size`.
    kept_ok: bool,
    kept_made: u64,
    /// The next frame may shade only a region (`keep_output`).
    keep_next: bool,
    /// The last run shaded only rects (tests).
    shaded_region: bool,
    /// The copies that put a new frame under a built frame's rects (`Shade::Fresh`): one for a
    /// pass in the middle, one with the last pass's write mask. Made with the first kept output.
    fill: Option<(wgpu::BindGroupLayout, wgpu::RenderPipeline, wgpu::RenderPipeline)>,
}

/// What a run of the passes shades (`Chain::run_with`).
#[derive(Clone, Copy, Debug)]
pub enum Shade<'a> {
    /// Every pixel.
    Full,
    /// An animation-only frame: these rects, every other pixel kept from last frame.
    Kept(&'a [PxRect]),
    /// A frame drawn anew: its plain picture everywhere, these rects shaded over it.
    Fresh(&'a [PxRect]),
}

#[cfg(not(target_os = "macos"))]
/// The chain's kept output (raw format, window size) and the copy that puts it in the window.
struct Kept {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// With view formats: the copy into the window's raw view and its bind group (without view
    /// formats the encode copy reads `view` instead).
    copy: Option<(wgpu::BindGroupLayout, wgpu::RenderPipeline, wgpu::BindGroup)>,
    /// The `fill` copies' bind group: the frame as the first pass reads it.
    fill: wgpu::BindGroup,
}

#[cfg(not(target_os = "macos"))]
/// The format the passes write: the window's, without the sRGB suffix.
pub fn raw_format(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    format.remove_srgb_suffix()
}

#[cfg(not(target_os = "macos"))]
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
        let last_packed = vec![Vec::new(); passes.len()];
        let scratch = vec![Vec::new(); passes.len()];
        Ok(Chain { passes, bind_layout, sampler, format, size: (0, 0), targets: Vec::new(), blits: None, plain: None, keep_alpha, frame_drawn: false, binds: Vec::new(), copy_binds: Vec::new(), last_packed, binds_made: 0, uniform_writes: 0, ran: false, skipped: 0, scratch, staged: false, region_terms: None, kept: None, kept_ok: false, kept_made: 0, keep_next: true, shaded_region: false, fill: None })
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
        self.binds.clear();
        self.copy_binds.clear();
        self.kept = None;
        self.kept_ok = false;
        self.size = (0, 0);
        self
    }

    /// Where the chain's effects can draw (`render_fx::chain_region`). With terms the chain keeps
    /// its output so `run_in` can shade only a region; the textures are made again for it.
    pub fn set_region_terms(&mut self, terms: Option<Vec<RegionTerm>>) {
        if terms.is_some() != self.region_terms.is_some() {
            self.size = (0, 0);
            self.frame_drawn = false;
            self.kept = None;
            self.kept_ok = false;
        }
        self.region_terms = terms;
    }

    pub fn region_terms(&self) -> Option<&[RegionTerm]> {
        self.region_terms.as_deref()
    }

    /// Whether the next frame may shade only a region: false when this frame's own region is full
    /// (`render_fx::own_region`), so the next one is full too. With view formats such a frame then
    /// writes the window directly and skips the kept output and its copy.
    pub fn keep_output(&mut self, keep: bool) {
        self.keep_next = keep;
    }

    /// Whether the last run shaded only rects.
    pub fn shaded_region(&self) -> bool {
        self.shaded_region
    }

    /// How many times the kept output was made (it follows the size, not the frames).
    pub fn kept_made(&self) -> u64 {
        self.kept_made
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
                self.frame_drawn = false;
                self.remake_kept(device);
                self.remake_binds(device);
            }
            let p = self.plain.as_ref().expect("made above");
            return (&p.frame, None);
        }
        let wanted = if self.passes.len() > 1 { 2 } else { 1 };
        if self.size != size || self.targets.len() != wanted {
            self.targets = (0..wanted).map(|_| Self::target(device, self.format, size)).collect();
            self.size = size;
            self.frame_drawn = false;
            self.remake_kept(device);
            self.remake_binds(device);
        }
        (&self.targets[0].srgb, Some(&self.targets[0].raw))
    }

    /// Whether the frame texture still holds the last picture drawn for the window. The copy
    /// passes never write it, and the ping-pong passes write it only from a third shader on.
    pub fn frame_kept(&self) -> bool {
        self.frame_drawn && (self.blits.is_some() || self.passes.len() <= 2)
    }

    /// Records whether the frame texture now holds the window's picture (false after a
    /// screenshot drew something else into it).
    pub fn set_frame_drawn(&mut self, drawn: bool) {
        self.frame_drawn = drawn;
        // Whatever replaced the frame also ran the passes: the kept output is not the window's.
        if !drawn {
            self.kept_ok = false;
            self.ran = false;
        }
    }

    /// Whether a frame with `uniforms` would show what the window already shows: the passes
    /// last ran on this window's picture at `size`, and every pass packs the bytes its buffer
    /// holds. `eligible` is the caller's word that the scene and everything drawn over it are
    /// unchanged; without it the answer is no. Counts the frame it answers yes to (`skips`).
    /// The packed bytes wait for the `run_with` of this frame, so a drawn frame packs once.
    pub fn skip_if_still(&mut self, uniforms: &Uniforms, size: (u32, u32), eligible: bool) -> bool {
        for (p, buf) in self.passes.iter().zip(&mut self.scratch) {
            if let Some(l) = &p.layout {
                uniforms.pack_into(l, buf);
            }
        }
        let same = self.passes.iter().zip(&self.scratch).zip(&self.last_packed).all(|((p, now), last)| p.layout.is_none() || now == last);
        let still = eligible && self.ran && self.frame_kept() && self.size == size && same;
        self.staged = true;
        self.skipped += u64::from(still);
        still
    }

    /// Forgets the bytes `skip_if_still` staged (a draw that is not that frame's).
    pub fn unstage(&mut self) {
        self.staged = false;
    }

    /// How many frames `skip_if_still` let the window keep.
    pub fn skips(&self) -> u64 {
        self.skipped
    }

    /// Makes the kept output at the chain's size, when the chain has region terms.
    fn remake_kept(&mut self, device: &wgpu::Device) {
        self.kept_ok = false;
        // A third pass writes the frame texture, so such a chain never shades a region alone.
        if self.region_terms.is_none() || self.passes.len() > 2 {
            self.kept = None;
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render_fx kept"),
            size: wgpu::Extent3d { width: self.size.0.max(1), height: self.size.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: raw_format(self.format),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let copy = self.blits.is_none().then(|| {
            // The pipeline outlives a resize; only its bind group follows the texture.
            let (layout, pipeline) = match self.kept.take().and_then(|k| k.copy) {
                Some((l, p, _)) => (l, p),
                None => {
                    let out_mask = if self.keep_alpha { wgpu::ColorWrites::ALL } else { wgpu::ColorWrites::COLOR };
                    copy_pipeline(device, raw_format(self.format), out_mask)
                }
            };
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("render_fx kept"), layout: &layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }] });
            (layout, pipeline, bind)
        });
        let keep_alpha = self.keep_alpha;
        let (fill_layout, _, _) = self.fill.get_or_insert_with(|| {
            let (layout, all) = copy_pipeline(device, raw_format(self.format), wgpu::ColorWrites::ALL);
            let last = if keep_alpha { all.clone() } else { copy_pipeline_with(device, &layout, raw_format(self.format), wgpu::ColorWrites::COLOR) };
            (layout, all, last)
        });
        let first_input = match &self.plain {
            Some(p) if self.blits.is_some() => &p.raw[0],
            _ => &self.targets[0].raw,
        };
        let fill = device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("render_fx fill"), layout: fill_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(first_input) }] });
        self.kept = Some(Kept { _texture: texture, view, copy, fill });
        self.kept_made += 1;
    }

    /// One copy pass from `src` into `dst`.
    fn blit(encoder: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup, pipeline: &wgpu::RenderPipeline, dst: &wgpu::TextureView) {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("render_fx copy"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: dst, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, bind, &[]);
        rp.draw(0..3, 0..1);
    }

    /// Makes the bind groups for the textures just made: the passes read the same views every
    /// frame, so `run` makes none.
    fn remake_binds(&mut self, device: &wgpu::Device) {
        let plain = match (&self.blits, &self.plain) {
            (Some(b), Some(p)) => Some((b, p)),
            _ => None,
        };
        self.binds = (0..self.passes.len())
            .map(|i| {
                let input = match plain {
                    Some((_, p)) => &p.raw[i % 2],
                    None => &self.targets[i % 2].raw,
                };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("render_fx"),
                    layout: &self.bind_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(input) },
                        wgpu::BindGroupEntry { binding: 1, resource: self.passes[i].uniforms.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                })
            })
            .collect();
        self.copy_binds = match plain {
            Some((b, p)) => {
                let last = self.passes.len().saturating_sub(1);
                let make = |src: &wgpu::TextureView| device.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("render_fx copy"), layout: &b.layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(src) }] });
                let result = self.kept.as_ref().map_or(&p.raw[(last + 1) % 2], |k| &k.view);
                vec![make(&p.frame), make(result)]
            }
            None => Vec::new(),
        };
        self.binds_made += (self.binds.len() + self.copy_binds.len()) as u64;
    }

    /// How many bind groups this chain has made (they follow the textures, not the frames).
    pub fn bind_groups_made(&self) -> u64 {
        self.binds_made
    }

    /// How many uniform buffer writes `run` has made.
    pub fn uniform_writes(&self) -> u64 {
        self.uniform_writes
    }

    /// Runs every pass; the last writes `out`, a non-sRGB view of the window texture.
    pub fn run(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, uniforms: &Uniforms, out: &wgpu::TextureView) {
        self.run_with(device, queue, None, uniforms, out, Shade::Full);
    }

    /// `run`, shading only `rects` when the kept output allows it: the chain has region terms
    /// and at most two passes (`remake_kept`), and its last run was this window's frame at this
    /// size. The caller passes rects only for an animation frame of a kept scene,
    /// united with last frame's (its glow outside them must go back to the plain picture).
    pub fn run_in(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, uniforms: &Uniforms, out: &wgpu::TextureView, rects: Option<&[PxRect]>) {
        self.run_with(device, queue, None, uniforms, out, rects.map_or(Shade::Full, Shade::Kept));
    }

    /// `run_in` for any `Shade`, writing the uniforms through the renderer's upload path when
    /// one is given. A fresh frame's rects need only a kept output (every pixel is written).
    pub fn run_with(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, mut uploads: Option<&mut glyphon::UploadBelt>, uniforms: &Uniforms, out: &wgpu::TextureView, shade: Shade<'_>) {
        // Nothing to draw into yet (no frame_views call).
        if self.targets.is_empty() && self.plain.is_none() {
            return;
        }
        let (rects, fresh) = match shade {
            Shade::Full => (None, false),
            Shade::Kept(r) => (Some(r).filter(|_| self.kept.is_some() && self.kept_ok), false),
            Shade::Fresh(r) => (Some(r).filter(|_| self.kept.is_some() && !self.region_terms.as_ref().is_some_and(|t| t.contains(&RegionTerm::StillLooks))), true),
        };
        // The uniform buffers hold what the last frame packed; only a change is written.
        let staged = std::mem::take(&mut self.staged);
        for (i, pass) in self.passes.iter().enumerate() {
            if let Some(layout) = &pass.layout {
                if !staged {
                    uniforms.pack_into(layout, &mut self.scratch[i]);
                }
                let (packed, last) = (&self.scratch[i], &self.last_packed[i]);
                if packed != last {
                    // A frame that moved only the clock writes just its span.
                    let range = upload_range(&layout.clock_span(), packed, last);
                    let (at, bytes) = (range.start, &packed[range]);
                    match uploads.as_deref_mut() {
                        Some(up) => up.write(device, &pass.uniforms, at as u64, bytes),
                        None => queue.write_buffer(&pass.uniforms, at as u64, bytes),
                    }
                    // The old bytes become the next frame's scratch: no allocation.
                    std::mem::swap(&mut self.scratch[i], &mut self.last_packed[i]);
                    self.uniform_writes += 1;
                }
            }
        }
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("render_fx") });
        let last = self.passes.len().saturating_sub(1);
        // Without view formats every pass reads and writes the plain pair, and the copies wrap them.
        let plain = match (&self.blits, &self.plain) {
            (Some(b), Some(p)) => Some((b, p)),
            (Some(_), None) => return,
            (None, _) => None,
        };
        // An animation-only region frame finds the decoded frame still there: with a kept output
        // no pass writes it.
        if let Some((b, p)) = plain.filter(|_| rects.is_none() || fresh) {
            Self::blit(&mut encoder, &self.copy_binds[0], &b.decode, &p.raw[0]);
        }
        // A full frame no next frame builds on writes the window directly (the copy passes
        // without view formats run anyway, so there the kept output stays).
        let skip_kept = rects.is_none() && !self.keep_next && plain.is_none();
        let kept = self.kept.as_ref().map(|k| &k.view).filter(|_| !skip_kept);
        // An animation-only region frame keeps every pixel outside its rects from the last frame.
        let load = if rects.is_some() && !fresh { wgpu::LoadOp::Load } else { wgpu::LoadOp::Clear(wgpu::Color::BLACK) };
        // A fresh region frame starts each pass's output as the new frame, which is what every
        // pass leaves outside the effect's region.
        let fill = match (&self.fill, &self.kept) {
            (Some((_, all, last_mask)), Some(k)) if fresh && rects.is_some() => Some((all, last_mask, &k.fill)),
            _ => None,
        };
        for (i, pass) in self.passes.iter().enumerate() {
            // Ping-pong: pass i reads target i % 2 (its bind group) and writes the other, or the
            // kept output or the window when it is the last.
            let output = match plain {
                Some((_, p)) if i == last => kept.unwrap_or(&p.raw[(i + 1) % 2]),
                Some((_, p)) => &p.raw[(i + 1) % 2],
                None if i == last => kept.unwrap_or(out),
                None => &self.targets[(i + 1) % 2].raw,
            };
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("render_fx"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some((all, last_mask, bind)) = fill {
                rp.set_pipeline(if i == last { last_mask } else { all });
                rp.set_bind_group(0, bind, &[]);
                rp.draw(0..3, 0..1);
            }
            rp.set_pipeline(&pass.pipeline);
            rp.set_bind_group(0, &self.binds[i], &[]);
            match rects {
                Some(rects) => {
                    let (sw, sh) = self.size;
                    for r in rects {
                        // A scissor past the target is a validation error.
                        let (x, y) = (r.x.min(sw), r.y.min(sh));
                        let (w, h) = (r.w.min(sw - x), r.h.min(sh - y));
                        if w > 0 && h > 0 {
                            rp.set_scissor_rect(x, y, w, h);
                            rp.draw(0..3, 0..1);
                        }
                    }
                }
                None => rp.draw(0..3, 0..1),
            }
        }
        match (plain, &self.kept) {
            (Some((b, _)), _) => Self::blit(&mut encoder, &self.copy_binds[1], &b.encode, out),
            (None, Some(Kept { copy: Some((_, pipeline, bind)), .. })) if !skip_kept => Self::blit(&mut encoder, bind, pipeline, out),
            (None, _) => {}
        }
        match uploads {
            Some(up) => {
                up.submit(queue, [encoder.finish()]);
            }
            None => {
                queue.submit([encoder.finish()]);
            }
        }
        self.kept_ok = self.kept.is_some() && !skip_kept;
        self.shaded_region = rects.is_some();
        self.ran = true;
    }
}

#[cfg(not(target_os = "macos"))]
/// The kept output's copy into the window's raw view (`COPY`).
fn copy_pipeline(device: &wgpu::Device, format: wgpu::TextureFormat, write_mask: wgpu::ColorWrites) -> (wgpu::BindGroupLayout, wgpu::RenderPipeline) {
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("render_fx kept"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        }],
    });
    let pipeline = copy_pipeline_with(device, &layout, format, write_mask);
    (layout, pipeline)
}

#[cfg(not(target_os = "macos"))]
/// `copy_pipeline` for a layout already made.
fn copy_pipeline_with(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, write_mask: wgpu::ColorWrites) -> wgpu::RenderPipeline {
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("render_fx kept"), bind_group_layouts: &[Some(layout)], ..Default::default() });
    let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("render_fx vertex"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(VERTEX)) });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("render_fx kept"), source: wgpu::ShaderSource::Wgsl(Cow::Borrowed(COPY)) });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("render_fx kept"),
        layout: Some(&pl),
        vertex: wgpu::VertexState { module: &vertex, entry_point: Some("vs_main"), compilation_options: Default::default(), buffers: &[] },
        fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs_main"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask })] }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(not(target_os = "macos"))]
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

/// The bytes of a changed uniform buffer to upload: just the clock span when everything outside
/// it equals what the buffer holds, else all of it.
pub(crate) fn upload_range(span: &std::ops::Range<usize>, packed: &[u8], last: &[u8]) -> std::ops::Range<usize> {
    let only_clock = packed.len() == last.len() && span.end <= packed.len() && packed[..span.start] == last[..span.start] && packed[span.end..] == last[span.end..];
    if only_clock { span.clone() } else { 0..packed.len() }
}

#[cfg(test)]
mod upload_tests {
    use super::*;
    use crate::render_fx::Uniforms;

    #[test]
    fn a_clock_only_change_uploads_the_span_and_anything_else_the_whole_buffer() {
        let (list, _) = crate::render_fx::discover(None);
        let layout = list.iter().filter(|p| p.builtin()).flat_map(|p| p.compile().unwrap()).find_map(|c| crate::render_fx::uniform_layout(&c.module)).unwrap();
        let span = layout.clock_span();
        let mut u = Uniforms::default();
        let mut held = Vec::new();
        u.pack_into(&layout, &mut held);
        // A new frame: the clock and the cursor move.
        u.time += 0.5;
        u.frame += 1;
        u.current_cursor = [4.0, 5.0, 6.0, 7.0];
        let mut next = Vec::new();
        u.pack_into(&layout, &mut next);
        let r = upload_range(&span, &next, &held);
        assert_eq!(r, span, "only the span");
        let mut buffer = held.clone();
        buffer[r.clone()].copy_from_slice(&next[r]);
        assert_eq!(buffer, next, "the buffer after the partial write is the full pack");
        // A color moves too: everything goes.
        u.background = [0.25; 3];
        u.pack_into(&layout, &mut next);
        assert_eq!(upload_range(&span, &next, &held), 0..next.len());
        // The first frame (nothing held) writes it all.
        assert_eq!(upload_range(&span, &next, &[]), 0..next.len());
    }
}
