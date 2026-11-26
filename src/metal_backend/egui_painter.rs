//! egui on Metal: egui-wgpu 0.36.2's `Renderer` (`update_texture`, `update_buffers`, `render` and
//! `free_texture`) drawn with Metal directly, with its shader (`egui.wgsl`, copied unchanged), its
//! blend, its non-sRGB target and its scissor rules.

use std::collections::HashMap;
use std::ops::Range;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::Message;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCullMode, MTLDevice, MTLDrawable, MTLIndexType, MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState,
    MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerMinMagFilter, MTLSamplerState, MTLScissorRect, MTLSize, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLTriangleFillMode, MTLViewport, MTLWinding,
};
use objc2_quartz_core::CAMetalDrawable;

use super::draw::{Buffer, Staging};
use super::quads::{Attr, Blend, Layout, Shader};
use super::shaders::{self, Entry, Kind};
use super::surface::{Next, Surface};
use super::MetalShared;

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Sampler = Retained<ProtocolObject<dyn MTLSamplerState>>;

/// What egui windows draw in. egui wants a target without sRGB encoding
/// (`fs_main_gamma_framebuffer`), and the wgpu window took the first such format offered.
pub const FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm;

/// `epaint::Vertex`: position, texture coordinates and an sRGB color, 20 bytes, `repr(C)`.
const VERTEX_SIZE: usize = 20;
const _: () = assert!(std::mem::size_of::<egui::epaint::Vertex>() == VERTEX_SIZE);

const ATTRS: [Attr; 3] = [
    Attr { location: 0, offset: 0, format: wgpu::VertexFormat::Float32x2 },
    Attr { location: 1, offset: 8, format: wgpu::VertexFormat::Float32x2 },
    Attr { location: 2, offset: 16, format: wgpu::VertexFormat::Uint32 },
];
const BUFFERS: [Layout; 1] = [Layout { stride: VERTEX_SIZE as u32, instance: false, attrs: &ATTRS }];
/// Group 0: the locals, seen by both stages. Group 1: the texture and its sampler.
const LOCALS: [Entry; 1] = [Entry::new(0, Kind::Buffer, true, true)];
const PICTURE: [Entry; 2] = [Entry::new(0, Kind::Texture, false, true), Entry::new(1, Kind::Sampler, false, true)];

/// egui-wgpu's first buffer sizes: 1024 vertices, 3072 indices.
const VERTEX_START: usize = VERTEX_SIZE * 1024;
const INDEX_START: usize = 4 * 1024 * 3;

/// egui-wgpu's `UniformBuffer`: the screen in points, dithering on, hardware filtering.
#[repr(C)]
struct Locals {
    screen: [f32; 2],
    dithering: u32,
    predictable_filtering: u32,
}

/// The frame's size in pixels and its scale, as egui-wgpu's `ScreenDescriptor`.
#[derive(Clone, Copy, Debug)]
pub struct ScreenDescriptor {
    pub size_in_pixels: [u32; 2],
    pub pixels_per_point: f32,
}

/// Where a frame goes.
pub enum Target<'a> {
    /// A window's drawable: drawn, then presented.
    Drawable(&'a ProtocolObject<dyn CAMetalDrawable>),
    /// A texture in `FORMAT` (tests).
    Texture(&'a ProtocolObject<dyn MTLTexture>),
    /// Nowhere: only the texture changes are applied (a measuring frame, a covered window).
    None,
}

/// A texture change not applied yet, in egui's order.
enum Op {
    Set(egui::TextureId, egui::epaint::ImageDelta),
    Free(egui::TextureId),
}

/// The slots the translation gave the locals, the sizes buffer, the texture and the sampler.
struct Slots {
    vs_locals: usize,
    vs_sizes: usize,
    fs_locals: usize,
    fs_texture: usize,
    fs_sampler: usize,
}

pub struct EguiPainter {
    pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    slots: Slots,
    vertices: Buffer,
    vertex_cap: usize,
    indices: Buffer,
    index_cap: usize,
    /// Each meshes' bytes in `vertices` and `indices`, in job order.
    slices: Vec<(Range<usize>, Range<usize>)>,
    textures: HashMap<egui::TextureId, (Texture, egui::TextureOptions)>,
    samplers: HashMap<egui::TextureOptions, Sampler>,
    /// Texture changes a failed frame did not apply: the next frame applies them first, so the
    /// GPU side never misses one egui will not send again.
    pending: Vec<Op>,
    staging: Staging,
    /// The device generation command buffer errors and failed allocations are reported for.
    generation: u64,
    /// Textures, buffers and samplers made (tests).
    made: u64,
}

fn private_buffer(device: &ProtocolObject<dyn MTLDevice>, size: usize) -> Result<Buffer, String> {
    super::new_buffer(device, size, MTLResourceOptions::StorageModePrivate).ok_or_else(|| format!("no Metal buffer of {size} bytes"))
}

/// An egui texture: RGBA8 without sRGB decoding, as egui-wgpu makes it.
fn texture(device: &ProtocolObject<dyn MTLDevice>, [w, h]: [usize; 2]) -> Result<Texture, String> {
    // SAFETY: a plain 2D descriptor; egui keeps its textures inside the side it was given.
    let desc = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(MTLPixelFormat::RGBA8Unorm, w, h, false) };
    desc.setUsage(MTLTextureUsage::ShaderRead);
    desc.setStorageMode(MTLStorageMode::Private);
    super::new_texture(device, &desc).ok_or_else(|| format!("no Metal texture of {w} x {h} for an egui image"))
}

