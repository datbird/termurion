//! One terminal window on Metal: its layer, its renderer's CPU half and the Metal draw, with the
//! same frame contract as the wgpu window (`gpu_window::WgpuWindow`).

use std::sync::Arc;
use std::time::Instant;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{MTLCommandBuffer, MTLDevice, MTLDrawable, MTLStorageMode, MTLTexture, MTLTextureDescriptor, MTLTextureUsage};
use winit::window::Window;

use super::chain::MetalChain;
use super::draw::{raw_format, MetalDraw};
use super::text::TextStart;
use super::surface::{Next, Surface, FORMAT};
use super::{MetalShared, MAX_TEXTURE_SIDE};
use crate::gpu_shared::GpuShared;
use crate::gpu_window::{Acquire, Drawn, FxFrame, OverlayDraw, RenderError};
use crate::render::Renderer;
use crate::render_fx::{Compiled, FrameRegion, RegionTerms, Uniforms};
use crate::render_fx_gpu::Shade;
use crate::scene::Frame;

type Texture = Retained<ProtocolObject<dyn MTLTexture>>;

pub struct MetalWindow {
    shared: MetalShared,
    /// None for an offscreen window (tests): it captures but never presents.
    surface: Option<Surface>,
    size: (u32, u32),
    scale: f64,
    renderer: Renderer,
    draw: MetalDraw,
    /// The render plugin chain (`set_effect`); None when no effect runs.
    chain: Option<MetalChain>,
    /// The shaders and terms `chain` was built from, for `capture`'s own chain.
    effect: Option<(Vec<Compiled>, RegionTerms)>,
    /// `capture`'s chain, made on first use, so a capture leaves the live chain's state alone.
    capture_chain: Option<MetalChain>,
    /// The device generation this window's command buffers report a loss for.
    generation: u64,
    /// Set when a Metal failure left the GPU side behind the CPU side: both start over first.
    stale: Option<String>,
    acquire: Acquire,
    timing: bool,
    /// Keeps the view the layer sits in alive as long as the window draws.
    _window: Option<Arc<Window>>,
    /// A test window draws into a texture of its own in place of a drawable.
    #[cfg(test)]
    offscreen: Option<Texture>,
}

/// A `w` by `h` render target in the window format; `views` lets it be viewed as non-sRGB too.
fn target(device: &ProtocolObject<dyn MTLDevice>, (w, h): (u32, u32), views: bool) -> Result<Texture, String> {
    // SAFETY: a plain 2D descriptor; callers keep the size inside the limit.
    let desc = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(FORMAT, w as usize, h as usize, false) };
    let usage = MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead;
    desc.setUsage(if views { usage | MTLTextureUsage::PixelFormatView } else { usage });
    desc.setStorageMode(MTLStorageMode::Private);
    super::new_texture(device, &desc).ok_or_else(|| format!("no Metal texture of {w} x {h}"))
}

/// Why a frame did not draw: the CPU prepare (nothing reached the GPU side; the next frame takes
/// it up) or the Metal side (the two are apart until `MetalWindow::in_step`).
enum Failed {
    Prepare(String),
    Metal(String),
}

/// Restarts `renderer`'s atlas, rows, spare lists and buffer shadows, and makes a draw whose
/// textures and buffers start at their sizes.
fn fresh_draw(device: &ProtocolObject<dyn MTLDevice>, renderer: &mut Renderer) -> Result<MetalDraw, String> {
    renderer.restart_for_backend(MAX_TEXTURE_SIDE);
    let (atlas, rows) = renderer.glyph_state();
    MetalDraw::new(device, FORMAT, &TextStart::of(atlas, rows))
}

