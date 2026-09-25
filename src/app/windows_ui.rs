//! Many terminal windows. The current window's state lives in `App`'s own fields; every
//! other window's state waits in a `WinSlot`, and `enter` swaps a window in before its
//! events run or its frame is drawn.

use super::*;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Fullscreen;

/// What belongs to one window.
pub(super) struct WinSlot {
    gpu: Option<Gpu>,
    tabs: Vec<Tab>,
    active: usize,
    menu_bar: Option<menu::MenuBar>,
    cursor: (f32, f32),
    drag: Drag,
    pointer_down: Option<PointerDown>,
    hover: Option<(PaneId, crate::find::Match)>,
    find_bar_rect: Option<Rect>,
    find_mode_rect: Option<Rect>,
    tab_rects: Vec<Rect>,
    block_buttons: Vec<blocks_ui::BlockButton>,
    block_hover: Option<(PaneId, u32, i32, i32)>,
    porthole_hits: Vec<porthole_ui::PortholeHit>,
    focused: bool,
    wheel_acc: f32,
    wheel_acc_x: f32,
    font_sig: String,
    win_style: i64,
    win_slot: Option<(String, u32)>,
}

impl WinSlot {
    pub(super) fn new(gpu: Gpu) -> WinSlot {
        WinSlot {
            gpu: Some(gpu),
            tabs: Vec::new(),
            active: 0,
            menu_bar: None,
            cursor: (0.0, 0.0),
            drag: Drag::None,
            pointer_down: None,
            hover: None,
            find_bar_rect: None,
            find_mode_rect: None,
            tab_rects: Vec::new(),
            block_buttons: Vec::new(),
            block_hover: None,
            porthole_hits: Vec::new(),
            focused: false,
            wheel_acc: 0.0,
            wheel_acc_x: 0.0,
            font_sig: String::new(),
            win_style: 0,
            win_slot: None,
        }
    }
}

/// The window id as the registry keys it.
pub(super) fn key(id: WindowId) -> u64 {
    u64::from(id)
}

impl App {
    /// Swaps a slot's fields with the live ones.
    fn swap_live(&mut self, s: &mut WinSlot) {
        use std::mem::swap;
        swap(&mut self.gpu, &mut s.gpu);
        swap(&mut self.tabs, &mut s.tabs);
        swap(&mut self.active, &mut s.active);
        swap(&mut self.menu_bar, &mut s.menu_bar);
        swap(&mut self.cursor, &mut s.cursor);
        swap(&mut self.drag, &mut s.drag);
        swap(&mut self.pointer_down, &mut s.pointer_down);
        swap(&mut self.hover, &mut s.hover);
        swap(&mut self.find_bar_rect, &mut s.find_bar_rect);
        swap(&mut self.find_mode_rect, &mut s.find_mode_rect);
        swap(&mut self.tab_rects, &mut s.tab_rects);
        swap(&mut self.block_buttons, &mut s.block_buttons);
        swap(&mut self.block_hover, &mut s.block_hover);
        swap(&mut self.porthole_hits, &mut s.porthole_hits);
        swap(&mut self.focused, &mut s.focused);
        swap(&mut self.wheel_acc, &mut s.wheel_acc);
        swap(&mut self.wheel_acc_x, &mut s.wheel_acc_x);
        swap(&mut self.font_sig, &mut s.font_sig);
        swap(&mut self.win_style, &mut s.win_style);
        swap(&mut self.win_slot, &mut s.win_slot);
    }

    /// Makes a terminal window current. False for a window that is not one.
    pub(super) fn enter(&mut self, id: WindowId) -> bool {
        let k = key(id);
        if self.windows.current() == Some(k) {
            return true;
        }
        let Some(mut slot) = self.windows.remove(k) else { return false };
        self.swap_live(&mut slot);
        // `slot` now holds the window that was current.
        if let Some(old) = self.windows.current() {
            self.windows.park(old, slot);
        }
        self.windows.start(k);
        true
    }