/// egui-wgpu's `create_sampler`: the options' filters and wrap mode on wgpu's default sampler.
fn sampler(device: &ProtocolObject<dyn MTLDevice>, o: egui::TextureOptions) -> Result<Sampler, String> {
    use egui::{TextureFilter, TextureWrapMode};
    let filter = |f| match f {
        TextureFilter::Nearest => MTLSamplerMinMagFilter::Nearest,
        TextureFilter::Linear => MTLSamplerMinMagFilter::Linear,
    };
    let uv = match o.wrap_mode {
        TextureWrapMode::ClampToEdge => MTLSamplerAddressMode::ClampToEdge,
        TextureWrapMode::Repeat => MTLSamplerAddressMode::Repeat,
        TextureWrapMode::MirroredRepeat => MTLSamplerAddressMode::MirrorRepeat,
    };
    super::images::sampler_with(device, (filter(o.minification), filter(o.magnification)), uv)
}

/// egui-wgpu's `ScissorRect::new`: the clip in pixels, rounded, kept inside the target.
pub fn scissor(clip: &egui::Rect, ppp: f32, [w, h]: [u32; 2]) -> MTLScissorRect {
    let min_x = ((ppp * clip.min.x).round() as u32).clamp(0, w);
    let min_y = ((ppp * clip.min.y).round() as u32).clamp(0, h);
    let max_x = ((ppp * clip.max.x).round() as u32).clamp(min_x, w);
    let max_y = ((ppp * clip.max.y).round() as u32).clamp(min_y, h);
    MTLScissorRect { x: min_x as usize, y: min_y as usize, width: (max_x - min_x) as usize, height: (max_y - min_y) as usize }
}

/// Copies `bytes` into the staging chunk at `at` and moves `at` past them; returns where they went.
fn put(staging: &Buffer, at: &mut usize, bytes: &[u8]) -> usize {
    // SAFETY: the shared chunk holds every byte the frame counted, and no command buffer still
    // reads it (`Staging::take`).
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), staging.contents().as_ptr().cast::<u8>().add(*at), bytes.len()) };
    let from = *at;
    *at += bytes.len();
    from
}

/// `screen` cut to a target of `w` by `h` pixels.
pub fn fit(screen: ScreenDescriptor, (w, h): (u32, u32)) -> ScreenDescriptor {
    let [sw, sh] = screen.size_in_pixels;
    ScreenDescriptor { size_in_pixels: [sw.min(w), sh.min(h)], pixels_per_point: screen.pixels_per_point }
}

fn vertex_bytes(v: &[egui::epaint::Vertex]) -> &[u8] {
    // SAFETY: `Vertex` is `repr(C)` plain data of VERTEX_SIZE bytes with no padding (checked above).
    unsafe { std::slice::from_raw_parts(v.as_ptr().cast::<u8>(), v.len() * VERTEX_SIZE) }
}

