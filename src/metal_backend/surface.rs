//! The window's `CAMetalLayer`: a sublayer of winit's view that follows the view's bounds and
//! scale, handing out drawables.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyClass, AnyObject, NSObject, ProtocolObject};
use objc2::{class, define_class, msg_send, sel, AllocAnyThread, ClassType, DefinedClass, MainThreadMarker};
use objc2_foundation::{ns_string, NSDictionary, NSRunLoop, NSRunLoopCommonModes, NSKeyValueChangeKey, NSKeyValueChangeNewKey, NSKeyValueObservingOptions, NSNumber, NSObjectNSKeyValueObserverRegistration, NSSize, NSString, NSValue};
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLDevice, MTLPixelFormat, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage};
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
    /// One of the layer's own surfaces (`Surface::next_contents`): its index, for `show`, and its texture.
    Contents(usize, Retained<ProtocolObject<dyn MTLTexture>>),
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

/// `struct __IOSurface`, behind an `IOSurfaceRef`.
#[repr(C)]
struct IoSurfaceRef {
    _opaque: [u8; 0],
}

// SAFETY: the encoding Metal's `newTextureWithDescriptor:iosurface:plane:` declares.
unsafe impl objc2::encode::RefEncode for IoSurfaceRef {
    const ENCODING_REF: objc2::encode::Encoding = objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("__IOSurface", &[]));
}

#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceCreate(properties: *const c_void) -> *mut IoSurfaceRef;
    fn IOSurfaceIsInUse(surface: *const IoSurfaceRef) -> u8;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// An IOSurface, released when the last holder (the pool, a pending show) lets go.
struct IoSurface(NonNull<IoSurfaceRef>);

// SAFETY: an IOSurface is a thread-safe CoreFoundation object.
unsafe impl Send for IoSurface {}
unsafe impl Sync for IoSurface {}

impl Drop for IoSurface {
    fn drop(&mut self) {
        // SAFETY: the one reference IOSurfaceCreate gave.
        unsafe { CFRelease(self.0.as_ptr().cast()) };
    }
}

type Shown = block2::RcBlock<dyn Fn(NonNull<ProtocolObject<dyn MTLCommandBuffer>>)>;

/// A surface the window draws a frame into and the layer then shows as its contents, the way
/// Ghostty presents: no drawable is made per frame, so none of Core Animation's per-drawable
/// work (a shared event, frame pacing state, the layer's event queue) runs.
struct Target {
    surface: Arc<IoSurface>,
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    /// Drawn into and not shown yet.
    busy: Arc<AtomicBool>,
    /// The completed handler that shows it, made once so a frame makes no block.
    show: Shown,
}

/// How many surfaces a layer draws into at most (the drawables' count before).
const TARGETS: usize = 3;
/// `kCVPixelFormatType_32BGRA`, the drawables' own layout.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");

impl Target {
    fn new(device: &ProtocolObject<dyn MTLDevice>, layer: &Retained<CAMetalLayer>, (w, h): (u32, u32)) -> Result<Target, String> {
        use objc2_foundation::NSNumber;
        if super::fails_now(true) {
            return Err(format!("no surface of {w} x {h}"));
        }
        let keys = [ns_string!("IOSurfaceWidth"), ns_string!("IOSurfaceHeight"), ns_string!("IOSurfaceBytesPerElement"), ns_string!("IOSurfacePixelFormat")];
        let values = [NSNumber::new_u32(w), NSNumber::new_u32(h), NSNumber::new_u32(4), NSNumber::new_u32(BGRA)];
        let values: Vec<&NSNumber> = values.iter().map(|v| &**v).collect();
        let props = NSDictionary::from_slices(&keys, &values);
        // SAFETY: an NSDictionary is a CFDictionary; the keys are IOSurface's property names.
        let surface = NonNull::new(unsafe { IOSurfaceCreate(Retained::as_ptr(&props).cast()) }).ok_or_else(|| format!("no surface of {w} x {h}"))?;
        let surface = Arc::new(IoSurface(surface));
        // SAFETY: a plain 2D descriptor; the window keeps its size inside the limit.
        let desc = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(FORMAT, w as usize, h as usize, false) };
        // As the drawables: drawn into and readable (the layer was not framebuffer only).
        desc.setUsage(MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        desc.setStorageMode(if device.hasUnifiedMemory() { MTLStorageMode::Shared } else { MTLStorageMode::Managed });
        // SAFETY: a texture over plane 0 of a BGRA surface of the descriptor's size and layout.
        let texture: Option<Retained<ProtocolObject<dyn MTLTexture>>> = unsafe { msg_send![device, newTextureWithDescriptor: &*desc, iosurface: surface.0.as_ptr(), plane: 0usize] };
        let texture = texture.ok_or_else(|| format!("no Metal texture over a surface of {w} x {h}"))?;
        let busy = Arc::new(AtomicBool::new(false));
        let show = {
            let (layer, surface, busy) = (layer.clone(), surface.clone(), busy.clone());
            block2::RcBlock::new(move |cb: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                // SAFETY: Metal hands the handler the live command buffer it completed.
                if unsafe { cb.as_ref() }.status() == MTLCommandBufferStatus::Completed {
                    // On Metal's completion queue: Core Animation takes property changes from
                    // any thread in an explicit transaction, so the main loop never wakes for it.
                    without_animation(|| {
                        // SAFETY: an IOSurface is valid layer contents.
                        let contents: &AnyObject = unsafe { surface.0.cast::<AnyObject>().as_ref() };
                        unsafe { layer.setContents(Some(contents)) };
                    });
                }
                busy.store(false, Ordering::Release);
            })
        };
        Ok(Target { surface, texture, busy, show })
    }

    fn free(&self) -> bool {
        // SAFETY: a live surface. In use: the window server still shows it.
        !self.busy.load(Ordering::Acquire) && unsafe { IOSurfaceIsInUse(self.surface.0.as_ptr()) } == 0
    }
}

