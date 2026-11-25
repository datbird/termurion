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
    /// How long the draw and the present took.
    pub draw: Duration,
    pub present: Duration,
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
    acquire: Acquire,
    window: Option<Arc<Window>>,
}

/// The format a surface draws in: the first sRGB one it offers, else its first.
fn pick_format(caps: &wgpu::SurfaceCapabilities) -> Option<wgpu::TextureFormat> {
    caps.formats.iter().copied().find(|f| f.is_srgb()).or_else(|| caps.formats.first().copied())
}

impl WgpuWindow {
    /// A window's surface on `on` (the handles it draws with), configured at the window's size, and
    /// a renderer on the same device. `primary` is what windows opened from it share. A window on the
    /// see-through instance (`alpha`) gets the non-sRGB view the fade pass needs.
    pub fn open(window: Arc<Window>, surface: wgpu::Surface<'static>, on: &GpuHandles, primary: &GpuHandles, alpha: bool, font: &str, px: f32) -> WindowGpu {
        let caps = surface.get_capabilities(&on.adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
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
        WindowGpu::Wgpu(WgpuWindow { surface: Some(surface), config, primary: primary.clone(), alpha: alpha.then(|| on.clone()), renderer, chain: None, acquire: Acquire::default(), window: Some(window) })
    }

    /// `open` with a new surface made on `on`'s instance.
    pub fn open_new(window: Arc<Window>, on: &GpuHandles, primary: &GpuHandles, alpha: bool, font: &str, px: f32) -> Result<WindowGpu, String> {
        let surface = on.instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        Ok(WgpuWindow::open(window, surface, on, primary, alpha, font, px))
    }

    /// A window with no surface, `w` by `h` in `format`, for capturing frames in tests.
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
        WgpuWindow { surface: None, config, primary: on.clone(), alpha: None, renderer, chain: None, acquire: Acquire::default(), window: None }
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
        let Some(surface) = self.surface.as_ref() else { return Err(RenderError::Skip) };
        let wait_start = Instant::now();
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
        let (w, h) = (self.config.width, self.config.height);
        let fx = fx.filter(|_| self.chain.is_some());
        let bar_draws = overlay.is_some();
        // An effect chain that writes the raw view never reads the sRGB one, unless the bar draws on it.
        let fx_raw = fx.is_some() && self.wants_raw_out();
        let view = (!fx_raw || bar_draws).then(|| texture.texture.create_view(&wgpu::TextureViewDescriptor::default()));
        // The non-sRGB view a fade needs on an sRGB target; no fade, no view.
        let raw = self.config.view_formats.first().filter(|_| frame.fade < 1.0).map(|f| texture.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(*f), ..Default::default() }));
        let draw_start = Instant::now();
        let kept = match (fx, self.chain.as_mut()) {
            (Some(fx), Some(chain)) => {
                // The passes write the window through its non-sRGB view (set_effect made sure it has one).
                // A frame shades only its region (`draw_fx_for`); a frame whose own region is
                // full tells the chain no next frame can, so it skips the kept output.
                chain.keep_output(fx.keep_output);
                // Without view formats (OpenGL) the chain's last copy writes the window's own view.
                let out = if chain.wants_raw_out() { texture.texture.create_view(&wgpu::TextureViewDescriptor { format: Some(render_fx_gpu::raw_format(self.config.format)), ..Default::default() }) } else { view.clone().expect("made unless the chain writes the raw view") };
                self.renderer.draw_fx_for(frame, chain, fx.uniforms, &out, w, h, fx.same_scene, fx.region)
            }
            _ => self.renderer.draw_with(frame, view.as_ref().expect("made without effects"), raw.as_ref(), w, h),
        };
        let draw = draw_start.elapsed();
        // The menu bar and any open menu go over the terminal, on the same frame.
        let bar = overlay.and_then(|paint| {
            let on = self.alpha.as_ref().unwrap_or(&self.primary);
            paint(&Overlay { device: &on.device, queue: &on.queue, target: view.as_ref().expect("made when the bar draws"), format: self.config.format, size: (w, h) })
        });
        let present_start = Instant::now();
        self.on().queue.present(texture);
        Ok(Drawn { bar, kept, draw, present: present_start.elapsed() })
    }

    fn capture(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, w: u32, h: u32) -> Result<Vec<u8>, String> {
        match (fx, self.chain.as_mut()) {
            (Some(fx), Some(chain)) => self.renderer.draw_offscreen_fx(frame, Some((chain, fx.uniforms)), w, h),
            _ => self.renderer.draw_offscreen(frame, w, h),
        }
    }

    fn set_effect(&mut self, compiled: Option<Vec<Compiled>>, terms: RegionTerms) -> Result<(), String> {
        self.chain = None;
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
        let keep_alpha = self.keeps_alpha();
        let device = self.on().device.clone();
        let mut c = Chain::new(&device, format, shaders, keep_alpha)?;
        c.set_region_terms(terms);
        self.chain = Some(if views { c } else { c.without_view_formats(&device) });
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
        }
    }

    /// Takes a new size. The surface's contents are undefined after it (the caller invalidates
    /// what it kept). `scale` is the window's backing scale.
    pub fn resize(&mut self, w: u32, h: u32, _scale: f64) {
        match self {
            WindowGpu::Wgpu(g) => {
                g.config.width = w;
                g.config.height = h;
                g.configure();
            }
        }
    }

    /// Acquires a drawable, draws `frame` (through the effects with `fx`), lets `overlay` draw over
    /// it, and presents.
    pub fn render(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, overlay: Option<OverlayDraw<'_>>) -> Result<Drawn, RenderError> {
        match self {
            WindowGpu::Wgpu(w) => w.render(frame, fx, overlay),
        }
    }

    /// Draws `frame` into an offscreen image of `w` by `h` and returns its RGBA pixels.
    pub fn capture(&mut self, frame: &Frame, fx: Option<FxFrame<'_>>, w: u32, h: u32) -> Result<Vec<u8>, String> {
        match self {
            WindowGpu::Wgpu(g) => g.capture(frame, fx, w, h),
        }
    }

    /// Builds the effect chain from `compiled` (None: no effect), or says why the card refused it.
    /// An empty list still readies the surface for the passes.
    pub fn set_effect(&mut self, compiled: Option<Vec<Compiled>>, terms: RegionTerms) -> Result<(), String> {
        match self {
            WindowGpu::Wgpu(w) => w.set_effect(compiled, terms),
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
        }
    }

    /// Moves the window onto a new GPU after a device loss: a new surface, the renderer's
    /// resources made again, no effect chain. `alpha` is the see-through instance's handles, for a
    /// window drawn on it. On an error the window is left as it was.
    pub fn rebuild(&mut self, shared: &GpuShared, alpha: Option<&GpuShared>) -> Result<(), String> {
        match self {
            WindowGpu::Wgpu(w) => w.rebuild(shared, alpha),
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
        }
    }

    /// The surface keeps the shaders' alpha (a see-through window).
    pub fn keeps_alpha(&self) -> bool {
        match self {
            WindowGpu::Wgpu(w) => w.keeps_alpha(),
        }
    }

    /// The window draws on the see-through instance.
    pub fn has_alpha(&self) -> bool {
        match self {
            WindowGpu::Wgpu(w) => w.alpha.is_some(),
        }
    }

    /// How the last drawable request went.
    pub fn last_acquire(&self) -> Acquire {
        match self {
            WindowGpu::Wgpu(w) => w.acquire,
        }
    }

    /// The handles every window opened from this one shares.
    pub fn shared(&self) -> GpuShared {
        match self {
            WindowGpu::Wgpu(w) => w.primary.shared(),
        }
    }

    /// The adapter the window's surface draws on.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        match self {
            WindowGpu::Wgpu(w) => w.on().adapter.get_info(),
        }
    }

    /// The largest texture side the shared device allows.
    pub fn max_texture_side(&self) -> u32 {
        match self {
            WindowGpu::Wgpu(w) => w.primary.device.limits().max_texture_dimension_2d,
        }
    }

    /// The device the window draws on, for the debug build's frame capture.
    #[cfg(debug_assertions)]
    pub fn wgpu_device(&self) -> Option<&wgpu::Device> {
        match self {
            WindowGpu::Wgpu(w) => Some(&w.on().device),
        }
    }

    pub fn renderer(&self) -> &Renderer {
        match self {
            WindowGpu::Wgpu(w) => &w.renderer,
        }
    }

    pub fn renderer_mut(&mut self) -> &mut Renderer {
        match self {
            WindowGpu::Wgpu(w) => &mut w.renderer,
        }
    }
}

