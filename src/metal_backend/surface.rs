//! The window's `CAMetalLayer`: a sublayer of winit's view that follows the view's bounds and
//! scale, handing out drawables.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::time::Duration;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, NSObject, ProtocolObject};
use objc2::{class, define_class, msg_send, sel, AllocAnyThread, ClassType, DefinedClass, MainThreadMarker};
use objc2_foundation::{ns_string, NSDictionary, NSRunLoop, NSRunLoopCommonModes, NSKeyValueChangeKey, NSKeyValueChangeNewKey, NSKeyValueObservingOptions, NSNumber, NSObjectNSKeyValueObserverRegistration, NSSize, NSString, NSValue};
use objc2_metal::{MTLDevice, MTLPixelFormat, MTLTexture};
use objc2_quartz_core::{CADisplayLink, CAFrameRateRange, CALayer, CAMetalDrawable, CAMetalLayer};

// The layer wgpu draws on is raw-window-metal 1.1.0's `ObserverLayer` (src/observer.rs): a
// CAMetalLayer subclass, added as a sublayer, that copies the root layer's bounds and
// contentsScale by key-value observing. It does not use the autoresizing mask (which rounds
// fractional bounds wrongly with auto layout on macOS 14 and later). This is the same layer,
// under its own class name, with each change made with implicit animations off.
define_class!(
    #[unsafe(super(CAMetalLayer))]
    #[name = "TermurionMetalLayer"]
    #[ivars = Weak<CALayer>]
    struct TrackingLayer;

    impl TrackingLayer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn _observe_value(&self, key_path: Option<&NSString>, object: Option<&AnyObject>, change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, context: *mut c_void) {
            self.observe_value(key_path, object, change, context);
        }
    }
);

impl Drop for TrackingLayer {
    fn drop(&mut self) {
        // The root layer may be gone already; then the observers went with it.
        if let Some(root) = self.ivars().load() {
            // SAFETY: both observers were registered in `new`.
            unsafe {
                root.removeObserver_forKeyPath(self, ns_string!("contentsScale"));
                root.removeObserver_forKeyPath(self, ns_string!("bounds"));
            }
        }
    }
}

impl TrackingLayer {
    fn context() -> *mut c_void {
        TrackingLayer::class() as *const AnyClass as *mut c_void
    }

    fn new(root: &CALayer) -> Retained<TrackingLayer> {
        let this = Self::alloc().set_ivars(Weak::new(root));
        // SAFETY: initializing a CAMetalLayer subclass with no requirements of its own.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };
        root.addSublayer(&this);
        // SAFETY: the observer removes itself in Drop; the options ask for the initial value too.
        unsafe {
            for key in [ns_string!("contentsScale"), ns_string!("bounds")] {
                root.addObserver_forKeyPath_options_context(&this, key, NSKeyValueObservingOptions::New | NSKeyValueObservingOptions::Initial, Self::context());
            }
        }
        this
    }

    fn observe_value(&self, key_path: Option<&NSString>, object: Option<&AnyObject>, change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, context: *mut c_void) {
        if context != Self::context() {
            // SAFETY: the superclass's own observations go on to it unchanged.
            return unsafe { msg_send![super(self), observeValueForKeyPath: key_path, ofObject: object, change: change, context: context] };
        }
        let Some(new) = change.and_then(|c| c.objectForKey(unsafe { NSKeyValueChangeNewKey })) else { return };
        without_animation(|| {
            if key_path == Some(ns_string!("contentsScale")) {
                if let Ok(n) = new.downcast::<NSNumber>() {
                    self.setContentsScale(n.as_cgfloat());
                }
            } else if key_path == Some(ns_string!("bounds")) {
                if let Some(bounds) = new.downcast::<NSValue>().ok().and_then(|v| v.get_rect()) {
                    self.setFrame(bounds);
                }
            }
        });
    }
}

/// Runs `f` in a Core Animation transaction with implicit actions off, so the layer's frame, scale
/// and drawable size change at once and never animate behind the drawable.
fn without_animation(f: impl FnOnce()) {
    let tx = class!(CATransaction);
    // SAFETY: CATransaction's class methods; begin and commit pair up around `f`.
    unsafe {
        let _: () = msg_send![tx, begin];
        let _: () = msg_send![tx, setDisableActions: true];
    }
    f();
    // SAFETY: as above.
    unsafe {
        let _: () = msg_send![tx, commit];
    }
}

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

/// What the display link's callback leaves for the loop.
struct Ticks {
    /// A refresh came since the last `display_tick`.
    ticked: Cell<bool>,
    /// The display's refresh period, in seconds.
    period: Cell<f64>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "TermurionDisplayTicker"]
    #[ivars = Ticks]
    struct Ticker;

    impl Ticker {
        #[unsafe(method(tick:))]
        fn _tick(&self, link: &CADisplayLink) {
            // Waking the main loop is the point; the effect frame is asked for in its idle pass.
            let t = self.ivars();
            t.ticked.set(true);
            t.period.set(link.duration());
        }
    }
);

/// Wakes the main loop at the display's refresh while an effect animates, so the frame asks for
/// a drawable right after the display let one go instead of waiting in `nextDrawable`.
struct DisplayLink {
    link: Retained<CADisplayLink>,
    ticker: Retained<Ticker>,
    /// The frame gap the link runs for (None: the display's rate); None while paused.
    running: Cell<Option<Option<Duration>>>,
}

