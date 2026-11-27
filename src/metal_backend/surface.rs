//! The window's `CAMetalLayer`: a sublayer of winit's view that follows the view's bounds and
//! scale, handing out drawables.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
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
    #[ivars = Tracking]
    struct TrackingLayer;

    impl TrackingLayer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn _observe_value(&self, key_path: Option<&NSString>, object: Option<&AnyObject>, change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, context: *mut c_void) {
            self.observe_value(key_path, object, change, context);
        }
    }
);

/// The root layer a `TrackingLayer` follows, whether it still observes it, and the thread that
/// made it.
struct Tracking {
    root: Weak<CALayer>,
    observing: Cell<bool>,
    made_on: std::thread::ThreadId,
}

impl Drop for TrackingLayer {
    fn drop(&mut self) {
        // `Surface::drop` stops observing on the owner's thread. Core Animation may still let go
        // of the layer itself on another thread later (a presented drawable, its own deferred
        // releases), which is safe for a layer out of the tree; the root's observers never are.
        if self.ivars().observing.get() {
            debug_assert_eq!(std::thread::current().id(), self.ivars().made_on, "the Metal layer's observers were removed off its own thread");
            self.stop_observing();
        }
    }
}

impl TrackingLayer {
    /// Removes the observers from the root layer, once. On the thread that added them.
    fn stop_observing(&self) {
        if !self.ivars().observing.replace(false) {
            return;
        }
        // The root layer may be gone already; then the observers went with it.
        if let Some(root) = self.ivars().root.load() {
            // SAFETY: both observers were registered in `new`.
            unsafe {
                root.removeObserver_forKeyPath(self, ns_string!("contentsScale"));
                root.removeObserver_forKeyPath(self, ns_string!("bounds"));
            }
        }
    }

    fn context() -> *mut c_void {
        TrackingLayer::class() as *const AnyClass as *mut c_void
    }

