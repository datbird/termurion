//! Runs a `DrawList` on Metal in the wgpu backend's order (`render_wgpu::WgpuDraw::run`): the
//! buffer writes first, then one pass with the backdrop, the quads under the pictures, the
//! pictures, the popup boxes, the text and the strokes and overlay, then the fade pass.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::Message;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLBlitCommandEncoder, MTLBuffer, MTLClearColor, MTLCommandBuffer, MTLCommandEncoder, MTLCullMode, MTLDevice, MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineState, MTLResourceOptions, MTLStoreAction,
    MTLTexture, MTLTriangleFillMode, MTLWinding,
};

use super::images::ImagePipeline;
use super::quads::QuadPipeline;
use super::shaders;
use crate::draw_list::{BufferWrite, DrawList, ImageDraw, QuadBuf, Shadow, Uploads, IMAGE_STRIDE, QUAD_STRIDE};

/// What a frame with text says until the Metal backend draws text (its quads and images draw).
pub const NO_TEXT_YET: &str = "the Metal renderer does not draw text yet";

/// The format a pass writes when it scales stored values: the target's without the sRGB suffix.
pub fn raw_format(format: MTLPixelFormat) -> MTLPixelFormat {
    match format {
        MTLPixelFormat::BGRA8Unorm_sRGB => MTLPixelFormat::BGRA8Unorm,
        MTLPixelFormat::RGBA8Unorm_sRGB => MTLPixelFormat::RGBA8Unorm,
        other => other,
    }
}

type Buffer = Retained<ProtocolObject<dyn MTLBuffer>>;

/// The GPU half on Metal: pipelines and the persistent vertex buffers.
pub(crate) struct MetalDraw {
    quads: QuadPipeline,
    pub(super) images: ImagePipeline,
    bufs: [Buffer; 6],
    /// The quad and image shaders' screen size the last frame drew with (tests).
    pub(super) last_uniform: [f32; 4],
    staging: Staging,
}

/// The smallest staging chunk (wgpu's upload belt uses chunks of this size too).
const CHUNK_MIN: usize = 256 << 10;
/// A spare chunk larger than this is let go once free, so one huge frame does not pin its memory.
const KEEP_BYTES: usize = 8 << 20;
/// Most spare chunks kept.
const KEEP_CHUNKS: usize = 4;

/// Shared staging chunks the buffer writes go through. A chunk belongs to one command buffer
/// until it completes (its handler frees it), so the CPU never writes one the GPU still reads,
/// and steady frames reuse chunks instead of making buffers.
#[derive(Default)]
struct Staging {
    chunks: Vec<(Buffer, Arc<AtomicBool>)>,
    made: u64,
}

impl Staging {
    /// A free chunk of at least `len` bytes, now held by `cb`.
    fn take(&mut self, device: &ProtocolObject<dyn MTLDevice>, cb: &ProtocolObject<dyn MTLCommandBuffer>, len: usize) -> Result<Buffer, String> {
        let free = |f: &AtomicBool| f.load(Ordering::Acquire);
        // Large spares go, then spares past the count.
        self.chunks.retain(|(b, f)| !free(f) || b.length() <= KEEP_BYTES);
        let mut spare = 0;
        self.chunks.retain(|(_, f)| {
            spare += usize::from(free(f));
            !free(f) || spare <= KEEP_CHUNKS
        });
        let i = match self.chunks.iter().position(|(b, f)| free(f) && b.length() >= len) {
            Some(i) => i,
            None => {
                let size = len.next_power_of_two().max(CHUNK_MIN);
                let buf = device.newBufferWithLength_options(size, MTLResourceOptions::StorageModeShared).ok_or("no Metal staging buffer")?;
                self.made += 1;
                self.chunks.push((buf, Arc::new(AtomicBool::new(true))));
                self.chunks.len() - 1
            }
        };
        let (buf, flag) = &self.chunks[i];
        flag.store(false, Ordering::Release);
        let flag = flag.clone();
        let handler = block2::RcBlock::new(move |_cb: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| flag.store(true, Ordering::Release));
        // SAFETY: Metal copies the block and calls it once the command buffer is done.
        unsafe { cb.addCompletedHandler(block2::RcBlock::as_ptr(&handler)) };
        Ok(buf.clone())
    }
}

fn private_buffer(device: &ProtocolObject<dyn MTLDevice>, size: u64) -> Result<Buffer, String> {
    device.newBufferWithLength_options(size as usize, MTLResourceOptions::StorageModePrivate).ok_or_else(|| format!("no Metal buffer of {size} bytes"))
}

