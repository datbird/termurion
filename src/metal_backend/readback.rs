//! A texture's pixels back on the CPU, as the wgpu backend's screenshots read them.

use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLBlitCommandEncoder, MTLBuffer, MTLCommandBuffer, MTLCommandEncoder, MTLCommandQueue, MTLDevice, MTLOrigin, MTLPixelFormat, MTLResourceOptions, MTLSize, MTLTexture};

/// The `w` by `h` texture's pixels as RGBA, row by row (BGRA textures swapped). The copy runs on
/// `queue` after the work already committed there.
pub fn read_rgba(queue: &ProtocolObject<dyn MTLCommandQueue>, texture: &ProtocolObject<dyn MTLTexture>, w: u32, h: u32) -> Result<Vec<u8>, String> {
    let row = w as usize * 4;
    let len = row * h as usize;
    let buffer = queue.device().newBufferWithLength_options(len.max(4), MTLResourceOptions::StorageModeShared).ok_or("no Metal readback buffer")?;
    let cb = queue.commandBuffer().ok_or("no Metal command buffer")?;
    let blit = cb.blitCommandEncoder().ok_or("no Metal blit encoder")?;
    // SAFETY: the region is the texture's whole level 0 and the buffer holds it.
    unsafe {
        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            texture,
            0,
            0,
            MTLOrigin { x: 0, y: 0, z: 0 },
            MTLSize { width: w as usize, height: h as usize, depth: 1 },
            &buffer,
            0,
            row,
            len,
        );
    }
    blit.endEncoding();
    cb.commit();
    cb.waitUntilCompleted();
    if let Some(e) = cb.error() {
        return Err(format!("Metal readback: {}", e.localizedDescription()));
    }
    // SAFETY: the shared buffer holds `len` bytes the finished copy wrote.
    let data = unsafe { std::slice::from_raw_parts(buffer.contents().as_ptr().cast::<u8>(), len) };
    let bgra = matches!(texture.pixelFormat(), MTLPixelFormat::BGRA8Unorm | MTLPixelFormat::BGRA8Unorm_sRGB);
    Ok(if bgra { data.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect() } else { data.to_vec() })
}
