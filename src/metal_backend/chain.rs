//! Render plugins on Metal: `render_fx_gpu::Chain` with view formats, pass for pass. The frame is
//! drawn into an sRGB texture, each shader reads it through a non-sRGB view, and the last pass
//! writes the window's raw bytes. With region terms the last pass writes a kept texture, and a
//! blit copies it into the drawable (whose contents are undefined each frame).

use objc2::rc::Retained;
use objc2::Message;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLColorWriteMask, MTLCommandBuffer, MTLCommandEncoder, MTLCullMode, MTLDevice, MTLFunction, MTLLoadAction, MTLPixelFormat, MTLPrimitiveTopologyClass, MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor,
    MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLSamplerState, MTLScissorRect, MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLTriangleFillMode, MTLWinding,
};

use super::draw::{raw_format, MetalDraw};
use super::shaders::{self, COPY_INPUT, EFFECT};
use crate::render_fx::{Compiled, Layout, PxRect, RegionTerm, Uniforms, uniform_layout};
use crate::render_fx_gpu::Shade;

type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;
type Texture = Retained<ProtocolObject<dyn MTLTexture>>;
type Pipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;
type Function = Retained<ProtocolObject<dyn MTLFunction>>;

struct Pass {
    pipeline: Pipeline,
    uniforms: Buffer,
    layout: Option<Layout>,
}

/// An sRGB texture the renderer draws into, and its non-sRGB view the shaders read and write.
struct Target {
    texture: Texture,
    raw: Texture,
}

/// The fragment stage's slots for the effect layout and the fill copy's texture.
#[derive(Clone, Copy)]
struct Slots {
    texture: usize,
    buffer: usize,
    sampler: usize,
    fill: usize,
}

impl Slots {
    fn new() -> Result<Slots, String> {
        let fs = shaders::stage_resources(&[&EFFECT], naga::ShaderStage::Fragment);
        let copy = shaders::stage_resources(&[&COPY_INPUT], naga::ShaderStage::Fragment);
        let at = |r: &naga::back::msl::EntryPointResources, binding: u32| r.resources.get(&naga::ResourceBinding { group: 0, binding }).cloned().ok_or_else(|| format!("no slot for binding {binding}"));
        let sampler = match at(&fs, 2)?.sampler {
            Some(naga::back::msl::BindSamplerTarget::Resource(s)) => usize::from(s),
            _ => return Err("no sampler slot".to_string()),
        };
        Ok(Slots {
            texture: usize::from(at(&fs, 0)?.texture.ok_or("no texture slot")?),
            buffer: usize::from(at(&fs, 1)?.buffer.ok_or("no buffer slot")?),
            sampler,
            fill: usize::from(at(&copy, 0)?.texture.ok_or("no texture slot")?),
        })
    }
}

pub(crate) struct MetalChain {
    passes: Vec<Pass>,
    vertex: Function,
    copy: Function,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    slots: Slots,
    /// The window's format (sRGB); the passes write it without the suffix.
    format: MTLPixelFormat,
    keep_alpha: bool,
    size: (u32, u32),
    /// [0] is the frame; [1] is the ping-pong partner for chains of more than one shader.
    targets: Vec<Target>,
    frame_drawn: bool,
    last_packed: Vec<Vec<u8>>,
    scratch: Vec<Vec<u8>>,
    staged: bool,
    /// The uniform writes of this frame (pass, byte range), reused so a frame allocates nothing.
    writes: Vec<(usize, std::ops::Range<usize>)>,
    uniform_writes: u64,
    ran: bool,
    skipped: u64,
    region_terms: Option<Vec<RegionTerm>>,
    /// With region terms: the last pass's output, kept so a frame can shade only its region.
    kept: Option<Target>,
    kept_ok: bool,
    kept_made: u64,
    keep_next: bool,
    shaded_region: bool,
    /// The `Shade::Fresh` copies: one for a pass in the middle, one with the last pass's mask.
    fill: Option<(Pipeline, Pipeline)>,
    /// Metal buffers, textures and views made after `new` (tests).
    made: u64,
    /// Non-sRGB views of the drawables a frame wrote directly, by drawable texture: the layer
    /// cycles a few textures, so a steady frame makes no view.
    views: Vec<(Texture, Texture)>,
}

