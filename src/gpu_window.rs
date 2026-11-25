//! Each terminal window's GPU side as one object: its surface, its renderer, its effect chain and
//! the frame loop's acquire, draw and present. The app asks it for frames and never touches a
//! surface or a texture, so a second backend adds one arm here.

use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::window::Window;

use crate::gpu_shared::GpuShared;
use crate::render::Renderer;
use crate::render_fx::{Compiled, FrameRegion, RegionTerms, Uniforms};
use crate::render_fx_gpu::{self, Chain};
use crate::scene::Frame;

/// One wgpu instance with its adapter, device and queue.
#[derive(Clone)]
pub struct GpuHandles {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl GpuHandles {
    pub fn shared(&self) -> GpuShared {
        GpuShared::Wgpu { instance: self.instance.clone(), adapter: self.adapter.clone(), device: self.device.clone(), queue: self.queue.clone() }
    }

    /// The wgpu handles of a shared GPU; an error for a GPU wgpu does not drive.
    pub fn of(shared: &GpuShared) -> Result<GpuHandles, String> {
        match shared {
            GpuShared::Wgpu { instance, adapter, device, queue } => Ok(GpuHandles { instance: instance.clone(), adapter: adapter.clone(), device: device.clone(), queue: queue.clone() }),
            #[cfg(target_os = "macos")]
            GpuShared::Metal(_) => Err("the Metal device has no wgpu handles".to_string()),
        }
    }
}

/// What a frame's effects need: the packed uniforms and where they may draw.
pub struct FxFrame<'a> {
    pub uniforms: &'a Uniforms,
    pub region: Option<&'a FrameRegion>,
    /// The frame is the one drawn last time: with the picture kept, only the passes run.
    pub same_scene: bool,
    /// The chain may keep its output for a next frame that shades only a region.
    pub keep_output: bool,
}

/// Why a frame was not drawn.
#[derive(Debug, PartialEq)]
pub enum RenderError {
    /// No drawable this time (a timeout, an occluded window): the frame is left out.
    Skip,
    /// The surface was out of date and is configured again: the next frame draws.
    Outdated,
    /// The surface was lost (the text says so); `remake_surface` makes a new one.
    Lost(String),
    /// The GPU refused the frame (the text is the banner's).
    Refused(String),
}

/// A frame that was drawn and presented.
#[derive(Debug)]
pub struct Drawn {
    /// The menu bar item the overlay chose.
    pub bar: Option<String>,
    /// Ok when the picture drew and the screen now shows it (it may be kept), else why not.
    pub kept: Result<(), String>,
    /// How long the draw and the present took, with draw timing on (`set_timing`).
    pub draw: Option<Duration>,
    pub present: Option<Duration>,
}

/// How the last drawable request went, for Copy Performance Stats.
#[derive(Clone, Copy, Debug, Default)]
pub struct Acquire {
    pub wait: Duration,
    /// The drawable did not come (timeout, occluded).
    pub dropped: bool,
}

/// What the in-window menu bar draws on, over the frame and before the present (Linux).
pub struct Overlay<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub target: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
}