    /// Makes the window that shows `pane` current. False when no window shows it.
    pub(super) fn enter_pane_window(&mut self, pane: PaneId) -> bool {
        if self.tabs.iter().any(|t| t.layout.panes().contains(&pane)) {
            return true;
        }
        let found = self.windows.parked().find(|(_, s)| s.tabs.iter().any(|t| t.layout.panes().contains(&pane))).map(|(k, _)| *k);
        let Some(k) = found else { return false };
        let id = self.windows.parked().find(|(kk, _)| **kk == k).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()));
        id.is_some_and(|id| self.enter(id))
    }

    /// The current window's id.
    pub(super) fn current_window(&self) -> Option<WindowId> {
        self.gpu.as_ref().map(|g| g.window.id())
    }

    /// Asks every window to draw.
    pub(super) fn redraw_all(&self) {
        for (_, s) in self.windows.parked() {
            if let Some(g) = &s.gpu {
                g.window.request_redraw();
            }
        }
    }

    /// Opens a terminal window with one tab of `guid` (the default profile when None).
    pub(super) fn open_window(&mut self, event_loop: &ActiveEventLoop, guid: Option<&str>) -> Result<WindowId, String> {
        self.open_window_with(event_loop, NewWindow::Profile(guid.map(str::to_string)))
    }

    fn open_window_with(&mut self, event_loop: &ActiveEventLoop, first: NewWindow) -> Result<WindowId, String> {
        let attrs = Window::default_attributes().with_title("Tandem").with_inner_size(LogicalSize::new(1000.0, 650.0));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        let gpu = match self.gpu.as_ref() {
            Some(g) => init_gpu_shared(window, g)?,
            None => init_gpu(window, event_loop)?,
        };
        let id = gpu.window.id();
        let mut slot = WinSlot::new(gpu);
        self.swap_live(&mut slot);
        if let Some(old) = self.windows.current() {
            self.windows.park(old, slot);
        }
        self.windows.start(key(id));
        self.registry.open(key(id));
        self.apply_fonts();
        self.size_window();
        let first_guid = match &first {
            NewWindow::Profile(g) => g.clone(),
            NewWindow::Profiles(gs) => gs.first().cloned().flatten(),
            NewWindow::Tab(_) => None,
        };
        let style = first_guid.as_deref().and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).int("Window Type");
        match first {
            NewWindow::Profile(guid) => self.new_tab(guid.as_deref()),
            NewWindow::Profiles(guids) => {
                for g in &guids {
                    self.new_tab(g.as_deref());
                }
                self.active = 0;
            }
            NewWindow::Tab(tab) => {
                self.tabs.push(tab);
                self.active = 0;
                self.relayout();
            }
        }
        self.install_menus();
        self.place_window();
        if style != crate::frames::NORMAL {
            self.apply_window_style(style, false);
        }
        if self.tabs.is_empty() {
            // The shell did not start: the window closes again, and the banner says why.
            let why = self.banners.text().unwrap_or_default();
            self.drop_current_window();
            return Err(why);
        }
        Ok(id)
    }

    /// The current window is gone (its last tab closed): another window becomes current.
    /// False when it was the last one.
    pub(super) fn drop_current_window(&mut self) -> bool {
        let Some(k) = self.windows.current() else { return false };
        self.save_window_frame();
        self.registry.close(k);
        if let Some(h) = self.hwnd() {
            crate::winkeys::remove_window(h);
        }
        let mut live = WinSlot::new_empty();
        self.swap_live(&mut live);
        // `live` now holds the closed window; dropping it closes the OS window.
        drop(live);
        let Some((next, mut slot)) = self.windows.pop_parked() else { return false };
        self.swap_live(&mut slot);
        self.windows.start(next);
        self.relayout();
        self.redraw();
        true
    }

    /// Closes one window and its sessions (Cmd+Shift+W, the close box). The last window
    /// closing ends Tandem.
    pub(super) fn close_window(&mut self, id: WindowId, event_loop: &ActiveEventLoop) {
        if !self.enter(id) || !self.confirm_close(close_ui::Closing::Window) {
            return;
        }
        let panes: Vec<PaneId> = self.tabs.iter().flat_map(|t| t.layout.panes()).collect();
        for p in panes {
            self.close_pane(p, event_loop);
        }
    }

    pub(super) fn window_number(&self) -> usize {
        self.windows.current().and_then(|k| self.registry.number(k)).unwrap_or(0)
    }
}