/// Drawable views kept: the layer's `maximumDrawableCount` (`surface::Surface::configure`).
const DRAWABLE_VIEWS: usize = 3;

/// A pipeline drawing the full-screen triangle with `fs` into `format`, no blending.
fn pipeline(device: &ProtocolObject<dyn MTLDevice>, vs: &ProtocolObject<dyn MTLFunction>, fs: &ProtocolObject<dyn MTLFunction>, format: MTLPixelFormat, color_only: bool) -> Result<Pipeline, String> {
    let d = MTLRenderPipelineDescriptor::new();
    d.setVertexFunction(Some(vs));
    d.setFragmentFunction(Some(fs));
    // SAFETY: a triangle list is of the triangle class.
    unsafe { d.setInputPrimitiveTopology(MTLPrimitiveTopologyClass::Triangle) };
    // SAFETY: index 0 is the one color target.
    let at = unsafe { d.colorAttachments().objectAtIndexedSubscript(0) };
    at.setPixelFormat(format);
    at.setWriteMask(if color_only { MTLColorWriteMask::Red | MTLColorWriteMask::Green | MTLColorWriteMask::Blue } else { MTLColorWriteMask::All });
    at.setBlendingEnabled(false);
    device.newRenderPipelineStateWithDescriptor_error(&d).map_err(|e| format!("Metal pipeline: {}", e.localizedDescription()))
}

/// A `w` by `h` texture in `format` with its non-sRGB view, for drawing into and for the passes.
fn target(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat, (w, h): (u32, u32)) -> Result<Target, String> {
    // SAFETY: a plain 2D descriptor; the window keeps its size inside the limit.
    let desc = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(format, w.max(1) as usize, h.max(1) as usize, false) };
    desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead | MTLTextureUsage::PixelFormatView);
    desc.setStorageMode(MTLStorageMode::Private);
    let texture = super::new_texture(device, &desc).ok_or_else(|| format!("no Metal effect texture of {w} x {h}"))?;
    let raw = texture.newTextureViewWithPixelFormat(raw_format(format)).ok_or("no Metal texture view")?;
    Ok(Target { texture, raw })
}

/// What a render pass of the chain loads.
fn begin(cb: &ProtocolObject<dyn MTLCommandBuffer>, output: &ProtocolObject<dyn MTLTexture>, load: bool) -> Result<Retained<ProtocolObject<dyn MTLRenderCommandEncoder>>, String> {
    let pass = MTLRenderPassDescriptor::new();
    // SAFETY: index 0 is the one color attachment.
    let at = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
    at.setTexture(Some(output));
    // wgpu's LoadOp::Clear(BLACK).
    at.setClearColor(MTLClearColor { red: 0.0, green: 0.0, blue: 0.0, alpha: 1.0 });
    at.setLoadAction(if load { MTLLoadAction::Load } else { MTLLoadAction::Clear });
    at.setStoreAction(MTLStoreAction::Store);
    let enc = cb.renderCommandEncoderWithDescriptor(&pass).ok_or("no Metal render encoder")?;
    enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
    enc.setCullMode(MTLCullMode::None);
    enc.setTriangleFillMode(MTLTriangleFillMode::Fill);
    Ok(enc)
}

fn triangle(enc: &ProtocolObject<dyn MTLRenderCommandEncoder>) {
    // SAFETY: the triangle comes from the vertex index alone.
    unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3) };
}

