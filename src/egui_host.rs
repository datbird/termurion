//! One egui window: its surface, egui state, and renderer. The settings window and the
//! prompt window draw through it.

use std::sync::Arc;

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowId};

/// The egui windows that show a taskbar button (Settings, panels, chat), for `presence_tick`.
static TASKBAR: std::sync::Mutex<Vec<std::sync::Weak<Window>>> = std::sync::Mutex::new(Vec::new());

/// The tracked windows still open.
pub fn taskbar_windows() -> Vec<Arc<Window>> {
    let mut list = TASKBAR.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|w| w.strong_count() > 0);
    list.iter().filter_map(|w| w.upgrade()).collect()
}

/// The earliest delayed repaint each egui window asked for (`request_repaint_after`: a tooltip's
/// show delay, a debounce, a timed refresh). `about_to_wait` wakes at the earliest one and redraws
/// the windows whose time came. A window egui reports idle for has no entry, so the loop sleeps.
pub struct Deadlines<T> {
    by_window: std::collections::HashMap<u64, (T, std::time::Instant)>,
}

impl<T> Default for Deadlines<T> {
    fn default() -> Self {
        Deadlines { by_window: std::collections::HashMap::new() }
    }
}

impl<T> Deadlines<T> {
    /// Records what a frame asked for: zero (drawn at once elsewhere) and `Duration::MAX` (idle)
    /// keep nothing; any other delay keeps `now + delay`, replacing the window's older deadline.
    pub fn record(&mut self, id: u64, handle: T, now: std::time::Instant, delay: std::time::Duration) {
        match (delay.is_zero(), now.checked_add(delay)) {
            (false, Some(at)) => {
                self.by_window.insert(id, (handle, at));
            }
            _ => {
                self.by_window.remove(&id);
            }
        }
    }

    /// Forgets a window.
    pub fn clear(&mut self, id: u64) {
        self.by_window.remove(&id);
    }

    /// Removes and returns the handles whose deadline passed.
    pub fn take_due(&mut self, now: std::time::Instant) -> Vec<T> {
        let due: Vec<u64> = self.by_window.iter().filter(|(_, (_, at))| *at <= now).map(|(id, _)| *id).collect();
        due.into_iter().filter_map(|id| self.by_window.remove(&id)).map(|(h, _)| h).collect()
    }

    /// The earliest deadline kept, if any.
    pub fn next(&self) -> Option<std::time::Instant> {
        self.by_window.values().map(|(_, at)| *at).min()
    }
}

/// The sooner of two optional wake times (never replaces a sooner deadline).
pub fn sooner(a: Option<std::time::Instant>, b: Option<std::time::Instant>) -> Option<std::time::Instant> {
    [a, b].into_iter().flatten().min()
}

static DELAYED: std::sync::Mutex<Option<Deadlines<std::sync::Weak<Window>>>> = std::sync::Mutex::new(None);

fn with_delayed<R>(f: impl FnOnce(&mut Deadlines<std::sync::Weak<Window>>) -> R) -> R {
    let mut g = DELAYED.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Deadlines::default))
}

/// Redraws the egui windows whose delayed repaint came due and returns the next wake time
/// (`about_to_wait`). None when no window waits.
pub fn delayed_repaints(now: std::time::Instant) -> Option<std::time::Instant> {
    with_delayed(|d| {
        for w in d.take_due(now) {
            if let Some(w) = w.upgrade() {
                w.request_redraw();
            }
        }
        d.next()
    })
}

/// Gives `ctx` the fonts the menu bar and the key lists need: egui's own, then a face with the
/// check mark and the Mac modifier glyphs (see `fallback_faces`).
pub fn install_menu_fonts(ctx: &egui::Context) {
    let mut defs = egui::FontDefinitions::default();
    add_fallback_faces(&mut defs);
    ctx.set_fonts(defs);
}