/// Draws over a frame and answers the menu item chosen.
pub type OverlayDraw<'a> = &'a mut dyn FnMut(&Overlay<'_>) -> Option<String>;

/// A terminal window's GPU backend.
pub enum WindowGpu {
    Wgpu(WgpuWindow),
    /// The native Metal backend (macOS, `TERMURION_RENDERER=metal` for now).
    #[cfg(target_os = "macos")]
    Metal(crate::metal_backend::MetalWindow),
}

/// The wgpu backend of one window.
pub struct WgpuWindow {
    /// None for an offscreen window (tests): it captures but never presents.
    surface: Option<wgpu::Surface<'static>>,
    config: wgpu::SurfaceConfiguration,
    /// The primary handles (the first window's): every egui window, prompt and new opaque window
    /// takes them from here, whatever kind this window's own surface is.
    primary: GpuHandles,
    /// Set on a window that needs alpha: the handles its surface and renderer were made on.
    alpha: Option<GpuHandles>,
    renderer: Renderer,
    /// The render plugin chain (`set_effect`); None when no effect runs.
    chain: Option<Chain>,
    /// The shaders and terms `chain` was built from, for `capture`'s own chain.
    effect: Option<(Vec<Compiled>, RegionTerms)>,
    /// `capture`'s chain, made on first use, so a capture leaves the live chain's state alone.
    capture_chain: Option<Chain>,
    acquire: Acquire,
    /// Draw and present times are measured (Copy Performance Stats' draw timing).
    timing: bool,
    window: Option<Arc<Window>>,
    /// A test window draws into a texture of its own in place of a surface's.
    #[cfg(test)]
    offscreen: Option<wgpu::Texture>,
}

/// What a frame draws into.
enum Target {
    Surface(wgpu::SurfaceTexture),
    #[cfg(test)]
    Offscreen(wgpu::Texture),
}

impl Target {
    fn texture(&self) -> &wgpu::Texture {
        match self {
            Target::Surface(t) => &t.texture,
            #[cfg(test)]
            Target::Offscreen(t) => t,
        }
    }
}

/// The format a surface draws in: the first sRGB one it offers, else its first.
fn pick_format(caps: &wgpu::SurfaceCapabilities) -> Option<wgpu::TextureFormat> {
    caps.formats.iter().copied().find(|f| f.is_srgb()).or_else(|| caps.formats.first().copied())
}

impl WgpuWindow {
    /// A window's surface on `on` (the handles it draws with), configured at the window's size, and
    /// a renderer on the same device. `primary` is what windows opened from it share. A window on the
    /// see-through instance (`alpha`) gets the non-sRGB view the fade pass needs.
    pub fn open(window: Arc<Window>, surface: wgpu::Surface<'static>, on: &GpuHandles, primary: &GpuHandles, alpha: bool, font: &str, px: f32) -> Result<WindowGpu, String> {
        let caps = surface.get_capabilities(&on.adapter);
        let format = pick_format(&caps).ok_or("the surface offers no format")?;
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: crate::transparency::pick_alpha_mode(&caps.alpha_modes, false),
            // The fade pass scales the stored values on a non-sRGB view of the target (render.rs).
            view_formats: if alpha && format.is_srgb() && render_fx_gpu::view_formats_supported(&on.adapter) { vec![format.remove_srgb_suffix()] } else { vec![] },
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&on.device, &config);
        let mut renderer = Renderer::new(&on.device, &on.queue, format, font, px);
        renderer.set_view_formats(render_fx_gpu::view_formats_supported(&on.adapter));
        Ok(WindowGpu::Wgpu(WgpuWindow::new(Some(surface), config, primary.clone(), alpha.then(|| on.clone()), renderer, Some(window))))
    }

    fn new(surface: Option<wgpu::Surface<'static>>, config: wgpu::SurfaceConfiguration, primary: GpuHandles, alpha: Option<GpuHandles>, renderer: Renderer, window: Option<Arc<Window>>) -> WgpuWindow {
        WgpuWindow {
            surface,
            config,
            primary,
            alpha,
            renderer,
            chain: None,
            effect: None,
            capture_chain: None,
            acquire: Acquire::default(),
            timing: false,
            window,
            #[cfg(test)]
            offscreen: None,
        }
    }

    /// `open` with a new surface made on `on`'s instance.
    pub fn open_new(window: Arc<Window>, on: &GpuHandles, primary: &GpuHandles, alpha: bool, font: &str, px: f32) -> Result<WindowGpu, String> {
        let surface = on.instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        WgpuWindow::open(window, surface, on, primary, alpha, font, px)
    }

    /// A window with no surface, `w` by `h` in `format`, for tests: `render` draws into a
    /// texture of its own (`offscreen_texture`).
    #[cfg(test)]
    pub fn offscreen(on: &GpuHandles, renderer: Renderer, format: wgpu::TextureFormat, (w, h): (u32, u32)) -> WgpuWindow {
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w,
            height: h,
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        let mut w = WgpuWindow::new(None, config, on.clone(), None, renderer, None);
        w.offscreen = Some(w.offscreen_target());
        w
    }

    /// A texture as the surface would hand out: its format, size and view formats.
    #[cfg(test)]
    fn offscreen_target(&self) -> wgpu::Texture {
        self.on().device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen window"),
            size: wgpu::Extent3d { width: self.config.width, height: self.config.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &self.config.view_formats,
        })
    }

    /// The test window's texture, as the last `render` left it.
    #[cfg(test)]
    pub fn offscreen_texture(&self) -> &wgpu::Texture {
        self.offscreen.as_ref().expect("an offscreen window")
    }

    fn on(&self) -> &GpuHandles {
        self.alpha.as_ref().unwrap_or(&self.primary)
    }

    fn configure(&self) {
        if let Some(s) = &self.surface {
            s.configure(&self.on().device, &self.config);
        }
    }

    fn wants_raw_out(&self) -> bool {
        self.chain.as_ref().is_some_and(|c| c.wants_raw_out())
    }

    fn render(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, overlay: Option<OverlayDraw<'_>>) -> Result<Drawn, RenderError> {
        let wait_start = Instant::now();
        let Some(surface) = self.surface.as_ref() else {
            #[cfg(test)]
            if self.offscreen.is_some() {
                // Made again each frame: the size or the view formats may have changed.
                let t = self.offscreen_target();
                self.offscreen = Some(t.clone());
                self.acquire = Acquire { wait: wait_start.elapsed(), dropped: false };
                return self.draw_into(Target::Offscreen(t), frame, fx, overlay);
            }
            self.acquire = Acquire { wait: wait_start.elapsed(), dropped: true };
            return Err(RenderError::Skip);
        };
        let acquired = surface.get_current_texture();
        let dropped = matches!(acquired, wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded);
        self.acquire = Acquire { wait: wait_start.elapsed(), dropped };
        let texture = match acquired {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return Err(RenderError::Skip),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.configure();
                return Err(RenderError::Outdated);
            }
            wgpu::CurrentSurfaceTexture::Lost => return Err(RenderError::Lost("Lost the window surface".into())),
            wgpu::CurrentSurfaceTexture::Validation => return Err(RenderError::Refused("GPU validation error".into())),
        };
        self.draw_into(Target::Surface(texture), frame, fx, overlay)
    }

    /// Draws `frame` into a drawable, lets `overlay` draw over it, and presents it.
    fn draw_into(&mut self, target: Target, frame: &Frame, fx: Option<FxFrame<'_>>, overlay: Option<OverlayDraw<'_>>) -> Result<Drawn, RenderError> {
        let texture = target.texture();
        let (w, h) = (self.config.width, self.config.height);
        let fx = fx.filter(|_| self.chain.is_some());
        let bar_draws = overlay.is_some();
        // An effect chain that writes the raw view never reads the sRGB one, unless the bar draws on it.
        let fx_raw = fx.is_some() && self.wants_raw_out();
        let view = (!fx_raw || bar_draws).then(|| texture.create_view(&wgpu::TextureViewDescriptor::default()));
        // The non-sRGB view a fade needs on an sRGB target; no fade, no view.
        let raw = self.config.view_formats.first().filter(|_| frame.fade < 1.0).map(|f| texture.create_view(&wgpu::TextureViewDescriptor { format: Some(*f), ..Default::default() }));
        let draw_start = self.timing.then(Instant::now);
        let kept = match (fx, self.chain.as_mut()) {
            (Some(fx), Some(chain)) => {
                // The passes write the window through its non-sRGB view (set_effect made sure it has one).
                // A frame shades only its region (`draw_fx_for`); a frame whose own region is
                // full tells the chain no next frame can, so it skips the kept output.
                chain.keep_output(fx.keep_output);
                // Without view formats (OpenGL) the chain's last copy writes the window's own view.
                let out = if chain.wants_raw_out() { texture.create_view(&wgpu::TextureViewDescriptor { format: Some(render_fx_gpu::raw_format(self.config.format)), ..Default::default() }) } else { view.clone().expect("made unless the chain writes the raw view") };
                self.renderer.draw_fx_for(frame, chain, fx.uniforms, &out, w, h, fx.same_scene, fx.region)
            }
            _ => self.renderer.draw_with(frame, view.as_ref().expect("made without effects"), raw.as_ref(), w, h),
        };
        let draw = draw_start.map(|t| t.elapsed());
        // The menu bar and any open menu go over the terminal, on the same frame.
        let bar = overlay.and_then(|paint| {
            let on = self.on();
            paint(&Overlay { device: &on.device, queue: &on.queue, target: view.as_ref().expect("made when the bar draws"), format: self.config.format, size: (w, h) })
        });
        let present_start = self.timing.then(Instant::now);
        match target {
            Target::Surface(t) => self.on().queue.present(t),
            #[cfg(test)]
            Target::Offscreen(_) => {}
        }
        Ok(Drawn { bar, kept, draw, present: present_start.map(|t| t.elapsed()) })
    }

    fn capture(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, w: u32, h: u32) -> Result<Vec<u8>, String> {
        let Some(fx) = fx.filter(|_| self.chain.is_some()) else { return self.renderer.draw_offscreen(frame, w, h) };
        // Its own chain: the live one keeps its kept picture and still-check state.
        if self.capture_chain.is_none() {
            let (shaders, terms) = self.effect.clone().expect("kept with the chain");
            self.capture_chain = Some(self.make_chain(shaders, terms)?);
        }
        let chain = self.capture_chain.as_mut().expect("made above");
        self.renderer.draw_offscreen_fx(frame, Some((chain, fx.uniforms)), w, h)
    }

    /// A chain as the window runs it: on its device, through copy passes where views are missing.
    fn make_chain(&self, shaders: Vec<Compiled>, terms: RegionTerms) -> Result<Chain, String> {
        let device = &self.on().device;
        let mut c = Chain::new(device, self.config.format, shaders, self.keeps_alpha())?;
        c.set_region_terms(terms);
        Ok(if render_fx_gpu::view_formats_supported(&self.on().adapter) { c } else { c.without_view_formats(device) })
    }

    fn set_effect(&mut self, compiled: Option<Vec<Compiled>>, terms: RegionTerms) -> Result<(), String> {
        self.chain = None;
        self.effect = None;
        self.capture_chain = None;
        let Some(shaders) = compiled else { return Ok(()) };
        let format = self.config.format;
        // The passes write the window through its non-sRGB view, so the surface must offer one.
        // A card with no second view (OpenGL) gets copy passes instead (`without_view_formats`).
        let views = render_fx_gpu::view_formats_supported(&self.on().adapter);
        let raw = render_fx_gpu::raw_format(format);
        if views && raw != format && !self.config.view_formats.contains(&raw) {
            self.config.view_formats.push(raw);
            self.configure();
        }
        if shaders.is_empty() {
            return Ok(());
        }
        self.chain = Some(self.make_chain(shaders.clone(), terms.clone())?);
        self.effect = Some((shaders, terms));
        Ok(())
    }

    fn keeps_alpha(&self) -> bool {
        self.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied
    }

    fn rebuild(&mut self, shared: &GpuShared, alpha: Option<&GpuShared>) -> Result<(), String> {
        let primary = GpuHandles::of(shared)?;
        let target = match (self.alpha.is_some(), alpha) {
            (true, Some(a)) => Some(GpuHandles::of(a)?),
            (true, None) => return Err("no see-through instance".to_string()),
            (false, _) => None,
        };
        let on = target.as_ref().unwrap_or(&primary);
        let window = self.window.clone().ok_or("no window")?;
        // Made before the old surface goes: a failure leaves the window as it was.
        let surface = on.instance.create_surface(window).map_err(|e| format!("no surface: {e}"))?;
        let caps = surface.get_capabilities(&on.adapter);
        let Some(format) = pick_format(&caps) else { return Err("the surface offers no format".to_string()) };
        // One swap chain per window: the old one goes before the new one is configured.
        drop(self.surface.take());
        let old = &self.config;
        let alpha_mode = if caps.alpha_modes.contains(&old.alpha_mode) { old.alpha_mode } else { crate::transparency::pick_alpha_mode(&caps.alpha_modes, false) };
        let view_formats = if target.is_some() && format.is_srgb() && render_fx_gpu::view_formats_supported(&on.adapter) { vec![format.remove_srgb_suffix()] } else { vec![] };
        self.config = wgpu::SurfaceConfiguration { format, alpha_mode, view_formats, ..self.config.clone() };
        surface.configure(&on.device, &self.config);
        // The atlas, rows and spare lists start over with the backend (`Renderer::rebuild_gpu`).
        self.renderer.rebuild_gpu(&on.device, &on.queue, format);
        self.renderer.set_view_formats(render_fx_gpu::view_formats_supported(&on.adapter));
        self.surface = Some(surface);
        self.chain = None;
        self.effect = None;
        self.capture_chain = None;
        self.alpha = target;
        self.primary = primary;
        Ok(())
    }
}