impl MetalChain {
    /// A pipeline per shader, as `Chain::new` builds them: the last pass writes color only on an
    /// opaque window. Fails with the first shader Metal rejects.
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat, shaders: Vec<Compiled>, keep_alpha: bool) -> Result<MetalChain, String> {
        let (vs_src, vs_name) = shaders::effect_vertex_msl()?;
        let vertex = super::quads::function(device, &vs_src, &vs_name)?;
        let (copy_src, copy_name) = shaders::copy_msl()?;
        let copy = super::quads::function(device, &copy_src, &copy_name)?;
        let last = shaders.len().saturating_sub(1);
        let mut passes = Vec::new();
        for (i, shader) in shaders.into_iter().enumerate() {
            let (src, name) = shaders::effect_to_msl(&shader)?;
            let fs = super::quads::function(device, &src, &name)?;
            let pipeline = pipeline(device, &vertex, &fs, raw_format(format), i == last && !keep_alpha)?;
            let layout = uniform_layout(&shader.module);
            let size = layout.as_ref().map_or(16, |l| l.size.max(16)) as usize;
            let uniforms = super::new_buffer(device, size, MTLResourceOptions::StorageModePrivate).ok_or("no Metal uniform buffer")?;
            passes.push(Pass { pipeline, uniforms, layout });
        }
        let n = passes.len();
        Ok(MetalChain {
            passes,
            vertex,
            copy,
            sampler: super::images::sampler(device, false)?,
            slots: Slots::new()?,
            format,
            keep_alpha,
            size: (0, 0),
            targets: Vec::new(),
            frame_drawn: false,
            last_packed: vec![Vec::new(); n],
            scratch: vec![Vec::new(); n],
            staged: false,
            writes: Vec::with_capacity(n),
            uniform_writes: 0,
            ran: false,
            skipped: 0,
            region_terms: None,
            kept: None,
            kept_ok: false,
            kept_made: 0,
            keep_next: true,
            shaded_region: false,
            fill: None,
            made: 0,
            views: Vec::with_capacity(DRAWABLE_VIEWS),
        })
    }

    /// `Chain::set_region_terms`.
    pub fn set_region_terms(&mut self, terms: Option<Vec<RegionTerm>>) {
        if terms.is_some() != self.region_terms.is_some() {
            self.size = (0, 0);
            self.frame_drawn = false;
            self.kept = None;
            self.kept_ok = false;
        }
        self.region_terms = terms;
    }

    /// `Chain::keep_output`.
    pub fn keep_output(&mut self, keep: bool) {
        self.keep_next = keep;
    }

    #[cfg(test)]
    pub fn shaded_region(&self) -> bool {
        self.shaded_region
    }

    #[cfg(test)]
    pub fn kept_made(&self) -> u64 {
        self.kept_made
    }

    #[cfg(test)]
    pub fn uniform_writes(&self) -> u64 {
        self.uniform_writes
    }

    #[cfg(test)]
    pub fn skips(&self) -> u64 {
        self.skipped
    }

    #[cfg(test)]
    pub fn objects_made(&self) -> u64 {
        self.made
    }

    /// The frame texture and its non-sRGB view at `size`, made again on a resize (the kept
    /// output with them). Nothing changes when a texture cannot be made.
    pub fn frame_views(&mut self, device: &ProtocolObject<dyn MTLDevice>, size: (u32, u32)) -> Result<(Texture, Texture), String> {
        let wanted = if self.passes.len() > 1 { 2 } else { 1 };
        if self.size != size || self.targets.len() != wanted {
            let targets = (0..wanted).map(|_| target(device, self.format, size)).collect::<Result<Vec<_>, _>>()?;
            // A third pass writes the frame texture, so such a chain never shades a region alone.
            let kept = if self.region_terms.is_some() && self.passes.len() <= 2 { Some(target(device, self.format, size)?) } else { None };
            if kept.is_some() && self.fill.is_none() {
                let raw = raw_format(self.format);
                let all = pipeline(device, &self.vertex, &self.copy, raw, false)?;
                let last = if self.keep_alpha { all.clone() } else { pipeline(device, &self.vertex, &self.copy, raw, true)? };
                self.fill = Some((all, last));
            }
            self.made += (targets.len() + usize::from(kept.is_some())) as u64;
            self.kept_made += u64::from(kept.is_some());
            self.targets = targets;
            self.kept = kept;
            // The drawables of the old size are gone.
            self.views.clear();
            self.kept_ok = false;
            self.size = size;
            self.frame_drawn = false;
        }
        let t = &self.targets[0];
        Ok((t.texture.clone(), t.raw.clone()))
    }

    /// `Chain::frame_kept`: the frame texture still holds the window's last picture.
    pub fn frame_kept(&self) -> bool {
        self.frame_drawn && self.passes.len() <= 2
    }

    /// `Chain::set_frame_drawn`.
    pub fn set_frame_drawn(&mut self, drawn: bool) {
        self.frame_drawn = drawn;
        if !drawn {
            self.kept_ok = false;
            self.ran = false;
        }
    }

    /// After a frame that failed part way: nothing the chain holds is the window's, and the
    /// uniform buffers are written whole again.
    pub fn forget(&mut self) {
        self.set_frame_drawn(false);
        self.staged = false;
        self.last_packed.iter_mut().for_each(Vec::clear);
    }

    /// `Chain::skip_if_still`.
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

    /// `Chain::unstage`.
    pub fn unstage(&mut self) {
        self.staged = false;
    }

    /// `Chain::run_with` into `out` (a texture of the window's sRGB format, written byte for byte).
    /// The uniform writes go through the draw's staging chunks, ahead of the passes in `cb`.
    pub fn run(&mut self, device: &ProtocolObject<dyn MTLDevice>, cb: &ProtocolObject<dyn MTLCommandBuffer>, draw: &mut MetalDraw, uniforms: &Uniforms, out: &ProtocolObject<dyn MTLTexture>, shade: Shade<'_>) -> Result<(), String> {
        if self.targets.is_empty() {
            return Ok(());
        }
        let (rects, fresh): (Option<&[PxRect]>, bool) = match shade {
            Shade::Full => (None, false),
            Shade::Kept(r) => (Some(r).filter(|_| self.kept.is_some() && self.kept_ok), false),
            Shade::Fresh(r) => (Some(r).filter(|_| self.kept.is_some() && !self.region_terms.as_ref().is_some_and(|t| t.contains(&RegionTerm::StillLooks))), true),
        };
        self.write_uniforms(device, cb, draw, uniforms)?;
        let last = self.passes.len().saturating_sub(1);
        // A full frame no next frame builds on writes the window directly.
        let skip_kept = rects.is_none() && !self.keep_next;
        // The drawable's own non-sRGB view, only for a frame that writes it directly.
        let direct: Option<&ProtocolObject<dyn MTLTexture>> = match self.kept.is_none() || skip_kept {
            false => None,
            true if out.pixelFormat() == raw_format(out.pixelFormat()) => Some(out),
            true => {
                let i = self.view_of(out)?;
                Some(&self.views[i].1)
            }
        };
        let kept = self.kept.as_ref().map(|k| &k.raw).filter(|_| !skip_kept);
        let load = rects.is_some() && !fresh;
        let fill = self.fill.as_ref().filter(|_| fresh && rects.is_some() && self.kept.is_some());
        for (i, pass) in self.passes.iter().enumerate() {
            let output: &ProtocolObject<dyn MTLTexture> = match (i == last, kept, &direct) {
                (true, Some(k), _) => k,
                (true, None, Some(d)) => d,
                _ => &self.targets[(i + 1) % 2].raw,
            };
            let enc = begin(cb, output, load)?;
            // SAFETY: the slots are the ones the shaders were translated with; every resource
            // outlives the command buffer (the chain keeps them, Metal retains them).
            unsafe {
                if let Some((all, last_mask)) = fill {
                    enc.setRenderPipelineState(if i == last { last_mask } else { all });
                    enc.setFragmentTexture_atIndex(Some(&self.targets[0].raw), self.slots.fill);
                    triangle(&enc);
                }
                enc.setRenderPipelineState(&pass.pipeline);
                enc.setFragmentTexture_atIndex(Some(&self.targets[i % 2].raw), self.slots.texture);
                enc.setFragmentBuffer_offset_atIndex(Some(&pass.uniforms), 0, self.slots.buffer);
                enc.setFragmentSamplerState_atIndex(Some(&self.sampler), self.slots.sampler);
            }
            match rects {
                Some(rects) => {
                    let (sw, sh) = self.size;
                    for r in rects {
                        // A scissor past the target is not allowed.
                        let (x, y) = (r.x.min(sw), r.y.min(sh));
                        let (w, h) = (r.w.min(sw - x), r.h.min(sh - y));
                        if w > 0 && h > 0 {
                            enc.setScissorRect(MTLScissorRect { x: x as usize, y: y as usize, width: w as usize, height: h as usize });
                            triangle(&enc);
                        }
                    }
                }
                None => triangle(&enc),
            }
            enc.endEncoding();
        }
        if let Some(k) = self.kept.as_ref().filter(|_| !skip_kept) {
            let blit = cb.blitCommandEncoder().ok_or("no Metal blit encoder")?;
            // SAFETY: the same format and size; both textures outlive the command buffer.
            unsafe { blit.copyFromTexture_toTexture(&k.texture, out) };
            blit.endEncoding();
        }
        self.kept_ok = self.kept.is_some() && !skip_kept;
        self.shaded_region = rects.is_some();
        self.ran = true;
        Ok(())
    }

    /// The index in `views` of `out`'s non-sRGB view, made when the drawable is new to the chain.
    fn view_of(&mut self, out: &ProtocolObject<dyn MTLTexture>) -> Result<usize, String> {
        if let Some(i) = self.views.iter().position(|(t, _)| std::ptr::eq(&**t, out)) {
            return Ok(i);
        }
        let view = out.newTextureViewWithPixelFormat(raw_format(out.pixelFormat())).ok_or("no Metal view of the window")?;
        if self.views.len() == DRAWABLE_VIEWS {
            self.views.remove(0);
        }
        self.views.push((out.retain(), view));
        self.made += 1;
        Ok(self.views.len() - 1)
    }

    /// Packs each pass's uniforms (unless `skip_if_still` did) and writes the ones that changed:
    /// just the clock span when only it moved, through one staging chunk and one blit.
    fn write_uniforms(&mut self, device: &ProtocolObject<dyn MTLDevice>, cb: &ProtocolObject<dyn MTLCommandBuffer>, draw: &mut MetalDraw, uniforms: &Uniforms) -> Result<(), String> {
        let staged = std::mem::take(&mut self.staged);
        self.writes.clear();
        for (i, pass) in self.passes.iter().enumerate() {
            let Some(layout) = &pass.layout else { continue };
            if !staged {
                uniforms.pack_into(layout, &mut self.scratch[i]);
            }
            let (packed, last) = (&self.scratch[i], &self.last_packed[i]);
            if packed != last {
                self.writes.push((i, crate::render_fx_gpu::upload_range(&layout.clock_span(), packed, last)));
            }
        }
        let total: usize = self.writes.iter().map(|(_, r)| r.len().next_multiple_of(4)).sum();
        if total == 0 {
            return Ok(());
        }
        let staging = draw.staging_chunk(device, cb, total)?;
        let blit = cb.blitCommandEncoder().ok_or("no Metal blit encoder")?;
        let mut at = 0;
        for (i, range) in &self.writes {
            let bytes = &self.scratch[*i][range.clone()];
            // SAFETY: the chunk is shared, holds `total` bytes and no command buffer still reads
            // it; the range lies inside the pass's buffer (both are the layout's size).
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), staging.contents().as_ptr().cast::<u8>().add(at), bytes.len());
                blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(&staging, at, &self.passes[*i].uniforms, range.start, bytes.len());
            }
            at += bytes.len().next_multiple_of(4);
        }
        blit.endEncoding();
        for (i, _) in &self.writes {
            // The old bytes become the next frame's scratch: no allocation.
            std::mem::swap(&mut self.scratch[*i], &mut self.last_packed[*i]);
            self.uniform_writes += 1;
        }
        Ok(())
    }
}