/// Appends the symbol faces behind every family already set, so a missing glyph falls through.
pub fn add_fallback_faces(defs: &mut egui::FontDefinitions) {
    for (i, bytes) in fallback_faces().into_iter().enumerate() {
        let name = format!("termurion-symbols-{i}");
        defs.font_data.insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
        for list in defs.families.values_mut() {
            list.push(name.clone());
        }
    }
}

/// The faces behind egui's own (a box showed in the Settings key lists, the gateway buttons and the
/// password manager's gear menu where egui's fonts lack a glyph). A subset of DejaVu Sans,
/// licensed in resources/fonts.
fn fallback_faces() -> Vec<Vec<u8>> {
    vec![symbols_face()]
}

/// The bundled symbols face: the check mark, arrows, Mac modifier keys, gear and the other shapes
/// the windows draw, and stand-ins for the Segoe icon code points (E7xx and its kin) so a window
/// without Segoe draws a shape, not a box.
pub fn symbols_face() -> Vec<u8> {
    include_bytes!("../resources/fonts/TermurionSymbols.ttf").to_vec()
}

pub struct EguiHost {
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    pub window: Arc<Window>,
    /// Set to read the selected label text on the next frame (see `paint`).
    probe: bool,
    /// What the last probe found.
    pub probed: Option<String>,
    /// An alert or small dialog (`open_dialog`): its name, the height last fitted and the least
    /// height it keeps, in points.
    dialog: Option<(String, crate::dialog_frame::DialogFit)>,
    /// The window system sized the window since the last dialog frame (a fit retries only then).
    answered: bool,
}

impl EguiHost {
    #[allow(clippy::too_many_arguments)]
    pub fn open(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        title: &str,
        w: f64,
        h: f64,
    ) -> Result<EguiHost, String> {
        let attrs = crate::platform::window::attributes().with_title(title).with_inner_size(winit::dpi::LogicalSize::new(w, h));
        let host = Self::open_with(event_loop, instance, adapter, device, queue, attrs)?;
        host.track_taskbar();
        Ok(host)
    }

    /// An alert or small dialog: on Linux it draws its own frame (`dialog_frame`), elsewhere it
    /// is the window `open` makes. Draw it with `paint_dialog`.
    #[allow(clippy::too_many_arguments)]
    pub fn open_dialog(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        title: &str,
        w: f64,
        h: f64,
    ) -> Result<EguiHost, String> {
        let host = Self::open_with(event_loop, instance, adapter, device, queue, crate::dialog_frame::attributes(title, w, h))?;
        host.track_taskbar();
        Ok(host.as_dialog(title, h))
    }

    /// Marks a window made from `dialog_frame::attributes` as a dialog `paint_dialog` draws.
    pub fn as_dialog(mut self, title: &str, h: f64) -> EguiHost {
        self.dialog = Some((title.to_string(), crate::dialog_frame::DialogFit::new(h as f32)));
        // The first frame measures it (the window shows once it has its height).
        self.window.request_redraw();
        self
    }

    /// A dialog that keeps at least this height (points) for its whole life: a drop-down list is
    /// sized in the frame it opens, to the room under it, and never grows into a window that
    /// grows after, so the room has to be there before it opens.
    pub fn set_dialog_floor(&mut self, h: f64) {
        if let Some((_, fit)) = self.dialog.as_mut() {
            fit.floor = h as f32;
        }
    }

