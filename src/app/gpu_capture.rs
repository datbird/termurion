//! Capture GPU Frame (`captureNextMetalFrame:`): debug builds only (B8). The next frame is
//! bracketed with a graphics-debugger capture: wgpu's on a wgpu window, `MTLCaptureManager`'s on a
//! Metal window. A tool such as Xcode, RenderDoc or PIX must be attached, otherwise nothing
//! happens. A release build has no menu item, no chord, no handler and no capture call.

#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(debug_assertions)]
use crate::gpu_window::WindowGpu;

#[cfg(debug_assertions)]
static ARMED: AtomicBool = AtomicBool::new(false);

/// The next frame is captured.
#[cfg(debug_assertions)]
pub(super) fn arm() {
    ARMED.store(true, Ordering::Relaxed);
}

/// Starts the capture of `win`'s device when armed; true when it did (pass it to `end`).
#[cfg(debug_assertions)]
pub(super) fn begin(win: &WindowGpu) -> bool {
    if !ARMED.swap(false, Ordering::Relaxed) {
        return false;
    }
    match win {
        WindowGpu::Wgpu(_) => {
            let Some(device) = win.wgpu_device() else { return false };
            // SAFETY: wgpu requires a capture tool to be attached for this to do anything; with none it is a no-op.
            unsafe { device.start_graphics_debugger_capture() };
            true
        }
        #[cfg(target_os = "macos")]
        WindowGpu::Metal(_) => match win.shared() {
            crate::gpu_shared::GpuShared::Metal(m) => m.start_capture(),
            _ => false,
        },
    }
}

/// Ends the capture `begin` started, after the frame was presented.
#[cfg(debug_assertions)]
pub(super) fn end(win: &WindowGpu) {
    match win {
        WindowGpu::Wgpu(_) => {
            if let Some(device) = win.wgpu_device() {
                // SAFETY: pairs with the start made by `begin`.
                unsafe { device.stop_graphics_debugger_capture() };
            }
        }
        #[cfg(target_os = "macos")]
        WindowGpu::Metal(_) => crate::metal_backend::MetalShared::stop_capture(),
    }
}

#[cfg(all(test, debug_assertions, target_os = "macos"))]
mod tests {
    /// A Metal window's frame is bracketed by the Metal capture manager when armed, and only then.
    #[test]
    fn capture_brackets_a_metal_frame() {
        let win = crate::gpu_window::WindowGpu::Metal(crate::metal_backend::MetalWindow::offscreen(crate::metal_backend::test_device(), crate::render::Renderer::cpu_only(crate::render_pixel_tests::FACE, 14.0), (32, 16)));
        assert!(!super::begin(&win), "not armed");
        super::arm();
        assert!(super::begin(&win), "armed: the Metal capture starts");
        super::end(&win);
        // SAFETY: the process-wide capture manager.
        let manager = unsafe { objc2_metal::MTLCaptureManager::sharedCaptureManager() };
        assert!(!manager.isCapturing(), "the capture ended with the frame");
        assert!(!super::begin(&win), "one frame per arm");
    }
}