impl WindowGpu {
    /// The surface's size in pixels.
    pub fn size(&self) -> (u32, u32) {
        match self {
            WindowGpu::Wgpu(w) => (w.config.width, w.config.height),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.size(),
        }
    }

    /// Takes a new size. The surface's contents are undefined after it (the caller invalidates
    /// what it kept). `scale` is the window's backing scale.
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    pub fn resize(&mut self, w: u32, h: u32, scale: f64) {
        match self {
            WindowGpu::Wgpu(g) => {
                g.config.width = w;
                g.config.height = h;
                g.configure();
            }
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(g) => g.resize(w, h, scale),
        }
    }

    /// Acquires a drawable, draws `frame` (through the effects with `fx`), lets `overlay` draw over
    /// it, and presents.
    pub fn render(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, overlay: Option<OverlayDraw<'_>>) -> Result<Drawn, RenderError> {
        match self {
            WindowGpu::Wgpu(w) => w.render(frame, fx, overlay),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.render(frame, fx, overlay),
        }
    }

    /// Draws `frame` into an offscreen image of `w` by `h` and returns its RGBA pixels.
    pub fn capture(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, w: u32, h: u32) -> Result<Vec<u8>, String> {
        match self {
            WindowGpu::Wgpu(g) => g.capture(frame, fx, w, h),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(g) => g.capture(frame, fx, w, h),
        }
    }

