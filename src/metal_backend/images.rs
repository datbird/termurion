//! Inline and background images (`src/image.wgsl`): one texture per picture, kept while frames
//! draw it, as the wgpu backend's `ImagePipeline`.

use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLRenderPipelineState, MTLSamplerAddressMode, MTLSamplerDescriptor, MTLSamplerMinMagFilter, MTLSamplerMipFilter, MTLSamplerState, MTLSize, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage,
};

use super::quads::{Attr, Blend, Shader, SCREEN};
use super::shaders::{Entry, Kind};
use crate::draw_list::{ImageDraw, IMAGE_STRIDE};

/// A texture no frame drew for this many frames is dropped.
const TEXTURE_KEEP: u64 = 120;

const IMAGE_ATTRS: [Attr; 2] = [Attr { location: 0, offset: 0, floats: 2 }, Attr { location: 1, offset: 8, floats: 2 }];

/// The picture's texture and sampler, group 1, seen by the fragment stage.
const PICTURE: [Entry; 2] = [Entry::new(0, Kind::Texture, false, true), Entry::new(1, Kind::Sampler, false, true)];

pub(super) struct ImagePipeline {
    pub pipeline: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    /// Tiled background images repeat.
    repeat_sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    /// Textures by picture (and whether it repeats), with the frame count they were last drawn at.
    cache: HashMap<((u64, usize), bool), (Retained<ProtocolObject<dyn MTLTexture>>, u64)>,
    pub uploads: usize,
    frames: u64,
}

/// wgpu's default sampler with linear filters (mip filter Nearest, LOD 0 to 32, anisotropy 1),
/// as wgpu-hal `create_sampler` builds it.
fn sampler(device: &ProtocolObject<dyn MTLDevice>, repeat: bool) -> Result<Retained<ProtocolObject<dyn MTLSamplerState>>, String> {
    let d = MTLSamplerDescriptor::new();
    d.setMinFilter(MTLSamplerMinMagFilter::Linear);
    d.setMagFilter(MTLSamplerMinMagFilter::Linear);
    d.setMipFilter(MTLSamplerMipFilter::Nearest);
    let uv = if repeat { MTLSamplerAddressMode::Repeat } else { MTLSamplerAddressMode::ClampToEdge };
    d.setSAddressMode(uv);
    d.setTAddressMode(uv);
    d.setRAddressMode(MTLSamplerAddressMode::ClampToEdge);
    d.setMaxAnisotropy(1);
    d.setLodMinClamp(0.0);
    d.setLodMaxClamp(32.0);
    device.newSamplerStateWithDescriptor(&d).ok_or_else(|| "no Metal sampler".to_string())
}

impl ImagePipeline {
    pub fn new(device: &ProtocolObject<dyn MTLDevice>, format: MTLPixelFormat) -> Result<ImagePipeline, String> {
        let shader = Shader::new(device, include_str!("../image.wgsl"), &[&SCREEN, &PICTURE], IMAGE_STRIDE as u32, &IMAGE_ATTRS)?;
        Ok(ImagePipeline { pipeline: shader.pipeline(device, format, Blend::ALPHA)?, sampler: sampler(device, false)?, repeat_sampler: sampler(device, true)?, cache: HashMap::new(), uploads: 0, frames: 0 })
    }

    /// The texture and sampler `d` draws with; None for a picture with no texture (too big, bad data).
    pub fn bound(&self, d: &ImageDraw) -> Option<(&ProtocolObject<dyn MTLTexture>, &ProtocolObject<dyn MTLSamplerState>)> {
        let (texture, _) = self.cache.get(&(d.key, d.repeat))?;
        Some((texture, if d.repeat { &self.repeat_sampler } else { &self.sampler }))
    }

    /// Makes sure every picture in `images` has a texture; drops ones long unused.
    pub fn prepare<'a>(&mut self, device: &ProtocolObject<dyn MTLDevice>, images: impl IntoIterator<Item = &'a ImageDraw>) {
        self.frames += 1;
        for q in images {
            if let Some(entry) = self.cache.get_mut(&(q.key, q.repeat)) {
                entry.1 = self.frames;
                continue;
            }
            let (w, h) = q.size;
            let max = super::MAX_TEXTURE_SIDE;
            if w == 0 || h == 0 || w > max || h > max || q.pixels.len() != (w as usize) * (h as usize) * 4 {
                continue;
            }
            // SAFETY: a plain 2D descriptor of a size inside the device's limits.
            let desc = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(MTLPixelFormat::RGBA8Unorm_sRGB, w as usize, h as usize, false) };
            desc.setUsage(MTLTextureUsage::ShaderRead);
            // Managed: written from the CPU once, then read by the GPU (Intel Macs allow no shared textures).
            desc.setStorageMode(MTLStorageMode::Managed);
            let Some(texture) = device.newTextureWithDescriptor(&desc) else { continue };
            let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: w as usize, height: h as usize, depth: 1 } };
            // SAFETY: `pixels` holds exactly w * h RGBA texels (checked above).
            unsafe { texture.replaceRegion_mipmapLevel_withBytes_bytesPerRow(region, 0, std::ptr::NonNull::new(q.pixels.as_ptr() as *mut _).expect("a non-empty picture"), w as usize * 4) };
            self.cache.insert((q.key, q.repeat), (texture, self.frames));
            self.uploads += 1;
        }
        let now = self.frames;
        self.cache.retain(|_, (_, used)| now - *used <= TEXTURE_KEEP);
    }
}
