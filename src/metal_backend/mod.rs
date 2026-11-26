//! The native Metal backend (macOS): the same CPU draw list the wgpu backend runs, drawn with
//! Metal directly. `shaders` builds on every OS, so its translation test runs everywhere.

pub mod shaders;

#[cfg(target_os = "macos")]
mod chain;
#[cfg(target_os = "macos")]
mod draw;
#[cfg(target_os = "macos")]
pub mod egui_painter;
#[cfg(target_os = "macos")]
mod images;
#[cfg(target_os = "macos")]
mod quads;
#[cfg(target_os = "macos")]
pub mod readback;
#[cfg(all(test, target_os = "macos"))]
mod reference;
#[cfg(target_os = "macos")]
mod surface;
#[cfg(all(test, target_os = "macos"))]
mod tests;
#[cfg(target_os = "macos")]
mod text;
#[cfg(target_os = "macos")]
mod window;

#[cfg(target_os = "macos")]
pub use egui_painter::{EguiCanvas, EguiPainter};
#[cfg(target_os = "macos")]
pub use window::MetalWindow;

#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::runtime::ProtocolObject;
#[cfg(target_os = "macos")]
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferError, MTLCommandBufferStatus, MTLCommandQueue, MTLDevice};

/// The largest texture side the backend uses. wgpu's device is asked for the default limits, so
/// its glyph atlas, pictures and screenshots stop at 8192; Metal keeps the same limit.
pub const MAX_TEXTURE_SIDE: u32 = 8192;

/// The Metal device every window draws on, and its one queue.
#[cfg(target_os = "macos")]
#[derive(Clone)]
pub struct MetalShared {
    pub device: Retained<ProtocolObject<dyn MTLDevice>>,
    pub queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
}

#[cfg(target_os = "macos")]
impl MetalShared {
    /// The device wgpu would pick for `pref` (`gpu_pick::metal_order`), falling back to the system
    /// default when the list is empty.
    pub fn new(pref: crate::gpu_pick::PowerPreference) -> Result<MetalShared, String> {
        let devices = devices();
        let facts: Vec<crate::gpu_pick::MetalFacts> = devices.iter().map(|d| facts_of(d)).collect();
        let pick = crate::gpu_pick::metal_order(&facts, pref).first().map(|&i| devices[i].clone());
        let device = pick.or_else(|| objc2_metal::MTLCreateSystemDefaultDevice()).ok_or("no Metal device")?;
        let queue = device.newCommandQueue().ok_or("no Metal command queue")?;
        Ok(MetalShared { device, queue })
    }

    pub fn name(&self) -> String {
        self.device.name().to_string()
    }

    /// The device as the adapter list describes one (About, Copy Performance Stats, the log).
    pub fn facts(&self) -> crate::gpu_pick::Facts {
        crate::gpu_pick::Facts { backend: crate::gpu_pick::Backend::Metal, name: self.name(), kind: facts_of(&self.device).kind() }
    }

    /// Starts a graphics debugger capture of the device (Xcode's), as wgpu-hal's
    /// `start_graphics_debugger_capture` does; with no debugger attached nothing is recorded.
    /// Debug builds only (Capture GPU Frame).
    #[cfg(debug_assertions)]
    pub fn start_capture(&self) -> bool {
        // SAFETY: the shared capture manager is a process-wide singleton.
        let manager = unsafe { objc2_metal::MTLCaptureManager::sharedCaptureManager() };
        let scope = manager.newCaptureScopeWithDevice(&self.device);
        manager.setDefaultCaptureScope(Some(&scope));
        #[allow(deprecated)]
        manager.startCaptureWithScope(&scope);
        objc2_metal::MTLCaptureScope::beginScope(&*scope);
        true
    }

    /// Ends the capture `start_capture` began.
    #[cfg(debug_assertions)]
    pub fn stop_capture() {
        // SAFETY: as in `start_capture`.
        let manager = unsafe { objc2_metal::MTLCaptureManager::sharedCaptureManager() };
        if let Some(scope) = manager.defaultCaptureScope() {
            objc2_metal::MTLCaptureScope::endScope(&*scope);
        }
        manager.stopCapture();
    }

    /// A command buffer whose errors are reported for device generation `generation`.
    pub(crate) fn command_buffer(&self, generation: u64) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>, String> {
        let cb = self.queue.commandBuffer().ok_or("no Metal command buffer")?;
        watch(&cb, generation);
        Ok(cb)
    }
}

/// Every Metal device on the system.
#[cfg(target_os = "macos")]
fn devices() -> Vec<Retained<ProtocolObject<dyn MTLDevice>>> {
    objc2_metal::MTLCopyAllDevices().iter().collect()
}

/// What the pick needs to know about `d`, read as wgpu-hal reads it.
#[cfg(target_os = "macos")]
fn facts_of(d: &ProtocolObject<dyn MTLDevice>) -> crate::gpu_pick::MetalFacts {
    let unified = objc2::available!(macos = 10.15).then(|| d.hasUnifiedMemory());
    crate::gpu_pick::MetalFacts { low_power: d.isLowPower(), headless: d.isHeadless(), unified }
}

/// The system's Metal devices as the adapter list host.log and "Prefer integrated GPU" read.
#[cfg(target_os = "macos")]
pub fn adapter_facts() -> Vec<crate::gpu_pick::Facts> {
    devices().iter().map(|d| crate::gpu_pick::Facts { backend: crate::gpu_pick::Backend::Metal, name: d.name().to_string(), kind: facts_of(d).kind() }).collect()
}