impl EguiPainter {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>) -> Result<EguiPainter, String> {
        let shader = Shader::new(device, include_str!("egui.wgsl"), ("vs_main", "fs_main_gamma_framebuffer"), &[&LOCALS, &PICTURE], &BUFFERS)?;
        let slots = Slots { vs_locals: shader.vertex_buffer(0, 0)?, vs_sizes: shader.sizes_slot(), fs_locals: shader.fragment_slot(0, 0)?, fs_texture: shader.fragment_slot(1, 0)?, fs_sampler: shader.fragment_slot(1, 1)? };
        Ok(EguiPainter {
            pipeline: shader.pipeline(device, FORMAT, Blend::EGUI)?,
            slots,
            vertices: private_buffer(device, VERTEX_START)?,
            vertex_cap: VERTEX_START,
            indices: private_buffer(device, INDEX_START)?,
            index_cap: INDEX_START,
            slices: Vec::with_capacity(64),
            textures: HashMap::new(),
            samplers: HashMap::new(),
            pending: Vec::new(),
            staging: Staging::default(),
            generation: crate::gpu_pick::current_generation(),
            made: 2,
        })
    }

    /// Draws `jobs` into `target` after applying `deltas`, as egui-wgpu does: the texture sets, the
    /// buffers, one pass cleared to black, then the frees. A Metal failure skips the frame and is
    /// reported as a lost device; the texture changes it did not apply wait for the next frame.
    pub fn paint(&mut self, shared: &MetalShared, target: Target<'_>, jobs: &[egui::ClippedPrimitive], deltas: &egui::TexturesDelta, screen: ScreenDescriptor) -> Result<(), String> {
        objc2::rc::autoreleasepool(|_| self.paint_frame(shared, target, jobs, deltas, screen))
    }

    fn paint_frame(&mut self, shared: &MetalShared, target: Target<'_>, jobs: &[egui::ClippedPrimitive], deltas: &egui::TexturesDelta, screen: ScreenDescriptor) -> Result<(), String> {
        self.pending.extend(deltas.set.iter().flat_map(|(id, ds)| ds.iter().map(|d| Op::Set(*id, d.clone()))));
        let out: Option<Texture> = match target {
            Target::Drawable(d) => Some(d.texture()),
            Target::Texture(t) => Some(t.retain()),
            Target::None => None,
        };
        // A drawable can lag the window during a resize: the frame is cut to the target, never
        // drawn past it (a scissor or viewport outside the target is a Metal error). Positions
        // stay in pixels, so what fits draws where it would.
        let screen = match &out {
            Some(t) => fit(screen, (t.width() as u32, t.height() as u32)),
            None => screen,
        };
        let drawn = shared.command_buffer(self.generation).and_then(|cb| {
            let encoded = self.encode(&shared.device, &cb, out.as_deref(), jobs, screen);
            if let (Ok(()), Target::Drawable(d)) = (&encoded, &target) {
                cb.presentDrawable(ProtocolObject::<dyn MTLDrawable>::from_ref(*d));
            }
            // Committed either way: what was encoded belongs together, and the staging chunk comes back.
            cb.commit();
            encoded
        });
        match drawn {
            Ok(()) => {
                for id in &deltas.free {
                    self.textures.remove(id);
                }
                Ok(())
            }
            Err(e) => {
                self.pending.extend(deltas.free.iter().map(|id| Op::Free(*id)));
                let why = format!("Metal: an egui window: {e}");
                crate::gpu_pick::report_lost(self.generation, why.clone());
                Err(why)
            }
        }
    }

    /// Everything the frame needs is made first, so a failed allocation changes nothing; then the
    /// copies go through one staging chunk and one blit, ahead of the pass.
    fn encode(&mut self, device: &ProtocolObject<dyn MTLDevice>, cb: &ProtocolObject<dyn MTLCommandBuffer>, out: Option<&ProtocolObject<dyn MTLTexture>>, jobs: &[egui::ClippedPrimitive], screen: ScreenDescriptor) -> Result<(), String> {
        let mut texture_bytes = 0;
        let mut fresh: Vec<Option<Texture>> = Vec::with_capacity(self.pending.len());
        for op in &self.pending {
            let made = match op {
                Op::Set(_, d) => {
                    texture_bytes += d.image.width() * d.image.height() * 4;
                    if !self.samplers.contains_key(&d.options) {
                        self.samplers.insert(d.options, sampler(device, d.options)?);
                        self.made += 1;
                    }
                    if d.pos.is_none() { Some(texture(device, d.image.size())?) } else { None }
                }
                Op::Free(_) => None,
            };
            fresh.push(made);
        }
        let meshes = || jobs.iter().filter_map(|j| if let egui::epaint::Primitive::Mesh(m) = &j.primitive { Some(m) } else { None });
        let (vertex_bytes_total, index_bytes) = if out.is_some() { meshes().fold((0, 0), |(v, i), m| (v + m.vertices.len() * VERTEX_SIZE, i + m.indices.len() * 4)) } else { (0, 0) };
        // egui-wgpu's growth: twice the old capacity, at least what the frame needs.
        let grow = |cap: usize, need: usize| (cap < need).then(|| (cap * 2).max(need));
        let new_vertices = grow(self.vertex_cap, vertex_bytes_total).map(|cap| private_buffer(device, cap).map(|b| (b, cap))).transpose()?;
        let new_indices = grow(self.index_cap, index_bytes).map(|cap| private_buffer(device, cap).map(|b| (b, cap))).transpose()?;
        let total = texture_bytes + vertex_bytes_total + index_bytes;
        let staging = if total > 0 { Some(self.staging.take(device, cb, total)?) } else { None };
        let blit = match &staging {
            Some(_) => Some(cb.blitCommandEncoder().ok_or("no Metal blit encoder")?),
            None => None,
        };
        // Nothing fails from here until the pass.
        if let Some((b, cap)) = new_vertices {
            (self.vertices, self.vertex_cap) = (b, cap);
            self.made += 1;
        }
        if let Some((b, cap)) = new_indices {
            (self.indices, self.index_cap) = (b, cap);
            self.made += 1;
        }
        let mut at = 0usize;
        let mut ops = std::mem::take(&mut self.pending);
        for (op, made) in ops.drain(..).zip(fresh) {
            match op {
                Op::Set(id, d) => {
                    let tex = match made {
                        Some(t) => {
                            self.made += 1;
                            self.textures.insert(id, (t.clone(), d.options));
                            t
                        }
                        None => match self.textures.get_mut(&id) {
                            Some((t, options)) => {
                                *options = d.options;
                                t.clone()
                            }
                            // egui-wgpu panics here: egui never updates a texture it did not make.
                            None => continue,
                        },
                    };
                    let (Some(staging), Some(blit)) = (&staging, &blit) else { continue };
                    let [w, h] = d.image.size();
                    let egui::ImageData::Color(image) = &d.image;
                    let from = put(staging, &mut at, image.as_raw());
                    let [x, y] = d.pos.unwrap_or([0, 0]);
                    // SAFETY: the copy lies inside the staging chunk and inside the texture (egui's delta).
                    unsafe {
                        blit.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                            staging,
                            from,
                            w * 4,
                            w * h * 4,
                            MTLSize { width: w, height: h, depth: 1 },
                            &tex,
                            0,
                            0,
                            MTLOrigin { x, y, z: 0 },
                        );
                    }
                }
                Op::Free(id) => {
                    self.textures.remove(&id);
                }
            }
        }
        self.pending = ops;
        if let (Some(staging), Some(blit), true) = (&staging, &blit, vertex_bytes_total > 0) {
            self.slices.clear();
            let (mut v, mut i) = (0, 0);
            for m in meshes() {
                let vb = vertex_bytes(&m.vertices);
                let ib: &[u8] = bytemuck::cast_slice(&m.indices);
                self.slices.push((v..v + vb.len(), i..i + ib.len()));
                v += vb.len();
                i += ib.len();
            }
            let vfrom = at;
            for m in meshes() {
                put(staging, &mut at, vertex_bytes(&m.vertices));
            }
            let ifrom = at;
            for m in meshes() {
                put(staging, &mut at, bytemuck::cast_slice(&m.indices));
            }
            // SAFETY: both copies lie inside the chunk and inside the buffers (grown above).
            unsafe {
                blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(staging, vfrom, &self.vertices, 0, vertex_bytes_total);
                if index_bytes > 0 {
                    blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(staging, ifrom, &self.indices, 0, index_bytes);
                }
            }
        }
        if let Some(blit) = blit {
            blit.endEncoding();
        }
        match out {
            Some(out) => self.draw(cb, out, jobs, screen),
            None => Ok(()),
        }
    }

    /// egui-wgpu's `render`, in one pass cleared to black.
    fn draw(&self, cb: &ProtocolObject<dyn MTLCommandBuffer>, out: &ProtocolObject<dyn MTLTexture>, jobs: &[egui::ClippedPrimitive], screen: ScreenDescriptor) -> Result<(), String> {
        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 is the one color attachment.
        let at = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        at.setTexture(Some(out));
        at.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 1.0 });
        at.setLoadAction(MTLLoadAction::Clear);
        at.setStoreAction(MTLStoreAction::Store);
        let enc = cb.renderCommandEncoderWithDescriptor(&pass).ok_or("no Metal render encoder")?;
        let size = screen.size_in_pixels;
        let ppp = screen.pixels_per_point;
        enc.setViewport(MTLViewport { originX: 0.0, originY: 0.0, width: f64::from(size[0]), height: f64::from(size[1]), znear: 0.0, zfar: 1.0 });
        enc.setRenderPipelineState(&self.pipeline);
        enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
        enc.setCullMode(MTLCullMode::None);
        enc.setTriangleFillMode(MTLTriangleFillMode::Fill);
        let locals = Locals { screen: [size[0] as f32 / ppp, size[1] as f32 / ppp], dithering: 1, predictable_filtering: 0 };
        // SAFETY: the slots are the translation's; the bytes outlive the calls.
        unsafe {
            enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&locals).cast(), std::mem::size_of::<Locals>(), self.slots.vs_locals);
            enc.setFragmentBytes_length_atIndex(std::ptr::NonNull::from(&locals).cast(), std::mem::size_of::<Locals>(), self.slots.fs_locals);
        }
        let mut slices = self.slices.iter();
        for job in jobs {
            let egui::epaint::Primitive::Mesh(mesh) = &job.primitive else {
                // Termurion has no paint callbacks.
                continue;
            };
            let Some((vs, is)) = slices.next() else { break };
            let rect = scissor(&job.clip_rect, ppp, size);
            if rect.width == 0 || rect.height == 0 {
                continue;
            }
            enc.setScissorRect(rect);
            // A texture egui never made is skipped, as egui-wgpu does (it warns).
            let Some((texture, options)) = self.textures.get(&mesh.texture_id) else { continue };
            let Some(sampler) = self.samplers.get(options) else { continue };
            if mesh.indices.is_empty() {
                continue;
            }
            let sizes = [u32::try_from(vs.len()).unwrap_or(u32::MAX)];
            // SAFETY: the slots are the translation's; the slices lie in the buffers written above.
            unsafe {
                enc.setFragmentTexture_atIndex(Some(texture), self.slots.fs_texture);
                enc.setFragmentSamplerState_atIndex(Some(sampler), self.slots.fs_sampler);
                enc.setVertexBuffer_offset_atIndex(Some(&self.vertices), vs.start, shaders::vertex_buffer_slot(0) as usize);
                enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&sizes).cast(), 4, self.slots.vs_sizes);
                enc.drawIndexedPrimitives_indexCount_indexType_indexBuffer_indexBufferOffset(MTLPrimitiveType::Triangle, mesh.indices.len(), MTLIndexType::UInt32, &self.indices, is.start);
            }
        }
        enc.setScissorRect(MTLScissorRect { x: 0, y: 0, width: size[0] as usize, height: size[1] as usize });
        enc.endEncoding();
        Ok(())
    }

    /// Textures, buffers and samplers made so far, and staging chunks (tests).
    #[cfg(test)]
    pub fn objects_made(&self) -> (u64, u64) {
        (self.made, self.staging.made)
    }

    /// Texture changes waiting for a frame that works (tests).
    #[cfg(test)]
    pub fn pending(&self) -> usize {
        self.pending.len()
    }
}

