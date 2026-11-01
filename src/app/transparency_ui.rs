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
        // The fade pass scales the stored values on a non-sRGB view of the target (render.rs).
        view_formats: if format.is_srgb() { vec![format.remove_srgb_suffix()] } else { vec![] },
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
        shape: None,
        fx: None,
        window,
    })
}

/// The profiles a window opens with, as (Transparency, Blur), for `transparency::wants_alpha_surface`.
fn facts_of(profile: &crate::profile::Profile) -> (f64, bool) {
    (profile.float("Transparency"), profile.bool("Blur"))
}

impl App {
    /// The current window's Use Transparency toggle (iTerm2's `useTransparency`). It does not mean some
    /// session is see-through: a window whose profiles all have Transparency 0 can answer true. Termurion has
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
        crate::platform::display::set_blur(hwnd, want);
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
            match make_alpha_gpu(&event_loop.owned_display_handle(), window) {
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
            NewWindow::ProfilesIn(first, rest) => std::iter::once(first).chain(rest.guids()).map(|g| facts_of(of_guid(Some(g)))).collect(),
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
        // Every terminal window allows the IME, one that is recreated for alpha too.
        window.set_ime_allowed(true);
        if kind == SurfaceKind::Alpha {
            let alpha = self.alpha_gpu(event_loop, &window).cloned().ok_or_else(|| UNAVAILABLE.to_string())?;
            let primary = self.gpu.as_ref().ok_or("no window to take the primary device from")?;
            return init_gpu_alpha(window, primary, &alpha);
        }
        match self.gpu.as_ref() {
            Some(g) => init_gpu_shared(window, g),
            None => init_gpu(window, event_loop, power::chosen_preference(self.globals.bool("preferIntegratedGPU"))),
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
            if let Some(g) = &s.gpu
                && transparency::recreate_due(s.win_alpha_surface, &facts(&s.tabs, &self.panes, &self.store), s.win_alpha_failed) {
                    due.push(g.window.id());
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
        let floating_hotkey = self.hotkey.as_ref().is_some_and(|h| h.shown) && self.hotkey.as_ref().and_then(|h| self.store.get(&h.guid)).is_some_and(|p| crate::hotkey::Prefs::from_profile(p).floats);
        let old_hwnd = menu::hwnd_of(&old);
        let placed = old_hwnd.and_then(placement_of).unwrap_or_else(|| transparency::placed(1, 0, [0; 4], (0, 0)));
        let carried = transparency::carry(old.fullscreen().is_some(), self.win_style == crate::frames::ACCESSORY, floating_hotkey, old.is_visible() != Some(false), self.focused, &placed);
        // A manager owned by the old window would be destroyed with it: close it, so it can open again.
        self.close_pwm();
        // A minimized or maximized window's rectangle is the iconic or screen-filling one: its normal
        // frame (from the placement) is what the new window takes, and the state is applied after.
        let restore_frame = placed.minimized || placed.maximized;
        let [pl, pt, pr, pb] = placed.frame;
        let position = if restore_frame { Some(PhysicalPosition::new(pl, pt)) } else { old.outer_position().ok() };
        let size: PhysicalSize<u32> = if restore_frame { PhysicalSize::new((pr - pl).max(1) as u32, (pb - pt).max(1) as u32) } else { old.inner_size() };
        // The old window's desktop (None for a cloaked hotkey window or a session without a shell): the
        // new window is created on the current desktop and moved there before it shows.
        let old_desktop = old_hwnd.and_then(|h| self.desktops().and_then(|m| m.window_desktop(h)));
        let hotkey_shown = self.hotkey.as_ref().map(|h| h.shown);
        let look = crate::presence::with_accessory(crate::presence::window_look(self.ui_element_now, hotkey_shown, self.globals.bool(crate::presence::EXCLUDED_FROM_CYCLING_KEY)), self.win_style == crate::frames::ACCESSORY);
        let mut attrs = Window::default_attributes()
            .with_title(old.title())
            .with_window_icon(crate::presence::window_icon())
            .with_inner_size(size)
            .with_decorations(old.is_decorated())
            .with_resizable(old.is_resizable())
            .with_skip_taskbar(look.skip_taskbar)
            .with_visible(false)
            .with_active(false)
            .with_no_redirection_bitmap(true);
        if let Some(p) = position {
            attrs = attrs.with_position(p);
        }
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let gpu = self.gpu_for(event_loop, window.clone(), SurfaceKind::Alpha)?;
        if let Some(p) = position {
            window.set_outer_position(PhysicalPosition::new(p.x, p.y));
        }
        let (old_id, new_id) = (old.id(), window.id());
        let old_gpu = self.gpu.replace(gpu);
        self.win_alpha_surface = SurfaceKind::Alpha;
        // A hotkey window that was fading shows at once on its new window.
        self.end_fade_on_recreate();
        // The new handle has no backdrop yet.
        self.blur_applied = false;
        self.look_seen = None;
        // The new handle's IME has been given no candidate area yet.
        self.ime.system_reset();
        self.rekey_window(old_id, new_id);
        // Hosted web views (tool views, popovers) follow the new handle before the old window is destroyed.
        if let Some(h) = menu::hwnd_of(&window) {
            self.window_remade(h);
        }
        if let Some(h) = old_hwnd {
            crate::platform::keys::remove_window(h);
        }
        // Everything the code installs per window handle, on the new one (as `open_window_with` does).
        self.menu_bar = None;
        self.border_applied = None;
        self.theme_applied = None;
        self.install_sizing();
        self.guard_theme();
        self.access_reinstall_host(key(old_id));
        // The display change subclass (the old handle's goes with its window).
        if let Some(h) = menu::hwnd_of(&window) {
            crate::platform::display::watch(h);
            #[cfg(windows)]
            {
                crate::firstmouse_win::watch(h);
                self.taskbar_watch(h);
                super::logout_ui::watch(h);
            }
        }
        self.refresh_caption_frame();
        self.apply_corners();
        self.font_sig = String::new();
        self.pane_faces = Default::default();
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
        if restore_frame {
            super::chrome_ui::set_outer_frame(&window, crate::layout::Rect { x: pl as f32, y: pt as f32, w: (pr - pl) as f32, h: (pb - pt) as f32 });
        }
        if carried.maximized {
            window.set_maximized(true);
        }
        // Before the window shows, so it never appears on the wrong desktop (P18b: our own window).
        if let (Some(id), Some(h)) = (old_desktop, menu::hwnd_of(&window))
            && let Some(m) = self.desktops()
                && let Err(e) = m.move_window(h, id) {
                    eprintln!("transparency: could not move the new window to its desktop: {e}");
                }
        if carried.show {
            match menu::hwnd_of(&window) {
                // Minimized shows without activation, over the frame set above.
                Some(h) if carried.minimized => unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(h as windows_sys::Win32::Foundation::HWND, windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE);
                },
                _ => window.set_visible(true),
            }
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

/// The old window's placement (`GetWindowPlacement`): its normal frame in screen coordinates and its
/// minimized and maximized state. A tool window's placement is in screen coordinates already; any other
/// window's is relative to the work area.
fn placement_of(hwnd: isize) -> Option<transparency::Placed> {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GetWindowPlacement, GWL_EXSTYLE, WINDOWPLACEMENT, WS_EX_TOOLWINDOW};
    let h = hwnd as HWND;
    let mut wp: WINDOWPLACEMENT = unsafe { std::mem::zeroed() };
    wp.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if unsafe { GetWindowPlacement(h, &mut wp) } == 0 {
        return None;
    }
    let tool = unsafe { GetWindowLongW(h, GWL_EXSTYLE) } as u32 & WS_EX_TOOLWINDOW != 0;
    let mut shift = (0, 0);
    if !tool {
        let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if unsafe { GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi) } != 0 {
            shift = (mi.rcWork.left - mi.rcMonitor.left, mi.rcWork.top - mi.rcMonitor.top);
        }
    }
    let r = wp.rcNormalPosition;
    Some(transparency::placed(wp.showCmd, wp.flags, [r.left, r.top, r.right, r.bottom], shift))
}

/// The second instance: DX12 only, DirectComposition swapchains (the only kind that offers a premultiplied
/// alpha mode), its adapter chosen against `window`'s surface. The probe surface is dropped before the
/// window's real one is made.
pub(super) fn make_alpha_gpu(display: &winit::event_loop::OwnedDisplayHandle, window: &Arc<Window>) -> Result<AlphaGpu, String> {
    let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(display.clone()));
    desc.backends = wgpu::Backends::DX12;
    desc.backend_options.dx12.presentation_system = wgpu::Dx12SwapchainKind::DxgiFromVisual;
    desc.backend_options.dx12.shader_compiler = wgpu::Dx12Compiler::Fxc;
    let instance = wgpu::Instance::new(desc);
    let adapter = {
        let probe = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: power::chosen(), compatible_surface: Some(&probe), ..Default::default() })).map_err(|e| e.to_string())?
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
    crate::gpu_pick::watch(&device, crate::gpu_pick::current_generation());
    Ok(GpuHandles { instance, adapter, device, queue })
}