    fn new(root: &CALayer) -> Retained<TrackingLayer> {
        let this = Self::alloc().set_ivars(Tracking { root: Weak::new(root), observing: Cell::new(true), made_on: std::thread::current().id() });
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

/// Commits this thread's open Core Animation transactions now, releasing the layers they hold.
fn flush_transactions() {
    // SAFETY: a CATransaction class method.
    unsafe {
        let _: () = msg_send![class!(CATransaction), flush];
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
#[derive(Default)]
pub(super) struct Ticks {
    /// The link fired since the last `take_display_tick`.
    ticked: Cell<bool>,
    /// The time between the link's callbacks, in seconds: its own rate, which is the effect's
    /// where the display allows it, not the display's refresh.
    interval: Cell<f64>,
    /// The last callback's timestamp.
    last: Cell<f64>,
    /// A callback gap that did not match `interval`: a second one like it is the new rate.
    odd: Cell<f64>,
}

impl Ticks {
    fn tick(&self, at: f64) {
        self.ticked.set(true);
        let gap = at - self.last.replace(at);
        if !(gap > 0.0 && gap < 0.25) {
            return;
        }
        // One late callback (a busy main thread) does not change the rate; two alike do.
        let near = |a: f64, b: f64| (a - b).abs() < b * 0.25;
        if near(gap, self.interval.get()) || near(gap, self.odd.get()) {
            self.interval.set(gap);
            self.odd.set(0.0);
        } else {
            self.odd.set(gap);
        }
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "TermurionDisplayTicker"]
    #[ivars = Rc<Ticks>]
    struct Ticker;

    impl Ticker {
        #[unsafe(method(tick:))]
        fn _tick(&self, link: &CADisplayLink) {
            // Waking the main loop is the point; the frame is asked for in its idle pass.
            self.ivars().tick(link.timestamp());
        }
    }
);

/// Wakes the main loop on the display link while an effect animates or a frame is owed, so a
/// frame starts right after the display let a surface go.
struct DisplayLink {
    link: Retained<CADisplayLink>,
    /// The frame gap the link runs for (None inside: the display's rate); None: no effect frames.
    running: Cell<Option<Option<Duration>>>,
    /// The rate `Ticks::interval` was measured for.
    measured: Cell<Option<Option<Duration>>>,
    paused: Cell<bool>,
}

impl DisplayLink {
    /// The view's display link, paused (macOS 14 and later).
    fn new(view: &NSObject, ticks: &Rc<Ticks>) -> Option<DisplayLink> {
        if !objc2::available!(macos = 14.0) {
            return None;
        }
        let ticker: Retained<Ticker> = {
            let this = Ticker::alloc().set_ivars(ticks.clone());
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
        Some(DisplayLink { link, running: Cell::new(None), measured: Cell::new(None), paused: Cell::new(true) })
    }

    /// Pauses the link when nothing wants it (no effect frames, no `owed` frame), runs it when
    /// something does.
    fn sync(&self, owed: bool) {
        let pause = self.running.get().is_none() && !owed;
        if self.paused.replace(pause) != pause {
            self.link.setPaused(pause);
        }
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

#[cfg(test)]
#[link(name = "IOSurface", kind = "framework")]
unsafe extern "C" {
    fn IOSurfaceIncrementUseCount(surface: *const IoSurfaceRef);
    fn IOSurfaceDecrementUseCount(surface: *const IoSurfaceRef);
}

/// An IOSurface, released when the last holder (the pool, a pending show) lets go.
struct IoSurface(NonNull<IoSurfaceRef>);

// SAFETY: an IOSurface is a thread-safe CoreFoundation object.
unsafe impl Send for IoSurface {}
unsafe impl Sync for IoSurface {}

impl Drop for IoSurface {
    fn drop(&mut self) {
        // SAFETY: the one reference IOSurfaceCreate gave.
        unsafe { core_foundation_sys::base::CFRelease(self.0.as_ptr().cast()) };
    }
}

/// What the completed handlers share: the layer they show surfaces on and the frame it shows.
struct Presenter {
    /// Taken out by `Surface::drop` on the main thread, so a handler never holds the layer's
    /// last reference: it only borrows it under the lock.
    layer: Mutex<Option<Retained<CAMetalLayer>>>,
    /// The serial of the frame the layer shows (0: none yet). A handler shows its frame only if
    /// it is newer, whatever order Metal runs the handlers in.
    shown: AtomicU64,
}

type Shown = block2::RcBlock<dyn Fn(NonNull<ProtocolObject<dyn MTLCommandBuffer>>)>;

/// A surface the window draws a frame into and the layer then shows as its contents, the way
/// Ghostty presents: no drawable is made per frame, so none of Core Animation's per-drawable
/// work (a shared event, frame pacing state, the layer's event queue) runs.
struct Target {
    surface: Arc<IoSurface>,
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    /// Drawn into and its command buffer not done yet.
    pending: Arc<AtomicBool>,
    /// The serial of the frame drawn into it last (0: none, or one that failed).
    serial: Arc<AtomicU64>,
    /// The completed handler that shows it, made once so a frame makes no block.
    show: Shown,
}

/// How many surfaces a layer draws into at most (the drawables' count before).
const TARGETS: usize = 3;
/// `kCVPixelFormatType_32BGRA`, the drawables' own layout.
const BGRA: u32 = u32::from_be_bytes(*b"BGRA");

impl Target {
    fn new(device: &ProtocolObject<dyn MTLDevice>, presenter: &Arc<Presenter>, (w, h): (u32, u32)) -> Result<Target, String> {
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
        let (pending, serial) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let show = {
            let (presenter, surface, pending, serial) = (presenter.clone(), surface.clone(), pending.clone(), serial.clone());
            block2::RcBlock::new(move |cb: NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
                // SAFETY: Metal hands the handler the live command buffer it completed.
                if unsafe { cb.as_ref() }.status() == MTLCommandBufferStatus::Completed {
                    let frame = serial.load(Ordering::Acquire);
                    let layer = presenter.layer.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(layer) = layer.as_ref().filter(|_| frame > presenter.shown.load(Ordering::Acquire)) {
                        // On Metal's completion queue: Core Animation takes property changes
                        // from any thread in an explicit transaction, so the main loop never
                        // wakes for it.
                        without_animation(|| {
                            // SAFETY: an IOSurface is valid layer contents.
                            let contents: &AnyObject = unsafe { surface.0.cast::<AnyObject>().as_ref() };
                            unsafe { layer.setContents(Some(contents)) };
                        });
                        // This thread has no run loop to commit an implicit transaction, which
                        // would keep the layer until some later flush on it.
                        flush_transactions();
                        presenter.shown.store(frame, Ordering::Release);
                    }
                }
                pending.store(false, Ordering::Release);
            })
        };
        Ok(Target { surface, texture, pending, serial, show })
    }

    /// Free to draw into: its frame is done and the layer shows a later one (or it never showed
    /// one), and the window server no longer uses it.
    fn free(&self, presenter: &Presenter) -> bool {
        self.free_with(presenter, || {})
    }

    /// `free`, with `between` run after the pending flag is read and before the shown serial is
    /// (tests put a completed handler there). The order matters: the handler stores the shown
    /// serial and then clears `pending` (both Release), so a `pending` read as false (Acquire)
    /// guarantees the shown serial read after it is at least that handler's. Read the other way
    /// round, a handler finishing in between leaves the surface it just put on screen looking
    /// free.
    fn free_with(&self, presenter: &Presenter, between: impl FnOnce()) -> bool {
        if self.pending.load(Ordering::Acquire) {
            return false;
        }
        between();
        let shown = presenter.shown.load(Ordering::Acquire);
        let serial = self.serial.load(Ordering::Acquire);
        // SAFETY: a live surface.
        (serial == 0 || serial != shown) && unsafe { IOSurfaceIsInUse(self.surface.0.as_ptr()) } == 0
    }
}

pub struct Surface {
    pub layer: Retained<CAMetalLayer>,
    /// None on macOS before 14, for egui windows and in tests: effect frames then follow the loop timer.
    display: Option<DisplayLink>,
    /// The link's ticks (tests tick them by hand).
    ticks: Rc<Ticks>,
    /// Frames go to `targets` (`next_contents`), not to drawables.
    contents: bool,
    /// A frame found no free surface: the link runs for it until a tick asks for it again.
    owed: Cell<bool>,
    presenter: Arc<Presenter>,
    /// The surfaces `next_contents` hands out, all of `targets_size`.
    targets: RefCell<Vec<Target>>,
    targets_size: Cell<(u32, u32)>,
    /// Where the search for a free surface starts: the one after the last handed out, so the
    /// one shown longest ago goes first.
    next_target: Cell<usize>,
    /// The last frame's serial.
    serial: Cell<u64>,
    /// The layer as the tracking layer it is (all but `detached`), to stop its observers in drop.
    tracking: Option<Retained<TrackingLayer>>,
    /// Test layers keep their root here.
    _root: Option<Retained<CALayer>>,
}

impl Surface {
    fn with(layer: Retained<CAMetalLayer>, display: Option<DisplayLink>, ticks: Rc<Ticks>, contents: bool, root: Option<Retained<CALayer>>) -> Surface {
        let presenter = Arc::new(Presenter { layer: Mutex::new(Some(layer.clone())), shown: AtomicU64::new(0) });
        Surface { layer, display, ticks, contents, owed: Cell::new(false), presenter, targets: RefCell::default(), targets_size: Cell::new((0, 0)), next_target: Cell::new(0), serial: Cell::new(0), tracking: None, _root: root }
    }

    fn tracked(layer: Retained<TrackingLayer>, display: Option<DisplayLink>, ticks: Rc<Ticks>, contents: bool, root: Option<Retained<CALayer>>) -> Surface {
        let mut s = Surface::with(layer.clone().into_super(), display, ticks, contents, root);
        s.tracking = Some(layer);
        s
    }

    /// The root layer of `ns_view`, which is made layer-backed. Main thread only.
    fn root_of(ns_view: NonNull<c_void>) -> Result<(&'static NSObject, Retained<CALayer>), String> {
        MainThreadMarker::new().ok_or("the Metal layer must be made on the main thread")?;
        // SAFETY: winit's AppKit handle is a live NSView, alive as long as its window.
        let view: &NSObject = unsafe { ns_view.cast().as_ref() };
        let _: () = unsafe { msg_send![view, setWantsLayer: true] };
        let root: Option<Retained<CALayer>> = unsafe { msg_send![view, layer] };
        Ok((view, root.ok_or("the view has no layer")?))
    }

    /// A new layer over the whole of `ns_view` that shows drawables (an egui window). Main thread only.
    pub fn attach(ns_view: NonNull<c_void>) -> Result<Surface, String> {
        let (_, root) = Self::root_of(ns_view)?;
        Ok(Surface::tracked(TrackingLayer::new(&root), None, Rc::default(), false, None))
    }

    /// A terminal window's layer over `ns_view`: on macOS 14 and later it shows its own
    /// surfaces, paced by the view's display link. Main thread only.
    pub fn attach_terminal(ns_view: NonNull<c_void>) -> Result<Surface, String> {
        let (view, root) = Self::root_of(ns_view)?;
        let ticks: Rc<Ticks> = Rc::default();
        let display = DisplayLink::new(view, &ticks);
        let contents = display.is_some();
        Ok(Surface::tracked(TrackingLayer::new(&root), display, ticks, contents, None))
    }

    /// A layer in no window (tests).
    #[cfg(test)]
    pub fn detached() -> Surface {
        Surface::with(CAMetalLayer::new(), None, Rc::default(), false, None)
    }

    /// A tracking layer under `root`, a layer in no window (tests).
    #[cfg(test)]
    pub fn under(root: &CALayer) -> Surface {
        Surface::tracked(TrackingLayer::new(root), None, Rc::default(), false, None)
    }

    /// A tracking layer under a root of its own that shows its own surfaces, with ticks given by
    /// hand (`tick`) instead of a display link (tests).
    #[cfg(test)]
    pub fn contents_for_tests() -> Surface {
        let root = CALayer::new();
        Surface::tracked(TrackingLayer::new(&root), None, Rc::default(), true, Some(root))
    }

    /// A display tick, as the link's callback gives it (tests).
    #[cfg(test)]
    pub fn tick(&self) {
        self.ticks.ticked.set(true);
    }

    /// How many surfaces the layer has made (tests).
    #[cfg(test)]
    pub fn targets_made(&self) -> usize {
        self.targets.borrow().len()
    }

    /// Holds surface `i` as the window server would while it shows it, or lets it go (tests).
    #[cfg(test)]
    pub fn hold_in_server(&self, i: usize, held: bool) {
        let targets = self.targets.borrow();
        let s = targets[i].surface.0.as_ptr();
        // SAFETY: a live surface; each hold is let go once.
        unsafe {
            if held {
                IOSurfaceIncrementUseCount(s);
            } else {
                IOSurfaceDecrementUseCount(s);
            }
        }
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
    /// stops it for effects (None; it still runs for an owed frame).
    pub fn follow_display(&self, gap: Option<Option<Duration>>) {
        let Some(d) = &self.display else { return };
        if d.running.replace(gap) != gap {
            // The link ticks at the effect's own rate where the display allows it; 0 is the
            // display's rate, which is also what an owed frame alone waits for.
            let fps = gap.flatten().map_or(0.0, |g| (1.0 / g.as_secs_f64()).round() as f32);
            d.link.setPreferredFrameRateRange(CAFrameRateRange { minimum: fps, maximum: fps, preferred: fps });
            if let Some(gap) = gap.filter(|g| d.measured.replace(Some(*g)) != Some(*g)) {
                // A new rate: until the link's own callbacks say otherwise. The same rate again
                // after a pause keeps the interval measured for it.
                self.ticks.interval.set(gap.map_or(1.0 / 60.0, |g| g.as_secs_f64()));
                self.ticks.last.set(0.0);
            }
            self.ticks.ticked.set(false);
        }
        d.sync(self.owed.get());
    }

    /// The time between display link ticks while effect frames follow it; None when they
    /// follow the loop timer.
    pub fn display_period(&self) -> Option<Duration> {
        self.display.as_ref().filter(|d| d.running.get().is_some())?;
        Some(Duration::from_secs_f64(self.ticks.interval.get().clamp(1.0 / 480.0, 1.0 / 10.0)))
    }

    /// Whether the link fired since the last call, and whether that tick is the one an owed
    /// frame waits for (then it is no longer owed).
    pub fn take_display_tick(&self) -> crate::gpu_window::DisplayTick {
        let ticked = self.ticks.ticked.replace(false);
        let owed = ticked && self.owed.get();
        if owed {
            self.set_owed(false);
        }
        crate::gpu_window::DisplayTick { ticked, owed }
    }

    fn set_owed(&self, owed: bool) {
        self.owed.set(owed);
        if let Some(d) = &self.display {
            d.sync(owed);
        }
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
    /// terminal windows with the display link only (macOS 14 and later), since nothing else
    /// paces them.
    pub fn shows_contents(&self) -> bool {
        self.contents
    }

    /// A free surface of `size` to draw the next frame into, unless the window is covered; then
    /// `show` puts it on screen once the frame's command buffer completes. Timeout: none is free
    /// (drawn into and not done, shown now, or still used by the window server); the frame is
    /// then owed and the next display tick asks for it again. Err: a surface could not be made.
    pub fn next_contents(&self, device: &ProtocolObject<dyn MTLDevice>, size: (u32, u32)) -> Result<Next, String> {
        if self.occluded() {
            return Ok(Next::Occluded);
        }
        let size = (size.0.max(1), size.1.max(1));
        let mut targets = self.targets.borrow_mut();
        if self.targets_size.replace(size) != size {
            // A frame still on its way keeps its own surface alive until it is shown.
            targets.clear();
        }
        let start = self.next_target.get();
        let n = targets.len();
        let i = match (0..n).map(|k| (start + k) % n).find(|&i| targets[i].free(&self.presenter)) {
            Some(i) => i,
            None if n < TARGETS => {
                targets.push(Target::new(device, &self.presenter, size)?);
                n
            }
            None => {
                drop(targets);
                self.set_owed(true);
                return Ok(Next::Timeout);
            }
        };
        let serial = self.serial.get() + 1;
        self.serial.set(serial);
        targets[i].serial.store(serial, Ordering::Release);
        targets[i].pending.store(true, Ordering::Release);
        self.next_target.set(i + 1);
        Ok(Next::Contents(i, targets[i].texture.clone()))
    }

    /// Shows surface `i` once `cb` completes, unless a later frame was shown first.
    pub fn show(&self, i: usize, cb: &ProtocolObject<dyn MTLCommandBuffer>) {
        let targets = self.targets.borrow();
        let Some(t) = targets.get(i) else { return };
        // SAFETY: Metal copies the block (a retain: it is already on the heap) and calls it
        // once the command buffer is done.
        unsafe { cb.addCompletedHandler(block2::RcBlock::as_ptr(&t.show)) };
    }

    /// Gives surface `i` back unshown: its frame failed or never reached a command buffer. A
    /// command buffer that drew into it runs before any later one on the queue that draws into
    /// it again.
    pub fn give_back(&self, i: usize) {
        if let Some(t) = self.targets.borrow().get(i) {
            t.serial.store(0, Ordering::Release);
            t.pending.store(false, Ordering::Release);
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
        // The handlers of frames still on their way lose the layer here, on this thread, so its
        // last reference goes with `self.layer` below and never on Metal's completion queue.
        let layer = self.presenter.layer.lock().unwrap_or_else(|e| e.into_inner()).take();
        drop(layer);
        // The root's observers go here, on the owner's thread, whoever lets go of the layer last
        // (on the drawable path a presented drawable can, on Metal's thread).
        if let Some(t) = &self.tracking {
            t.stop_observing();
        }
        self.layer.removeFromSuperlayer();
        // A thread with no run loop (a test's) commits its implicit transaction here, so the
        // layers it holds are let go on this thread.
        if MainThreadMarker::new().is_none() {
            flush_transactions();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A surface is free only once a later frame replaced it on screen; a handler finishing
    /// between `free`'s two reads (the frame it just showed is this surface's) must not make it
    /// look free.
    #[test]
    fn a_handler_finishing_mid_check_never_frees_the_shown_surface() {
        let device = &super::super::test_device().device;
        let presenter = Presenter { layer: Mutex::new(None), shown: AtomicU64::new(2) };
        let t = Target::new(device, &Arc::new(Presenter { layer: Mutex::new(None), shown: AtomicU64::new(0) }), (8, 8)).unwrap();
        // Frame 5 is drawn into it and its command buffer not done.
        t.serial.store(5, Ordering::Release);
        t.pending.store(true, Ordering::Release);
        // The handler shows frame 5, as `Target::new`'s block does: shown, then pending.
        let complete = || {
            presenter.shown.store(5, Ordering::Release);
            t.pending.store(false, Ordering::Release);
        };
        let ran = Cell::new(false);
        assert!(!t.free_with(&presenter, || ran.set(true)), "pending: drawn into, not done");
        // With `pending` read first, a handler that finishes right after that read is the same
        // as one that finishes after the check.
        assert!(!ran.get(), "nothing runs after a pending read");
        complete();
        // And one that finishes between the reads of a surface whose pending flag was already
        // clear (an earlier frame of it) still reads its own newer shown serial.
        assert!(!t.free_with(&presenter, || presenter.shown.store(5, Ordering::Release)), "it is on screen");
        assert!(!t.free(&presenter), "and afterwards");
        presenter.shown.store(6, Ordering::Release);
        assert!(t.free(&presenter), "a later frame replaced it");
    }
}
