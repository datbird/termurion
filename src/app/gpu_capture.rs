//! Capture GPU Frame (`captureNextMetalFrame:`): debug builds only (B8). The next frame is
//! bracketed with wgpu's graphics-debugger capture; a tool such as RenderDoc or PIX must be
//! attached, otherwise nothing happens. A release build has no menu item, no chord, no handler
//! and no call to wgpu's capture functions.

#[cfg(debug_assertions)]
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(debug_assertions)]
static ARMED: AtomicBool = AtomicBool::new(false);

/// The next frame is captured.
#[cfg(debug_assertions)]
pub(super) fn arm() {
    ARMED.store(true, Ordering::Relaxed);
}

/// Starts the capture when armed; true when it did (pass it to `end`).
#[cfg(debug_assertions)]
pub(super) fn begin(device: &wgpu::Device) -> bool {
    if !ARMED.swap(false, Ordering::Relaxed) {
        return false;
    }
    // SAFETY: wgpu requires a capture tool to be attached for this to do anything; with none it is a no-op.
    unsafe { device.start_graphics_debugger_capture() };
    true
}

/// Ends the capture `begin` started, after the frame was presented.
#[cfg(debug_assertions)]
pub(super) fn end(device: &wgpu::Device) {
    // SAFETY: pairs with the start made by `begin`.
    unsafe { device.stop_graphics_debugger_capture() };
}