#[cfg(test)]
mod watch_tests {
    /// A window recreated for alpha gets display change notifications like every other window
    /// (review of 13e): the recreate installs the subclass on the new handle.
    /// Important 2: the new window is moved to the old window's desktop before it shows (Restore windows
    /// to same desktops holds for a transparent first window).
    #[test]
    fn recreate_moves_the_new_window_to_the_old_desktop_before_showing() {
        let src = include_str!("transparency_ui.rs");
        let body = src.split("fn recreate_window_for_alpha").nth(1).expect("recreate_window_for_alpha");
        let body = body.split("\n    }\n").next().unwrap();
        let read = body.find("window_desktop(").expect("the old window's desktop is read");
        let made = body.find("create_window(").expect("the new window is created");
        let moved = body.find("move_window(").expect("the new window is moved to that desktop");
        let shown = body.find("set_visible(true)").expect("the new window is shown");
        assert!(read < made && made < moved && moved < shown, "read, create, move, then show");
    }

    /// Minor 3: the new window's taskbar skip comes from its current look, so the presence pass has nothing
    /// to flip (no second hide and show, no button flash).
    #[test]
    fn recreate_builds_the_taskbar_skip_from_the_look() {
        let src = include_str!("transparency_ui.rs");
        let body = src.split("fn recreate_window_for_alpha").nth(1).expect("recreate_window_for_alpha");
        let body = body.split("\n    }\n").next().unwrap();
        assert!(body.contains("with_skip_taskbar(look.skip_taskbar)"), "skip from the look, not only for hotkey windows");
        assert!(body.contains("placement_of"), "the old placement is read");
    }

    #[test]
    fn recreate_watches_the_new_window() {
        let src = include_str!("transparency_ui.rs");
        let body = src.split("fn recreate_window_for_alpha").nth(1).expect("recreate_window_for_alpha");
        let body = body.split("\n    }\n").next().unwrap();
        assert!(body.contains("display::watch"), "the new HWND must be watched");
    }
}