impl WinSlot {
    pub(super) fn new_empty() -> WinSlot {
        WinSlot {
            gpu: None,
            tabs: Vec::new(),
            active: 0,
            menu_bar: None,
            cursor: (0.0, 0.0),
            drag: Drag::None,
            pointer_down: None,
            hover: None,
            find_bar_rect: None,
            find_mode_rect: None,
            tab_rects: Vec::new(),
            block_buttons: Vec::new(),
            block_hover: None,
            porthole_hits: Vec::new(),
            focused: false,
            wheel_acc: 0.0,
            wheel_acc_x: 0.0,
            font_sig: String::new(),
            win_style: 0,
            win_slot: None,
        }
    }
}

/// A window's surface and renderer on the first window's device.
pub(super) fn init_gpu_shared(window: Arc<Window>, from: &Gpu) -> Result<Gpu, String> {
    let surface = from.instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
    let caps = surface.get_capabilities(&from.adapter);
    let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
    let size = window.inner_size();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: size.width.max(1),
        height: size.height.max(1),
        present_mode: wgpu::PresentMode::Fifo,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
        color_space: wgpu::SurfaceColorSpace::Auto,
    };
    surface.configure(&from.device, &config);
    let renderer = Renderer::new(&from.device, &from.queue, format, FONT, FONT_PX * window.scale_factor() as f32);
    Ok(Gpu { surface, adapter: from.adapter.clone(), device: from.device.clone(), queue: from.queue.clone(), config, renderer, instance: from.instance.clone(), window })
}

impl App {
    /// Window > New Window and New Window with Current Profile.
    pub(super) fn new_window(&mut self, event_loop: &ActiveEventLoop, guid: Option<&str>) {
        if let Err(e) = self.open_window(event_loop, guid) {
            self.banners.set("window", format!("Could not open a window: {e}"));
            self.redraw();
        }
    }
}

impl App {
    /// The Profiles menu lists every profile after its fixed items.
    pub(super) fn refresh_profiles_menu(&mut self) {
        let items: Vec<(String, String)> = self.store.profiles.iter().map(|p| (p.guid(), p.name())).collect();
        let Some(bar) = &mut self.menu_bar else { return };
        if bar.profile_items == items {
            return;
        }
        let Some((sub, fixed)) = &bar.profiles else { return };
        while sub.remove_at(*fixed).is_some() {}
        for (guid, name) in &items {
            let entry = muda::MenuItem::with_id(format!("{}{guid}", menu::PROFILE_ID), name.replace('&', "&&"), true, None);
            if let Err(e) = sub.append(&entry) {
                self.banners.set("menu", format!("Profiles menu: {e}"));
            }
        }
        bar.profile_items = items;
    }

    /// A profile picked in the Profiles menu: a tab, or a window with Alt held (swapped by
    /// openProfilesInNewWindow; a profile that refuses tabs always gets a window).
    pub(super) fn open_profile_from_menu(&mut self, guid: &str) {
        use crate::winmodel::{OpenRequest, Target, open_target};
        let option = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x12) } < 0;
        let prevent = self.store.get(guid).is_some_and(|p| p.bool("Prevent Opening in a Tab"));
        match open_target(OpenRequest::ProfileMenu { option }, prevent, self.globals.bool("openProfilesInNewWindow")) {
            Target::Tab => self.new_tab(Some(guid)),
            Target::Window => {
                self.pending_windows.push(NewWindow::Profile(Some(guid.to_string())));
                self.redraw();
            }
        }
    }
}