#[cfg(test)]
mod tests {
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

    /// The window object's capture is the renderer's offscreen draw, with and without an effect,
    /// byte for byte.
    #[test]
    fn capture_matches_the_renderer() {
        let (instance, adapter, device, queue) = crate::render::test_gpu::shared_all();
        crate::render::test_gpu::serial_on_gl(&adapter);
        let on = GpuHandles { instance, adapter, device, queue };
        let (w, h) = (320, 120);
        let f = frame(w, h);
        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let mut win = WindowGpu::Wgpu(WgpuWindow::offscreen(&on, renderer(&on), format, (w, h)));
        let mut r = renderer(&on);
        assert_eq!(win.capture(&f, None, w, h).unwrap(), r.draw_offscreen(&f, w, h).unwrap());
        let plugin = crate::render_fx::discover(None).0.into_iter().next().expect("a shipped effect");
        let mut u = Uniforms::default();
        crate::render_fx::Clock::default().frame(&mut u, Instant::now(), (w, h), None, true);
        win.set_effect(Some(plugin.compile().unwrap()), None).unwrap();
        let fx = || FxFrame { uniforms: &u, region: None, same_scene: false, keep_output: false };
        let mut chain = Chain::new(&on.device, format, plugin.compile().unwrap(), false).unwrap();
        if !render_fx_gpu::view_formats_supported(&on.adapter) {
            chain = chain.without_view_formats(&on.device);
        }
        assert_eq!(win.capture(&f, Some(fx()), w, h).unwrap(), r.draw_offscreen_fx(&f, Some((&mut chain, &u)), w, h).unwrap());
        // No effect again: the chain is gone and the plain picture comes back.
        win.set_effect(None, None).unwrap();
        assert!(!win.skip_if_still(&u, true));
        assert_eq!(win.capture(&f, Some(fx()), w, h).unwrap(), r.draw_offscreen(&f, w, h).unwrap());
    }

