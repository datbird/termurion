//! See-through windows: which panes draw below full opacity, the surface alpha mode, the second wgpu
//! instance that serves only windows that need alpha, and recreating a window when it starts needing one.

use super::windows_ui::{NewWindow, init_gpu_shared, key};
use super::*;
use crate::transparency::{self, SurfaceKind};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::platform::windows::WindowAttributesExtWindows;

/// The text of the one banner a machine without Direct3D 12 shows.
pub(super) const UNAVAILABLE: &str = "Transparency needs Direct3D 12; this machine draws every window opaque.";

/// A window made for alpha: its surface, device and queue come from the second instance (`alpha`); the
/// primary handles ride along for everything that opens from this window (egui windows, new opaque windows).
fn init_gpu_alpha(window: Arc<Window>, primary: &Gpu, alpha: &AlphaGpu) -> Result<Gpu, String> {
    let surface = alpha.instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
    let caps = surface.get_capabilities(&alpha.adapter);
    let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
    let size = window.inner_size();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: transparency::pick_alpha_mode(&caps.alpha_modes, false),
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&alpha.device, &config);
    let renderer = Renderer::new(&alpha.device, &alpha.queue, format, FONT, FONT_PX * window.scale_factor() as f32);
    Ok(Gpu {
        surface,
        adapter: primary.adapter.clone(),
        device: primary.device.clone(),
        queue: primary.queue.clone(),
        config,
        renderer,
        instance: primary.instance.clone(),
        alpha: Some(alpha.clone()),
        window,
    })
}

/// The profiles a window opens with, as (Transparency, Blur), for `transparency::wants_alpha_surface`.
fn facts_of(profile: &crate::profile::Profile) -> (f64, bool) {
    (profile.float("Transparency"), profile.bool("Blur"))
}

impl App {
    /// The current window's Use Transparency toggle (iTerm2's `useTransparency`). It does not mean some
    /// session is see-through: a window whose profiles all have Transparency 0 can answer true. Tandem has
    /// no native full screen, so iTerm2's Lion check never applies.
    pub(super) fn window_uses_transparency(&self) -> bool {
        self.win_transparency.on
    }

    /// View > Use Transparency (Win+U, `PT:6296-6305`): flips this window's state and redraws it.
    pub(super) fn toggle_use_transparency(&mut self) {
        self.win_transparency.toggle();
        self.transparency_changed();
    }

    /// View > Disable Transparency for Active Window (`AD:3331-3335`): flips the global, saves it and
    /// redraws every window.
    pub(super) fn toggle_disable_for_key_window(&mut self) {
        let now = self.globals.bool(transparency::DISABLE_KEY_WINDOW_KEY);
        self.globals.set(transparency::DISABLE_KEY_WINDOW_KEY, toml::Value::Boolean(!now));
        self.save_globals();
        self.menus_dirty = true;
        self.sync_checks();
        self.transparency_changed();
    }

    /// Everything that follows a change of a pane's alpha: the border rule (13f reads
    /// `any_pane_is_transparent`), the menu check marks and every window's drawing.
    fn transparency_changed(&mut self) {
        self.apply_border();
        self.sync_blur();
        self.menus_dirty = true;
        self.sync_checks();
        self.redraw();
    }

    /// Runs the Use Transparency full screen state machine for a window about to enter or leave full
    /// screen (`PTWS:587-600`), then the border rule. Callers that set full screen themselves call this
    /// with the window's state before the change.
    pub(super) fn before_fullscreen_toggle(&mut self, full_screen_now: bool) {
        let disable = self.globals.bool(transparency::DISABLE_FULLSCREEN_KEY);
        self.win_transparency.before_fullscreen_toggle(full_screen_now, disable);
    }

