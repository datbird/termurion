//! The native Metal backend (macOS): the same CPU draw list the wgpu backend runs, drawn with
//! Metal directly. `shaders` builds on every OS, so its translation test runs everywhere.

pub mod shaders;

#[cfg(target_os = "macos")]
mod draw;
#[cfg(target_os = "macos")]
mod images;
#[cfg(target_os = "macos")]
mod quads;
#[cfg(target_os = "macos")]
pub mod readback;
#[cfg(target_os = "macos")]
mod surface;
#[cfg(all(test, target_os = "macos"))]
mod tests;
#[cfg(target_os = "macos")]
mod window;

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
    /// The device for the GPU preference: the low-power one when asked and present, else a
    /// high-performance one, else the system default (as wgpu ranks Metal's adapters).
    pub fn new(prefer_low_power: bool) -> Result<MetalShared, String> {
        let all = objc2_metal::MTLCopyAllDevices();
        let devices: Vec<Retained<ProtocolObject<dyn MTLDevice>>> = all.iter().collect();
        let pick = devices.iter().find(|d| d.isLowPower() == prefer_low_power).or_else(|| devices.first()).cloned();
        let device = pick.or_else(|| objc2_metal::MTLCreateSystemDefaultDevice()).ok_or("no Metal device")?;
        let queue = device.newCommandQueue().ok_or("no Metal command queue")?;
        Ok(MetalShared { device, queue })
    }

    pub fn name(&self) -> String {
        self.device.name().to_string()
    }

    /// The device described as wgpu describes an adapter (About, Copy Performance Stats, the log).
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        let kind = if self.device.isLowPower() { wgpu::DeviceType::IntegratedGpu } else { wgpu::DeviceType::DiscreteGpu };
        wgpu::AdapterInfo {
            name: self.name(),
            vendor: 0,
            device: 0,
            device_type: kind,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Metal,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: None,
            limit_bucket: None,
        }
    }

    /// A command buffer whose errors are reported for device generation `generation`.
    pub(crate) fn command_buffer(&self, generation: u64) -> Result<Retained<ProtocolObject<dyn MTLCommandBuffer>>, String> {
        let cb = self.queue.commandBuffer().ok_or("no Metal command buffer")?;
        watch(&cb, generation);
        Ok(cb)
    }
}

/// Reports a failed command buffer: a removed device or a revoked one is a loss (the app's
/// recovery remakes the device); any other error is only logged.
#[cfg(target_os = "macos")]
fn watch(cb: &ProtocolObject<dyn MTLCommandBuffer>, generation: u64) {
    let handler = block2::RcBlock::new(move |cb: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
        // SAFETY: Metal hands the handler the live command buffer it completed.
        let cb = unsafe { cb.as_ref() };
        if cb.status() != MTLCommandBufferStatus::Error {
            return;
        }
        let (code, text) = cb.error().map_or((0, "unknown error".to_string()), |e| (e.code(), e.localizedDescription().to_string()));
        command_failed(generation, code, &text);
    });
    // SAFETY: the block is copied by Metal and lives until the handler ran.
    unsafe { cb.addCompletedHandler(block2::RcBlock::as_ptr(&handler)) };
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

/// One Metal device per test process, as `render::test_gpu` keeps one wgpu device.
#[cfg(all(test, target_os = "macos"))]
pub fn test_device() -> &'static MetalShared {
    static DEVICE: std::sync::OnceLock<MetalShared> = std::sync::OnceLock::new();
    // High performance, as the shared wgpu test device asks.
    DEVICE.get_or_init(|| MetalShared::new(false).expect("a Metal device"))
}

#[cfg(target_os = "macos")]
static DISPLAY: std::sync::OnceLock<winit::event_loop::OwnedDisplayHandle> = std::sync::OnceLock::new();

/// The display the settings windows' wgpu instance is made for (`egui_wgpu`).
#[cfg(target_os = "macos")]
pub fn set_display(display: winit::event_loop::OwnedDisplayHandle) {
    let _ = DISPLAY.set(display);
}

/// egui windows still draw with wgpu while terminal windows draw on Metal (until egui moves to
/// Metal): one wgpu device on wgpu's Metal backend, made on first use and kept.
#[cfg(target_os = "macos")]
pub fn egui_wgpu() -> Result<crate::gpu_shared::GpuShared, String> {
    static GPU: std::sync::OnceLock<Result<crate::gpu_shared::GpuShared, String>> = std::sync::OnceLock::new();
    GPU.get_or_init(|| {
        let mut desc = match DISPLAY.get() {
            Some(d) => wgpu::InstanceDescriptor::new_with_display_handle(Box::new(d.clone())),
            None => wgpu::InstanceDescriptor::new_without_display_handle(),
        };
        desc.backends = wgpu::Backends::METAL;
        let instance = wgpu::Instance::new(desc);
        let pick = wgpu::RequestAdapterOptions { power_preference: crate::power::chosen(), ..Default::default() };
        let adapter = pollster::block_on(instance.request_adapter(&pick)).map_err(|e| format!("no adapter for the settings windows: {e}"))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| format!("no device for the settings windows: {e}"))?;
        crate::gpu_pick::watch(&device, crate::gpu_pick::current_generation());
        Ok(crate::gpu_shared::GpuShared::Wgpu { instance, adapter, device, queue })
    })
    .clone()
}