    /// Every pixel case drawn through the window object equals the renderer's own draw (the path
    /// the app took before), byte for byte. The flood is cut short: both sides see the same
    /// sequence, so a shorter run proves the same.
    #[test]
    fn window_frames_match_the_renderer_on_the_pixel_cases() {
        let (instance, adapter, device, queue) = crate::render::test_gpu::shared_all();
        crate::render::test_gpu::serial_on_gl(&adapter);
        let on = GpuHandles { instance, adapter, device, queue };
        let face = crate::render_pixel_tests::FACE;
        let make = || {
            let mut r = Renderer::new(&on.device, &on.queue, wgpu::TextureFormat::Bgra8UnormSrgb, face, 14.0);
            r.set_view_formats(render_fx_gpu::view_formats_supported(&on.adapter));
            r
        };
        let mut r = make();
        let cases = crate::render_pixel_tests::pixel_cases(&r);
        let mut win = WindowGpu::Wgpu(WgpuWindow::offscreen(&on, make(), wgpu::TextureFormat::Bgra8UnormSrgb, (808, 486)));
        let mut n = 0;
        for c in cases.iter().filter(|c| c.name.strip_prefix("cjk_flood-").is_none_or(|i| i < "040")) {
            let (w, h) = c.size;
            let a = win.capture(&c.frame, None, w, h).unwrap();
            let b = r.draw_offscreen(&c.frame, w, h).unwrap();
            crate::render_pixel_tests::assert_same(&c.name, &a, &b, w);
            n += 1;
        }
        assert!(n > 40, "{n} cases");
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