impl MetalWindow {
    /// A window's layer on `shared`, configured at the window's size and scale, and a renderer.
    pub fn open(window: Arc<Window>, shared: &MetalShared, font: &str, px: f32) -> Result<crate::gpu_window::WindowGpu, String> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let view = match window.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::AppKit(h) => h.ns_view,
            _ => return Err("not an AppKit window".to_string()),
        };
        let surface = Surface::attach(view)?;
        let size = window.inner_size();
        let w = MetalWindow::new(shared, Some(surface), (size.width.max(1), size.height.max(1)), window.scale_factor(), Renderer::cpu_only(font, px), Some(window))?;
        w.configure();
        Ok(crate::gpu_window::WindowGpu::Metal(w))
    }

    fn new(shared: &MetalShared, surface: Option<Surface>, size: (u32, u32), scale: f64, mut renderer: Renderer, window: Option<Arc<Window>>) -> Result<MetalWindow, String> {
        let draw = fresh_draw(&shared.device, &mut renderer)?;
        Ok(MetalWindow {
            shared: shared.clone(),
            surface,
            size,
            scale,
            renderer,
            draw,
            chain: None,
            effect: None,
            capture_chain: None,
            generation: crate::gpu_pick::current_generation(),
            stale: None,
            acquire: Acquire::default(),
            timing: false,
            _window: window,
            #[cfg(test)]
            offscreen: None,
        })
    }

    /// A window with no layer, `w` by `h`, for tests: `render` draws into a texture of its own.
    #[cfg(test)]
    pub fn offscreen(shared: &MetalShared, renderer: Renderer, size: (u32, u32)) -> MetalWindow {
        let mut w = MetalWindow::new(shared, None, size, 1.0, renderer, None).expect("the Metal pipelines");
        w.offscreen = Some(target(&shared.device, size, true).expect("a texture"));
        w
    }

    /// A window over a layer in no window (tests): the layer is configured as a window's is.
    #[cfg(test)]
    pub fn on_detached_layer(shared: &MetalShared, renderer: Renderer, size: (u32, u32), scale: f64) -> MetalWindow {
        let w = MetalWindow::new(shared, Some(Surface::detached()), size, scale, renderer, None).expect("the Metal pipelines");
        w.configure();
        w
    }

    /// The layer (tests).
    #[cfg(test)]
    pub fn layer(&self) -> &objc2_quartz_core::CAMetalLayer {
        &self.surface.as_ref().expect("a layer").layer
    }

    /// The test window's texture, as the last `render` left it.
    #[cfg(test)]
    pub fn offscreen_texture(&self) -> &ProtocolObject<dyn MTLTexture> {
        self.offscreen.as_ref().expect("an offscreen window")
    }

    /// The screen size the last frame's quads and images were drawn with (tests).
    #[cfg(test)]
    pub fn last_uniform(&self) -> [f32; 4] {
        self.draw.last_uniform
    }

    /// Staging chunks the window's draw made so far (tests).
    #[cfg(test)]
    pub fn staging_made(&self) -> u64 {
        self.draw.staging_made()
    }

    /// Text buffers and atlas textures the window's draw made so far (tests).
    #[cfg(test)]
    pub fn text_objects_made(&self) -> u64 {
        self.draw.text.made
    }

    /// The device generation the window reports a loss for (tests).
    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn configure(&self) {
        if let Some(s) = &self.surface {
            s.configure(&self.shared.device, self.size, self.scale);
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn resize(&mut self, w: u32, h: u32, scale: f64) {
        self.size = (w, h);
        self.scale = scale;
        self.configure();
    }

    pub fn render(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, _overlay: Option<OverlayDraw<'_>>) -> Result<Drawn, RenderError> {
        // Each frame's command buffer, encoders and drawable go with the frame, not with AppKit's
        // event pool (or a test thread's, which never drains).
        objc2::rc::autoreleasepool(|_| self.render_frame(frame, fx))
    }

    fn render_frame(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>) -> Result<Drawn, RenderError> {
        self.in_step().map_err(RenderError::Refused)?;
        // An effect frame with no chain draws as a plain one, as on a wgpu window; the in-window
        // menu bar is Linux only.
        let fx = fx.filter(|_| self.chain.is_some());
        let wait_start = Instant::now();
        let Some(surface) = self.surface.as_ref() else {
            #[cfg(test)]
            if let Some(old) = self.offscreen.clone() {
                // Made again when the size changed.
                let t = if (old.width() as u32, old.height() as u32) == self.size { old } else { target(&self.shared.device, self.size, true).map_err(|e| self.metal_failed(e))? };
                self.offscreen = Some(t.clone());
                self.acquire = Acquire { wait: wait_start.elapsed(), dropped: false };
                return self.draw_into(None, &t, frame, fx);
            }
            self.acquire = Acquire { wait: wait_start.elapsed(), dropped: true };
            return Err(RenderError::Skip);
        };
        let next = surface.next();
        self.acquire = Acquire { wait: wait_start.elapsed(), dropped: !matches!(next, Next::Drawable(..)) };
        match next {
            Next::Drawable(drawable, texture) => self.draw_into(Some(drawable), &texture, frame, fx),
            Next::Occluded | Next::Timeout => Err(RenderError::Skip),
        }
    }

    /// Draws `frame` into `texture` and presents `drawable` with it.
    fn draw_into(&mut self, drawable: Option<Retained<ProtocolObject<dyn objc2_quartz_core::CAMetalDrawable>>>, texture: &ProtocolObject<dyn MTLTexture>, frame: &Frame, fx: Option<FxFrame<'_>>) -> Result<Drawn, RenderError> {
        let cb = self.shared.command_buffer(self.generation).map_err(RenderError::Refused)?;
        let draw_start = self.timing.then(Instant::now);
        let kept = match fx {
            Some(fx) => self.encode_fx(&cb, frame, fx, texture),
            // A drawable has no non-sRGB view: the fade runs on its own view, as on a wgpu surface
            // without view formats.
            None => self.encode(&cb, frame, texture, None),
        };
        let kept = match kept {
            Ok(()) => Ok(()),
            Err(Failed::Prepare(e)) => Err(e),
            Err(Failed::Metal(_)) => {
                cb.commit();
                // Reported as a lost device: the app's recovery rebuilds the window, with its
                // tries and backoff, and shows the restart banner once they run out.
                return Err(RenderError::Skip);
            }
        };
        let draw = draw_start.map(|t| t.elapsed());
        let present_start = self.timing.then(Instant::now);
        // A frame that failed before it drew (no buffer, a refused prepare) is not shown: the
        // window keeps its last picture. It is still committed, so its staging chunk comes back.
        if let Some(d) = drawable.as_ref().filter(|_| kept.is_ok()) {
            cb.presentDrawable(ProtocolObject::<dyn MTLDrawable>::from_ref(&**d));
        }
        cb.commit();
        #[cfg(test)]
        if drawable.is_none() {
            cb.waitUntilCompleted();
        }
        Ok(Drawn { bar: None, kept, draw, present: present_start.map(|t| t.elapsed()) })
    }

    /// The CPU half prepares the frame and the Metal draw encodes it. A Metal failure leaves the
    /// GPU side without changes the CPU side counts as sent (a grown atlas, a row slot), so the
    /// window is marked to start both over (`in_step`) before it draws again.
    fn encode(&mut self, cb: &ProtocolObject<dyn MTLCommandBuffer>, frame: &Frame, view: &ProtocolObject<dyn MTLTexture>, raw: Option<&ProtocolObject<dyn MTLTexture>>) -> Result<(), Failed> {
        let (w, h) = (view.width() as u32, view.height() as u32);
        let (list, up) = self.renderer.prepare(frame, w, h).map_err(Failed::Prepare)?;
        let drawn = self.draw.run(&self.shared.device, cb, &list, &up, view, raw);
        self.renderer.recycle(list, up);
        drawn.map_err(|e| {
            self.metal_failed(e);
            Failed::Metal(self.stale.clone().unwrap_or_default())
        })
    }

    /// `Renderer::draw_fx_for` on Metal: the frame into the chain's frame texture unless the kept
    /// picture is this frame's, then the passes into `out`. A Metal failure in the chain is a
    /// Metal failure of the frame.
    fn encode_fx(&mut self, cb: &ProtocolObject<dyn MTLCommandBuffer>, frame: &Frame, fx: FxFrame<'_>, out: &ProtocolObject<dyn MTLTexture>) -> Result<(), Failed> {
        let mut chain = self.chain.take().expect("only called with a chain");
        let drawn = self.encode_chain(cb, &mut chain, frame, fx.uniforms, fx.same_scene, fx.region, Some(fx.keep_output), out);
        self.chain = Some(chain);
        drawn
    }

    /// Runs `chain` over `frame` into `out`. `keep` is the app's word on whether a next frame may
    /// shade only a region (None: a screenshot, which never tells the chain). A screenshot's own
    /// chain failing is only an error: its textures are the screenshot's, and a scene it drew is
    /// committed with the text changes.
    #[allow(clippy::too_many_arguments)]
    fn encode_chain(&mut self, cb: &ProtocolObject<dyn MTLCommandBuffer>, chain: &mut MetalChain, frame: &Frame, uniforms: &Uniforms, same_scene: bool, region: Option<&FrameRegion>, keep: Option<bool>, out: &ProtocolObject<dyn MTLTexture>) -> Result<(), Failed> {
        if let Some(k) = keep {
            chain.keep_output(k);
        }
        let size = (out.width() as u32, out.height() as u32);
        let live = keep.is_some();
        let failed = |w: &mut MetalWindow, chain: &mut MetalChain, e: String| {
            chain.forget();
            if !live {
                return Failed::Prepare(e);
            }
            w.metal_failed(e);
            Failed::Metal(w.stale.clone().unwrap_or_default())
        };
        let (view, raw) = match chain.frame_views(&self.shared.device, size) {
            Ok(v) => v,
            Err(e) => return Err(failed(self, chain, e)),
        };
        let animation_only = same_scene && chain.frame_kept();
        if !animation_only {
            // Not the window's picture until the draw succeeds.
            chain.set_frame_drawn(false);
            self.encode(cb, frame, &view, Some(&raw))?;
            chain.set_frame_drawn(true);
        }
        let rects = region.and_then(|r| if animation_only { r.with_last.as_deref().map(Shade::Kept) } else { r.own.as_deref().map(Shade::Fresh) });
        match chain.run(&self.shared.device, cb, &mut self.draw, uniforms, out, rects.unwrap_or(Shade::Full)) {
            Ok(()) => Ok(()),
            Err(e) => Err(failed(self, chain, e)),
        }
    }

    /// A Metal allocation failed: the window starts its text over before it draws again, and the
    /// failure goes to the app as a lost device (`gpu_pick::report_lost`), so recovery remakes
    /// the device a limited number of times and then shows the restart banner. The frame itself
    /// is skipped, so nothing redraws in a loop meanwhile.
    fn metal_failed(&mut self, why: String) -> RenderError {
        let why = format!("Metal: {why}");
        crate::gpu_pick::report_lost(self.generation, why.clone());
        self.stale = Some(why);
        RenderError::Skip
    }

    /// Starts the text over on both sides when a Metal failure left them apart: the CPU atlas,
    /// rows, spare lists and buffer shadows, and a new draw sized from them.
    fn in_step(&mut self) -> Result<(), String> {
        if self.stale.is_none() {
            return Ok(());
        }
        // The chains' pictures and uniform buffers may be a failed frame's.
        for c in self.chain.iter_mut().chain(self.capture_chain.iter_mut()) {
            c.forget();
        }
        self.draw = fresh_draw(&self.shared.device, &mut self.renderer)?;
        self.stale = None;
        Ok(())
    }

    /// Why the window must start over before its next frame, if it must (tests).
    #[cfg(test)]
    pub fn stale(&self) -> Option<&str> {
        self.stale.as_deref()
    }

    /// Starts the atlas over in `mode`, on both sides (tests).
    #[cfg(test)]
    pub fn set_color_mode(&mut self, mode: glyphon::ColorMode) {
        self.renderer.set_color_mode(mode);
        self.draw = fresh_draw(&self.shared.device, &mut self.renderer).expect("the Metal draw");
    }

    /// Draws `frame` into an offscreen texture of `w` by `h` and reads it back as RGBA.
    pub fn capture(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, w: u32, h: u32) -> Result<Vec<u8>, String> {
        let fx = fx.filter(|_| self.chain.is_some()).map(|f| f.uniforms);
        objc2::rc::autoreleasepool(|_| self.capture_frame(frame, fx, w, h))
    }

    fn capture_frame(&mut self, frame: &Frame, fx: Option<&Uniforms>, w: u32, h: u32) -> Result<Vec<u8>, String> {
        self.in_step()?;
        let max = MAX_TEXTURE_SIDE;
        if w == 0 || h == 0 || w > max || h > max {
            return Err(format!("an image of {w} x {h} pixels is larger than the graphics card allows ({max})"));
        }
        // With a second view, as the wgpu backend's screenshot texture has where the card allows.
        // Nothing has changed yet: a screenshot too big for the memory left is only an error, not
        // a lost device.
        let tex = target(&self.shared.device, (w, h), true)?;
        let cb = self.shared.command_buffer(self.generation)?;
        let drawn = match fx {
            Some(u) => self.capture_fx(&cb, frame, u, &tex),
            None => {
                let raw = tex.newTextureViewWithPixelFormat(raw_format(FORMAT)).ok_or("no Metal texture view")?;
                self.encode(&cb, frame, &tex, Some(&raw))
            }
        };
        cb.commit();
        drawn.map_err(|(Failed::Prepare(e) | Failed::Metal(e))| e)?;
        super::readback::read_rgba(&self.shared.queue, &tex, w, h)
    }

    /// The screenshot through the effects, on its own chain (`Renderer::draw_offscreen_fx`).
    fn capture_fx(&mut self, cb: &ProtocolObject<dyn MTLCommandBuffer>, frame: &Frame, u: &Uniforms, tex: &ProtocolObject<dyn MTLTexture>) -> Result<(), Failed> {
        let mut chain = match self.capture_chain.take() {
            Some(c) => c,
            None => {
                let (shaders, terms) = self.effect.clone().expect("kept with the chain");
                self.make_chain(shaders, terms).map_err(Failed::Prepare)?
            }
        };
        chain.unstage();
        let drawn = self.encode_chain(cb, &mut chain, frame, u, false, None, None, tex);
        // The screenshot's picture now fills the frame texture, not the window's.
        chain.set_frame_drawn(false);
        self.capture_chain = Some(chain);
        drawn
    }

    /// A chain as the window runs it: opaque, on the window's device.
    fn make_chain(&self, shaders: Vec<Compiled>, terms: RegionTerms) -> Result<MetalChain, String> {
        let mut c = MetalChain::new(&self.shared.device, FORMAT, shaders, false)?;
        c.set_region_terms(terms);
        Ok(c)
    }

    /// Builds the effect chain from `compiled` (None: no effect), or says why Metal refused it.
    pub fn set_effect(&mut self, compiled: Option<Vec<Compiled>>, terms: RegionTerms) -> Result<(), String> {
        self.chain = None;
        self.effect = None;
        self.capture_chain = None;
        let Some(shaders) = compiled.filter(|s| !s.is_empty()) else { return Ok(()) };
        self.chain = Some(self.make_chain(shaders.clone(), terms.clone())?);
        self.effect = Some((shaders, terms));
        Ok(())
    }

    /// `Chain::skip_if_still` on the window's chain; false with none.
    pub fn skip_if_still(&mut self, u: &Uniforms, eligible: bool) -> bool {
        let size = self.size;
        self.chain.as_mut().is_some_and(|c| c.skip_if_still(u, size, eligible))
    }

    /// The window's chain (tests).
    #[cfg(test)]
    pub(crate) fn chain(&self) -> Option<&MetalChain> {
        self.chain.as_ref()
    }

    /// Moves the window onto a new Metal device after a loss: new pipelines, buffers and atlas
    /// textures, the layer set to the new device, the glyph atlas, rows and spare lists started
    /// over with them. On an error the window starts over again at its next frame.
    pub fn rebuild(&mut self, shared: &GpuShared, _alpha: Option<&GpuShared>) -> Result<(), String> {
        let GpuShared::Metal(m) = shared else { return Err("a Metal window needs a Metal device".to_string()) };
        self.shared = m.clone();
        self.generation = crate::gpu_pick::current_generation();
        // As on wgpu: the effects are built again by the app (`fx_sync`) for the new device.
        self.chain = None;
        self.effect = None;
        self.capture_chain = None;
        self.configure();
        self.stale = Some("a new Metal device".to_string());
        self.in_step()
    }

    /// The layer outlives a lost drawable: it is only set up again, and text a Metal failure left
    /// apart starts over.
    pub fn remake_surface(&mut self) -> Result<(), String> {
        self.configure();
        self.in_step()
    }

    pub fn set_timing(&mut self, on: bool) {
        self.timing = on;
    }

    pub fn last_acquire(&self) -> Acquire {
        self.acquire
    }

    pub fn shared(&self) -> GpuShared {
        GpuShared::Metal(self.shared.clone())
    }

    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.shared.adapter_info()
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }

    pub fn renderer_mut(&mut self) -> &mut Renderer {
        &mut self.renderer
    }
}