/// What a new window starts with.
pub(super) enum NewWindow {
    /// One tab of a profile (the default one when None).
    Profile(Option<String>),
    /// A tab for each profile (Duplicate Window).
    Profiles(Vec<Option<String>>),
    /// A tab moved from another window.
    Tab(Tab),
}

impl App {
    pub(super) fn open_pending(&mut self, event_loop: &ActiveEventLoop, w: NewWindow) {
        if let Err(e) = self.open_window_with(event_loop, w) {
            self.banners.set("window", format!("Could not open a window: {e}"));
            self.redraw();
        }
    }

    /// The window id of a registry key, current or parked.
    fn window_of_key(&self, k: u64) -> Option<WindowId> {
        if self.windows.current() == Some(k) {
            return self.current_window();
        }
        self.windows.parked().find(|(kk, _)| **kk == k).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()))
    }

    /// Brings a window to the front and makes it current.
    pub(super) fn focus_terminal_window(&mut self, id: WindowId) {
        if !self.enter(id) {
            return;
        }
        if let Some(g) = &self.gpu {
            g.window.set_minimized(false);
            g.window.focus_window();
        }
        self.last_window = Some(id);
    }

    /// Alt+Win+1 to 9.
    pub(super) fn select_window_number(&mut self, digit: u8) {
        if let Some(id) = self.registry.switch_target(digit).and_then(|k| self.window_of_key(k)) {
            self.focus_terminal_window(id);
        }
    }

    /// Next Window and Previous Window: by number, wrapping.
    pub(super) fn step_window(&mut self, forward: bool) {
        let Some(k) = self.windows.current() else { return };
        let next = if forward { self.registry.next(k) } else { self.registry.previous(k) };
        if let Some(id) = next.filter(|n| *n != k).and_then(|n| self.window_of_key(n)) {
            self.focus_terminal_window(id);
        }
    }

    /// Window > Move Tab to New Window (not the window's only tab).
    pub(super) fn move_tab_to_new_window(&mut self) {
        if self.tabs.len() < 2 {
            return;
        }
        let tab = self.tabs.remove(self.active);
        self.active = self.active.min(self.tabs.len() - 1);
        self.pending_windows.push(NewWindow::Tab(tab));
        self.relayout();
        self.redraw();
    }

    /// Session > Move Session > Move Session to Window: the focused pane leaves its tab
    /// for a new window (a one-pane tab moves whole).
    pub(super) fn move_session_to_window(&mut self) {
        let Some(t) = self.tabs.get_mut(self.active) else { return };
        let pane = t.focused;
        if t.layout.panes().len() < 2 {
            return self.move_tab_to_new_window();
        }
        if t.layout.remove(pane) {
            t.focused = t.layout.panes()[0];
            t.zoomed = None;
        }
        let tab = Tab { id: self.next_tab_id, layout: Layout::new(pane), focused: pane, zoomed: None };
        self.next_tab_id += 1;
        self.pending_windows.push(NewWindow::Tab(tab));
        self.relayout();
        self.redraw();
    }

    /// Window > Arrange Windows > Merge All Windows: every other window's tabs come here,
    /// in window-number order, and those windows close.
    pub(super) fn merge_all_windows(&mut self) {
        let Some(k) = self.windows.current() else { return };
        for other in self.registry.merge_order(k) {
            let Some(mut slot) = self.windows.remove(other) else { continue };
            self.tabs.append(&mut slot.tabs);
            self.registry.close(other);
            if let Some(g) = &slot.gpu {
                if let Ok(RawWindowHandle::Win32(h)) = g.window.window_handle().map(|h| h.as_raw()) {
                    crate::winkeys::remove_window(h.hwnd.get());
                }
            }
            drop(slot);
        }
        self.relayout();
        self.redraw();
    }

    /// Shell > Duplicate Window: a new window with a tab for each of this window's tabs, in
    /// the profile of its focused session.
    pub(super) fn duplicate_window(&mut self) {
        let guids: Vec<Option<String>> = self.tabs.iter().map(|t| self.panes.get(&t.focused).map(|p| self.store.shared_guid(&p.profile))).collect();
        if !guids.is_empty() {
            self.pending_windows.push(NewWindow::Profiles(guids));
            self.redraw();
        }
    }
}