    /// Enters or leaves full screen for the current window, the one entry point (the menu, the API, the key
    /// action). The window's Use Transparency follows `DisableFullscreenTransparency`.
    pub(super) fn toggle_fullscreen(&mut self) {
        let Some(w) = self.gpu.as_ref().map(|g| g.window.clone()) else { return };
        let is_full = w.fullscreen().is_some();
        self.before_fullscreen_toggle(is_full);
        w.set_fullscreen(if is_full { None } else { Some(winit::window::Fullscreen::Borderless(None)) });
        self.transparency_changed();
    }

    /// The alpha of each visible pane of `tab` (`1 - Transparency` of the pane's profile while the
    /// window's Use Transparency is on).
    pub(super) fn pane_alphas(&self, tab: &Tab) -> Vec<(PaneId, f32)> {
        let disable_for_key = self.globals.bool(transparency::DISABLE_KEY_WINDOW_KEY);
        let key_window = self.focused && self.app_active;
        self.visible(tab)
            .into_iter()
            .map(|(id, _)| {
                let t = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).map_or(0.0, |p| p.float("Transparency"));
                (id, transparency::pane_alpha(t, self.win_transparency.on, key_window, disable_for_key))
            })
            .collect()
    }

    /// Applies the DWM backdrop to the current window when the blur it should have differs from what DWM was
    /// last told (`PTYTab.m:3027-3066`): the active tab's sessions, the window's toggle, not while minimized.
    /// The radius is computed but acrylic has none. A refused backdrop (Windows 10, before 22H2) is not
    /// retried until the wanted state changes again.
    pub(super) fn sync_blur(&mut self) {
        let Some(g) = self.gpu.as_ref() else { return };
        if g.window.is_minimized().unwrap_or(false) {
            return;
        }
        let want = self.tabs.get(self.active).is_some_and(|tab| {
            let sessions: Vec<transparency::BlurInput> = tab
                .layout
                .panes()
                .into_iter()
                .filter_map(|id| self.panes.get(&id).and_then(|p| self.store.get(&p.profile)))
                .map(|p| transparency::BlurInput { transparency: p.float("Transparency"), blur: p.bool("Blur"), radius: p.get("Blur Radius").map(|_| p.float("Blur Radius")) })
                .collect();
            let _radius = transparency::tab_blur_radius(&sessions);
            transparency::tab_blur(&sessions, self.window_uses_transparency())
        });
        // Only a window that was blurred is switched back: margins set by other code stay.
        if want == self.blur_applied {
            return;
        }
        let Some(hwnd) = self.hwnd() else { return };
        crate::backdrop::set_blur(hwnd, want);
        self.blur_applied = want;
    }

    /// Reconfigures the current window's surface when the alpha mode it should have differs from the one
    /// it has. An opaque window never asks for a premultiplied mode.
    pub(super) fn sync_surface_alpha(&mut self, transparent: bool) {
        if self.win_alpha_surface != SurfaceKind::Alpha {
            return;
        }
        let Some(g) = self.gpu.as_mut() else { return };
        if (g.config.alpha_mode == wgpu::CompositeAlphaMode::PreMultiplied) == transparent {
            return;
        }
        let caps = g.surface.get_capabilities(g.render_adapter());
        let mode = transparency::pick_alpha_mode(&caps.alpha_modes, transparent);
        if mode != g.config.alpha_mode {
            g.config.alpha_mode = mode;
            g.surface.configure(g.render_device(), &g.config);
        }
    }

    /// The second instance, made on first use against `window`'s surface (a window made with
    /// `WS_EX_NOREDIRECTIONBITMAP`). None, with `transparency_unavailable` set, when no DX12 adapter answers.
    pub(super) fn alpha_gpu(&mut self, event_loop: &ActiveEventLoop, window: &Arc<Window>) -> Option<&AlphaGpu> {
        if self.alpha_gpu.is_none() && !self.transparency_unavailable {
            match make_alpha_gpu(event_loop, window) {
                Ok(a) => self.alpha_gpu = Some(a),
                Err(e) => {
                    eprintln!("transparency unavailable: {e}");
                    self.transparency_unavailable = true;
                }
            }
        }
        self.alpha_gpu.as_ref()
    }

    /// Shows the one banner when a pane wants transparency and this machine cannot draw it.
    pub(super) fn warn_transparency_unavailable(&mut self, alphas: &[(PaneId, f32)]) {
        if self.transparency_unavailable && !self.transparency_banner && transparency::needs_alpha(&alphas.iter().map(|(_, a)| *a).collect::<Vec<_>>()) {
            self.transparency_banner = true;
            self.banners.set("transparency", UNAVAILABLE);
        }
    }

    /// The (Transparency, Blur) of every profile a window opens with.
    pub(super) fn new_window_facts(&self, first: &NewWindow) -> Vec<(f64, bool)> {
        let of_guid = |g: Option<&str>| g.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile());
        let of_tab = |t: &Tab| t.layout.panes().into_iter().filter_map(|id| self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).map(facts_of)).collect::<Vec<_>>();
        match first {
            NewWindow::Profile(g) => vec![facts_of(of_guid(g.as_deref()))],
            NewWindow::Tab(t) | NewWindow::TornOff(t, _) => of_tab(t),
            NewWindow::Tabs(ts) => ts.iter().flat_map(of_tab).collect(),
            NewWindow::Arrangement(w, _) | NewWindow::Restored(w) => self.arrangement_facts(w),
            NewWindow::Orphans(_) => vec![facts_of(self.store.default_profile())],
            NewWindow::Hotkey(g, _) => vec![facts_of(of_guid(Some(g)))],
            NewWindow::HotkeyRestored(g, w) => {
                let mut v = self.arrangement_facts(w);
                v.push(facts_of(of_guid(Some(g))));
                v
            }
        }
    }

    /// Every session profile of an arrangement's window.
    fn arrangement_facts(&self, w: &crate::arrangement::Value) -> Vec<(f64, bool)> {
        let saved = self.profile_list();
        let num = |t: &toml::Table, k: &str| t.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)));
        let tabs = w.get("Tabs").and_then(|t| t.as_array()).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for tab in &tabs {
            let Some(tree) = tab.get("Root").and_then(|r| crate::arrangement::tree_of(r).ok()) else { continue };
            for leaf in tree.leaves() {
                out.push(match crate::arrangement::choose_profile(leaf, &saved) {
                    crate::arrangement::ProfileChoice::Saved(g) => facts_of(self.store.get(&g).unwrap_or_else(|| self.store.default_profile())),
                    crate::arrangement::ProfileChoice::Embedded(t) => (num(&t, "Transparency").unwrap_or(0.0), t.get("Blur").and_then(|v| v.as_bool()).unwrap_or(false)),
                    crate::arrangement::ProfileChoice::Broken | crate::arrangement::ProfileChoice::Default => facts_of(self.store.default_profile()),
                });
            }
        }
        out
    }

    /// The surface kind a window about to open should have.
    pub(super) fn surface_kind_for(&self, first: &NewWindow) -> SurfaceKind {
        if !self.transparency_unavailable && self.gpu.is_some() && transparency::wants_alpha_surface(&self.new_window_facts(first)) { SurfaceKind::Alpha } else { SurfaceKind::Opaque }
    }

    /// A `Gpu` for a new window of `kind`: alpha windows on the second instance, everything else exactly as
    /// before (on the first window's device, or a fresh instance for the very first window).
    pub(super) fn gpu_for(&mut self, event_loop: &ActiveEventLoop, window: Arc<Window>, kind: SurfaceKind) -> Result<Gpu, String> {
        if kind == SurfaceKind::Alpha {
            let alpha = self.alpha_gpu(event_loop, &window).cloned().ok_or_else(|| UNAVAILABLE.to_string())?;
            let primary = self.gpu.as_ref().ok_or("no window to take the primary device from")?;
            return init_gpu_alpha(window, primary, &alpha);
        }
        match self.gpu.as_ref() {
            Some(g) => init_gpu_shared(window, g),
            None => init_gpu(window, event_loop),
        }
    }

    /// Marks every window whose surface kind no longer fits the profiles it shows (after a profile edit, a
    /// new tab or split, an arrangement restore or a hotkey window opening); `alpha_recreate_tick` rebuilds them.
    pub(super) fn check_alpha_surfaces(&mut self) {
        if self.transparency_unavailable {
            return;
        }
        let facts = |tabs: &[Tab], panes: &HashMap<PaneId, Pane>, store: &Store| -> Vec<(f64, bool)> {
            tabs.iter().flat_map(|t| t.layout.panes()).filter_map(|id| panes.get(&id).and_then(|p| store.get(&p.profile)).map(facts_of)).collect()
        };
        let mut due: Vec<WindowId> = Vec::new();
        if let (Some(g), true) = (&self.gpu, transparency::recreate_due(self.win_alpha_surface, &facts(&self.tabs, &self.panes, &self.store), self.win_alpha_failed)) {
            due.push(g.window.id());
        }
        for (_, s) in self.windows.parked() {
            if let Some(g) = &s.gpu {
                if transparency::recreate_due(s.win_alpha_surface, &facts(&s.tabs, &self.panes, &self.store), s.win_alpha_failed) {
                    due.push(g.window.id());
                }
            }
        }
        for id in due {
            if !self.alpha_pending.contains(&id) {
                self.alpha_pending.push(id);
            }
        }
    }

    /// Recreates the windows `check_alpha_surfaces` marked, then re-enters the window that was current.
    pub(super) fn alpha_recreate_tick(&mut self, event_loop: &ActiveEventLoop) {
        if self.alpha_pending.is_empty() {
            return;
        }
        let back = self.current_window();
        let mut back_now = back;
        for id in std::mem::take(&mut self.alpha_pending) {
            if !self.enter(id) {
                continue;
            }
            match self.recreate_window_for_alpha(event_loop) {
                Ok(new) if back == Some(id) => back_now = Some(new),
                Ok(_) => {}
                Err(e) => {
                    // This window stays opaque and is not tried again; one banner says so.
                    eprintln!("transparency: could not recreate a window: {e}");
                    // Only this window stays opaque; windows that already work keep their alpha.
                    self.win_alpha_failed = true;
                    if !self.transparency_banner {
                        self.transparency_banner = true;
                        self.banners.set("transparency", UNAVAILABLE);
                    }
                }
            }
        }
        if let Some(b) = back_now {
            self.enter(b);
        }
    }

    /// Replaces the current window with one made for alpha: the same position, size, style, visibility and
    /// title, the same tabs and panes (the whole slot moves), every map keyed by its id re-keyed, the old
    /// window dropped last. `WS_EX_NOREDIRECTIONBITMAP` cannot be added to a live window, hence the new one.
    pub(super) fn recreate_window_for_alpha(&mut self, event_loop: &ActiveEventLoop) -> Result<WindowId, String> {
        let old = self.gpu.as_ref().ok_or("no current window")?.window.clone();
        if self.win_alpha_surface == SurfaceKind::Alpha {
            return Ok(old.id());
        }
        let hotkey = self.hotkey.is_some();
        let floating_hotkey = self.hotkey.as_ref().is_some_and(|h| h.shown) && self.hotkey.as_ref().and_then(|h| self.store.get(&h.guid)).is_some_and(|p| crate::hotkey::Prefs::from_profile(p).floats);
        let carried = transparency::carry(old.fullscreen().is_some(), self.win_style == crate::frames::ACCESSORY, floating_hotkey, old.is_visible() != Some(false), self.focused);
        // A manager owned by the old window would be destroyed with it: close it, so it can open again.
        self.close_pwm();
        let position = old.outer_position().ok();
        let size: PhysicalSize<u32> = old.inner_size();
        let mut attrs = Window::default_attributes()
            .with_title(old.title())
            .with_inner_size(size)
            .with_decorations(old.is_decorated())
            .with_resizable(old.is_resizable())
            .with_maximized(old.is_maximized())
            .with_visible(false)
            .with_active(false)
            .with_no_redirection_bitmap(true);
        if hotkey {
            attrs = attrs.with_skip_taskbar(true);
        }
        if let Some(p) = position {
            attrs = attrs.with_position(p);
        }
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let gpu = self.gpu_for(event_loop, window.clone(), SurfaceKind::Alpha)?;
        if let Some(p) = position {
            window.set_outer_position(PhysicalPosition::new(p.x, p.y));
        }
        let (old_id, new_id) = (old.id(), window.id());
        let old_hwnd = menu::hwnd_of(&old);
        let old_gpu = self.gpu.replace(gpu);
        self.win_alpha_surface = SurfaceKind::Alpha;
        // The new handle has no backdrop yet.
        self.blur_applied = false;
        self.rekey_window(old_id, new_id);
        if let Some(h) = old_hwnd {
            crate::winkeys::remove_window(h);
        }
        // Everything the code installs per window handle, on the new one (as `open_window_with` does).
        self.menu_bar = None;
        self.border_applied = None;
        self.theme_applied = None;
        self.install_sizing();
        self.guard_theme();
        self.refresh_caption_frame();
        self.apply_corners();
        self.font_sig = String::new();
        self.apply_fonts();
        self.install_menus();
        self.relayout();
        if carried.fullscreen {
            window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(old.current_monitor())));
        }
        // After full screen, so the border rule sees the state the window ends in.
        self.apply_border();
        if carried.on_top {
            window.set_window_level(winit::window::WindowLevel::AlwaysOnTop);
        }
        if carried.show {
            window.set_visible(true);
        }
        if carried.activate {
            window.focus_window();
        }
        window.request_redraw();
        // The old window goes last: its surface, then its window.
        drop(old_gpu);
        drop(old);
        Ok(new_id)
    }

    /// Every app-wide record keyed by a window's id follows the window to its new id.
    fn rekey_window(&mut self, old: WindowId, new: WindowId) {
        self.registry.rekey(key(old), key(new));
        self.windows.rekey_current(key(old), key(new));
        self.buried.rekey(key(old), key(new));
        let swap = |w: &mut WindowId| {
            if *w == old {
                *w = new;
            }
        };
        if let Some(w) = self.last_window.as_mut() {
            swap(w);
        }
        for w in self.app_hidden.iter_mut() {
            swap(w);
        }
        for (w, _) in self.pending_tab_closes.iter_mut() {
            swap(w);
        }
        if let Some((w, _)) = self.tab_join.as_mut() {
            swap(w);
        }
        if let Some((w, _)) = self.action_edit_undo.as_mut() {
            swap(w);
        }
        if let Some((Some(w), _, _)) = self.checks_key.as_mut() {
            swap(w);
        }
        if let Some(n) = self.jobs_wanted.remove(&old) {
            self.jobs_wanted.insert(new, n);
        }
    }
}

/// The second instance: DX12 only, DirectComposition swapchains (the only kind that offers a premultiplied
/// alpha mode), its adapter chosen against `window`'s surface. The probe surface is dropped before the
/// window's real one is made.
fn make_alpha_gpu(event_loop: &ActiveEventLoop, window: &Arc<Window>) -> Result<AlphaGpu, String> {
    let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(event_loop.owned_display_handle()));
    desc.backends = wgpu::Backends::DX12;
    desc.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
    let instance = wgpu::Instance::new(desc);
    let adapter = {
        let probe = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(&probe), ..Default::default() })).map_err(|e| e.to_string())?
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
    Ok(GpuHandles { instance, adapter, device, queue })
}