    /// Builds the effect chain from `compiled` (None: no effect), or says why the card refused it.
    /// An empty list still readies the surface for the passes.
    pub fn set_effect(&mut self, compiled: Option<Vec<Compiled>>, terms: RegionTerms) -> Result<(), String> {
        match self {
            WindowGpu::Wgpu(w) => w.set_effect(compiled, terms),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.set_effect(compiled, terms),
        }
    }

    /// Whether an effect frame with `u` would show what the window already shows
    /// (`Chain::skip_if_still`). False with no chain.
    pub fn skip_if_still(&mut self, u: &Uniforms, eligible: bool) -> bool {
        match self {
            WindowGpu::Wgpu(w) => {
                let size = (w.config.width, w.config.height);
                w.chain.as_mut().is_some_and(|c| c.skip_if_still(u, size, eligible))
            }
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.skip_if_still(u, eligible),
        }
    }

    /// Moves the window onto a new GPU after a device loss: a new surface, the renderer's
    /// resources made again, no effect chain. `alpha` is the see-through instance's handles, for a
    /// window drawn on it. On an error the window is left as it was.
    pub fn rebuild(&mut self, shared: &GpuShared, alpha: Option<&GpuShared>) -> Result<(), String> {
        match self {
            WindowGpu::Wgpu(w) => w.rebuild(shared, alpha),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.rebuild(shared, alpha),
        }
    }

