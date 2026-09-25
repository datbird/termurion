//! Many terminal windows. The current window's state lives in `App`'s own fields; every
//! other window's state waits in a `WinSlot`, and `enter` swaps a window in before its
//! events run or its frame is drawn.

use super::*;

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

    /// Every terminal window's id, by number.
    pub(super) fn window_ids(&self) -> Vec<WindowId> {
        let mut ids = Vec::new();
        for k in self.registry.order() {
            if self.windows.current() == Some(k) {
                ids.extend(self.current_window());
            } else if let Some(id) = self.windows.parked().find(|(kk, _)| **kk == k).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id())) {
                ids.push(id);
            }
        }
        ids
    }

    /// Opens a terminal window with one tab of `guid` (the default profile when None).
    pub(super) fn open_window(&mut self, event_loop: &ActiveEventLoop, guid: Option<&str>) -> Result<WindowId, String> {
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
        self.new_tab(guid);
        self.install_menus();
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
                self.pending_windows.push(Some(guid.to_string()));
                self.redraw();
            }
        }
    }
}