impl DisplayLink {
    /// The view's display link, paused (macOS 14 and later).
    fn new(view: &NSObject) -> Option<DisplayLink> {
        if !objc2::available!(macos = 14.0) {
            return None;
        }
        let ticker: Retained<Ticker> = {
            let this = Ticker::alloc().set_ivars(Ticks { ticked: Cell::new(false), period: Cell::new(1.0 / 60.0) });
            // SAFETY: a plain NSObject subclass.
            unsafe { msg_send![super(this), init] }
        };
        // SAFETY: NSView's displayLinkWithTarget:selector: (macOS 14), checked above; the link
        // keeps its target, and `tick:` takes the link.
        let link: Option<Retained<CADisplayLink>> = unsafe { msg_send![view, displayLinkWithTarget: &*ticker, selector: sel!(tick:)] };
        let link = link?;
        link.setPaused(true);
        // The common modes, as winit's own wake timer.
        // SAFETY: the main run loop and a constant mode.
        unsafe { link.addToRunLoop_forMode(&NSRunLoop::mainRunLoop(), NSRunLoopCommonModes) };
        Some(DisplayLink { link, ticker, running: Cell::new(None) })
    }

    fn follow(&self, gap: Option<Option<Duration>>) {
        if self.running.replace(gap) == gap {
            return;
        }
        if let Some(gap) = gap {
            // The link ticks at the effect's own rate where the display allows it; 0 is the display's rate.
            let fps = gap.map_or(0.0, |g| (1.0 / g.as_secs_f64()).round() as f32);
            self.link.setPreferredFrameRateRange(CAFrameRateRange { minimum: fps, maximum: fps, preferred: fps });
        }
        self.ticker.ivars().ticked.set(false);
        self.link.setPaused(gap.is_none());
    }
}

impl Drop for DisplayLink {
    fn drop(&mut self) {
        self.link.invalidate();
    }
}

pub struct Surface {
    pub layer: Retained<CAMetalLayer>,
    /// None on macOS before 14 and in tests: effect frames then follow the loop timer.
    display: Option<DisplayLink>,
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
        Ok(Surface { layer: TrackingLayer::new(&root).into_super(), display: DisplayLink::new(view) })
    }

    /// A layer in no window (tests).
    #[cfg(test)]
    pub fn detached() -> Surface {
        Surface { layer: CAMetalLayer::new(), display: None }
    }

    /// A tracking layer under `root`, a layer in no window (tests).
    #[cfg(test)]
    pub fn under(root: &CALayer) -> Surface {
        Surface { layer: TrackingLayer::new(root).into_super(), display: None }
    }

    /// Sets the layer up for `device` at `w` by `h` pixels and `scale`: sRGB, readable (not
    /// framebuffer only), synced to the display, opaque. The frame and scale normally come from the
    /// root layer (`TrackingLayer`); a layer with no root takes `scale` here.
    pub fn configure(&self, device: &ProtocolObject<dyn MTLDevice>, size: (u32, u32), scale: f64) {
        self.configure_as(device, size, scale, FORMAT, false);
    }

    /// `configure` in `format`; `framebuffer_only` for a layer whose drawables are only drawn
    /// into (an egui window's, as wgpu-hal sets it for a render-attachment-only surface).
    pub fn configure_as(&self, device: &ProtocolObject<dyn MTLDevice>, (w, h): (u32, u32), scale: f64, format: MTLPixelFormat, framebuffer_only: bool) {
        let l = &self.layer;
        l.setDevice(Some(device));
        l.setPixelFormat(format);
        l.setFramebufferOnly(framebuffer_only);
        // wgpu-hal sets desired_maximum_frame_latency + 1 drawables: the app asks for 2, so 3.
        l.setMaximumDrawableCount(3);
        l.setDisplaySyncEnabled(true);
        // As wgpu-hal: a drawable request waits for one rather than giving up after a second.
        l.setAllowsNextDrawableTimeout(false);
        l.setOpaque(true);
        without_animation(|| {
            match l.superlayer() {
                Some(root) => {
                    l.setFrame(root.bounds());
                    l.setContentsScale(root.contentsScale());
                }
                None => l.setContentsScale(scale),
            }
            l.setDrawableSize(NSSize::new(f64::from(w.max(1)), f64::from(h.max(1))));
        });
    }

    /// Runs the display link for effect frames `gap` apart (None inside: the display's rate), or
    /// pauses it (None).
    pub fn follow_display(&self, gap: Option<Option<Duration>>) {
        if let Some(d) = &self.display {
            d.follow(gap);
        }
    }

    /// The display's refresh period while effect frames follow it; None when they follow the
    /// loop timer.
    pub fn display_period(&self) -> Option<Duration> {
        let d = self.display.as_ref().filter(|d| d.running.get().is_some())?;
        Some(Duration::from_secs_f64(d.ticker.ivars().period.get().clamp(1.0 / 480.0, 1.0 / 15.0)))
    }

    /// Whether the display refreshed since the last call.
    pub fn take_display_tick(&self) -> bool {
        self.display.as_ref().is_some_and(|d| d.ticker.ivars().ticked.replace(false))
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