/// The slot of the vertex stage's buffer sizes (after the screen uniform; `shaders::stage_resources`).
const SIZES_SLOT: usize = 1;
/// The screen uniform's slot.
const SCREEN_SLOT: usize = 0;

impl MetalDraw {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat) -> Result<MetalDraw, String> {
        let buf = || private_buffer(device, Shadow::START);
        Ok(MetalDraw { quads: QuadPipeline::new(device, format)?, images: ImagePipeline::new(device, format)?, bufs: [buf()?, buf()?, buf()?, buf()?, buf()?, buf()?], last_uniform: [0.0; 4], staging: Staging::default() })
    }

    /// Encodes `list` into `cb`, drawing into `view`; `raw` is a non-sRGB view of the same target
    /// for the fade pass. A frame with text draws everything else and says so.
    pub fn run(&mut self, device: &ProtocolObject<dyn MTLDevice>, cb: &ProtocolObject<dyn MTLCommandBuffer>, list: &DrawList, up: &Uploads, view: &ProtocolObject<dyn MTLTexture>, raw: Option<&ProtocolObject<dyn MTLTexture>>) -> Result<(), String> {
        let has_text = !list.rows.batches.is_empty() || !list.rows.writes.is_empty() || !list.glyphs.instances.is_empty() || !up.atlas.is_empty();
        self.write_buffers(device, cb, up)?;
        self.images.prepare(device, list.backdrop.iter().chain(&list.picture));
        self.last_uniform = list.quad_uniform;

        let pass = MTLRenderPassDescriptor::new();
        // SAFETY: index 0 is the one color attachment.
        let at = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        at.setTexture(Some(view));
        let [r, g, b, a] = list.clear;
        at.setClearColor(MTLClearColor { red: r as f64, green: g as f64, blue: b as f64, alpha: a as f64 });
        at.setLoadAction(MTLLoadAction::Clear);
        at.setStoreAction(MTLStoreAction::Store);
        let enc = cb.renderCommandEncoderWithDescriptor(&pass).ok_or("no Metal render encoder")?;
        if !list.backdrop.is_empty() {
            self.draw_images(&enc, QuadBuf::Backdrop, &list.backdrop, list.quad_uniform);
        }
        if !list.under.is_empty() {
            self.draw_quads(&enc, &self.quads.pipeline, QuadBuf::Under, list.under.clone(), list.quad_uniform);
        }
        if !list.picture.is_empty() {
            self.draw_images(&enc, QuadBuf::Picture, &list.picture, list.quad_uniform);
        }
        // Popup boxes cover images and quads; their text and what they cut stay on top.
        if !list.pops.is_empty() {
            self.draw_quads(&enc, &self.quads.pipeline, QuadBuf::Pops, list.pops.clone(), list.quad_uniform);
        }
        if !list.over.is_empty() {
            self.draw_quads(&enc, &self.quads.pipeline, QuadBuf::Over, list.over.clone(), list.quad_uniform);
        }
        enc.endEncoding();
        // A fading window: the whole target scaled by the fade, in a pass of its own, on the
        // non-sRGB view when there is one (as the wgpu backend picks `fader` or `fader_view`).
        if let Some(f) = list.fade {
            let fader = self.quads.fader(device, raw.is_some())?.retain();
            let pass = MTLRenderPassDescriptor::new();
            // SAFETY: index 0 is the one color attachment.
            let at = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
            at.setTexture(Some(raw.unwrap_or(view)));
            at.setLoadAction(MTLLoadAction::Load);
            at.setStoreAction(MTLStoreAction::Store);
            let enc = cb.renderCommandEncoderWithDescriptor(&pass).ok_or("no Metal render encoder")?;
            let f = f as f64 as f32;
            enc.setBlendColorRed_green_blue_alpha(f, f, f, f);
            self.draw_quads(&enc, &fader, QuadBuf::Cover, 0..6, list.quad_uniform);
            enc.endEncoding();
        }
        if has_text { Err(NO_TEXT_YET.to_string()) } else { Ok(()) }
    }

    /// Applies the frame's buffer writes in order: a grown buffer is made again, and the bytes go
    /// through a staging chunk and one blit, ahead of the frame's passes (as wgpu's upload belt).
    /// Every buffer is made before anything is encoded, so a failed one leaves the frame empty.
    fn write_buffers(&mut self, device: &ProtocolObject<dyn MTLDevice>, cb: &ProtocolObject<dyn MTLCommandBuffer>, up: &Uploads) -> Result<(), String> {
        let total: usize = up.buffers.iter().map(|w| if let BufferWrite::Quads { bytes, .. } = w { bytes.len().next_multiple_of(4) } else { 0 }).sum();
        let mut grown = Vec::new();
        for w in &up.buffers {
            if let BufferWrite::Grow { size, .. } = w {
                grown.push(private_buffer(device, *size)?);
            }
        }
        let staging = if total > 0 { Some(self.staging.take(device, cb, total)?) } else { None };
        let blit = match &staging {
            Some(_) => Some(cb.blitCommandEncoder().ok_or("no Metal blit encoder")?),
            None => None,
        };
        let mut grown = grown.into_iter();
        let mut at = 0usize;
        for w in &up.buffers {
            match w {
                BufferWrite::Grow { which, .. } => self.bufs[which.index()] = grown.next().expect("made above"),
                BufferWrite::Quads { which, offset, bytes } => {
                    let (Some(staging), Some(blit)) = (&staging, &blit) else { continue };
                    // SAFETY: the chunk is shared, at least `total` bytes, and no command buffer
                    // still reads it (`Staging::take`); this write fits at `at`.
                    unsafe {
                        let dst = staging.contents().as_ptr().cast::<u8>().add(at);
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, bytes.len());
                        blit.copyFromBuffer_sourceOffset_toBuffer_destinationOffset_size(staging, at, &self.bufs[which.index()], *offset as usize, bytes.len());
                    }
                    at += bytes.len().next_multiple_of(4);
                }
            }
        }
        if let Some(blit) = blit {
            blit.endEncoding();
        }
        Ok(())
    }

    /// Staging chunks made so far (tests).
    #[cfg(test)]
    pub(super) fn staging_made(&self) -> u64 {
        self.staging.made
    }

    /// Binds a vertex buffer as wgpu-hal does: the buffer at the top slot, the bound length in the
    /// sizes buffer vertex pulling reads, and the screen uniform.
    fn bind_vertices(&self, enc: &ProtocolObject<dyn MTLRenderCommandEncoder>, pipeline: &ProtocolObject<dyn MTLRenderPipelineState>, which: QuadBuf, bytes: u64, screen: [f32; 4]) {
        enc.setRenderPipelineState(pipeline);
        enc.setFrontFacingWinding(MTLWinding::CounterClockwise);
        enc.setCullMode(MTLCullMode::None);
        enc.setTriangleFillMode(MTLTriangleFillMode::Fill);
        let sizes = [u32::try_from(bytes).unwrap_or(u32::MAX)];
        // SAFETY: the slots are the ones the shaders were translated with; the bytes outlive the calls.
        unsafe {
            enc.setVertexBuffer_offset_atIndex(Some(&self.bufs[which.index()]), 0, shaders::vertex_buffer_slot(0) as usize);
            enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&sizes).cast(), 4, SIZES_SLOT);
            enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&screen).cast(), 16, SCREEN_SLOT);
        }
    }

    fn draw_quads(&self, enc: &ProtocolObject<dyn MTLRenderCommandEncoder>, pipeline: &ProtocolObject<dyn MTLRenderPipelineState>, which: QuadBuf, verts: std::ops::Range<u32>, screen: [f32; 4]) {
        // Only the list's own length: a list that shrank leaves its old tail unread.
        self.bind_vertices(enc, pipeline, which, verts.end as u64 * QUAD_STRIDE, screen);
        // SAFETY: the range lies in what the buffer holds.
        unsafe { enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, verts.start as usize, verts.len()) };
    }

    /// Draws `list` from `which`; a picture with no texture is skipped.
    fn draw_images(&self, enc: &ProtocolObject<dyn MTLRenderCommandEncoder>, which: QuadBuf, list: &[ImageDraw], screen: [f32; 4]) {
        self.bind_vertices(enc, &self.images.pipeline, which, list.len() as u64 * 6 * IMAGE_STRIDE, screen);
        for d in list {
            let Some((texture, sampler)) = self.images.bound(d) else { continue };
            // SAFETY: slot 0 of each kind is the picture's (`images::PICTURE`); the range is the picture's six vertices.
            unsafe {
                enc.setFragmentTexture_atIndex(Some(texture), 0);
                enc.setFragmentSamplerState_atIndex(Some(sampler), 0);
                enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, d.vertices.start as usize, d.vertices.len());
            }
        }
    }
}