impl App {
    /// The current window's monitor: its work area and whole area.
    fn monitor_rects(&self) -> Option<(Rect, Rect)> {
        use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
        let hwnd = self.hwnd()?;
        let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let m = unsafe { MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST) };
        if unsafe { GetMonitorInfoW(m, &mut info) } == 0 {
            return None;
        }
        let r = |r: windows_sys::Win32::Foundation::RECT| Rect { x: r.left as f32, y: r.top as f32, w: (r.right - r.left) as f32, h: (r.bottom - r.top) as f32 };
        Some((r(info.rcWork), r(info.rcMonitor)))
    }

    /// Applies an iTerm2 window style to the current window. The Window Style menu makes
    /// the edge styles span the whole edge (100 percent), as iTerm2's menu does.
    pub(super) fn apply_window_style(&mut self, style: i64, from_menu: bool) {
        use crate::frames::{self, Input};
        let Some((work, screen)) = self.monitor_rects() else {
            return self.banners.set("window", "Could not read this window's monitor.");
        };
        let profile = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).and_then(|p| self.store.get(&p.profile)).unwrap_or_else(|| self.store.default_profile()).clone();
        let mut percent = (profile.float("Width Percentage"), profile.float("Height Percentage"));
        let s = frames::unthemed(style);
        if from_menu {
            match s {
                frames::TOP_PERCENT | frames::BOTTOM_PERCENT => percent.0 = 100.0,
                frames::LEFT_PERCENT | frames::RIGHT_PERCENT => percent.1 = 100.0,
                _ => {}
            }
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let (outer, inner) = (g.window.outer_size(), g.window.inner_size());
        let pos = g.window.outer_position().unwrap_or_default();
        let (cw, ch) = self.cell();
        let area = self.content_area();
        let pad = 2.0 * PAD * self.scale();
        let chrome = (pad, inner.height as f32 - area.h + pad);
        let input = Input {
            style: s,
            percent,
            cells: (profile.int("Columns").clamp(1, 1000) as u16, profile.int("Rows").clamp(1, 1000) as u16),
            cell: (cw, ch),
            chrome,
            initial: Rect { x: pos.x as f32, y: pos.y as f32, w: outer.width as f32, h: outer.height as f32 },
            work,
            screen,
        };
        let f = frames::frame(&input);
        let w = &g.window;
        if f.fullscreen {
            w.set_fullscreen(Some(Fullscreen::Borderless(w.current_monitor())));
        } else {
            w.set_fullscreen(None);
            w.set_decorations(f.decorations);
            w.set_maximized(f.maximized);
            if !f.maximized && s != frames::NORMAL && s != frames::NO_TITLE_BAR {
                // The frame is the outer frame; the inner size leaves the borders out.
                let (dw, dh) = if f.decorations { (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height)) } else { (0, 0) };
                w.set_outer_position(PhysicalPosition::new(f.rect.x.round() as i32, f.rect.y.round() as i32));
                let _ = w.request_inner_size(PhysicalSize::new((f.rect.w as u32).saturating_sub(dw).max(1), (f.rect.h as u32).saturating_sub(dh).max(1)));
            }
        }
        self.win_style = s;
        self.redraw();
    }
}

const SAVED_FRAMES: &str = "SavedWindowFrames";

fn to_rect(t: (f64, f64, f64, f64)) -> Rect {
    Rect { x: t.0 as f32, y: t.1 as f32, w: t.2 as f32, h: t.3 as f32 }
}

impl App {
    fn outer_rect(g: &Gpu) -> Rect {
        let p = g.window.outer_position().unwrap_or_default();
        let s = g.window.outer_size();
        Rect { x: p.x as f32, y: p.y as f32, w: s.width as f32, h: s.height as f32 }
    }