pub struct Surface {
    pub layer: Retained<CAMetalLayer>,
    /// None on macOS before 14 and in tests: effect frames then follow the loop timer.
    display: Option<DisplayLink>,
    /// The surfaces `next_contents` hands out, all of `targets_size`.
    targets: RefCell<Vec<Target>>,
    targets_size: Cell<(u32, u32)>,
    /// Where the search for a free surface starts: the one after the last handed out, so the
    /// one shown longest ago goes first.
    next_target: Cell<usize>,
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
        Ok(Surface { layer: TrackingLayer::new(&root).into_super(), display: DisplayLink::new(view), targets: RefCell::default(), targets_size: Cell::new((0, 0)), next_target: Cell::new(0) })
    }

    /// A layer in no window (tests).
    #[cfg(test)]
    pub fn detached() -> Surface {
        Surface { layer: CAMetalLayer::new(), display: None, targets: RefCell::default(), targets_size: Cell::new((0, 0)), next_target: Cell::new(0) }
    }

    /// A tracking layer under `root`, a layer in no window (tests).
    #[cfg(test)]
    pub fn under(root: &CALayer) -> Surface {
        Surface { layer: TrackingLayer::new(root).into_super(), display: None, targets: RefCell::default(), targets_size: Cell::new((0, 0)), next_target: Cell::new(0) }
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
        // Made again on the next frame, for this device and size.
        self.targets.borrow_mut().clear();
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

    /// Whether frames go to the layer's own surfaces (`next_contents`) rather than drawables:
    /// with the display link only (macOS 14 and later), since nothing else paces them.
    pub fn shows_contents(&self) -> bool {
        self.display.is_some()
    }

    /// A free surface of `size` to draw the next frame into, unless the window is covered; then
    /// `show` puts it on screen once the frame's command buffer completes. Timeout: all are
    /// drawn into and not shown yet, or still on screen.
    pub fn next_contents(&self, device: &ProtocolObject<dyn MTLDevice>, size: (u32, u32)) -> Result<Next, String> {
        if self.occluded() {
            return Ok(Next::Occluded);
        }
        let mut targets = self.targets.borrow_mut();
        if self.targets_size.replace(size) != size {
            // A frame still on its way keeps its own surface alive until it is shown.
            targets.clear();
        }
        let start = self.next_target.get();
        let n = targets.len();
        let i = match (0..n).map(|k| (start + k) % n).find(|&i| targets[i].free()) {
            Some(i) => i,
            None if n < TARGETS => {
                targets.push(Target::new(device, &self.layer, size)?);
                n
            }
            None => return Ok(Next::Timeout),
        };
        targets[i].busy.store(true, Ordering::Release);
        self.next_target.set(i + 1);
        Ok(Next::Contents(i, targets[i].texture.clone()))
    }

    /// Shows surface `i` once `cb` completes (`show_now`: false for a frame that failed, which
    /// only gives the surface back).
    pub fn show(&self, i: usize, cb: &ProtocolObject<dyn MTLCommandBuffer>, show_now: bool) {
        let targets = self.targets.borrow();
        let Some(t) = targets.get(i) else { return };
        if show_now {
            // SAFETY: Metal copies the block (a retain: it is already on the heap) and calls it
            // once the command buffer is done.
            unsafe { cb.addCompletedHandler(block2::RcBlock::as_ptr(&t.show)) };
        } else {
            // Its command buffer runs before any later one on the queue that draws into it again.
            t.busy.store(false, Ordering::Release);
        }
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