    /// Makes the surface again after it was lost, then configures it (the old one when that fails).
    pub fn remake_surface(&mut self) -> Result<(), String> {
        match self {
            WindowGpu::Wgpu(w) => {
                let made = match &w.window {
                    Some(window) => w.on().instance.create_surface(window.clone()).map_err(|e| e.to_string()),
                    None => Err("no window".to_string()),
                };
                let made = made.map(|s| w.surface = Some(s));
                w.configure();
                made
            }
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.remake_surface(),
        }
    }

    /// Makes the surface see-through or opaque. True when it changed (the caller invalidates
    /// what it kept).
    pub fn set_transparent(&mut self, transparent: bool) -> bool {
        match self {
            WindowGpu::Wgpu(w) => {
                if w.keeps_alpha() == transparent {
                    return false;
                }
                let Some(surface) = &w.surface else { return false };
                let caps = surface.get_capabilities(&w.on().adapter);
                let mode = crate::transparency::pick_alpha_mode(&caps.alpha_modes, transparent);
                if mode == w.config.alpha_mode {
                    return false;
                }
                w.config.alpha_mode = mode;
                w.configure();
                true
            }
            // Metal windows are opaque (see-through windows are Windows only).
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(_) => false,
        }
    }

    /// The surface keeps the shaders' alpha (a see-through window).
    pub fn keeps_alpha(&self) -> bool {
        match self {
            WindowGpu::Wgpu(w) => w.keeps_alpha(),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(_) => false,
        }
    }