    /// Lists this window for the taskbar and Alt+Tab exclusion (a window with a taskbar button).
    pub fn track_taskbar(&self) {
        TASKBAR.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(&self.window));
    }

    /// A window made from `attrs` (a popover's: no frame, placed, no taskbar button).
    pub fn open_with(
        event_loop: &ActiveEventLoop,
        instance: &wgpu::Instance,
        adapter: &wgpu::Adapter,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        attrs: winit::window::WindowAttributes,
    ) -> Result<EguiHost, String> {
        let attrs = attrs.with_window_icon(crate::presence::window_icon());
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        let caps = surface.get_capabilities(adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            // Explicit: an egui window is never see-through (a visual swapchain lists Auto first).
            alpha_mode: crate::transparency::pick_alpha_mode(&caps.alpha_modes, false),
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(device, &config);
        let ctx = egui::Context::default();
        install_menu_fonts(&ctx);
        let state = egui_winit::State::new(ctx.clone(), egui::ViewportId::ROOT, &window, Some(window.scale_factor() as f32), None, Some(device.limits().max_texture_dimension_2d as usize));
        let renderer = egui_wgpu::Renderer::new(device, format, egui_wgpu::RendererOptions::default());
        Ok(EguiHost { surface, config, device: device.clone(), queue: queue.clone(), ctx, state, renderer, window, probe: false, probed: None, dialog: None, answered: false })
    }

    pub fn id(&self) -> WindowId {
        self.window.id()
    }

    pub fn focus(&self) {
        self.window.focus_window();
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// Follows a forced appearance (`tab_theme::forced_dark`), or Windows' when `None`: the
    /// window's theme, and the egui visuals to match. winit does not track a forced theme
    /// (`Window::theme` stays Windows' mode), so the mode is the forced one, else the window's.
    pub fn follow_theme(&self, forced: Option<bool>) {
        self.window.set_theme(crate::tab_theme::window_theme(forced));
        let dark = forced.unwrap_or_else(|| self.window.theme().is_none_or(|t| t == winit::window::Theme::Dark));
        self.ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
    }

    /// The window's native view handle for menus: the HWND on Windows, the NSView on macOS.
    pub fn view(&self) -> Option<isize> {
        crate::platform::window::view_of(&self.window)
    }

    /// The window's Win32 handle.
    pub fn hwnd(&self) -> Option<isize> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match self.window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }

    /// Feeds a window event to egui. Returns true when the window should redraw.
    pub fn event(&mut self, event: &WindowEvent) -> bool {
        if let WindowEvent::Resized(s) = event
            && s.width > 0 && s.height > 0 {
                self.config.width = s.width;
                self.config.height = s.height;
                self.surface.configure(&self.device, &self.config);
                self.answered = true;
            }
        let r = self.state.on_window_event(&self.window, event);
        if r.repaint {
            self.window.request_redraw();
        }
        r.repaint
    }

    /// Asks for the text selected in the labels: the next frame copies it into `probed`
    /// instead of the clipboard.
    pub fn probe_selection(&mut self) {
        self.probe = true;
        self.window.request_redraw();
    }

    /// Runs one egui frame with `f` and draws it.
    /// Draws a dialog from `open_dialog`: its panel and `body`, and on Linux its own title row,
    /// then follows the row's close and drag and fits the height to the content.
    pub fn paint_dialog(&mut self, mut body: impl FnMut(&mut egui::Ui)) {
        let own = crate::dialog_frame::OWN_FRAME && self.dialog.is_some();
        let title = self.dialog.as_ref().map(|(t, _)| t.clone()).unwrap_or_default();
        let mut framed = crate::dialog_frame::Framed::default();
        if !own {
            self.paint(|ui| framed = crate::dialog_frame::show(ui, &title, own, &mut body));
            return;
        }
        // The first frame is measured, not shown (see `DialogFit`).
        let present = self.dialog.as_ref().is_some_and(|(_, f)| f.shown());
        self.paint_with(|ui| framed = crate::dialog_frame::show(ui, &title, own, &mut body), present);
        if framed.drag {
            let _ = self.window.drag_window();
        }
        if framed.close {
            crate::dialog_frame::request_close(self.window.id());
        }
        let scale = self.window.scale_factor();
        let actual = (f64::from(self.window.inner_size().height) / scale) as f32;
        let answered = std::mem::take(&mut self.answered);
        let Some((_, fit)) = self.dialog.as_mut() else { return };
        let step = fit.step(framed.height, actual, answered);
        let w = f64::from(self.window.inner_size().width) / scale;
        for op in step.ops(fit.fit) {
            match op {
                // Measured: the size is fixed now, before anything shows. The window was made
                // resizable so its first commit carried no size of its own for the compositor to keep.
                crate::dialog_frame::WinOp::Fix => self.window.set_resizable(false),
                // A fixed-size window keeps its min and max at its first size on Wayland, so both
                // move with each fit (X11 takes the new size the same way).
                crate::dialog_frame::WinOp::Size(h) => {
                    let size = winit::dpi::LogicalSize::new(w, f64::from(h));
                    self.window.set_min_inner_size(Some(size));
                    self.window.set_max_inner_size(Some(size));
                    let _ = self.window.request_inner_size(size);
                }
                // Sized: now it may show (X11 maps it; Wayland maps it with its first drawn frame,
                // the next one).
                crate::dialog_frame::WinOp::Show => {
                    self.window.set_visible(true);
                    self.window.request_redraw();
                }
            }
        }
    }


    pub fn paint(&mut self, f: impl FnMut(&mut egui::Ui)) {
        self.paint_with(f, true);
    }

    /// A frame; with `present` false it is laid out (and its textures loaded) but not drawn.
    fn paint_with(&mut self, f: impl FnMut(&mut egui::Ui), present: bool) {
        // The surface is always the window's size: a size winit took without a Resized event
        // (a requested one) would otherwise leave egui drawing a smaller frame into it, and the
        // rest unpainted.
        let size = self.window.inner_size();
        if size.width > 0 && size.height > 0 && (size.width, size.height) != (self.config.width, self.config.height) {
            self.config.width = size.width;
            self.config.height = size.height;
            self.surface.configure(&self.device, &self.config);
        }
        let mut input = self.state.take_egui_input(&self.window);
        let mut probing = false;
        if self.probe {
            let real_copy = input.events.iter().any(|e| matches!(e, egui::Event::Copy | egui::Event::Cut));
            match probe_plan(real_copy, self.ctx.memory(|m| m.focused().is_some())) {
                ProbePlan::Run => {
                    self.probe = false;
                    probing = true;
                    input.events.push(egui::Event::Copy);
                }
                // The real copy goes through alone; the probe asks again on the next frame.
                ProbePlan::Wait => self.window.request_redraw(),
                ProbePlan::Skip => {
                    self.probe = false;
                    self.probed = Some(String::new());
                }
            }
        }
        let mut out = self.ctx.run_ui(input, f);
        if probing {
            // The copy is ours: taken out of the frame's output so the clipboard is untouched.
            // One source: the last answer (`CVC:3184-3193`), never several glued together.
            let mut found = String::new();
            out.platform_output.commands.retain(|c| match c {
                egui::OutputCommand::CopyText(t) => {
                    found = t.clone();
                    false
                }
                _ => true,
            });
            self.probed = Some(found);
        }
        // egui asks for the next frame itself (a popup's sizing pass, its fade-in, a pressed
        // button): without the redraw those frames only came with the next mouse move.
        if wants_redraw_now(&out) {
            self.window.request_redraw();
        }
        // A repaint asked for after a delay: the app wakes then (`delayed_repaints`).
        let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map_or(std::time::Duration::MAX, |v| v.repaint_delay);
        with_delayed(|d| d.record(u64::from(self.window.id()), Arc::downgrade(&self.window), std::time::Instant::now(), delay));
        self.state.handle_platform_output(&self.window, out.platform_output);
        let jobs = self.ctx.tessellate(out.shapes, out.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor { size_in_pixels: [self.config.width, self.config.height], pixels_per_point: out.pixels_per_point };
        for (id, deltas) in &out.textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(&self.device, &self.queue, *id, delta);
            }
        }
        if !present {
            for id in &out.textures_delta.free {
                self.renderer.free_texture(id);
            }
            out.textures_delta.clear();
            return;
        }
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) => f,
            _ => {
                // The frame is skipped; its texture changes were applied above and are done with.
                for id in &out.textures_delta.free {
                    self.renderer.free_texture(id);
                }
                out.textures_delta.clear();
                self.surface.configure(&self.device, &self.config);
                self.window.request_redraw();
                return;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("settings") });
        let extra = self.renderer.update_buffers(&self.device, &self.queue, &mut encoder, &jobs, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("settings"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &jobs, &screen);
        }
        self.queue.submit(extra.into_iter().chain(Some(encoder.finish())));
        self.queue.present(frame);
        for id in &out.textures_delta.free {
            self.renderer.free_texture(id);
        }
        // Applied: egui's debug build checks that no delta is dropped unhandled.
        out.textures_delta.clear();
    }
}