/// Reports a failed command buffer: a removed device or a revoked one is a loss (the app's
/// recovery remakes the device); any other error is only logged.
#[cfg(target_os = "macos")]
fn watch(cb: &ProtocolObject<dyn MTLCommandBuffer>, generation: u64) {
    let handler = block2::RcBlock::new(move |cb: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        // SAFETY: Metal hands the handler the live command buffer it completed.
        let cb = unsafe { cb.as_ref() };
        completed(generation, cb.status(), cb.error().as_deref());
    });
    // SAFETY: the block is copied by Metal and lives until the handler ran.
    unsafe { cb.addCompletedHandler(block2::RcBlock::as_ptr(&handler)) };
}

/// A finished command buffer's outcome: an error in Metal's command buffer domain goes on to
/// `command_failed`; any other error is only logged.
#[cfg(target_os = "macos")]
pub(crate) fn completed(generation: u64, status: MTLCommandBufferStatus, error: Option<&objc2_foundation::NSError>) {
    if status != MTLCommandBufferStatus::Error {
        return;
    }
    let Some(e) = error else { return command_failed(generation, 0, "unknown error") };
    let text = e.localizedDescription().to_string();
    // SAFETY: a constant NSString Metal exports.
    let metal = *e.domain() == *unsafe { objc2_metal::MTLCommandBufferErrorDomain };
    if metal {
        command_failed(generation, e.code(), &text);
    } else {
        crate::gpu_pick::log_gpu_error(&format!("Metal command buffer error {} {}: {text}", e.domain(), e.code()));
    }
}

/// What a command buffer error with `code` means.
#[cfg(target_os = "macos")]
pub(crate) fn command_failed(generation: u64, code: isize, text: &str) {
    let lost = code == MTLCommandBufferError::DeviceRemoved.0 as isize || code == MTLCommandBufferError::AccessRevoked.0 as isize;
    if lost {
        crate::gpu_pick::report_lost(generation, format!("device lost (Metal error {code}): {text}"));
    } else {
        crate::gpu_pick::log_gpu_error(&format!("Metal command buffer error {code}: {text}"));
    }
}

/// One Metal device per test process: every Metal test draws on it.
#[cfg(all(test, target_os = "macos"))]
pub fn test_device() -> &'static MetalShared {
    static DEVICE: std::sync::OnceLock<MetalShared> = std::sync::OnceLock::new();
    // High performance, as the shared wgpu test device asks elsewhere.
    DEVICE.get_or_init(|| MetalShared::new(crate::gpu_pick::PowerPreference::HighPerformance).expect("a Metal device"))
}

/// A Metal buffer, or None when the device has no room (or a test made it fail).
#[cfg(target_os = "macos")]
pub(crate) fn new_buffer(device: &ProtocolObject<dyn MTLDevice>, size: usize, options: objc2_metal::MTLResourceOptions) -> Option<Retained<ProtocolObject<dyn objc2_metal::MTLBuffer>>> {
    if fails_now(false) {
        return None;
    }
    device.newBufferWithLength_options(size, options)
}

/// A Metal sampler, or None when the device made none (or a test made it fail).
#[cfg(target_os = "macos")]
pub(crate) fn new_sampler(device: &ProtocolObject<dyn MTLDevice>, desc: &objc2_metal::MTLSamplerDescriptor) -> Option<Retained<ProtocolObject<dyn objc2_metal::MTLSamplerState>>> {
    if fails_now(false) {
        return None;
    }
    device.newSamplerStateWithDescriptor(desc)
}

/// A Metal texture, or None when the device has no room (or a test made it fail).
#[cfg(target_os = "macos")]
pub(crate) fn new_texture(device: &ProtocolObject<dyn MTLDevice>, desc: &objc2_metal::MTLTextureDescriptor) -> Option<Retained<ProtocolObject<dyn objc2_metal::MTLTexture>>> {
    if fails_now(true) {
        return None;
    }
    device.newTextureWithDescriptor(desc)
}

// How many more allocations of this test thread fail: any kind; then textures only, after how
// many textures are made first.
#[cfg(all(test, target_os = "macos"))]
thread_local!(static FAILS: std::cell::Cell<(u32, u32, u32)> = const { std::cell::Cell::new((0, 0, 0)) });

/// Makes the next `n` buffers or textures this thread asks for fail (tests).
#[cfg(all(test, target_os = "macos"))]
pub fn fail_allocations(n: u32) {
    FAILS.with(|f| f.set((n, f.get().1, f.get().2)));
}

/// Makes the next `n` textures this thread asks for fail; buffers are still made (tests).
#[cfg(all(test, target_os = "macos"))]
pub fn fail_textures(n: u32) {
    fail_textures_after(0, n);
}

/// `fail_textures`, once `made` more textures were made (tests).
#[cfg(all(test, target_os = "macos"))]
pub fn fail_textures_after(made: u32, n: u32) {
    FAILS.with(|f| f.set((f.get().0, n, made)));
}

#[cfg(target_os = "macos")]
fn fails_now(texture: bool) -> bool {
    #[cfg(test)]
    return FAILS.with(|f| {
        let (any, textures, skip) = f.get();
        if any > 0 {
            f.set((any - 1, textures, skip));
            return true;
        }
        if texture && textures > 0 {
            if skip > 0 {
                f.set((any, textures, skip - 1));
                return false;
            }
            f.set((any, textures - 1, 0));
            return true;
        }
        false
    });
    #[cfg(not(test))]
    {
        let _ = texture;
        false
    }
}