    /// The window draws on the see-through instance.
    pub fn has_alpha(&self) -> bool {
        match self {
            WindowGpu::Wgpu(w) => w.alpha.is_some(),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(_) => false,
        }
    }

    /// Measures the draw and the present of the frames to come (`Drawn::draw`, `Drawn::present`).
    pub fn set_timing(&mut self, on: bool) {
        match self {
            WindowGpu::Wgpu(w) => w.timing = on,
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.set_timing(on),
        }
    }

    /// How the last drawable request went.
    pub fn last_acquire(&self) -> Acquire {
        match self {
            WindowGpu::Wgpu(w) => w.acquire,
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.last_acquire(),
        }
    }

    /// The handles every window opened from this one shares.
    pub fn shared(&self) -> GpuShared {
        match self {
            WindowGpu::Wgpu(w) => w.primary.shared(),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.shared(),
        }
    }

    /// The adapter the window's surface draws on.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        match self {
            WindowGpu::Wgpu(w) => w.on().adapter.get_info(),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.adapter_info(),
        }
    }

    /// The largest texture side the shared device allows.
    pub fn max_texture_side(&self) -> u32 {
        match self {
            WindowGpu::Wgpu(w) => w.primary.device.limits().max_texture_dimension_2d,
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(_) => crate::metal_backend::MAX_TEXTURE_SIDE,
        }
    }

    /// The device the window draws on, for the debug build's frame capture.
    #[cfg(debug_assertions)]
    pub fn wgpu_device(&self) -> Option<&wgpu::Device> {
        match self {
            WindowGpu::Wgpu(w) => Some(&w.on().device),
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(_) => None,
        }
    }

    pub fn renderer(&self) -> &Renderer {
        match self {
            WindowGpu::Wgpu(w) => &w.renderer,
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.renderer(),
        }
    }

    pub fn renderer_mut(&mut self) -> &mut Renderer {
        match self {
            WindowGpu::Wgpu(w) => &mut w.renderer,
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(w) => w.renderer_mut(),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A frame with some text and quads.
    fn frame(w: u32, h: u32) -> Frame {
        let mut s = crate::screen::Screen::new(40, 6);
        s.feed("\x1b[1;31mred\x1b[0m plain \x1b[44m blue \x1b[0m\r\n$ ls -la".as_bytes());
        let mut f = Frame { clear: [0.1, 0.1, 0.12, 1.0], fade: 1.0, ..Default::default() };
        let rect = crate::layout::Rect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let draw = crate::scene::DrawPrefs::default();
        crate::scene::pane_with(&mut f, &s, rect, (9.0, 18.0), true, &crate::cursor_look::CursorLook::shown(true), &draw, None, 0, None);
        f
    }

    fn renderer(on: &GpuHandles) -> Renderer {
        let mut r = Renderer::new(&on.device, &on.queue, wgpu::TextureFormat::Bgra8UnormSrgb, "monospace", 16.0);
        r.set_view_formats(render_fx_gpu::view_formats_supported(&on.adapter));
        r
    }

    /// A texture's pixels as RGBA, row by row (BGRA targets swapped), as `draw_offscreen` reads them.
    pub(crate) fn read_back(on: &GpuHandles, tex: &wgpu::Texture) -> Vec<u8> {
        let (w, h) = (tex.width(), tex.height());
        let row = w * 4;
        let padded = row.div_ceil(256) * 256;
        let buf = on.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (padded * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = on.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        on.queue.submit(Some(enc.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
        on.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buf.slice(..).get_mapped_range().unwrap().to_vec();
        let bgra = matches!(tex.format(), wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((row * h) as usize);
        for y in 0..h as usize {
            for p in data[y * padded as usize..y * padded as usize + row as usize].as_chunks::<4>().0 {
                out.extend_from_slice(&if bgra { [p[2], p[1], p[0], p[3]] } else { *p });
            }
        }
        out
    }

    pub(crate) fn handles() -> GpuHandles {
        let (instance, adapter, device, queue) = crate::render::test_gpu::shared_all();
        crate::render::test_gpu::serial_on_gl(&adapter);
        GpuHandles { instance, adapter, device, queue }
    }

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8UnormSrgb;

    /// The wgpu window inside (these tests make only wgpu ones).
    fn wgpu_of(win: &WindowGpu) -> &WgpuWindow {
        match win {
            WindowGpu::Wgpu(w) => w,
            #[cfg(target_os = "macos")]
            WindowGpu::Metal(_) => panic!("a wgpu window"),
        }
    }

    /// Draws through `render` (views, the effect's out view, the fade's raw view) and reads it back.
    fn rendered(on: &GpuHandles, win: &mut WindowGpu, f: &Frame, fx: Option<FxFrame<'_>>) -> Vec<u8> {
        let drawn = win.render(f, fx, None).expect("an offscreen window always draws");
        drawn.kept.unwrap();
        read_back(on, wgpu_of(win).offscreen_texture())
    }

    fn uniforms_at(ms: u64, size: (u32, u32)) -> Uniforms {
        let mut u = Uniforms::default();
        let t0 = Instant::now();
        let mut clock = crate::render_fx::Clock::default();
        clock.frame(&mut u, t0, size, None, true);
        clock.frame(&mut u, t0 + Duration::from_millis(ms), size, None, true);
        u
    }

    /// Frames drawn through the window's `render` equal the renderer's own offscreen draw (the
    /// old app path drew with the same calls): plain, fading through the raw view, and
    /// through an effect, a fresh frame and then an animation-only one that reuses the kept picture.
    #[test]
    fn render_matches_the_renderer() {
        let on = handles();
        let (w, h) = (320, 120);
        let f = frame(w, h);
        let fading = Frame { fade: 0.6, ..frame(w, h) };
        let mut win = WindowGpu::Wgpu(WgpuWindow::offscreen(&on, renderer(&on), FORMAT, (w, h)));
        let mut r = renderer(&on);
        crate::render_pixel_tests::assert_same("plain", &rendered(&on, &mut win, &f, None), &r.draw_offscreen(&f, w, h).unwrap(), w);
        // An effect adds the raw view where the card has one; the fade then draws through it.
        let plugin = crate::render_fx::discover(None).0.into_iter().next().expect("a shipped effect");
        win.set_effect(Some(plugin.compile().unwrap()), None).unwrap();
        crate::render_pixel_tests::assert_same("fade with views", &rendered(&on, &mut win, &fading, None), &r.draw_offscreen(&fading, w, h).unwrap(), w);
        let mut chain = Chain::new(&on.device, FORMAT, plugin.compile().unwrap(), false).unwrap();
        if !render_fx_gpu::view_formats_supported(&on.adapter) {
            chain = chain.without_view_formats(&on.device);
        }
        for (name, ms, same_scene) in [("fx fresh", 0, false), ("fx animation only", 400, true)] {
            let u = uniforms_at(ms, (w, h));
            let fx = FxFrame { uniforms: &u, region: None, same_scene, keep_output: false };
            crate::render_pixel_tests::assert_same(name, &rendered(&on, &mut win, &f, Some(fx)), &r.draw_offscreen_fx(&f, Some((&mut chain, &u)), w, h).unwrap(), w);
        }
        let inner = wgpu_of(&win);
        assert!(inner.chain.as_ref().unwrap().frame_kept(), "the animation-only frame reused the kept picture");
    }

    /// Every baseline and agent pixel case drawn through `render` equals the renderer's own draw,
    /// each at its own size (the window resized to it).
    #[test]
    fn render_matches_the_renderer_on_the_pixel_cases() {
        let on = handles();
        let make = || {
            let mut r = Renderer::new(&on.device, &on.queue, FORMAT, crate::render_pixel_tests::FACE, 14.0);
            r.set_view_formats(render_fx_gpu::view_formats_supported(&on.adapter));
            r
        };
        let mut r = make();
        let cases = crate::render_pixel_tests::pixel_cases(&r);
        let mut win = WindowGpu::Wgpu(WgpuWindow::offscreen(&on, make(), FORMAT, (808, 486)));
        let mut n = 0;
        for c in cases.iter().filter(|c| !c.name.starts_with("cjk_flood-")) {
            let (w, h) = c.size;
            win.resize(w, h, 1.0);
            crate::render_pixel_tests::assert_same(&c.name, &rendered(&on, &mut win, &c.frame, None), &r.draw_offscreen(&c.frame, w, h).unwrap(), w);
            n += 1;
        }
        assert!(n > 10, "{n} cases");
    }

    /// A capture through the effect leaves the live chain as it was: the next frame with the same
    /// uniforms is still left out, as in a window that never captured, and the pictures agree.
    #[test]
    fn a_capture_leaves_the_live_chain_alone() {
        let on = handles();
        let (w, h) = (320, 120);
        let f = frame(w, h);
        let plugin = crate::render_fx::discover(None).0.into_iter().next().expect("a shipped effect");
        let mut a = WindowGpu::Wgpu(WgpuWindow::offscreen(&on, renderer(&on), FORMAT, (w, h)));
        let mut b = WindowGpu::Wgpu(WgpuWindow::offscreen(&on, renderer(&on), FORMAT, (w, h)));
        let u = uniforms_at(0, (w, h));
        let fx = || FxFrame { uniforms: &u, region: None, same_scene: false, keep_output: false };
        for win in [&mut a, &mut b] {
            win.set_effect(Some(plugin.compile().unwrap()), None).unwrap();
            rendered(&on, win, &f, Some(fx()));
        }
        let shot = a.capture(&frame(200, 80), Some(fx()), 200, 80).unwrap();
        assert_eq!(shot.len(), 200 * 80 * 4);
        assert!(b.skip_if_still(&u, true), "an unchanged effect frame is left out");
        assert!(a.skip_if_still(&u, true), "the capture did not touch the live chain");
        let later = uniforms_at(300, (w, h));
        let next = |u| FxFrame { uniforms: u, region: None, same_scene: true, keep_output: false };
        assert_eq!(rendered(&on, &mut a, &f, Some(next(&later))), rendered(&on, &mut b, &f, Some(next(&later))));
        let kept = |win: &WindowGpu| wgpu_of(win).chain.as_ref().unwrap().kept_made();
        assert_eq!(kept(&a), kept(&b));
    }

    /// A window with no drawable reports the request like any other.
    #[test]
    fn a_window_without_a_surface_counts_a_dropped_frame() {
        let on = handles();
        let mut w = WgpuWindow::offscreen(&on, renderer(&on), FORMAT, (40, 20));
        w.offscreen = None;
        let mut win = WindowGpu::Wgpu(w);
        assert_eq!(win.render(&frame(40, 20), None, None).unwrap_err(), RenderError::Skip);
        assert!(win.last_acquire().dropped);
    }

    /// The app asks the window object for frames; it never touches a surface or a texture itself.
    #[test]
    fn app_names_no_surface() {
        let src = include_str!("app/mod.rs");
        for banned in ["get_current_texture", "surface.configure", "create_view"] {
            assert!(!src.contains(banned), "src/app/mod.rs still calls {banned}");
        }
    }
}