    /// Where a new window goes: New window placement, the profile's saved slot, and its
    /// Screen setting.
    pub(super) fn place_window(&mut self) {
        use crate::placement::{self, Rect as R};
        let guid = self.tabs.first().and_then(|t| self.panes.get(&t.focused)).map(|p| self.store.shared_guid(&p.profile)).unwrap_or_else(|| self.store.default_guid.clone());
        let slot = self.slots.take(&guid);
        self.win_slot = Some((guid.clone(), slot));
        let monitors: Vec<R> = super::requests_ui::work_areas().into_iter().map(to_rect).collect();
        let Some(g) = self.gpu.as_ref() else { return };
        let current = Self::outer_rect(g);
        let others: Vec<R> = self.windows.parked().filter_map(|(_, s)| s.gpu.as_ref().map(Self::outer_rect)).collect();
        let inside = |r: &R, x: f32, y: f32| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        let window_monitor = monitors.iter().position(|m| inside(m, current.x + current.w / 2.0, current.y + current.h / 2.0)).unwrap_or(0);
        let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) };
        let cursor_monitor = monitors.iter().position(|m| inside(m, pt.x as f32, pt.y as f32)).unwrap_or(0);
        let profile = self.store.get(&guid).unwrap_or_else(|| self.store.default_profile());
        let pref = profile.get("Screen").and_then(|v| v.as_integer()).unwrap_or(-1);
        let work = placement::screen_for(pref, &monitors, window_monitor, cursor_monitor);
        let saved = self.globals.table(SAVED_FRAMES).get(&placement::slot_key(&guid, slot)).and_then(|v| v.as_array()).and_then(|a| {
            let n: Vec<f32> = a.iter().filter_map(|x| x.as_float().or_else(|| x.as_integer().map(|i| i as f64))).map(|f| f as f32).collect();
            (n.len() == 4).then(|| R { x: n[0], y: n[1], w: n[2], h: n[3] })
        });
        let policy = self.globals.get("WindowPlacement").and_then(|v| v.as_integer()).unwrap_or(placement::POSITION);
        let count = self.registry.order().len();
        let target = placement::place(policy, saved, count, work, &others, current, &monitors).or_else(|| {
            // A Screen preference moves even a window Windows placed.
            (pref != -1).then(|| placement::move_to(current, monitors.get(window_monitor).copied().unwrap_or(work), work))
        });
        let Some(r) = target else { return };
        let w = &g.window;
        w.set_outer_position(PhysicalPosition::new(r.x.round() as i32, r.y.round() as i32));
        if (r.w, r.h) != (current.w, current.h) {
            let (outer, inner) = (w.outer_size(), w.inner_size());
            let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
            let _ = w.request_inner_size(PhysicalSize::new((r.w as u32).saturating_sub(dw).max(1), (r.h as u32).saturating_sub(dh).max(1)));
        }
    }

    /// Keeps the current window's frame for its profile slot (when it closes, or at quit).
    pub(super) fn save_window_frame(&mut self) {
        let Some((guid, slot)) = self.win_slot.take() else { return };
        self.slots.give(&guid, slot);
        let Some(g) = self.gpu.as_ref() else { return };
        if g.window.is_minimized() == Some(true) || g.window.fullscreen().is_some() {
            return;
        }
        let r = Self::outer_rect(g);
        let mut t = self.globals.table(SAVED_FRAMES);
        t.insert(crate::placement::slot_key(&guid, slot), toml::Value::Array([r.x, r.y, r.w, r.h].iter().map(|v| toml::Value::Float(*v as f64)).collect()));
        self.globals.set(SAVED_FRAMES, toml::Value::Table(t));
        self.save_globals();
    }

    /// Quit: every window keeps its frame.
    pub(super) fn save_all_window_frames(&mut self) {
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_of_key(k)).collect();
        for id in ids {
            if self.enter(id) {
                self.save_window_frame();
            }
        }
    }
}