/// An egui window's layer and painter on the Metal device.
pub struct EguiCanvas {
    shared: MetalShared,
    /// None for an offscreen canvas (tests).
    surface: Option<Surface>,
    painter: EguiPainter,
    size: (u32, u32),
    scale: f64,
    #[cfg(test)]
    offscreen: Option<Texture>,
}

/// What a frame did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Drawn,
    /// The window is covered or gave no drawable: ask again later.
    Later,
    /// A Metal failure skipped it (reported as a lost device).
    Skipped,
}

impl EguiCanvas {
    /// A layer over `window`'s view, on `shared`. Main thread only.
    pub fn open(window: &winit::window::Window, shared: &MetalShared) -> Result<EguiCanvas, String> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let view = match window.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::AppKit(h) => h.ns_view,
            _ => return Err("not an AppKit window".to_string()),
        };
        let size = window.inner_size();
        let c = EguiCanvas::new(shared, Some(Surface::attach(view)?), (size.width.max(1), size.height.max(1)), window.scale_factor())?;
        c.configure();
        Ok(c)
    }

    fn new(shared: &MetalShared, surface: Option<Surface>, size: (u32, u32), scale: f64) -> Result<EguiCanvas, String> {
        Ok(EguiCanvas {
            shared: shared.clone(),
            surface,
            painter: EguiPainter::new(&shared.device)?,
            size,
            scale,
            #[cfg(test)]
            offscreen: None,
        })
    }

    /// A canvas with no layer, `w` by `h` (tests): frames go into a texture of its own.
    #[cfg(test)]
    pub fn offscreen(shared: &MetalShared, size: (u32, u32)) -> Result<EguiCanvas, String> {
        EguiCanvas::new(shared, None, size, 1.0)
    }

    fn configure(&self) {
        if let Some(s) = &self.surface {
            // Drawn into only, as wgpu-hal sets a render-attachment-only surface.
            s.configure_as(&self.shared.device, self.size, self.scale, FORMAT, true);
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn resize(&mut self, w: u32, h: u32, scale: f64) {
        (self.size, self.scale) = ((w.max(1), h.max(1)), scale);
        self.configure();
    }

    /// A frame that is not shown: its texture changes are applied.
    pub fn apply(&mut self, deltas: &egui::TexturesDelta) {
        let _ = self.painter.paint(&self.shared, Target::None, &[], deltas, self.screen(1.0));
    }

    /// Draws `jobs` into the next drawable and presents it.
    pub fn paint(&mut self, jobs: &[egui::ClippedPrimitive], deltas: &egui::TexturesDelta, pixels_per_point: f32) -> Shown {
        let screen = self.screen(pixels_per_point);
        let Some(surface) = self.surface.as_ref() else {
            #[cfg(test)]
            {
                let t = match self.offscreen.take() {
                    Some(t) if (t.width() as u32, t.height() as u32) == self.size => t,
                    _ => match offscreen_target(&self.shared.device, self.size) {
                        Ok(t) => t,
                        Err(_) => return Shown::Skipped,
                    },
                };
                let drawn = self.painter.paint(&self.shared, Target::Texture(&t), jobs, deltas, screen);
                self.offscreen = Some(t);
                return if drawn.is_ok() { Shown::Drawn } else { Shown::Skipped };
            }
            #[cfg(not(test))]
            return Shown::Skipped;
        };
        match surface.next() {
            Next::Drawable(d, _) => match self.painter.paint(&self.shared, Target::Drawable(&d), jobs, deltas, screen) {
                Ok(()) => Shown::Drawn,
                Err(_) => Shown::Skipped,
            },
            Next::Occluded | Next::Timeout => {
                let _ = self.painter.paint(&self.shared, Target::None, &[], deltas, screen);
                Shown::Later
            }
        }
    }

    fn screen(&self, pixels_per_point: f32) -> ScreenDescriptor {
        ScreenDescriptor { size_in_pixels: [self.size.0, self.size.1], pixels_per_point }
    }

    /// The last frame's pixels as RGBA (tests).
    #[cfg(test)]
    pub fn pixels(&self) -> Vec<u8> {
        let t = self.offscreen.as_ref().expect("an offscreen canvas that drew");
        super::readback::read_rgba(&self.shared.queue, t, t.width() as u32, t.height() as u32).expect("the canvas pixels")
    }

    /// The painter (tests).
    #[cfg(test)]
    pub fn painter(&self) -> &EguiPainter {
        &self.painter
    }
}

/// A texture an egui frame can be drawn into and read back from (tests).
#[cfg(test)]
pub fn offscreen_target(device: &ProtocolObject<dyn MTLDevice>, (w, h): (u32, u32)) -> Result<Texture, String> {
    // SAFETY: a plain 2D descriptor.
    let desc = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(FORMAT, w as usize, h as usize, false) };
    desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
    desc.setStorageMode(MTLStorageMode::Private);
    super::new_texture(device, &desc).ok_or_else(|| format!("no Metal texture of {w} x {h}"))
}
