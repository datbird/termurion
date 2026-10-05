//! Draws a scene Frame: rectangles with a small wgpu pipeline, text with glyphon.

use glyphon::{
    Attrs, Buffer, Cache, Color, ContentType, CustomGlyph, Family, FontSystem, Metrics, RasterizeCustomGlyphRequest, RasterizedCustomGlyph,
    Resolution, Shaping, Style, SwashCache, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport, Weight, Wrap,
};
use wgpu::util::DeviceExt;

use glyphon::cosmic_text::CacheKeyFlags;

use crate::fonts;
use crate::scene::{Frame, Quad, Span, TextRow};

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
        ImagePipeline { pipeline, screen_group, texture_layout, sampler, repeat_sampler, cache: Default::default(), uploads: 0, frames: 0 }
    }

    /// Makes sure every picture in `images` has a texture; drops ones long unused.
    fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, images: &[crate::scene::ImageQuad]) {
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

/// The most glyph placements kept for gap cutting.
const INK_CACHE_MAX: usize = 16384;

/// The face every row without a pane of its own draws with, and the one `set_fonts` makes.
pub const DEFAULT_FACE: u16 = 0;

/// The most faces one renderer keeps; past it `face` hands back the nearest size.
const MAX_FACES: usize = 512;
/// The names `resolve_family` and `has_family` remember before they start over.
const LOOKUP_CAP: usize = 4096;

/// The ids glyphon caches Tandem's custom glyphs by. Its cache key is only the id, the size and
/// the subpixel bin, and every `TextArea` here draws at scale 1, so the device scale (which sets
/// a stroke's thickness) is part of the id: a character at another scale is another glyph, and a
/// mask is never reused across a DPI change. The rasterizer maps an id back to its pair.
#[derive(Default)]
pub struct CustomIds {
    ids: std::collections::HashMap<(char, u32), u16>,
    list: Vec<(char, f32)>,
}

impl CustomIds {
    /// The id of `c` at `scale`; `None` once the table is full (the glyph is skipped).
    pub fn id(&mut self, c: char, scale: f32) -> Option<u16> {
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let key = (c, scale.to_bits());
        if let Some(id) = self.ids.get(&key) {
            return Some(*id);
        }
        let id = u16::try_from(self.list.len()).ok().filter(|id| *id < u16::MAX)?;
        self.ids.insert(key, id);
        self.list.push((c, scale));
        Some(id)
    }

    /// The character and scale an id stands for.
    pub fn get(&self, id: u16) -> Option<(char, f32)> {
        self.list.get(id as usize).copied()
    }
}

/// What makes two faces the same: the fonts, the pixel size, the spacings, and the profile's
/// drawing switches later tasks pack into `profile_bits`. The size is in physical pixels,
/// rounded to 1/64 pixel (`FaceKey::new`), so a key built twice from the same profile is equal.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct FaceKey {
    pub family: String,
    pub alt_family: Option<String>,
    pub px_bits: u32,
    pub h_bits: u32,
    pub v_bits: u32,
    pub profile_bits: u64,
    /// The profile's Use Bold Font and Use Italic Font (a face that ignores a switch is another face).
    pub use_bold: bool,
    pub use_italic: bool,
    /// The profile's ASCII Ligatures and Non-ASCII Ligatures (a face that ignores a switch is
    /// another face, as with bold).
    pub ascii_ligatures: bool,
    pub non_ascii_ligatures: bool,
    /// The profile's ASCII Anti Aliased and Non-ASCII Anti Aliased (a face that ignores a switch
    /// is another face, as with bold).
    pub ascii_aa: bool,
    pub non_ascii_aa: bool,
    /// The profile's Thin Strokes setting (0 to 4).
    pub thin: u8,
    /// The profile's Draw Powerline Glyphs (a face that ignores the switch is another face).
    pub powerline: bool,
    /// The profile's Special Font Config, normalized (`font_table::normalized`): the table itself
    /// and not a hash of it, so two tables never share a face (P5).
    pub table: Option<std::sync::Arc<str>>,
}

impl FaceKey {
    pub fn new(family: &str, alt_family: Option<&str>, px: f32, h: f32, v: f32) -> FaceKey {
        FaceKey { family: family.to_string(), alt_family: alt_family.map(str::to_string), px_bits: quantize_px(px).to_bits(), h_bits: h.to_bits(), v_bits: v.to_bits(), profile_bits: 0, use_bold: true, use_italic: true, ascii_ligatures: false, non_ascii_ligatures: false, ascii_aa: true, non_ascii_aa: true, thin: 4, powerline: false, table: None }
    }

    pub fn px(&self) -> f32 {
        f32::from_bits(self.px_bits)
    }
}

/// A pixel size to 1/64 pixel: faces re-measured after a scale change land on the keys the app
/// builds for the new scale.
pub fn quantize_px(px: f32) -> f32 {
    (px * 64.0).round() / 64.0
}

/// A measured face: what `draw` shapes a row with and what the cursor and underlines read.
#[derive(Clone, Debug)]
pub struct Face {
    pub key: FaceKey,
    /// The cell with the spacings applied.
    pub cell: (f32, f32),
    /// The font's own cell, without the spacings (`cellSizeWithoutSpacing`).
    pub base: (f32, f32),
    /// The underline numbers at this size (zero when the face did not load).
    pub ul: crate::underline::Metrics,
    /// The baseline's distance from the cell top (`ul.baseline`).
    pub baseline: f32,
    pub px: f32,
    /// The weight of the bold face the family has for this regular face (`fonts::bold_weight`);
    /// None when no weight is heavy enough and bold is double-struck.
    pub bold: Option<u16>,
    pub use_bold: bool,
    pub use_italic: bool,
    /// The Normal Font's and the Non-ASCII Font's PostScript names are on iTerm2's ligature
    /// blacklist (`fonts::blacklisted`).
    pub blacklisted: bool,
    pub alt_blacklisted: bool,
    /// The profile's Special Font Config, and the font each of its entries draws in (same order).
    pub table: Option<std::sync::Arc<crate::font_table::Config>>,
    pub table_fonts: Vec<TableFont>,
}