/// True when egui wants another frame at once (a popup's sizing pass, a fade, a pressed button).
pub fn wants_redraw_now(out: &egui::FullOutput) -> bool {
    out.viewport_output.get(&egui::ViewportId::ROOT).is_some_and(|v| v.repaint_delay.is_zero())
}

/// What a selection probe does with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbePlan {
    /// Ask the labels: inject a copy and take what they answer.
    Run,
    /// A real copy is in this frame: it goes through untouched and the probe waits a frame.
    Wait,
    /// A text field has the focus and would answer the copy with its own selection: no label
    /// selection can be meant, the probe answers nothing.
    Skip,
}

/// How a probe frame goes (`CVC:3184-3193` has one source, the first responder's selection).
pub fn probe_plan(real_copy: bool, text_field_focused: bool) -> ProbePlan {
    if real_copy {
        ProbePlan::Wait
    } else if text_field_focused {
        ProbePlan::Skip
    } else {
        ProbePlan::Run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `paint_dialog` draws the measuring frame without presenting it and shows the window only
    /// through `FitStep::ops` (Fix, Size, Show, tested in `dialog_frame`); `paint_with` leaves
    /// before the surface texture when it must not present.
    #[test]
    fn the_measuring_frame_is_not_presented_and_the_window_shows_after_its_size() {
        let src = include_str!("egui_host.rs").split("#[cfg(test)]").next().unwrap();
        let pd = &src[src.find("pub fn paint_dialog(").unwrap()..];
        let pd = &pd[..pd.find("\n    pub fn paint(").unwrap()];
        let at = |s: &str, n: &str| s.find(n).unwrap_or_else(|| panic!("{n} missing"));
        assert!(at(pd, "let present = self.dialog.as_ref().is_some_and(|(_, f)| f.shown());") < at(pd, "self.paint_with(|ui|"));
        assert!(pd.contains("&mut body), present);"), "the frame is presented only once shown");
        assert!(at(pd, "self.paint_with(") < at(pd, "fit.step(framed.height, actual, answered)"), "measured, then stepped");
        assert!(at(pd, "for op in step.ops(fit.fit)") < at(pd, "WinOp::Show =>"));
        assert_eq!(pd.matches("set_visible(true)").count(), 1, "only Show maps it");
        assert!(at(pd, "WinOp::Show =>") < at(pd, "set_visible(true)"));
        let pw = &src[src.find("fn paint_with(").unwrap()..];
        assert!(at(pw, "if !present {") < at(pw, "self.surface.get_current_texture()"), "no texture for the measuring frame");
        let skip = &pw[at(pw, "if !present {")..at(pw, "self.surface.get_current_texture()")];
        assert!(skip.contains("return;"));
    }

    /// Every character the menu bar, the shortcut text and the Keys page draw.
    fn shown_text() -> String {
        let mut all = String::from("\u{2713}\u{2303}\u{2325}\u{2318}\u{21e7}\u{2026}");
        for t in crate::app::menu_texts() {
            all.push_str(&t);
        }
        // The Keys page spells a chord from its key names.
        // The macOS function-key codes the Keys page names (arrows to Page Down, F1 to F35, Clear).
        for ch in (0x20u32..0x7f).chain(0xf700..0xf72a).chain(0xf72b..0xf72e).chain([0xf739]) {
            all.push_str(&crate::keystroke::Keystroke::new(ch, crate::keystroke::SHIFT | crate::keystroke::OPTION | crate::keystroke::CONTROL | crate::keystroke::COMMAND, None, ch).label());
        }
        all.push_str(&crate::keystroke::modifiers_label(crate::keystroke::SHIFT | crate::keystroke::OPTION | crate::keystroke::CONTROL | crate::keystroke::COMMAND));
        all
    }

    #[test]
    fn menu_and_key_text_glyphs_are_in_the_font() {
        let ctx = egui::Context::default();
        install_menu_fonts(&ctx);
        let mut missing = std::collections::BTreeSet::new();
        // Fonts exist after a first frame; the set takes effect on the next.
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0))), ..Default::default() }, |_| {});
            out.textures_delta.clear();
        }
        let text = shown_text();
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
            let id = egui::FontId::new(14.0, family);
            for c in text.chars().filter(|c| !c.is_control()) {
                // `has_glyph` calls the main face's own letters missing; a missing character has no width.
                if ctx.fonts_mut(|f| f.glyph_width(&id, c)) <= 0.0 {
                    missing.insert(c);
                }
            }
        }
        assert!(missing.is_empty(), "no glyph in the configured fonts for: {}", missing.iter().map(|c| format!("{c} U+{:04X}", *c as u32)).collect::<Vec<_>>().join(", "));
    }

    /// Item (e): every symbol a window draws, read from the source of the windows that use egui: an
    /// arrow, shape or dingbat (U+2190 to U+2BFF) or a private-use icon code point in a string or
    /// char literal outside the tests. Each has a glyph in the fonts a window gets, so no box
    /// shows. Emoji (they are color pictures) and plain letters are not in the scan.
    #[test]
    fn every_symbol_the_windows_draw_is_in_the_font() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let mut files = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut wanted: std::collections::BTreeMap<char, String> = Default::default();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            if !text.contains("egui::") {
                continue;
            }
            let code = text.split("#[cfg(test)]").next().unwrap_or("");
            let name = f.file_name().unwrap().to_string_lossy().to_string();
            let mut found = Vec::new();
            // Escapes ("\u{2699}"), then characters written out.
            let mut rest = code;
            while let Some(i) = rest.find("\\u{") {
                rest = &rest[i + 3..];
                if let Some(end) = rest.find('}') {
                    if let Some(c) = u32::from_str_radix(&rest[..end], 16).ok().and_then(char::from_u32) {
                        found.push(c);
                    }
                }
            }
            found.extend(code.chars());
            for c in found {
                let n = c as u32;
                if (0x2190..=0x2bff).contains(&n) || (0xe000..=0xf8ff).contains(&n) {
                    wanted.entry(c).or_insert_with(|| name.clone());
                }
            }
        }
        // Not drawn by egui: box drawing and block elements are the terminal's own glyphs, and the
        // dingbats the Windows-only wizard prints as emoji.
        wanted.retain(|c, _| !(0x2500..=0x259f).contains(&(*c as u32)) && !matches!(*c as u32, 0x23f3 | 0x2705 | 0x27a1));
        assert!(wanted.len() > 20, "the scan found only {} symbols", wanted.len());
        let ctx = egui::Context::default();
        install_menu_fonts(&ctx);
        for _ in 0..2 {
            let mut out = ctx.run_ui(egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(100.0, 100.0))), ..Default::default() }, |_| {});
            out.textures_delta.clear();
        }
        let mut missing = Vec::new();
        let id = egui::FontId::new(14.0, egui::FontFamily::Proportional);
        for (c, file) in &wanted {
            if ctx.fonts_mut(|f| f.glyph_width(&id, *c)) <= 0.0 {
                missing.push(format!("U+{:04X} in {file}", *c as u32));
            }
        }
        assert!(missing.is_empty(), "no glyph for: {}", missing.join(", "));
    }

    #[test]
    fn the_selection_probe_never_takes_a_real_copy_or_a_text_fields_selection() {
        assert_eq!(probe_plan(false, false), ProbePlan::Run);
        assert_eq!(probe_plan(true, false), ProbePlan::Wait, "a real Ctrl+C or a Copy click is never swallowed");
        assert_eq!(probe_plan(true, true), ProbePlan::Wait);
        assert_eq!(probe_plan(false, true), ProbePlan::Skip, "the focused field would answer with its own text");
    }

    /// One frame of a ComboBox the way the host runs it: a frame per input batch, then more only
    /// while `again` says the last frame asked for one. Returns (popup open, popup shapes, asked).
    fn frame(ctx: &egui::Context, events: Vec<egui::Event>, t: f64) -> (bool, usize, bool) {
        let mut input = egui::RawInput { time: Some(t), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0))), events, ..Default::default() };
        input.viewport_id = egui::ViewportId::ROOT;
        let mut v = 0;
        let mut out = ctx.run_ui(input, |ui| {
            egui::ComboBox::from_id_salt("theme").selected_text("Regular").show_ui(ui, |ui| {
                for n in ["Regular", "Light", "Dark", "Minimal", "Compact"] {
                    ui.selectable_value(&mut v, 0, n);
                }
            });
        });
        out.textures_delta.clear();
        let shapes = out.shapes.len();
        let asked = wants_redraw_now(&out);
        (egui::Popup::is_any_open(ctx), shapes, asked)
    }

    fn click(ctx: &egui::Context, t: &mut f64, honor: bool) -> Vec<(bool, usize, bool)> {
        let at = egui::pos2(40.0, 12.0);
        let ev = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let mut log = Vec::new();
        for events in [vec![egui::Event::PointerMoved(at)], vec![ev(true)], vec![ev(false)]] {
            *t += 0.016;
            let mut r = frame(ctx, events, *t);
            log.push(r);
            // A real host redraws at once when the last frame asked for it.
            let mut n = 0;
            while honor && r.2 && n < 30 {
                *t += 0.016;
                r = frame(ctx, vec![], *t);
                log.push(r);
                n += 1;
            }
        }
        log
    }

    #[test]
    fn a_combo_box_popup_finishes_opening_when_the_host_draws_the_frames_egui_asks_for() {
        let ctx = egui::Context::default();
        let mut t = 0.0;
        frame(&ctx, vec![], t);
        frame(&ctx, vec![], t + 0.016);
        let log = click(&ctx, &mut t, true);
        let opened = log.iter().position(|f| f.0).expect("the click opens the popup");
        assert!(log[opened].2, "the opening frame is a sizing pass: egui asks for the next frame at once");
        let last = log.last().unwrap();
        assert!(last.0 && last.1 > log[opened].1, "the popup is fully drawn once the asked frames ran: {log:?}");
        assert!(!last.2, "and the window goes idle after it");
    }

    /// The repaint delay of the first frame that does not ask for an immediate one (the first
    /// frames of a context do), the way the host runs frames while egui settles.
    fn settled_delay(ctx: &egui::Context, mut f: impl FnMut(&mut egui::Ui)) -> std::time::Duration {
        let mut delay = std::time::Duration::ZERO;
        for i in 0..20 {
            let mut input = egui::RawInput { time: Some(f64::from(i) * 0.016), screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0))), ..Default::default() };
            input.viewport_id = egui::ViewportId::ROOT;
            let mut out = ctx.run_ui(input, &mut f);
            out.textures_delta.clear();
            delay = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            if !delay.is_zero() {
                break;
            }
        }
        delay
    }

    #[test]
    fn a_frame_asking_for_a_repaint_after_a_delay_keeps_a_deadline_that_far_ahead() {
        let ctx = egui::Context::default();
        let delay = settled_delay(&ctx, |ui| ui.ctx().request_repaint_after(std::time::Duration::from_millis(100)));
        assert!(!delay.is_zero() && delay <= std::time::Duration::from_millis(100), "{delay:?}");
        let now = std::time::Instant::now();
        let mut d = Deadlines::<()>::default();
        d.record(1, (), now, delay);
        let ahead = d.next().unwrap() - now;
        assert!(ahead > std::time::Duration::from_millis(50) && ahead <= std::time::Duration::from_millis(100), "{ahead:?}");
        assert!(d.take_due(now).is_empty(), "not due yet");
        assert_eq!(d.take_due(now + std::time::Duration::from_millis(101)).len(), 1);
        assert!(d.next().is_none(), "a taken deadline is gone: the loop sleeps until a frame asks again");
    }

    #[test]
    fn the_earliest_deadline_wins_across_windows_and_existing_deadlines() {
        use std::time::Duration as D;
        let now = std::time::Instant::now();
        let mut d = Deadlines::<u8>::default();
        d.record(1, 1, now, D::from_millis(300));
        d.record(2, 2, now, D::from_millis(80));
        d.record(3, 3, now, D::from_millis(500));
        assert_eq!(d.next(), Some(now + D::from_millis(80)));
        d.record(2, 2, now, D::from_millis(400));
        assert_eq!(d.next(), Some(now + D::from_millis(300)), "a window's newer ask replaces its older one");
        let existing = Some(now + D::from_millis(50));
        assert_eq!(sooner(existing, d.next()), existing, "a sooner existing deadline is never replaced");
        assert_eq!(sooner(Some(now + D::from_millis(900)), d.next()), d.next());
        assert_eq!(sooner(None, d.next()), d.next());
        assert_eq!(sooner(None, None), None);
    }

    #[test]
    fn no_deadline_is_kept_when_egui_reports_idle_or_asks_for_a_frame_now() {
        use std::time::Duration as D;
        let now = std::time::Instant::now();
        let mut d = Deadlines::<()>::default();
        d.record(1, (), now, D::from_millis(100));
        d.record(1, (), now, D::MAX);
        assert!(d.next().is_none(), "idle clears the window's deadline");
        d.record(2, (), now, D::from_millis(100));
        d.record(2, (), now, D::ZERO);
        assert!(d.next().is_none(), "an immediate ask is redrawn at once, nothing to wait for");
        d.record(3, (), now, D::MAX);
        assert!(d.next().is_none());
        let ctx = egui::Context::default();
        assert_eq!(settled_delay(&ctx, |_| {}), D::MAX, "a quiet frame is idle");
    }

    #[test]
    fn paint_keeps_the_delayed_repaint_and_the_app_wakes_for_it() {
        let source = include_str!("egui_host.rs").split("#[cfg(test)]").next().unwrap().to_string();
        let paint = source.split("pub fn paint(").nth(1).unwrap();
        assert!(paint.contains("d.record("), "a host that drops request_repaint_after loses tooltips and refresh timers");
        let app = include_str!("app/mod.rs");
        let wait = &app[app.find("fn about_to_wait").unwrap()..];
        assert!(wait.contains("egui_host::delayed_repaints("), "about_to_wait must redraw the due windows and wake for the next");
    }

    #[test]
    fn paint_redraws_when_egui_asks_for_a_frame() {
        let source = include_str!("egui_host.rs").split("#[cfg(test)]").next().unwrap().to_string();
        let paint = source.split("pub fn paint(").nth(1).unwrap();
        assert!(paint.contains("wants_redraw_now(&out)"), "a host that drops egui's repaint requests shows popups half drawn");
    }
}
