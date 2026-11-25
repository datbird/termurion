//! The window's `CAMetalLayer`: a sublayer of winit's view, sized to it, handing out drawables.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, ProtocolObject};
use objc2::{msg_send, MainThreadMarker};
use objc2_foundation::NSSize;
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CAAutoresizingMask, CALayer, CAMetalDrawable, CAMetalLayer};

/// The format every window draws in (wgpu's first sRGB surface format on Metal).
pub const FORMAT: MTLPixelFormat = MTLPixelFormat::BGRA8Unorm_sRGB;

/// What asking for a drawable gave.
pub enum Next {
    Drawable(Retained<ProtocolObject<dyn CAMetalDrawable>>, Retained<ProtocolObject<dyn MTLTexture>>),
    /// The window is fully covered: nothing to draw.
    Occluded,
    /// No drawable came.
    Timeout,
}

pub struct Surface {
    pub layer: Retained<CAMetalLayer>,
}

impl Surface {
    /// A new layer over the whole of `ns_view`, which is made layer-backed. Main thread only.
    pub fn attach(ns_view: NonNull<c_void>) -> Result<Surface, String> {
        MainThreadMarker::new().ok_or("the Metal layer must be made on the main thread")?;
        // SAFETY: winit's AppKit handle is a live NSView.
        let view: &NSObject = unsafe { ns_view.cast().as_ref() };
        let _: () = unsafe { msg_send![view, setWantsLayer: true] };
        let root: Option<Retained<CALayer>> = unsafe { msg_send![view, layer] };
        let root = root.ok_or("the view has no layer")?;
        let layer = CAMetalLayer::new();
        layer.setFrame(root.bounds());
        layer.setAutoresizingMask(CAAutoresizingMask::LayerWidthSizable | CAAutoresizingMask::LayerHeightSizable);
        root.addSublayer(&layer);
        Ok(Surface { layer })
    }

    /// A layer in no window (tests).
    #[cfg(test)]
    pub fn detached() -> Surface {
        Surface { layer: CAMetalLayer::new() }
    }

    /// Sets the layer up for `device` at `w` by `h` pixels and `scale`: sRGB, readable (not
    /// framebuffer only), two drawables (wgpu's frame latency of 2), synced to the display, opaque.
    pub fn configure(&self, device: &ProtocolObject<dyn MTLDevice>, (w, h): (u32, u32), scale: f64) {
        let l = &self.layer;
        l.setDevice(Some(device));
        l.setPixelFormat(FORMAT);
        l.setFramebufferOnly(false);
        l.setMaximumDrawableCount(2);
        l.setDisplaySyncEnabled(true);
        // As wgpu-hal: a drawable request waits for one rather than giving up after a second.
        l.setAllowsNextDrawableTimeout(false);
        l.setOpaque(true);
        l.setContentsScale(scale);
        if let Some(root) = l.superlayer() {
            l.setFrame(root.bounds());
        }
        l.setDrawableSize(NSSize::new(f64::from(w.max(1)), f64::from(h.max(1))));
    }

    /// The next drawable, unless the window is covered (wgpu-hal `acquire_texture`).
    pub fn next(&self) -> Next {
        if self.occluded() {
            return Next::Occluded;
        }
        objc2::rc::autoreleasepool(|_| match self.layer.nextDrawable() {
            Some(d) => {
                let t = d.texture();
                Next::Drawable(d, t)
            }
            None => Next::Timeout,
        })
    }

    /// Whether the window holding the layer is fully covered.
    fn occluded(&self) -> bool {
        const VISIBLE: usize = 1 << 1;
        let mut current: Option<Retained<CALayer>> = Some(Retained::into_super(self.layer.clone()));
        while let Some(layer) = current {
            if let Some(delegate) = layer.delegate() {
                let window: Option<Retained<NSObject>> = unsafe { msg_send![&*delegate, window] };
                return window.is_some_and(|w| {
                    let state: usize = unsafe { msg_send![&*w, occlusionState] };
                    state & VISIBLE == 0
                });
            }
            current = layer.superlayer();
        }
        false
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        self.layer.removeFromSuperlayer();
    }
}