/// One Special Font Config entry's font, resolved once when the face is built.
#[derive(Clone, Debug, PartialEq)]
pub struct TableFont {
    /// The installed family; None when the font is not installed (the face's own fonts draw).
    pub family: Option<String>,
    /// The entry's size in physical pixels; None draws at the face's size.
    pub px: Option<f32>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    quads: QuadPipeline,
    images: ImagePipeline,
    font_system: FontSystem,
    swash: SwashCache,
    /// Each glyph's ink placement (left, top, width, height) from the swash image, keyed by the
    /// glyph; `None` for a glyph with no ink. Cleared past `INK_CACHE_MAX`.
    ink_cache: std::collections::HashMap<glyphon::cosmic_text::CacheKey, Option<(i32, i32, u32, u32)>>,
    atlas: TextAtlas,
    viewport: Viewport,
    text: TextRenderer,
    /// One text buffer per row slot, with the spans it last shaped.
    buffers: Vec<(Buffer, Vec<Span>, f32, f32, u16)>,
    /// A porthole's prose font: Segoe UI Variable, else Segoe UI.
    prose_family: String,
    /// The badge font (bold): the default profile's Badge Font, Arial when not installed.
    badge_family: String,
    /// The toolbelt's list font: `toolbeltFont` when installed, else Consolas.
    toolbelt_family: String,
    /// The toolbelt font's size in points (`toolbeltFontSize`, 0 meaning 11).
    toolbelt_pt: f32,
    /// Widths of prose words already measured: (text, pixel size bits, bold) to pixels.
    widths: std::collections::HashMap<(String, u32, bool), f32>,
    /// Widths of runs already measured in the toolbelt font: (text, pixel size bits) to pixels.
    toolbelt_widths: WidthCache,
    family: String,
    alt_family: Option<String>,
    /// Segoe Fluent Icons (Windows 11), else Segoe MDL2 Assets (Windows 10, same code points).
    icon_family: String,
    h_spacing: f32,
    v_spacing: f32,
    font_px: f32,
    pub cell: (f32, f32),
    /// The primary face's underline numbers (`set_font_px`).
    ul_metrics: Option<crate::underline::Metrics>,
    /// The font's own cell, without the spacings (`fonts::cell_size`).
    base_cell: (f32, f32),
    /// `FontsForGenerousRounding`: the families that get the generous line height.
    generous_list: String,
    /// `MinimumWeightDifferenceForBoldFont`, read when faces are built.
    bold_difference: i32,
    /// `EnableContextualAlternates`: ligature level 2 instead of 1.
    calt: bool,
    /// Whether each family's PostScript name is on the ligature blacklist, looked up once.
    blacklist: std::cell::RefCell<std::collections::HashMap<String, bool>>,
    /// The default face's bold weight (`bold_face`).
    default_bold: Option<u16>,
    /// The weights each family's regular-style faces have (a variable face counts every 100-step
    /// of its range), looked up once per family.
    weights: std::cell::RefCell<std::collections::HashMap<String, Vec<u16>>>,
    /// The texture format the pipelines draw into.
    format: wgpu::TextureFormat,
    /// The faces, by id; id 0 is the default face `set_fonts` makes.
    faces: Vec<Face>,
    face_ids: std::collections::HashMap<FaceKey, u16>,
    /// Memoized `resolve_family` and `has_family`: one scan of the installed fonts per name.
    resolved: std::cell::RefCell<std::collections::HashMap<String, String>>,
    installed: std::cell::RefCell<std::collections::HashMap<String, bool>>,
    scans: std::cell::Cell<usize>,
    /// The device scale the faces' sizes are in (`set_scale`).
    scale: f32,
    /// The ids of the box and block glyphs drawn so far (`draw`).
    custom: CustomIds,
    /// An empty buffer: the `TextArea`s that carry only custom glyphs need one.
    blank: Buffer,
    /// The face cap was reached and logged.
    capped: bool,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat, family: &str, font_px: f32) -> Renderer {
        // Aliased and thin-stroke glyph masks are filtered in the vendored glyphon (patch 1); the
        // filter is process-wide, so a second window's renderer sets the same function again.
        glyphon::set_mask_filter(|flags, alpha| fonts::filter_mask(fonts::MaskMode::of_flags(flags), alpha));
        let cache = Cache::new(device);
        let viewport = Viewport::new(device, &cache);
        let mut atlas = TextAtlas::new(device, queue, &cache, format);
        let text = TextRenderer::new(&mut atlas, device, wgpu::MultisampleState::default(), None);
        let quads = QuadPipeline::new(device, format);
        let images = ImagePipeline::new(device, format, &quads.uniform);
        let mut font_system = FontSystem::new();
        let blank = Buffer::new(&mut font_system, Metrics::new(12.0, 14.0));
        let mut r = Renderer {
            device: device.clone(),
            queue: queue.clone(),
            quads,
            images,
            font_system,
            swash: SwashCache::new(),
            ink_cache: std::collections::HashMap::new(),
            atlas,
            viewport,
            text,
            buffers: Vec::new(),
            family: family.to_string(),
            format,
            alt_family: None,
            icon_family: String::new(),
            prose_family: String::new(),
            badge_family: "Arial".into(),
            toolbelt_family: "Consolas".into(),
            toolbelt_pt: 11.0,
            widths: std::collections::HashMap::new(),
            toolbelt_widths: WidthCache::default(),
            h_spacing: 1.0,
            v_spacing: 1.0,
            font_px,
            cell: (0.0, 0.0),
            ul_metrics: None,
            base_cell: (0.0, 0.0),
            generous_list: "consolas".into(),
            bold_difference: 4,
            calt: true,
            blacklist: Default::default(),
            default_bold: None,
            weights: Default::default(),
            faces: Vec::new(),
            face_ids: std::collections::HashMap::new(),
            resolved: Default::default(),
            installed: Default::default(),
            scans: Default::default(),
            scale: 1.0,
            custom: CustomIds::default(),
            blank,
            capped: false,
        };
        r.prose_family = if r.has_family("Segoe UI Variable") { "Segoe UI Variable" } else { "Segoe UI" }.to_string();
        r.icon_family = if r.has_family("Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" }.to_string();
        r.set_font_px(font_px);
        r
    }

    /// Sets the font size and measures the cell: the advance of "W" by the font's own line
    /// height (`fonts::cell_size`; R3, it was 1.2 times the size).
    /// The width is not rounded. Glyphs advance by the exact width, so a rounded cell
    /// would drift away from the text along a row (the cursor landed two cells short).
    pub fn set_font_px(&mut self, px: f32) {
        self.font_px = px;
        let family = self.family.clone();
        let (cell, base, ul) = self.measure(&family, px, self.h_spacing, self.v_spacing);
        (self.cell, self.base_cell, self.ul_metrics) = (cell, base, ul);
        self.default_bold = self.bold_face(&family);
        self.sync_default_face();
        self.buffers.clear();
        self.widths.clear();
    }

    /// A font's vertical metrics and the advance of one shaped "W" at `px` pixels (iTerm2
    /// measures "W", `FontSizeEstimator.m:75-120`), with the font the "W" came from for the
    /// underline numbers. None when the family has no glyph for "W".
    fn read_metrics(&mut self, family: &str, px: f32) -> Option<(fonts::Metrics, crate::underline::Metrics)> {
        let mut b = Buffer::new(&mut self.font_system, Metrics::new(px, px * 1.2));
        b.set_size(None, None);
        b.set_wrap(Wrap::None);
        b.set_text("W", &Attrs::new().family(Family::Name(family)), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.font_system, false);
        let run = b.layout_runs().next()?;
        let g = run.glyphs.first()?;
        let advance_w = run.line_w;
        let font = self.font_system.get_font(g.font_id, g.font_weight)?;
        let m = font.as_swash().metrics(&[]).scale(px);
        let metrics = fonts::Metrics { ascent: m.ascent, descent: m.descent, leading: m.leading, advance_w };
        let ul = crate::underline::Metrics { underline_offset: m.underline_offset.abs(), thickness: m.stroke_size, x_height: m.x_height, baseline: 0.0 };
        (advance_w > 0.0).then_some((metrics, ul))
    }

    /// The metrics `fonts::cell_size` is fed for a family at `px` (tests).
    pub fn face_font_metrics(&mut self, family: &str, px: f32) -> Option<fonts::Metrics> {
        self.read_metrics(family, px).map(|(m, _)| m)
    }

    /// A face's cell, spacing-free cell and underline numbers at `px` pixels.
    fn measure(&mut self, family: &str, px: f32, h: f32, v: f32) -> ((f32, f32), (f32, f32), Option<crate::underline::Metrics>) {
        let generous = fonts::generous(family, &self.generous_list);
        let Some((m, mut ul)) = self.read_metrics(family, px) else {
            // The family did not load: a plain 1.2 line with 0.6 em advance, until it does.
            let m = fonts::Metrics { ascent: px * 0.95, descent: px * 0.25, leading: 0.0, advance_w: px * 0.6 };
            let (cell, base) = fonts::cell_size(m, false, h, v);
            return (cell, base, None);
        };
        let (cell, base) = fonts::cell_size(m, generous, h, v);
        ul.baseline = fonts::baseline(m, cell.1);
        (cell, base, Some(ul))
    }

    /// A new `FontsForGenerousRounding`: every face is measured again in place (ids stay valid).
    pub fn set_generous_list(&mut self, list: &str) {
        if list == self.generous_list {
            return;
        }
        self.generous_list = list.to_string();
        self.set_font_px(self.font_px);
        for i in 1..self.faces.len() {
            let key = self.faces[i].key.clone();
            self.faces[i] = self.build_face(&key);
        }
        self.buffers.clear();
        self.widths.clear();
    }

    /// A new `MinimumWeightDifferenceForBoldFont`: every face picks its bold weight again in
    /// place (ids stay valid).
    pub fn set_bold_difference(&mut self, difference: i32) {
        if difference == self.bold_difference {
            return;
        }
        self.bold_difference = difference;
        self.default_bold = self.bold_face(&self.family.clone());
        for i in 0..self.faces.len() {
            let family = self.faces[i].key.family.clone();
            self.faces[i].bold = self.bold_face(&family);
        }
        self.buffers.clear();
    }

    /// The weights of `family`'s regular-style faces, sorted (looked up once per family).
    fn family_weights(&mut self, family: &str) -> Vec<u16> {
        if let Some(w) = self.weights.borrow().get(family) {
            return w.clone();
        }
        let db = self.font_system.db();
        let found: Vec<(glyphon::fontdb::ID, u16)> = db
            .faces()
            .filter(|f| f.style == glyphon::fontdb::Style::Normal && f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(family)))
            .map(|f| (f.id, f.weight.0))
            .collect();
        let mut out = Vec::new();
        for (id, weight) in found {
            // A variable face has one entry in the database and every weight of its axis.
            let range = self.font_system.get_font(id, glyphon::fontdb::Weight(weight)).and_then(|font| {
                let wght = u32::from_be_bytes(*b"wght");
                font.as_swash().variations().find(|v| v.tag() == wght).map(|v| (v.min_value(), v.max_value()))
            });
            match range {
                Some((lo, hi)) if hi > lo => out.extend(fonts::range_weights(lo.max(0.0) as u16, hi.min(1000.0) as u16)),
                _ => out.push(weight),
            }
        }
        out.sort_unstable();
        out.dedup();
        self.weights.borrow_mut().insert(family.to_string(), out.clone());
        out
    }

    /// The bold weight `family` has for its regular face, None when it needs the double strike.
    fn bold_face(&mut self, family: &str) -> Option<u16> {
        let weights = self.family_weights(family);
        let regular = fonts::regular_weight(&weights)?;
        fonts::bold_weight(regular, &weights, self.bold_difference)
    }

    /// The face a key describes, measured now.
    fn build_face(&mut self, key: &FaceKey) -> Face {
        let px = key.px();
        let (cell, base, ul) = self.measure(&key.family, px, f32::from_bits(key.h_bits), f32::from_bits(key.v_bits));
        let ul = ul.unwrap_or(crate::underline::Metrics { underline_offset: 0.0, thickness: 0.0, x_height: 0.0, baseline: 0.0 });
        let blacklisted = self.family_blacklisted(&key.family);
        let alt_blacklisted = key.alt_family.as_deref().map_or(blacklisted, |f| self.family_blacklisted(f));
        let (table, table_fonts) = self.table_of(key);
        Face { key: key.clone(), cell, base, baseline: ul.baseline, ul, px, bold: self.bold_face(&key.family), use_bold: key.use_bold, use_italic: key.use_italic, blacklisted, alt_blacklisted, table, table_fonts }
    }

    /// The Special Font Config a key carries and the font of each entry: every name is resolved
    /// through the memoized lookups (at most `font_table::MAX_ENTRIES` of them, once per name), and
    /// a name that is not installed draws as the face does. A key whose text does not parse has no
    /// table (the app only builds keys from `font_table::normalized`).
    fn table_of(&self, key: &FaceKey) -> (Option<std::sync::Arc<crate::font_table::Config>>, Vec<TableFont>) {
        let Some(config) = key.table.as_deref().and_then(crate::font_table::Config::parse).filter(|c| !c.entries.is_empty()) else { return (None, Vec::new()) };
        let fonts = config
            .entries
            .iter()
            .map(|e| {
                let family = self.resolve_family(&e.font_name);
                TableFont { family: self.has_family(&family).then_some(family), px: e.point_size.map(|pt| quantize_px(pt as f32 * 4.0 / 3.0 * self.scale)) }
            })
            .collect();
        (Some(std::sync::Arc::new(config)), fonts)
    }

    /// Whether the family's regular face (the one nearest weight 400) has a PostScript name on
    /// iTerm2's ligature blacklist; looked up once per family.
    fn family_blacklisted(&self, family: &str) -> bool {
        if let Some(b) = self.blacklist.borrow().get(family) {
            return *b;
        }
        let db = self.font_system.db();
        let name = db
            .faces()
            .filter(|f| f.style == glyphon::fontdb::Style::Normal && f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(family)))
            .min_by_key(|f| ((f.weight.0 as i32 - 400).abs(), f.weight.0))
            .map(|f| f.post_script_name.clone());
        let b = name.is_some_and(|n| fonts::blacklisted(&n));
        self.blacklist.borrow_mut().insert(family.to_string(), b);
        b
    }

    /// `EnableContextualAlternates` changed: faces keep their ids, rows shape again.
    pub fn set_contextual_alternates(&mut self, calt: bool) {
        if calt != self.calt {
            self.calt = calt;
            self.buffers.clear();
        }
    }

    /// The anti-aliasing rules of face `id` for `scene::DrawPrefs`: the profile's switches and
    /// Thin Strokes, with the window's Retina-ness and the global force switch.
    pub fn face_masks(&self, id: u16, retina: bool, force_aa_on_retina: bool) -> fonts::MaskRules {
        let k = &self.face_of(id).key;
        fonts::MaskRules { use_non_ascii_font: k.alt_family.is_some(), ascii_aa: k.ascii_aa, non_ascii_aa: k.non_ascii_aa, retina, force_aa_on_retina, thin: i64::from(k.thin) }
    }

    /// The Special Font Config of face `id` (None: the face has no table).
    pub fn face_table(&self, id: u16) -> Option<std::sync::Arc<crate::font_table::Config>> {
        self.face_of(id).table.clone()
    }

    /// The installed font families, each once, for the Special Exceptions window's picker. Scans
    /// the font database: called when the window opens, never per frame.
    pub fn installed_families(&self) -> Vec<String> {
        let mut names: Vec<String> = self.font_system.db().faces().flat_map(|f| f.families.iter().map(|(n, _)| n.clone())).collect();
        names.sort();
        names.dedup();
        names
    }

    /// Whether face `id`'s profile draws Powerline symbols itself.
    pub fn face_powerline(&self, id: u16) -> bool {
        self.face_of(id).key.powerline
    }

    /// The ligature rules of face `id` for `scene::DrawPrefs`: the profile's two switches, the
    /// font's level, and the Non-ASCII Font's level.
    pub fn face_ligatures(&self, id: u16, prefer_speed: bool) -> (fonts::LigatureRules, u8) {
        let f = self.face_of(id);
        let level = fonts::ligature_level(f.blacklisted, self.calt);
        let alt = fonts::ligature_level(f.alt_blacklisted, self.calt);
        (fonts::LigatureRules { ascii: f.key.ascii_ligatures, non_ascii: f.key.non_ascii_ligatures, prefer_speed, level }, alt)
    }

    /// Face 0 is the default fonts, kept in step with the fields `set_font_px` fills.
    fn sync_default_face(&mut self) {
        let mut key = FaceKey::new(&self.family, self.alt_family.as_deref(), self.font_px, self.h_spacing, self.v_spacing);
        key.profile_bits = 0;
        let ul = self.ul_metrics.unwrap_or(crate::underline::Metrics { underline_offset: 0.0, thickness: 0.0, x_height: 0.0, baseline: 0.0 });
        let face = Face { key: key.clone(), cell: self.cell, base: self.base_cell, baseline: ul.baseline, ul, px: self.font_px, bold: self.default_bold, use_bold: key.use_bold, use_italic: key.use_italic, blacklisted: false, alt_blacklisted: false, table: None, table_fonts: Vec::new() };
        if self.faces.is_empty() {
            self.faces.push(face);
        } else {
            let old = std::mem::replace(&mut self.faces[0], face);
            if self.face_ids.get(&old.key) == Some(&DEFAULT_FACE) {
                self.face_ids.remove(&old.key);
            }
        }
        self.face_ids.insert(key, DEFAULT_FACE);
    }

    /// The primary face's underline metrics in pixels (all zero when the face did not load, which
    /// `scene::DrawPrefs` reads as "derive from the cell").
    pub fn underline_metrics(&mut self) -> crate::underline::Metrics {
        self.ul_metrics.unwrap_or(crate::underline::Metrics { underline_offset: 0.0, thickness: 0.0, x_height: 0.0, baseline: 0.0 })
    }

    /// The font's own cell, without the spacings: what iTerm2 calls `cellSizeWithoutSpacing`.
    pub fn base_cell(&self) -> (f32, f32) {
        self.base_cell
    }

    /// The installed family for an iTerm2 font name, which is usually a PostScript
    /// name ("JetBrainsMono-Regular"). Tries a family name, then the installed fonts'
    /// PostScript names, then a guess from the dashes.
    pub fn resolve_family(&self, name: &str) -> String {
        if let Some(r) = self.resolved.borrow().get(name) {
            return r.clone();
        }
        self.scans.set(self.scans.get() + 1);
        let r = self.scan_family(name);
        let mut resolved = self.resolved.borrow_mut();
        // A script that keeps naming new fonts (a Special Font Config set over the API) cannot grow this without end.
        if resolved.len() >= LOOKUP_CAP {
            resolved.clear();
        }
        resolved.insert(name.to_string(), r.clone());
        r
    }

    /// `resolve_family`'s scan of the installed fonts, run once per name.
    fn scan_family(&self, name: &str) -> String {
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

    /// A porthole run's width: the cell width per column for the profile's font, the shaped
    /// width for the prose font (remembered per word).
    pub fn text_width(&mut self, text: &str, prose: bool, scale: f32) -> f32 {
        self.text_width_in(DEFAULT_FACE, text, prose, scale)
    }

    /// `text_width` in a pane's face: its cell, and its size for the prose font.
    pub fn text_width_in(&mut self, face: u16, text: &str, prose: bool, scale: f32) -> f32 {
        use unicode_width::UnicodeWidthStr;
        if !prose {
            return text.width() as f32 * self.face_cell(face).0 * scale;
        }
        self.prose_width_in(face, text, false, scale)
    }

    /// A text's shaped width in the prose font, bold or not (remembered per text).
    pub fn prose_width(&mut self, text: &str, bold: bool, scale: f32) -> f32 {
        self.prose_width_in(DEFAULT_FACE, text, bold, scale)
    }

    /// `prose_width` at a pane's face size.
    pub fn prose_width_in(&mut self, face: u16, text: &str, bold: bool, scale: f32) -> f32 {
        let (px, ch) = {
            let f = self.face_of(face);
            (f.px, f.cell.1)
        };
        let key = (text.to_string(), (px * scale).to_bits(), bold);
        if let Some(w) = self.widths.get(&key) {
            return *w;
        }
        let mut b = Buffer::new(&mut self.font_system, Metrics::new(px * scale, ch * scale));
        b.set_size(None, None);
        b.set_wrap(Wrap::None);
        let weight = if bold { Weight::BOLD } else { Weight::NORMAL };
        b.set_text(text, &Attrs::new().family(Family::Name(&self.prose_family)).weight(weight), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.font_system, false);
        let w = b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        if self.widths.len() > 200_000 {
            self.widths.clear();
        }
        self.widths.insert(key, w);
        w
    }

    /// The terminal font's size in pixels.
    pub fn font_px(&self) -> f32 {
        self.font_px
    }

    /// The badge font: `name` when installed (Helvetica is not on Windows), else Arial.
    pub fn set_badge_family(&mut self, name: &str) {
        let resolved = self.resolve_family(name);
        self.badge_family = if self.has_family(&resolved) { resolved } else { "Arial".into() };
    }

    /// The toolbelt font: `name` when installed (Menlo, iTerm2's default, is not on Windows),
    /// else Consolas, Windows' monospaced system font.
    pub fn set_toolbelt_family(&mut self, name: &str) {
        let resolved = self.resolve_family(name);
        self.toolbelt_family = if self.has_family(&resolved) { resolved } else { "Consolas".into() };
        self.toolbelt_widths.clear();
    }

    /// The toolbelt font's size in points; 0 is the system default, 11 points
    /// (`smallSystemFontSize`).
    pub fn set_toolbelt_size(&mut self, points: f32) {
        self.toolbelt_pt = if points > 0.0 { points } else { 11.0 };
    }

    /// The toolbelt font's size in pixels at `scale`.
    pub fn toolbelt_px(&self, scale: f32) -> f32 {
        self.toolbelt_pt * 4.0 / 3.0 * scale
    }

    /// A run's width in the toolbelt font at its size, at `scale` (remembered: the tools measure
    /// their rows every frame).
    pub fn toolbelt_width(&mut self, text: &str, scale: f32) -> f32 {
        let px = self.toolbelt_px(scale).max(1.0);
        if let Some(w) = self.toolbelt_widths.get(text, px.to_bits()) {
            return w;
        }
        let mut b = Buffer::new(&mut self.font_system, Metrics::new(px, px * 1.2));
        b.set_size(None, None);
        b.set_wrap(Wrap::None);
        b.set_text(text, &Attrs::new().family(Family::Name(&self.toolbelt_family)), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.font_system, false);
        let w = b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max);
        self.toolbelt_widths.insert(text, px.to_bits(), w);
        w
    }

    /// A key for the toolbelt font at `scale` (family and pixel size): a width measured with
    /// one key holds for as long as the key does (`text_edit::WrapCache`).
    pub fn toolbelt_font_key(&self, scale: f32) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        self.toolbelt_family.hash(&mut h);
        self.toolbelt_px(scale).max(1.0).to_bits().hash(&mut h);
        h.finish()
    }

    /// A badge line's width at `px` pixels, in the bold badge font.
    pub fn badge_width(&mut self, text: &str, px: f32) -> f32 {
        let mut b = Buffer::new(&mut self.font_system, Metrics::new(px.max(1.0), px.max(1.0) * 1.2));
        b.set_size(None, None);
        b.set_wrap(Wrap::None);
        b.set_text(text, &Attrs::new().family(Family::Name(&self.badge_family)).weight(Weight::BOLD), Shaping::Advanced, None);
        b.shape_until_scroll(&mut self.font_system, false);
        b.layout_runs().map(|r| r.line_w).fold(0.0, f32::max)
    }

    /// True when a font family with this name is installed.
    pub fn has_family(&self, name: &str) -> bool {
        if let Some(r) = self.installed.borrow().get(name) {
            return *r;
        }
        self.scans.set(self.scans.get() + 1);
        let r = self.font_system.db().faces().any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)));
        let mut installed = self.installed.borrow_mut();
        if installed.len() >= LOOKUP_CAP {
            installed.clear();
        }
        installed.insert(name.to_string(), r);
        r
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

    /// The id of the face for `key`, measured on first use (the GPU thread; never per frame).
    /// Past `MAX_FACES` a new key gets the existing face of its family nearest in size (the
    /// table never shrinks and never renumbers, so an id held in a slice stays valid).
    pub fn face(&mut self, key: &FaceKey) -> u16 {
        if let Some(id) = self.face_ids.get(key) {
            return *id;
        }
        if self.faces.len() >= MAX_FACES {
            if !std::mem::replace(&mut self.capped, true) {
                crate::host::log("fonts: too many faces; new sizes use the nearest one");
            }
            let nearest = (0..self.faces.len())
                .filter(|i| self.faces[*i].key.family == key.family)
                .min_by(|a, b| (self.faces[*a].px - key.px()).abs().total_cmp(&(self.faces[*b].px - key.px()).abs()));
            return nearest.unwrap_or(0) as u16;
        }
        let face = self.build_face(key);
        let id = self.faces.len() as u16;
        self.faces.push(face);
        self.face_ids.insert(key.clone(), id);
        id
    }

    fn face_of(&self, id: u16) -> &Face {
        self.faces.get(id as usize).unwrap_or(&self.faces[0])
    }

    /// The cell of face `id` (the default face's for an id that is not one).
    pub fn face_cell(&self, id: u16) -> (f32, f32) {
        self.face_of(id).cell
    }

    /// The font's own cell of face `id`, without the spacings.
    pub fn face_base(&self, id: u16) -> (f32, f32) {
        self.face_of(id).base
    }

    /// The bold weight of face `id` (None: bold is double-struck).
    pub fn face_bold(&self, id: u16) -> Option<u16> {
        self.face_of(id).bold
    }

    /// The underline numbers of face `id` (all zero when it did not load).
    pub fn face_underline(&self, id: u16) -> crate::underline::Metrics {
        self.face_of(id).ul
    }

    /// The scale the faces are measured at; faces re-measure in place when it changes: the same
    /// ids, new cells (a key the app builds at the new scale finds its face again).
    pub fn set_scale(&mut self, scale: f32) {
        if scale == self.scale || self.scale <= 0.0 {
            self.scale = scale;
            return;
        }
        let ratio = scale / self.scale;
        self.scale = scale;
        for i in 1..self.faces.len() {
            let mut key = self.faces[i].key.clone();
            key.px_bits = quantize_px(key.px() * ratio).to_bits();
            let face = self.build_face(&key);
            self.faces[i] = face;
        }
        self.face_ids.clear();
        for (i, f) in self.faces.iter().enumerate() {
            self.face_ids.insert(f.key.clone(), i as u16);
        }
        self.buffers.clear();
        self.widths.clear();
    }

    /// How many times `resolve_family` or `has_family` scanned the installed fonts (tests).
    pub fn resolve_scans(&self) -> usize {
        self.scans.get()
    }

    /// How many faces exist (tests).
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// How many image textures were uploaded (tests).
    pub fn uploads(&self) -> usize {
        self.images.uploads
    }

    /// Draws a frame into an offscreen texture and returns its pixels as RGBA, row by row.
    pub fn draw_offscreen(&mut self, frame: &Frame, w: u32, h: u32) -> Result<Vec<u8>, String> {
        let max = self.device.limits().max_texture_dimension_2d;
        if w == 0 || h == 0 || w > max || h > max {
            return Err(format!("an image of {w} x {h} pixels is larger than the graphics card allows ({max})"));
        }
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        self.draw(frame, &view, w, h)?;
        // Rows are copied out padded to 256 bytes, then the padding is dropped.
        let row = w * 4;
        let padded = row.div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("offscreen read"),
            size: (padded * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        self.queue.submit(Some(enc.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| format!("waiting for the graphics card: {e:?}"))?;
        rx.recv().map_err(|e| e.to_string())?.map_err(|e| format!("reading the image back: {e:?}"))?;
        let data = buf.slice(..).get_mapped_range().map_err(|e| format!("reading the image back: {e:?}"))?.to_vec();
        let bgra = matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            let line = &data[y * padded as usize..y * padded as usize + row as usize];
            if bgra {
                for p in line.chunks_exact(4) {
                    out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            } else {
                out.extend_from_slice(line);
            }
        }
        Ok(out)
    }

    /// `frame.underlines` cut around the ink of the glyphs in their row (`SolidUnderlines` off). Only
    /// rows that carry a stroke are measured, and each glyph's ink placement is looked up once and
    /// kept in `ink_cache`, so a steady screen costs a hash lookup per glyph, not a rasterization.
    fn gapped_underlines(&mut self, frame: &Frame, rows: &[TextRow]) -> Vec<Quad> {
        if !frame.gap_underlines || frame.underlines.is_empty() {
            return frame.underlines.clone();
        }
        if self.ink_cache.len() > INK_CACHE_MAX {
            self.ink_cache.clear();
        }
        let mut bands = Vec::new();
        for (slot, row) in self.buffers.iter().zip(rows) {
            let ch = self.faces.get(row.face as usize).unwrap_or(&self.faces[0]).cell.1;
            // A badge is drawn behind the text; iTerm2's mask never sees it.
            if row.spans.iter().any(|s| s.badge) {
                continue;
            }
            let (top, bottom) = (row.y, row.y + ch * row.scale);
            let (left, right) = (row.clip.x, row.clip.x + row.clip.w);
            let holds = |q: &Quad| q.rect.y >= top - 0.5 && q.rect.y + q.rect.h <= bottom + 0.5 && q.rect.x < right && q.rect.x + q.rect.w > left;
            if !frame.underlines.iter().any(holds) {
                continue;
            }
            let mut inks = Vec::new();
            for run in slot.0.layout_runs() {
                let line_y = run.line_y.round() as i32;
                for g in run.glyphs.iter() {
                    let p = g.physical((row.x, row.y), 1.0);
                    let swash = &mut self.swash;
                    let fonts = &mut self.font_system;
                    let place = *self.ink_cache.entry(p.cache_key).or_insert_with(|| {
                        swash.get_image_uncached(fonts, p.cache_key).filter(|i| i.placement.width > 0 && i.placement.height > 0).map(|i| (i.placement.left, i.placement.top, i.placement.width, i.placement.height))
                    });
                    if let Some((l, t, w, h)) = place {
                        inks.push(((p.x + l) as f32, (p.y + line_y - t) as f32, w as f32, h as f32));
                    }
                }
            }
            crate::underline::merge_band(&mut bands, crate::underline::Band { top, bottom, left, right, inks });
        }
        crate::underline::cut_quads_around(&frame.underlines, &bands, 1.0)
    }

    /// The second copies of bold terminal text whose face has no bold weight (iTerm2's `fakeBold`
    /// double strike): one row per such row, one physical pixel to the right, carrying only the
    /// bold spans (every other span is blank advance of the same width, so nothing else is
    /// struck twice). They come after `frame.rows`, which keeps the row indexes the underline
    /// gaps read (`gapped_underlines`) and never take part in those gaps.
    fn double_strikes(&self, rows: &[TextRow]) -> Vec<TextRow> {
        use unicode_width::UnicodeWidthChar;
        let struck = |s: &Span| s.fake_bold && s.bold && !(s.icon || s.badge || s.toolbelt || s.prose);
        let blank = |text: &str| text.chars().map(|c| " ".repeat(c.width().unwrap_or(0))).collect::<String>();
        let mut out = Vec::new();
        for row in rows {
            let face = self.face_of(row.face);
            if !(face.use_bold && face.bold.is_none() && row.spans.iter().any(struck)) {
                continue;
            }
            // iTerm2 shifts an anti-aliased copy by `antiAliasedShift` and an aliased one by a
            // pixel (`iTermTextDrawingHelper.m:2340`): one copy per shift among the row's struck spans.
            let mut shifts: Vec<f32> = Vec::new();
            for s in row.spans.iter().filter(|s| struck(s)) {
                let shift = fonts::double_strike_shift(fonts::MaskMode::from_code(s.mask), self.scale);
                if !shifts.contains(&shift) {
                    shifts.push(shift);
                }
            }
            for shift in shifts {
                let hit = |s: &Span| struck(s) && fonts::double_strike_shift(fonts::MaskMode::from_code(s.mask), self.scale) == shift;
                out.push(TextRow {
                    x: row.x + shift,
                    spans: row.spans.iter().map(|s| if hit(s) { s.clone() } else { Span { text: blank(&s.text), bold: false, italic: false, fake_bold: false, ..s.clone() } }).collect(),
                    ..row.clone()
                });
            }
        }
        out
    }

    /// A row whose spans differ in ligature level, as one row per run of equal level: cosmic-text
    /// shapes a whole word with the features of its first span, so a run at another level must
    /// be shaped on its own. Each run sits at its cell (the width of the text before it, in
    /// cells), and a row of one level is left as it is.
    fn split_levels(&self, rows: &[TextRow]) -> Vec<TextRow> {
        use unicode_width::UnicodeWidthChar;
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            if row.spans.windows(2).all(|w| w[0].ligatures == w[1].ligatures) {
                out.push(row.clone());
                continue;
            }
            let cw = self.face_of(row.face).cell.0 * row.scale;
            let mut at = 0.0;
            let mut run: Vec<Span> = Vec::new();
            let mut start = 0.0;
            for s in &row.spans {
                if run.last().is_some_and(|l| l.ligatures != s.ligatures) {
                    out.push(TextRow { x: row.x + start, spans: std::mem::take(&mut run), ..row.clone() });
                    start = at;
                }
                run.push(s.clone());
                at += s.text.chars().map(|c| c.width().unwrap_or(0)).sum::<usize>() as f32 * cw;
            }
            out.push(TextRow { x: row.x + start, spans: run, ..row.clone() });
        }
        out
    }

    /// The frame's box and block glyphs as glyphon custom glyphs, grouped by clip rectangle. Each
    /// cell is snapped to whole pixels on both edges (`round(x)` to `round(x + w)`), so neighbors
    /// meet exactly at a fractional cell width; a cell of no size, or too big, is skipped.
    fn glyph_groups(&mut self, glyphs: &[crate::scene::CellGlyph]) -> Vec<(crate::layout::Rect, Vec<CustomGlyph>)> {
        let mut groups: Vec<(crate::layout::Rect, Vec<CustomGlyph>)> = Vec::new();
        for g in glyphs {
            let (x0, x1) = (g.rect.x.round(), (g.rect.x + g.rect.w).round());
            let (y0, y1) = (g.rect.y.round(), (g.rect.y + g.rect.h).round());
            let (w, h) = (x1 - x0, y1 - y0);
            if !(w >= 1.0 && h >= 1.0 && w <= 1024.0 && h <= 1024.0) {
                continue;
            }
            let Some(id) = self.custom.id(g.c, self.scale) else { continue };
            let glyph = CustomGlyph { id, left: x0, top: y0, width: w, height: h, color: Some(Color::rgb(g.color.r, g.color.g, g.color.b)), snap_to_physical_pixel: true, metadata: 0 };
            match groups.iter_mut().rev().find(|(clip, _)| *clip == g.clip) {
                Some((_, list)) => list.push(glyph),
                None => groups.push((g.clip, vec![glyph])),
            }
        }
        groups
    }

    pub fn draw(&mut self, frame: &Frame, view: &wgpu::TextureView, width: u32, height: u32) -> Result<(), String> {
        let (_, ch) = self.cell;
        let metrics = Metrics::new(self.font_px, ch);
        let mixed = frame.rows.iter().any(|r| r.spans.windows(2).any(|w| w[0].ligatures != w[1].ligatures));
        let split;
        let rows: &[TextRow] = if mixed {
            split = self.split_levels(&frame.rows);
            &split
        } else {
            &frame.rows
        };
        let doubles = self.double_strikes(rows);
        let all_rows: Vec<&TextRow> = rows.iter().chain(&doubles).collect();
        while self.buffers.len() < all_rows.len() {
            self.buffers.push((Buffer::new(&mut self.font_system, metrics), Vec::new(), 0.0, 1.0, DEFAULT_FACE));
        }
        for (slot, row) in self.buffers.iter_mut().zip(all_rows.iter().copied()) {
            let (buffer, last, last_w, last_scale, last_face) = slot;
            if *last == row.spans && *last_w == row.clip.w && *last_scale == row.scale && *last_face == row.face {
                continue;
            }
            // The row's own face: its fonts, pixel size and cell (a row of another face is never
            // shaped with these).
            let face = self.faces.get(row.face as usize).unwrap_or(&self.faces[0]);
            let ((cw, ch), px) = (face.cell, face.px);
            let (family, alt_family) = (face.key.family.clone(), face.key.alt_family.clone());
            let (bold_weight, use_bold, use_italic) = (face.bold, face.use_bold, face.use_italic);
            let table_fonts = if row.spans.iter().any(|s| s.font != 0) { face.table_fonts.clone() } else { Vec::new() };
            let metrics = Metrics::new(px * row.scale, ch * row.scale);
            buffer.set_metrics_and_size(metrics, Some(row.clip.w + cw * row.scale), Some(ch * row.scale));
            buffer.set_wrap(Wrap::None);
            // The toolbelt font keeps its own advance (it is not the terminal's cell).
            buffer.set_monospace_width((!row.spans.iter().any(|s| s.toolbelt)).then_some(cw));
            let base = Attrs::new().family(Family::Name(&family));
            let alt = alt_family.as_deref().map(|f| Attrs::new().family(Family::Name(f)));
            let icons = Attrs::new().family(Family::Name(&self.icon_family));
            let prose = Attrs::new().family(Family::Name(&self.prose_family));
            let badge = Attrs::new().family(Family::Name(&self.badge_family));
            let toolbelt = Attrs::new().family(Family::Name(&self.toolbelt_family));
            let spans = row.spans.iter().map(|s| {
                // A terminal span a Special Font Config entry draws takes that entry's font and size
                // (an entry past the table, or a font that is not installed, draws as the face does).
                let entry = (s.font as usize).checked_sub(1).and_then(|i| table_fonts.get(i)).filter(|_| !(s.icon || s.badge || s.toolbelt || s.prose));
                let start = if let Some(family) = entry.and_then(|e| e.family.as_deref()) {
                    let attrs = Attrs::new().family(Family::Name(family));
                    match entry.and_then(|e| e.px) {
                        Some(px) => attrs.metrics(Metrics::new(px * row.scale, ch * row.scale)),
                        None => attrs,
                    }
                } else if s.icon {
                    icons.clone()
                } else if s.badge {
                    badge.clone()
                } else if s.toolbelt {
                    toolbelt.clone()
                } else if s.prose {
                    prose.clone()
                } else if s.alt_font {
                    alt.clone().unwrap_or_else(|| base.clone())
                } else {
                    base.clone()
                };
                // Terminal text follows its profile's switches (Use Bold Font, Use Italic Font);
                // chrome and prose never do. A terminal bold span draws in the face's bold weight,
                // or in the regular one when none is heavy enough (the double strike is a second row).
                let terminal = !(s.icon || s.badge || s.toolbelt || s.prose);
                let weight = if !s.bold {
                    Weight::NORMAL
                } else if s.fake_bold && terminal {
                    match (use_bold, bold_weight) {
                        (false, _) | (true, None) => Weight::NORMAL,
                        (true, Some(w)) => Weight(w),
                    }
                } else {
                    Weight::BOLD
                };
                let slant = s.italic && (use_italic || !terminal);
                // Terminal text shapes with its ligature level's features. Non-ASCII text at level 0
                // is one cell per piece (or a joined run of a cursive script), so it keeps the
                // shaper's own features: nothing can ligate there and Arabic keeps its forms.
                let start = if terminal && (s.ligatures > 0 || s.text.is_ascii()) { start.font_features(fonts::features(s.ligatures)) } else { start };
                // Terminal text carries its mask mode in the glyph cache key (`from_bits_retain`
                // keeps Tandem's bits), so each mode is cached apart and filtered in glyphon.
                let start = if terminal { start.cache_key_flags(CacheKeyFlags::from_bits_retain(fonts::MaskMode::from_code(s.mask).flags())) } else { start };
                let attrs = start
                    .color(Color::rgb(s.color.r, s.color.g, s.color.b))
                    .weight(weight)
                    .style(if slant { Style::Italic } else { Style::Normal });
                (s.text.as_str(), attrs)
            });
            buffer.set_rich_text(spans, &base, Shaping::Advanced, None);
            buffer.shape_until_scroll(&mut self.font_system, false);
            *last = row.spans.clone();
            *last_w = row.clip.w;
            *last_scale = row.scale;
            *last_face = row.face;
        }

        self.viewport.update(&self.queue, Resolution { width, height });
        let groups = self.glyph_groups(&frame.glyphs);
        let areas = self.buffers.iter().zip(all_rows.iter().copied()).map(|((buffer, _, _, _, _), row)| TextArea {
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
        // Box and block glyphs: one more `TextArea` per clip rectangle, an empty buffer carrying the
        // glyphs as custom glyphs (rasterized by `boxdraw` on request, cached by id and size).
        let glyph_areas = groups.iter().map(|(clip, glyphs)| TextArea {
            buffer: &self.blank,
            left: 0.0,
            top: 0.0,
            scale: 1.0,
            bounds: TextBounds { left: clip.x.round() as i32, top: clip.y.round() as i32, right: (clip.x + clip.w).round() as i32, bottom: (clip.y + clip.h).round() as i32 },
            default_color: Color::rgb(255, 255, 255),
            custom_glyphs: glyphs,
        });
        let custom = &self.custom;
        self.text
            .prepare_with_custom(&self.device, &self.queue, &mut self.font_system, &mut self.atlas, &self.viewport, areas.chain(glyph_areas), &mut self.swash, |request: RasterizeCustomGlyphRequest| {
                // Total for every interned id: the same mask each time, never `None` once it was `Some`.
                let (c, scale) = custom.get(request.id)?;
                Some(RasterizedCustomGlyph { data: crate::boxdraw::rasterize(c, request.width, request.height, scale), content_type: ContentType::Mask })
            })
            .map_err(|e| format!("text prepare: {e:?}"))?;

        self.queue.write_buffer(&self.quads.uniform, 0, bytemuck::cast_slice(&[width as f32, height as f32, 0.0, 0.0]));
        let under = QuadPipeline::vertices(&frame.quads);
        // Underline strokes lie under the overlay (the dimming, the cursor extras); with gaps on they
        // break around the ink of the glyphs in their row, strikethrough stays whole.
        let underlines = self.gapped_underlines(frame, rows);
        let over = QuadPipeline::vertices(&underlines.iter().chain(&frame.strikes).chain(&frame.scrollbars).chain(&frame.overlay).copied().collect::<Vec<_>>());
        let pops = QuadPipeline::vertices(&frame.popups);
        let make = |v: &[f32]| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(v),
                usage: wgpu::BufferUsages::VERTEX,
            })
        };
        let (under_buf, over_buf, pops_buf) = (make(&under), make(&over), make(&pops));
        // Background images and inline images share one texture cache.
        let all: Vec<crate::scene::ImageQuad> = frame.backgrounds.iter().chain(&frame.images).cloned().collect();
        self.images.prepare(&self.device, &self.queue, &all);
        let picture = ImagePipeline::vertices(&frame.images);
        let picture_buf = make(&picture);
        let backdrop = ImagePipeline::vertices(&frame.backgrounds);
        let backdrop_buf = make(&backdrop);

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
            if !backdrop.is_empty() {
                pass.set_pipeline(&self.images.pipeline);
                pass.set_bind_group(0, &self.images.screen_group, &[]);
                pass.set_vertex_buffer(0, backdrop_buf.slice(..));
                for (i, q) in frame.backgrounds.iter().enumerate() {
                    let Some((group, _)) = self.images.cache.get(&(q.key, q.repeat)) else { continue };
                    pass.set_bind_group(1, group, &[]);
                    let first = (i * 6) as u32;
                    pass.draw(first..first + 6, 0..1);
                }
            }
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
                    let Some((group, _)) = self.images.cache.get(&(q.key, q.repeat)) else { continue };
                    pass.set_bind_group(1, group, &[]);
                    let first = (i * 6) as u32;
                    pass.draw(first..first + 6, 0..1);
                }
            }
            // Popup boxes cover images and quads; their text and what they cut stay on top.
            if !pops.is_empty() {
                pass.set_pipeline(&self.quads.pipeline);
                pass.set_bind_group(0, &self.quads.bind_group, &[]);
                pass.set_vertex_buffer(0, pops_buf.slice(..));
                pass.draw(0..(pops.len() / 6) as u32, 0..1);
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

/// Measured widths, (text, size bits) to pixels, capped by the bytes of text held: a run
/// longer than `WIDTH_CACHE_LONGEST` is measured every time (a wrapped Notes line is short; a
/// whole pasted paragraph is not worth keeping), and past `WIDTH_CACHE_BYTES` the cache starts
/// over.
#[derive(Default)]
struct WidthCache {
    map: std::collections::HashMap<(String, u32), f32>,
    bytes: usize,
}

const WIDTH_CACHE_LONGEST: usize = 512;
const WIDTH_CACHE_BYTES: usize = 8 << 20;

impl WidthCache {
    fn get(&self, text: &str, size: u32) -> Option<f32> {
        if text.len() > WIDTH_CACHE_LONGEST {
            return None;
        }
        self.map.get(&(text.to_string(), size)).copied()
    }

    fn insert(&mut self, text: &str, size: u32, w: f32) {
        if text.len() > WIDTH_CACHE_LONGEST {
            return;
        }
        if self.bytes + text.len() > WIDTH_CACHE_BYTES {
            self.clear();
        }
        if self.map.insert((text.to_string(), size), w).is_none() {
            self.bytes += text.len();
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Rect;
    use crate::palette::rgb;
    use crate::scene::{Quad, Span, rgba};

    #[test]
    fn cell_metrics_keep_the_spacing_free_cell() {
        let close = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4;
        let m = fonts::Metrics { ascent: 12.4, descent: 3.2, leading: 0.0, advance_w: 9.6 };
        let (cell, base) = fonts::cell_size(m, false, 1.0, 1.0);
        assert!(close(cell, (9.6, 16.0)) && close(base, (9.6, 16.0)), "{cell:?} {base:?}");
        let (cell, base) = fonts::cell_size(m, false, 1.2, 1.25);
        assert!(close(cell, (11.52, 20.0)) && close(base, (9.6, 16.0)), "{cell:?} {base:?}");
    }

    #[test]
    fn cells_follow_font_metrics() {
        // R3: the cell height is the font's own (swash metrics, generous list), not round(px * 1.2).
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Consolas", 16.0);
        r.set_fonts("Consolas", None, 16.0, 1.0, 1.0);
        let m = r.face_font_metrics("Consolas", 16.0).expect("Consolas is installed on the test machine");
        let (cell, base) = fonts::cell_size(m, fonts::generous("Consolas", "consolas"), 1.0, 1.0);
        assert_eq!(r.cell, cell);
        assert_eq!(r.base_cell(), base);
        assert_ne!(r.cell.1, (16.0f32 * 1.2).round(), "still the old 1.2 line");
        // The width is the unrounded advance of "W".
        assert!((r.cell.0 - m.advance_w).abs() < 1e-3);
        // Underline baseline is where the shaped line puts the glyphs.
        let mut b = Buffer::new(&mut r.font_system, Metrics::new(16.0, r.cell.1));
        b.set_size(None, None);
        b.set_text("W", &Attrs::new().family(Family::Name("Consolas")), Shaping::Advanced, None);
        b.shape_until_scroll(&mut r.font_system, false);
        let y = b.layout_runs().next().unwrap().line_y;
        assert!((r.face_underline(DEFAULT_FACE).baseline - y).abs() < 0.51, "{} vs {y}", r.face_underline(DEFAULT_FACE).baseline);
        // Cascadia Mono is not in the generous list: ascent plus descent.
        let c = r.face(&key("Cascadia Mono", 16.0, 1.0, 1.0));
        if let Some(m) = r.face_font_metrics("Cascadia Mono", 16.0) {
            assert_eq!(r.face_cell(c), fonts::cell_size(m, false, 1.0, 1.0).0);
        }
        // A new generous list re-measures every face in place.
        r.set_generous_list("");
        assert_eq!(r.face_cell(c), fonts::cell_size(r.face_font_metrics("Cascadia Mono", 16.0).unwrap(), true, 1.0, 1.0).0);
        r.set_generous_list("consolas");
    }

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

    /// Today's backend per adapter, and whether a default instance and a DX12 `DxgiFromVisual` one
    /// make devices in one process (13g renderer ruling). Headless: no event loop, so both
    /// instances are built without a display handle.
    #[test]
    fn adapters_are_recorded() {
        let default = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        desc.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
        let visual = wgpu::Instance::new(desc);
        let describe = |label: &str, instance: &wgpu::Instance| {
            let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all()));
            if adapters.is_empty() {
                println!("ADAPTER {label}: no adapter");
            }
            for a in &adapters {
                let i = a.get_info();
                println!("ADAPTER {label}: backend={:?} name={} vendor={:#06x} device_type={:?}", i.backend, i.name, i.vendor, i.device_type);
            }
            adapters
        };
        let first = describe("default", &default);
        let second = describe("dx12-visual", &visual);
        let device_of = |label: &str, instance: &wgpu::Instance| {
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
            let made = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()));
            let i = adapter.get_info();
            println!("DEVICE {label}: picked {:?} {} -> {}", i.backend, i.name, if made.is_ok() { "created" } else { "failed" });
            made.ok()
        };
        let a = device_of("default", &default);
        let b = device_of("dx12-visual", &visual);
        if first.is_empty() || second.is_empty() {
            println!("COEXIST: skipped (an instance has no adapter)");
        } else {
            println!("COEXIST: both devices alive at once = {}", a.is_some() && b.is_some());
        }
        // The record must be in the rulings file.
        let rulings = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/docs/superpowers/rulings/m13g-rulings.txt"))
            .unwrap_or_default();
        assert!(rulings.contains("Today's backend per adapter on EXTREME:"), "m13g-rulings.txt must record the backend");
    }

    fn key(family: &str, px: f32, h: f32, v: f32) -> FaceKey {
        FaceKey::new(family, None, px, h, v)
    }

    #[test]
    fn faces_follow_profiles() {
        let (device, queue) = device();
        let format = wgpu::TextureFormat::Rgba8UnormSrgb;
        let mut r = Renderer::new(&device, &queue, format, "Cascadia Mono", 16.0);
        r.set_fonts("Cascadia Mono", None, 16.0, 1.0, 1.0);
        assert_eq!(r.face_cell(DEFAULT_FACE), r.cell, "the default face is what set_fonts made");
        assert_eq!(r.face(&key("Consolas", 16.0, 1.0, 1.0)), 1);
        assert_eq!(r.face(&key("Consolas", 16.0, 1.0, 1.0)), 1, "a key is measured once");
        assert_eq!(r.face(&key("Consolas", 20.0, 1.0, 1.0)), 2);
        assert!(r.face_cell(2).1 > r.face_cell(1).1, "{:?} {:?}", r.face_cell(2), r.face_cell(1));
        assert!(r.face_cell(2).0 > r.face_cell(1).0);
        // Two rows, one in each face: both draw, the larger one wider.
        let (w, h) = (256u32, 64u32);
        let tex = target(&device, w, h);
        let view = tex.create_view(&Default::default());
        let (c1, c2) = (r.face_cell(1), r.face_cell(2));
        let block = |x: f32, y: f32, face: u16| TextRow {
            x,
            y,
            spans: vec![Span { text: "\u{2588}".repeat(4), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }],
            clip: Rect { x: 0.0, y, w: w as f32, h: if face == 1 { c1.1 } else { c2.1 } },
            scale: 1.0,
            face,
        };
        let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), rows: vec![block(0.0, 0.0, 1), block(0.0, c1.1 + 2.0, 2)], ..Default::default() };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let right = |y0: f32, y1: f32| {
            (0..w).rev().find(|&x| (y0 as u32..y1 as u32).any(|y| px[((y * w + x) * 4) as usize] > 128)).map(|x| x as f32 + 1.0)
        };
        let one = right(0.0, c1.1).expect("the face 1 row drew");
        let two = right(c1.1 + 2.0, c1.1 + 2.0 + c2.1).expect("the face 2 row drew");
        assert!((one - 4.0 * c1.0).abs() < c1.0 / 2.0, "face 1 ends at {one}, its cells at {}", 4.0 * c1.0);
        assert!((two - 4.0 * c2.0).abs() < c2.0 / 2.0, "face 2 ends at {two}, its cells at {}", 4.0 * c2.0);
        assert!(two > one + 2.0, "the larger face is wider: {one} {two}");
    }

    /// A span of terminal text drawn by Special Font Config entry `font` (1 plus its index).
    fn tabled(text: &str, font: u16) -> Span {
        Span { font, ..masked(text, false, 0) }
    }

    fn table_key(family: &str, entries: Vec<crate::font_table::Entry>) -> FaceKey {
        let text = crate::font_table::Config::with_entries(entries).string();
        FaceKey { table: Some(std::sync::Arc::from(text.as_str())), ..key(family, 16.0, 1.0, 1.0) }
    }

    /// Task 9: a face with a table draws a cell its table names from the table's font, in the
    /// entry's size, while the face's own family draws the rest; a font that is not installed
    /// draws as the face does.
    #[test]
    fn a_face_table_picks_the_font_per_span() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        if !r.has_family("Consolas") || !r.has_family("Cascadia Mono") {
            println!("skip: Consolas or Cascadia Mono is not installed");
            return;
        }
        let entry = |font: &str, size: Option<f64>| crate::font_table::Entry { start: 0x3A9, count: 1, destination: None, font_name: font.into(), point_size: size };
        let plain = r.face(&key("Cascadia Mono", 16.0, 1.0, 1.0));
        let tabled_face = r.face(&table_key("Cascadia Mono", vec![entry("Consolas", None)]));
        let missing_face = r.face(&table_key("Cascadia Mono", vec![entry("No Such Font Anywhere", None)]));
        let big_face = r.face(&table_key("Cascadia Mono", vec![entry("Consolas", Some(40.0))]));
        assert!(plain != tabled_face && tabled_face != missing_face && tabled_face != big_face, "a table makes a face");
        assert_eq!(r.face_table(plain).map(|t| t.entries.len()), None);
        assert_eq!(r.face_table(tabled_face).map(|t| t.entries.len()), Some(1));
        let size = (128u32, 64u32);
        let mut shot = |face: u16, font: u16| shoot(&mut r, &device, &queue, face, vec![tabled("\u{3a9}", font)], size);
        let own = shot(plain, 0);
        assert!(own.chunks(4).any(|p| p[0] > 128), "the face draws the character");
        assert_eq!(shot(tabled_face, 0), own, "a span that names no entry draws in the face's family");
        let from_table = shot(tabled_face, 1);
        assert_ne!(from_table, own, "the cell the table names draws from Consolas");
        assert_eq!(shot(missing_face, 1), own, "a font that is not installed falls back to the face's");
        let lit = |px: &[u8]| px.chunks(4).filter(|p| p[0] > 64).count();
        assert!(lit(&shot(big_face, 1)) > lit(&from_table) * 3 / 2, "the entry's size is used");
        // An entry number past the table draws as the face does, never panics.
        assert_eq!(shot(tabled_face, 9), own);
    }

    /// The installed families that have a Normal face and no weight that `bold_weight` accepts
    /// at the default difference, found from the font database (tests).
    fn family_without_bold(r: &Renderer) -> Option<String> {
        let db = r.font_system.db();
        let mut names: Vec<String> = db.faces().flat_map(|f| f.families.iter().map(|(n, _)| n.clone())).collect();
        names.sort();
        names.dedup();
        names.into_iter().find(|n| {
            let w: Vec<u16> = db.faces().filter(|f| f.style == glyphon::fontdb::Style::Normal && f.families.iter().any(|(m, _)| m == n)).map(|f| f.weight.0).collect();
            !w.is_empty() && w.iter().all(|w| *w < 600) && w.contains(&400) && db.faces().filter(|f| f.families.iter().any(|(m, _)| m == n)).all(|f| f.weight.0 < 600)
        })
    }

    #[test]
    fn bold_and_italic_switches_follow_iterm2() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let (w, h) = (128u32, 40u32);
        let tex = target(&device, w, h);
        let view = tex.create_view(&Default::default());
        // The pixels of "W" drawn in `face` as a bold and italic span as asked.
        let shot = |r: &mut Renderer, face: u16, bold: bool, italic: bool, mask: u8| -> Vec<u8> {
            let span = Span { text: "W".into(), color: rgb(255, 255, 255), bold, italic, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: bold, ligatures: 0, mask, font: 0 };
            let cell = r.face_cell(face);
            let row = TextRow { x: 4.0, y: 4.0, spans: vec![span], clip: Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 }, scale: 1.0, face };
            let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), rows: vec![row], ..Default::default() };
            r.draw(&frame, &view, w, h).unwrap();
            let _ = cell;
            read(&device, &queue, &tex, w, h)
        };
        let on = r.face(&key("Cascadia Mono", 20.0, 1.0, 1.0));
        let mut k = key("Cascadia Mono", 20.0, 1.0, 1.0);
        (k.use_bold, k.use_italic) = (false, false);
        let off = r.face(&k);
        assert_ne!(on, off, "a face that ignores a switch is another face");
        let regular = shot(&mut r, on, false, false, 0);
        // Use Bold Font off: a bold span is the regular face; on: it is not.
        assert_eq!(shot(&mut r, off, true, false, 0), regular, "bold with Use Bold Font off draws regular");
        assert_ne!(shot(&mut r, on, true, false, 0), regular, "bold with Use Bold Font on draws the bold face");
        // Use Italic Font off: italics are upright.
        assert_eq!(shot(&mut r, off, false, true, 0), regular, "italic with Use Italic Font off draws upright");
        assert_ne!(shot(&mut r, on, false, true, 0), regular, "italic with Use Italic Font on slants");
        // A family without a heavy enough bold: double-struck, one column wider.
        let Some(family) = family_without_bold(&r) else {
            println!("skip: every installed family has a bold face");
            return;
        };
        let plain = r.face(&key(&family, 20.0, 1.0, 1.0));
        assert_eq!(r.face_bold(plain), None, "{family} has no bold weight");
        let cols = |px: &[u8]| -> (u32, u32) {
            let lit = |x: u32| (0..h).any(|y| px[((y * w + x) * 4) as usize] > 128);
            ((0..w).find(|x| lit(*x)).unwrap(), (0..w).rev().find(|x| lit(*x)).unwrap())
        };
        // Aliased text is double-struck one pixel right (the anti-aliased shift has its own test).
        let regular = shot(&mut r, plain, false, false, 1);
        let bold = shot(&mut r, plain, true, false, 1);
        let (l0, r0) = cols(&regular);
        let (l1, r1) = cols(&bold);
        assert!(r1 - l1 >= r0 - l0 + 1 && l1 == l0, "{family}: regular {l0}..{r0}, bold {l1}..{r1}");
    }

    /// The glyph ids `text` shapes to in `family` with `features` (None: the attributes the
    /// renderer used before this task, no features at all). A ligature font may keep two glyphs
    /// per pair (Cascadia Code's calt swaps each for a joined half), so a ligature is told by
    /// the ids changing, not by the count.
    fn glyph_ids(fs: &mut FontSystem, family: &str, text: &str, features: Option<glyphon::cosmic_text::FontFeatures>) -> Vec<u16> {
        let mut b = Buffer::new(fs, Metrics::new(20.0, 24.0));
        b.set_size(None, None);
        b.set_wrap(Wrap::None);
        let mut attrs = Attrs::new().family(Family::Name(family));
        if let Some(f) = features {
            attrs = attrs.font_features(f);
        }
        b.set_rich_text([(text, attrs.clone())], &attrs, Shaping::Advanced, None);
        b.shape_until_scroll(fs, false);
        b.layout_runs().flat_map(|r| r.glyphs.iter().map(|g| g.glyph_id).collect::<Vec<_>>()).collect()
    }

    /// Step 1 of 13n Task 4 (the live check, headless): do "->" and "!=" form ligatures in an
    /// installed ligature font with today's attributes, and does `features(0)` switch them off?
    #[test]
    fn ligature_check_on_this_machine() {
        let mut fs = FontSystem::new();
        let installed = |fs: &FontSystem, n: &str| fs.db().faces().any(|f| f.families.iter().any(|(m, _)| m.eq_ignore_ascii_case(n)));
        let Some(family) = ["Cascadia Code", "Fira Code", "JetBrains Mono"].into_iter().find(|n| installed(&fs, n)) else {
            println!("LIGATURES TODAY: no ligature font installed");
            return;
        };
        let mut shape = |f: Option<glyphon::cosmic_text::FontFeatures>| -> Vec<Vec<u16>> { ["->", "!=", "a->b"].iter().map(|t| glyph_ids(&mut fs, family, t, f.clone())).collect() };
        let today = shape(None);
        let off = shape(Some(fonts::features(0)));
        let (l1, l2) = (shape(Some(fonts::features(1))), shape(Some(fonts::features(2))));
        // Each character alone: what "no ligature" means.
        let alone: Vec<Vec<u16>> = ["->", "!=", "a->b"].iter().map(|t| t.chars().flat_map(|c| glyph_ids(&mut fs, family, &c.to_string(), Some(fonts::features(0)))).collect()).collect();
        let report = |label: &str, v: &[Vec<u16>]| {
            println!("{label}: {family} -> glyphs={} != glyphs={} a->b glyphs={} ids {:?} ligated={}", v[0].len(), v[1].len(), v[2].len(), v, v != alone.as_slice());
        };
        report("LIGATURES TODAY", &today);
        report("LIGATURES level 1", &l1);
        report("LIGATURES level 2", &l2);
        report("LIGATURES level 0", &off);
        assert_eq!(off, alone, "features(0) must shape each character as it does alone in {family}");
        assert!(off.iter().take(2).all(|v| v.len() == 2), "each pair is two glyphs with features(0) in {family}");
    }

    #[test]
    fn ligature_levels_draw_per_span() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        if !["Cascadia Code"].iter().all(|n| r.has_family(n)) {
            println!("skip: Cascadia Code is not installed");
            return;
        }
        let (w, h) = (256u32, 40u32);
        let tex = target(&device, w, h);
        let view = tex.create_view(&Default::default());
        let face = r.face(&key("Cascadia Code", 20.0, 1.0, 1.0));
        let cw = r.face_cell(face).0;
        let sp = |text: &str, ligatures: u8| Span { text: text.into(), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures, mask: 0, font: 0 };
        let clip = Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let mut shot = |rows: Vec<TextRow>| -> Vec<u8> {
            let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), rows, ..Default::default() };
            r.draw(&frame, &view, w, h).unwrap();
            read(&device, &queue, &tex, w, h)
        };
        let row = |x: f32, spans: Vec<Span>| TextRow { x, y: 4.0, spans, clip, scale: 1.0, face };
        let flat = shot(vec![row(4.0, vec![sp("a->b", 0)])]);
        let joined = shot(vec![row(4.0, vec![sp("a->b", 2)])]);
        assert_ne!(flat, joined, "level 2 joins the arrow in Cascadia Code, level 0 does not");
        // A mixed row draws as its runs side by side, each at its cell: the arrow joins even
        // though the letters around it are at level 0.
        let mixed = shot(vec![row(4.0, vec![sp("a", 0), sp("->", 2), sp("b", 0)])]);
        let apart = shot(vec![row(4.0, vec![sp("a", 0)]), row(4.0 + cw, vec![sp("->", 2)]), row(4.0 + 3.0 * cw, vec![sp("b", 0)])]);
        assert_eq!(mixed, apart, "the runs of a mixed row draw where their cells are");
        assert_ne!(mixed, flat, "the arrow in the mixed row is joined");
    }

    #[test]
    fn faces_keep_their_ids_across_a_scale_change() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let a = r.face(&key("Consolas", 16.0, 1.0, 1.0));
        let b = r.face(&key("Cascadia Mono", 18.0, 1.0, 1.25));
        let (before, count) = (r.face_cell(a), r.face_count());
        r.set_scale(2.0);
        let after = r.face_cell(a);
        assert!(after.0 > before.0 * 1.8 && after.1 > before.1 * 1.8, "{before:?} {after:?}");
        assert_eq!(r.face_count(), count, "no face is dropped or added");
        // The keys the app builds at the new scale find the same ids.
        assert_eq!(r.face(&key("Consolas", 32.0, 1.0, 1.0)), a);
        assert_eq!(r.face(&key("Cascadia Mono", 36.0, 1.0, 1.25)), b);
        assert_eq!(r.face_count(), count);
    }

    #[test]
    fn faces_carry_their_own_cell_metrics() {
        // `underline_metrics_follow_the_pane_face` and the base of `cursor_geometry_uses_the_pane_base_cell`:
        // a face reports its own spacing-free cell and underline numbers, not the default face's.
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        r.set_fonts("Cascadia Mono", None, 16.0, 1.0, 1.0);
        let big = r.face(&key("Cascadia Mono", 32.0, 1.0, 1.0));
        let spaced = r.face(&key("Cascadia Mono", 32.0, 1.25, 1.5));
        assert_eq!(r.face_base(DEFAULT_FACE), r.base_cell());
        assert_eq!(r.face_underline(DEFAULT_FACE), r.ul_metrics.unwrap());
        // The same font at twice the size: twice the cell, twice the underline offset and x-height.
        let (d, f) = (r.face_underline(DEFAULT_FACE), r.face_underline(big));
        assert!(f.underline_offset > d.underline_offset * 1.5 && f.x_height > d.x_height * 1.5, "{d:?} {f:?}");
        assert!(r.face_base(big).0 > r.face_base(DEFAULT_FACE).0 * 1.8);
        // Spacing multiplies the cell and never the base cell.
        assert_eq!(r.face_base(spaced), r.face_base(big));
        let (cell, base) = (r.face_cell(spaced), r.face_base(spaced));
        assert!((cell.0 - base.0 * 1.25).abs() < 1e-3, "{cell:?} {base:?}");
        let m = r.face_font_metrics("Cascadia Mono", 32.0).unwrap();
        assert_eq!(cell.1, fonts::cell_size(m, false, 1.25, 1.5).0.1);
        // An id that is not a face reads the default face.
        assert_eq!(r.face_cell(999), r.cell);
    }

    #[test]
    fn face_lookup_does_not_scan_fonts_per_call() {
        let (device, queue) = device();
        let r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let before = r.resolve_scans();
        for _ in 0..1000 {
            assert_eq!(r.resolve_family("CascadiaMono-Regular"), "Cascadia Mono");
            assert!(r.has_family("Cascadia Mono"));
        }
        assert_eq!(r.resolve_scans() - before, 2, "one scan for each name, then memoized");
    }

    #[test]
    fn renders_a_range_offscreen() {
        let (device, queue) = device();
        // The window's usual format: BGRA comes back as RGBA.
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Bgra8UnormSrgb, "Cascadia Mono", 16.0);
        let frame = Frame {
            clear: rgba(rgb(0, 0, 255), 1.0),
            quads: vec![Quad { rect: Rect { x: 0.0, y: 0.0, w: 10.0, h: 5.0 }, color: rgba(rgb(255, 0, 0), 1.0) }],
            ..Default::default()
        };
        // 300 wide: rows are not a multiple of 256 bytes, so padding must be removed.
        let px = r.draw_offscreen(&frame, 300, 20).unwrap();
        assert_eq!(px.len(), 300 * 20 * 4);
        assert_eq!(&px[0..4], &[255, 0, 0, 255], "a red quad at the top left, in RGBA order");
        let at = |x: usize, y: usize| &px[(y * 300 + x) * 4..(y * 300 + x) * 4 + 4];
        assert_eq!(at(299, 19), &[0, 0, 255, 255], "the clear color elsewhere");
        assert_eq!(at(20, 2), &[0, 0, 255, 255]);
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
                spans: vec![Span { text: "\u{2588}".repeat(n), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }],
                clip: Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 },
                scale: 1.0,
                face: crate::render::DEFAULT_FACE,
            }],
            glyphs: vec![],
            overlay: vec![],
            underlines: vec![],
            strikes: vec![],
            gap_underlines: false,
            popups: vec![],
            scrollbars: vec![],
            backgrounds: vec![],
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
                spans: vec![Span { text: "\u{2588}".into(), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }],
                clip: Rect { x: 32.0, y: 0.0, w: 32.0, h: 32.0 },
                scale: 1.0,
                face: crate::render::DEFAULT_FACE,
            }],
            glyphs: vec![],
            overlay: vec![],
            underlines: vec![],
            strikes: vec![],
            gap_underlines: false,
            popups: vec![],
            scrollbars: vec![],
            backgrounds: vec![],
        };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..((y * w + x) * 4 + 3) as usize];
        assert_eq!(at(4, 4), &[255, 0, 0]);
        let white = (32..64).flat_map(|x| (0..32).map(move |y| (x, y))).any(|(x, y)| at(x, y)[0] > 200 && at(x, y)[2] > 200);
        assert!(white, "no text pixels drawn");
    }

    #[test]
    fn underlines_break_around_glyph_ink() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 24.0);
        let (cw, ch) = r.cell;
        // Rows copy out in 256-byte multiples: 64 pixels.
        let (w, h) = (((cw * 3.0).ceil() as u32 + 2).next_multiple_of(64), ch.ceil() as u32 + 2);
        let tex = target(&device, w, h);
        let view = tex.create_view(&Default::default());
        let y = (ch / 2.0).floor();
        let red = rgba(rgb(255, 0, 0), 1.0);
        let frame = |gap: bool| Frame {
            clear: rgba(rgb(0, 0, 0), 1.0),
            rows: vec![TextRow {
                x: 0.0,
                y: 0.0,
                spans: vec![Span { text: " \u{2588} ".into(), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 }],
                clip: Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 },
                scale: 1.0,
                face: crate::render::DEFAULT_FACE,
            }],
            underlines: vec![Quad { rect: Rect { x: 0.0, y, w: cw * 3.0, h: 1.0 }, color: red }],
            strikes: vec![Quad { rect: Rect { x: 0.0, y: y + 2.0, w: cw * 3.0, h: 1.0 }, color: red }],
            gap_underlines: gap,
            ..Default::default()
        };
        let look = |r: &mut Renderer, f: &Frame| {
            r.draw(f, &view, w, h).unwrap();
            let px = read(&device, &queue, &tex, w, h);
            let at = move |x: f32, y: f32| px[((y as u32 * w + x as u32) * 4) as usize..((y as u32 * w + x as u32) * 4 + 3) as usize].to_vec();
            (at(cw * 0.5, y), at(cw * 1.5, y), at(cw * 2.5, y), at(cw * 1.5, y + 2.0))
        };
        // Gaps on: the block's columns are text, the cells either side carry the line, and the
        // strikethrough crosses the block whole.
        let (left, mid, right, strike) = look(&mut r, &frame(true));
        assert_eq!((left, right), (vec![255, 0, 0], vec![255, 0, 0]));
        assert_eq!(mid, vec![255, 255, 255], "the underline breaks around the ink");
        assert_eq!(strike, vec![255, 0, 0], "strikethrough never breaks");
        // SolidUnderlines: whole.
        let (_, mid, _, _) = look(&mut r, &frame(false));
        assert_eq!(mid, vec![255, 0, 0]);
    }

    /// A stroke gaps around the glyphs of its own line: every piece of the line counts (a
    /// non-ASCII character splits a line into rows), and a badge's rows, drawn behind the text,
    /// never cut a pane's underline.
    #[test]
    fn underline_gaps_use_the_whole_line_and_never_a_badge() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 24.0);
        let (cw, ch) = r.cell;
        let (w, h) = (((cw * 3.0).ceil() as u32 + 2).next_multiple_of(64), ch.ceil() as u32 + 2);
        let tex = target(&device, w, h);
        let view = tex.create_view(&Default::default());
        let y = (ch / 2.0).floor();
        let red = rgba(rgb(255, 0, 0), 1.0);
        let clip = Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let span = |text: &str, badge: bool| Span { text: text.into(), color: rgb(255, 255, 255), bold: false, italic: false, alt_font: false, icon: false, prose: false, badge, toolbelt: false, fake_bold: false, ligatures: 0, mask: 0, font: 0 };
        let row = |x: f32, text: &str, badge: bool| TextRow { x, y: 0.0, spans: vec![span(text, badge)], clip, scale: 1.0, face: crate::render::DEFAULT_FACE };
        let frame = |rows: Vec<TextRow>| Frame {
            clear: rgba(rgb(0, 0, 0), 1.0),
            rows,
            underlines: vec![Quad { rect: Rect { x: 0.0, y, w: cw * 3.0, h: 1.0 }, color: red }],
            gap_underlines: true,
            ..Default::default()
        };
        let mid = |r: &mut Renderer, f: &Frame, col: f32| {
            r.draw(f, &view, w, h).unwrap();
            let px = read(&device, &queue, &tex, w, h);
            let i = ((y as u32 * w + (cw * col) as u32) * 4) as usize;
            px[i..i + 3].to_vec()
        };
        // A blank first piece, then the block two cells in as its own row: the third cell gaps.
        let f = frame(vec![row(0.0, " ", false), row(cw * 2.0, "\u{2588}", false)]);
        assert_eq!(mid(&mut r, &f, 2.5), vec![255, 255, 255], "ink in a later piece of the line gaps the stroke");
        assert_eq!(mid(&mut r, &f, 0.5), vec![255, 0, 0], "the blank cell keeps its line");
        // A badge row with a full block over the pane's line: the pane's underline stays whole.
        let f = frame(vec![row(0.0, "\u{2588}\u{2588}\u{2588}", true), row(0.0, " ", false)]);
        assert_eq!(mid(&mut r, &f, 1.5), vec![255, 0, 0], "badge ink never cuts a pane underline");
    }

    /// A span of terminal text in `mask` mode (0 smooth, 1 aliased, 2 thin strokes).
    fn masked(text: &str, bold: bool, mask: u8) -> Span {
        Span { text: text.into(), color: rgb(255, 255, 255), bold, italic: false, alt_font: false, icon: false, prose: false, badge: false, toolbelt: false, fake_bold: bold, ligatures: 0, mask, font: 0 }
    }

    /// Draws one row of `spans` in `face` at (4, 4) on black and reads the target back.
    fn shoot(r: &mut Renderer, device: &wgpu::Device, queue: &wgpu::Queue, face: u16, spans: Vec<Span>, (w, h): (u32, u32)) -> Vec<u8> {
        let tex = target(device, w, h);
        let view = tex.create_view(&Default::default());
        let row = TextRow { x: 4.0, y: 4.0, spans, clip: Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 }, scale: 1.0, face };
        let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), rows: vec![row], ..Default::default() };
        r.draw(&frame, &view, w, h).unwrap();
        read(device, queue, &tex, w, h)
    }

    /// The distinct red levels of a readback, sorted.
    fn levels(px: &[u8]) -> Vec<u8> {
        let mut v: Vec<u8> = px.chunks(4).map(|p| p[0]).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Step 1 of 13n Task 5 (the live check, headless: an offscreen draw, no window): does text
    /// come out smoothed today, and does an Aliased span draw exactly two alpha levels with the
    /// vendored glyphon filter in place? Run with `-- --nocapture` for the ledger line.
    #[test]
    fn antialias_check_on_this_machine() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        if !r.has_family("Consolas") {
            println!("skip: Consolas is not installed");
            return;
        }
        let face = r.face(&key("Consolas", 16.0, 1.0, 1.0));
        let size = (64u32, 40u32);
        let smooth = levels(&shoot(&mut r, &device, &queue, face, vec![masked("W", false, 0)], size));
        println!("AA TODAY: distinct alpha levels={}", smooth.len());
        assert!(smooth.len() > 2, "an unfiltered span is smoothed: {smooth:?}");
        let aliased = levels(&shoot(&mut r, &device, &queue, face, vec![masked("W", false, 1)], size));
        assert_eq!(aliased, vec![0, 255], "an Aliased span has no partial coverage");
        // Thin strokes square the coverage: still smoothed, but lighter.
        let sum = |px: &[u8]| px.chunks(4).map(|p| p[0] as u32).sum::<u32>();
        let plain = shoot(&mut r, &device, &queue, face, vec![masked("W", false, 0)], size);
        let thin = shoot(&mut r, &device, &queue, face, vec![masked("W", false, 2)], size);
        assert!(levels(&thin).len() > 2 && sum(&thin) < sum(&plain), "thin {} plain {}", sum(&thin), sum(&plain));
    }

    /// The 13m-o review ruling: a glyph atlas that grows re-rasterizes every cached glyph, and
    /// the re-rasterized masks must go through the filter too. glyphon evicts the glyphs not in
    /// use before it grows, so the aliased and thin "W"s are in the growth frame itself: they are
    /// in use when the atlas grows, and their pixels in that frame are the re-rasterized masks.
    #[test]
    fn aliased_masks_survive_atlas_growth() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        if !r.has_family("Consolas") {
            println!("skip: Consolas is not installed");
            return;
        }
        let Some(cjk) = ["Microsoft YaHei", "Yu Gothic", "MS Gothic", "SimSun", "Malgun Gothic"].into_iter().find(|f| r.has_family(f)) else {
            println!("skip: no CJK family is installed");
            return;
        };
        let face = r.face(&key("Consolas", 16.0, 1.0, 1.0));
        let size = (64u32, 40u32);
        let both = |r: &mut Renderer| {
            (
                shoot(r, &device, &queue, face, vec![masked("W", false, 1)], size),
                shoot(r, &device, &queue, face, vec![masked("W", false, 2)], size),
            )
        };
        let (aliased, thin) = both(&mut r);
        assert_eq!(levels(&aliased), vec![0, 255]);
        let before = r.atlas.mask_size();
        // Enough distinct large glyphs to pass the atlas's initial size: a screen of ideographs,
        // with the aliased and thin "W"s drawn in the same frame, in the free right-hand strip (48
        // columns of 64 pixels leave it clear even for a family whose ideographs run wide).
        let big = r.face(&key(cjk, 64.0, 1.0, 1.0));
        let (bw, bh) = (3584u32, 3584u32);
        let clip = Rect { x: 0.0, y: 0.0, w: bw as f32, h: bh as f32 };
        let ideographs: Vec<TextRow> = (0..56u32)
            .map(|i| TextRow {
                x: 0.0,
                y: (i * 64) as f32,
                spans: vec![masked(&(0..48u32).filter_map(|j| char::from_u32(0x4e00 + i * 56 + j)).collect::<String>(), false, 0)],
                clip,
                scale: 1.0,
                face: big,
            })
            .collect();
        // The same rows `shoot` draws, moved 3456 right so their 64 by 40 crops are comparable, and
        // first in the frame: they are prepared (in use) before the ideographs make the atlas grow.
        let mut rows: Vec<TextRow> = [(100.0, 1u8), (300.0, 2u8)].into_iter().map(|(y, mask)| TextRow { x: 3460.0, y, spans: vec![masked("W", false, mask)], clip, scale: 1.0, face }).collect();
        rows.extend(ideographs);
        let tex = target(&device, bw, bh);
        let view = tex.create_view(&Default::default());
        r.draw(&Frame { clear: rgba(rgb(0, 0, 0), 1.0), rows, ..Default::default() }, &view, bw, bh).unwrap();
        let after = r.atlas.mask_size();
        assert!(after > before, "the mask atlas grew: {before} to {after}");
        let px = read(&device, &queue, &tex, bw, bh);
        let crop = |top: u32| -> Vec<u8> {
            (0..size.1).flat_map(|y| px[(((top + y) * bw + 3456) * 4) as usize..(((top + y) * bw + 3456 + size.0) * 4) as usize].to_vec()).collect()
        };
        assert_eq!(levels(&crop(96)), vec![0, 255], "an aliased glyph in use while the atlas grew is still filtered");
        assert_eq!(crop(96), aliased, "its re-rasterized mask equals the first draw's");
        assert_eq!(crop(296), thin, "and so does the thin one");
        let (aliased2, thin2) = both(&mut r);
        assert_eq!(aliased2, aliased);
        assert_eq!(thin2, thin);
    }

    #[test]
    fn custom_glyph_ids_split_by_scale() {
        let mut ids = CustomIds::default();
        let a = ids.id('\u{2500}', 1.0).unwrap();
        assert_eq!(ids.id('\u{2500}', 1.0), Some(a), "one id per character and scale");
        let b = ids.id('\u{2500}', 2.0).unwrap();
        let c = ids.id('\u{2502}', 1.0).unwrap();
        assert!(a != b && a != c && b != c, "{a} {b} {c}");
        assert_eq!(ids.get(a), Some(('\u{2500}', 1.0)));
        assert_eq!(ids.get(b), Some(('\u{2500}', 2.0)));
        assert_eq!(ids.get(c), Some(('\u{2502}', 1.0)));
        assert_eq!(ids.get(60000), None);
        // A table that fills up hands out nothing more instead of reusing an id.
        let mut seen = std::collections::HashSet::new();
        for n in 0..70_000u32 {
            let scale = 1.0 + n as f32 * 0.001;
            match ids.id('\u{2500}', scale) {
                Some(id) => assert!(seen.insert(id) || n < 3, "id {id} reused"),
                None => {
                    assert!(n > 60_000, "full too early: {n}");
                    return;
                }
            }
        }
        panic!("the table never filled");
    }

    /// Lines of three fractional-width cells meet with no gap, and the second draw (a new scale
    /// and a new size) is a new mask, not the first one scaled.
    #[test]
    fn box_glyphs_draw_edge_to_edge_offscreen() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let (w, h) = (64u32, 24u32);
        let tex = target(&device, w, h);
        let view = tex.create_view(&Default::default());
        let clip = Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let cw = 9.4_f32;
        let glyphs = |c: char| -> Vec<crate::scene::CellGlyph> { (0..3).map(|i| crate::scene::CellGlyph { c, rect: Rect { x: i as f32 * cw, y: 0.0, w: cw, h: 19.0 }, color: rgb(255, 255, 255), face: 0, clip }).collect() };
        let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), glyphs: glyphs('\u{2500}'), ..Default::default() };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let red = |x: u32, y: u32| px[((y * w + x) * 4) as usize];
        let rows: Vec<u32> = (0..h).filter(|&y| red(0, y) > 200).collect();
        assert_eq!(rows, vec![9], "one pixel row, at the center of 19");
        assert!((0..28).all(|x| red(x, 9) == 255), "no gap between the cells: {:?}", (0..30).map(|x| red(x, 9)).collect::<Vec<_>>());
        assert_eq!(red(28, 9), 0, "the line ends at the third cell's right edge");
        // A block fills the cell and nothing else.
        let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), glyphs: glyphs('\u{2588}'), ..Default::default() };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let red = |x: u32, y: u32| px[((y * w + x) * 4) as usize];
        assert!((0..28).all(|x| (0..19).all(|y| red(x, y) == 255)));
        assert_eq!((red(28, 5), red(5, 19)), (0, 0));
    }

    /// A glyph's clip trims it: a popup that cut a cell leaves the rest drawn and the rest dark.
    #[test]
    fn box_glyph_clip_trims_the_mask() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let (w, h) = (64u32, 24u32);
        let tex = target(&device, w, h);
        let view = tex.create_view(&Default::default());
        let cell = Rect { x: 0.0, y: 0.0, w: 20.0, h: 20.0 };
        let glyph = |clip: Rect| crate::scene::CellGlyph { c: '\u{2588}', rect: cell, color: rgb(255, 255, 255), face: 0, clip };
        let frame = Frame { clear: rgba(rgb(0, 0, 0), 1.0), glyphs: vec![glyph(Rect { x: 0.0, y: 0.0, w: 8.0, h: 20.0 }), glyph(Rect { x: 14.0, y: 0.0, w: 6.0, h: 20.0 })], ..Default::default() };
        r.draw(&frame, &view, w, h).unwrap();
        let px = read(&device, &queue, &tex, w, h);
        let row: Vec<u8> = (0..24).map(|x| px[((5 * w + x) * 4) as usize]).collect();
        let want: Vec<u8> = (0..24).map(|x| if x < 8 || (14..20).contains(&x) { 255 } else { 0 }).collect();
        assert_eq!(row, want);
    }

    /// A box mask re-rasterized after the atlas grew is the mask it was before (glyphon panics when
    /// a re-rasterization differs in size, and a stale mask would show as a broken line).
    #[test]
    fn box_mask_is_stable_across_atlas_growth() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let Some(cjk) = ["Microsoft YaHei", "Yu Gothic", "MS Gothic", "SimSun", "Malgun Gothic"].into_iter().find(|f| r.has_family(f)) else {
            println!("skip: no CJK family is installed");
            return;
        };
        let (w, h) = (64u32, 24u32);
        let clip = Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let line = |r: &mut Renderer| {
            let tex = target(&device, w, h);
            let view = tex.create_view(&Default::default());
            let glyphs = vec![crate::scene::CellGlyph { c: '\u{256D}', rect: Rect { x: 2.0, y: 2.0, w: 10.0, h: 19.0 }, color: rgb(255, 255, 255), face: 0, clip }];
            r.draw(&Frame { clear: rgba(rgb(0, 0, 0), 1.0), glyphs, ..Default::default() }, &view, w, h).unwrap();
            read(&device, &queue, &tex, w, h)
        };
        let before_px = line(&mut r);
        assert!(before_px.chunks(4).any(|p| p[0] > 200), "the arc drew");
        let before = r.atlas.mask_size();
        let big = r.face(&key(cjk, 64.0, 1.0, 1.0));
        let (bw, bh) = (3584u32, 3584u32);
        let rows: Vec<TextRow> = (0..56u32)
            .map(|i| TextRow {
                x: 0.0,
                y: (i * 64) as f32,
                spans: vec![masked(&(0..56u32).filter_map(|j| char::from_u32(0x4e00 + i * 56 + j)).collect::<String>(), false, 0)],
                clip: Rect { x: 0.0, y: 0.0, w: bw as f32, h: bh as f32 },
                scale: 1.0,
                face: big,
            })
            .collect();
        let tex = target(&device, bw, bh);
        let view = tex.create_view(&Default::default());
        r.draw(&Frame { clear: rgba(rgb(0, 0, 0), 1.0), rows, ..Default::default() }, &view, bw, bh).unwrap();
        assert!(r.atlas.mask_size() > before, "the mask atlas grew");
        assert_eq!(line(&mut r), before_px, "the arc is the same after the atlas grew");
    }

    /// iTerm2 shifts the second copy of fake bold by `antiAliasedShift` when anti-aliased (0.5 pt
    /// on a Retina-class scale, else 0) and by one pixel when not (`iTermTextDrawingHelper.m:2340`).
    #[test]
    fn double_strike_follows_the_anti_alias_setting() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let Some(family) = family_without_bold(&r) else {
            println!("skip: every installed family has a bold face");
            return;
        };
        let plain = r.face(&key(&family, 20.0, 1.0, 1.0));
        assert_eq!(r.face_bold(plain), None);
        let row = |mask: u8| TextRow { x: 10.0, y: 0.0, spans: vec![masked("W", true, mask)], clip: Rect { x: 0.0, y: 0.0, w: 100.0, h: 40.0 }, scale: 1.0, face: plain };
        let shift = |r: &Renderer, mask: u8| r.double_strikes(&[row(mask)]).iter().map(|d| d.x - 10.0).collect::<Vec<f32>>();
        assert_eq!(shift(&r, 1), vec![1.0], "aliased: one pixel");
        assert_eq!(shift(&r, 0), vec![0.0], "anti-aliased at 1x: the copy lands in place");
        assert_eq!(shift(&r, 2), vec![0.0], "thin strokes are anti-aliased");
        r.scale = 2.0;
        assert_eq!(shift(&r, 0), vec![1.0], "anti-aliased on a Retina-class scale: 0.5 pt");
        assert_eq!(shift(&r, 1), vec![1.0]);
        // A row that mixes an aliased and an anti-aliased bold span strikes each at its own shift.
        r.scale = 1.0;
        let mixed = TextRow { spans: vec![masked("W", true, 0), masked("W", true, 1)], ..row(0) };
        let copies = r.double_strikes(&[mixed]);
        assert_eq!(copies.iter().map(|d| d.x - 10.0).collect::<Vec<f32>>(), vec![0.0, 1.0]);
        // Each copy carries only its own spans; the other is blank advance.
        assert_eq!(copies[0].spans.iter().map(|s| s.text.trim().is_empty()).collect::<Vec<_>>(), vec![false, true]);
        assert_eq!(copies[1].spans.iter().map(|s| s.text.trim().is_empty()).collect::<Vec<_>>(), vec![true, false]);
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
            repeat: false,
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

    #[test]
    fn oversized_images_are_skipped() {
        let (device, queue) = device();
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8UnormSrgb, "Cascadia Mono", 16.0);
        let tex = target(&device, 64, 16);
        let view = tex.create_view(&Default::default());
        let wide = crate::scene::ImageQuad { size: (9000, 1), pixels: std::sync::Arc::new(vec![0; 9000 * 4]), ..picture(5) };
        let frame = Frame { images: vec![wide], ..Default::default() };
        r.draw(&frame, &view, 64, 16).unwrap();
        assert_eq!(r.uploads(), 0);
    }

    #[test]
    fn width_cache_is_capped_by_bytes() {
        let mut c = WidthCache::default();
        let long = "x".repeat(WIDTH_CACHE_LONGEST + 1);
        c.insert(&long, 1, 5.0);
        assert_eq!(c.get(&long, 1), None, "a long run is not kept");
        assert_eq!(c.bytes, 0);
        c.insert("ab", 1, 2.0);
        c.insert("ab", 1, 2.0);
        assert_eq!((c.get("ab", 1), c.get("ab", 2), c.bytes), (Some(2.0), None, 2));
        let run = "y".repeat(WIDTH_CACHE_LONGEST);
        for i in 0..WIDTH_CACHE_BYTES / WIDTH_CACHE_LONGEST + 1 {
            c.insert(&run, i as u32, 1.0);
            assert!(c.bytes <= WIDTH_CACHE_BYTES);
        }
        assert_eq!(c.get("ab", 1), None, "past the cap the cache starts over");
    }
}
