//! The Tandem window: tabs of split panes, drawn with wgpu.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::Selection;
use alacritty_terminal::term::{TermMode, viewport_to_point};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key as WKey, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

use crate::conpty::{self, Api, Source};
use crate::input::{self, Clicks, Wheel};
use crate::keys::{self, Action, Key};
use crate::layout::{Axis, Layout, PaneId, Rect};
use crate::mouse::{self, Button, Mods, Tracking};
use crate::pane::Pane;
use crate::pty::Notify;
use crate::palette::Palette;
use crate::profile::{self, Profile};
use crate::render::Renderer;
use crate::screen::Look;
use crate::store::{self, Store};
use crate::scene::{self, Frame, rgba};
use crate::settings_ui::{Edit, How, Mode, SettingsWindow};

const FONT: &str = "Cascadia Mono";
/// 12 pt, iTerm2's default size, at 96 dpi.
const FONT_PX: f32 = 16.0;
/// Space between a pane's edge and its text, and the divider width, in pixels at 100%.
const PAD: f32 = 4.0;
const GAP: f32 = 1.0;
/// Lines scrolled per wheel notch.
const WHEEL_LINES: f32 = 3.0;
/// Half a cursor blink cycle.
const BLINK: std::time::Duration = std::time::Duration::from_millis(530);
/// How long after the last settings edit the file is written.
const SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

mod menu;

pub enum UserEvent {
    /// A pane has output, or its shell exited.
    Wake,
    /// A Win chord from the keyboard hook.
    Chord(crate::menus::Chord),
    /// A menu item was clicked (its id).
    Menu(String),
}

struct Tab {
    layout: Layout,
    focused: PaneId,
    /// Maximize Active Pane: this pane fills the tab.
    zoomed: Option<PaneId>,
}

enum Drag {
    None,
    Select { pane: PaneId },
    Divider { path: Vec<bool>, axis: Axis, parent: Rect },
    App { pane: PaneId, button: Button },
}

struct Gpu {
    surface: wgpu::Surface<'static>,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    instance: wgpu::Instance,
    // Dropped last, after the surface.
    window: Arc<Window>,
}

pub struct App {
    notify: Notify,
    proxy: Arc<Mutex<EventLoopProxy<UserEvent>>>,
    menu_bar: Option<menu::MenuBar>,
    /// Make Text Bigger and Smaller, in points, on top of the profile font size.
    font_offset: f32,
    api: Api,
    shell: String,
    gpu: Option<Gpu>,
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    next_id: PaneId,
    mods: Mods,
    cursor: (f32, f32),
    drag: Drag,
    clicks: Clicks,
    wheel_acc: f32,
    focused: bool,
    tab_rects: Vec<Rect>,
    banners: Banners,
    clipboard: Option<arboard::Clipboard>,
    store: Store,
    settings: Option<SettingsWindow>,
    profiles_win: Option<SettingsWindow>,
    /// When unsaved settings edits are due to be written.
    save_due: Option<Instant>,
    /// The font settings the renderer last loaded.
    font_sig: String,
    /// Cursor blink phase, and when it last flipped.
    blink_on: bool,
    last_blink: Instant,
}

/// pwsh.exe when it is on the PATH, else Windows PowerShell.
fn default_shell() -> String {
    let found = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join("pwsh.exe").is_file()))
        .unwrap_or(false);
    if found { "pwsh.exe -NoLogo".into() } else { "powershell.exe -NoLogo".into() }
}

fn message_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_OK | MB_ICONERROR) };
}

pub fn run() {
    let event_loop = match EventLoop::<UserEvent>::with_user_event().build() {
        Ok(l) => l,
        Err(e) => return message_box(&format!("Could not start the event loop: {e}")),
    };
    let proxy = Arc::new(Mutex::new(event_loop.create_proxy()));
    let wake = proxy.clone();
    let notify: Notify = Arc::new(move || {
        let _ = wake.lock().unwrap().send_event(UserEvent::Wake);
    });

    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).unwrap_or_default();
    let mut banners = Banners::default();
    let api = match conpty::load(&Source::Shipped(exe_dir)) {
        Ok(api) => api,
        Err(e) => match conpty::load(&Source::Inbox) {
            Ok(api) => {
                banners.set("conpty", format!("Shipped ConPTY did not load ({e}). Using the Windows copy."));
                api
            }
            Err(e2) => return message_box(&format!("No ConPTY: {e}; {e2}")),
        },
    };

    let path = store::default_path();
    let store = match Store::load(&path) {
        Ok(s) => s,
        Err(e) => {
            // Never overwrite a settings file we could not read: work in a side file instead,
            // and keep the edits already made there in an earlier session.
            let side = path.with_extension("toml.recovered");
            let store = Store::load(&side).unwrap_or_else(|_| Store::fresh(&side));
            banners.set("settings", format!("Settings not loaded ({e}). Changes save to {}.", side.display()));
            store
        }
    };
    let mut app = App {
        notify,
        proxy,
        menu_bar: None,
        font_offset: 0.0,
        api,
        shell: default_shell(),
        gpu: None,
        tabs: Vec::new(),
        active: 0,
        panes: HashMap::new(),
        next_id: 1,
        mods: Mods::default(),
        cursor: (0.0, 0.0),
        drag: Drag::None,
        clicks: Clicks::default(),
        wheel_acc: 0.0,
        focused: true,
        tab_rects: Vec::new(),
        banners,
        clipboard: arboard::Clipboard::new().ok(),
        store,
        settings: None,
        save_due: None,
        font_sig: String::new(),
        profiles_win: None,
        blink_on: true,
        last_blink: Instant::now(),
    };
    if let Err(e) = event_loop.run_app(&mut app) {
        message_box(&format!("Tandem stopped: {e}"));
    }
}

impl App {
    fn scale(&self) -> f32 {
        self.gpu.as_ref().map_or(1.0, |g| g.window.scale_factor() as f32)
    }

    fn cell(&self) -> (f32, f32) {
        self.gpu.as_ref().map_or((8.0, 16.0), |g| g.renderer.cell)
    }

    fn tab_height(&self) -> f32 {
        if self.tabs.len() > 1 { (self.cell().1 * 1.6).round() } else { 0.0 }
    }

    /// The area the active tab's panes share.
    fn content_area(&self) -> Rect {
        let (w, h) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        let top = self.tab_height();
        let bottom = if self.banners.text().is_some() { self.cell().1 + 8.0 } else { 0.0 };
        Rect { x: 0.0, y: top, w, h: (h - top - bottom).max(1.0) }
    }

    /// The text area inside a pane rectangle.
    fn inner(&self, rect: Rect) -> Rect {
        rect.inset(PAD * self.scale())
    }

    fn grid_size(&self, rect: Rect) -> (u16, u16) {
        let (cw, ch) = self.cell();
        let inner = self.inner(rect);
        ((inner.w / cw).floor().max(2.0) as u16, (inner.h / ch).floor().max(1.0) as u16)
    }

    /// The profile with this Guid, or the default profile.
    fn profile(&self, guid: Option<&str>) -> Profile {
        guid.and_then(|g| self.store.get(g)).unwrap_or_else(|| self.store.default_profile()).clone()
    }

    fn dark(&self) -> bool {
        self.gpu.as_ref().and_then(|g| g.window.theme()).is_none_or(|t| t == winit::window::Theme::Dark)
    }

    fn start_pane(&mut self, rect: Rect, guid: Option<&str>) -> Option<PaneId> {
        let id = self.next_id;
        let started = self.start_pane_as(id, rect, guid);
        if started {
            self.next_id += 1;
        }
        started.then_some(id)
    }

    /// Restart Session: a new shell in the same pane, with the same profile.
    fn restart_pane(&mut self, id: PaneId) {
        let Some(rect) = self.pane_rect(id) else { return };
        let guid = self.panes.get(&id).map(|p| p.profile.clone());
        self.panes.remove(&id);
        self.start_pane_as(id, rect, guid.as_deref());
    }

    fn start_pane_as(&mut self, id: PaneId, rect: Rect, guid: Option<&str>) -> bool {
        let (cols, rows) = self.grid_size(rect);
        let p = self.profile(guid);
        let cmd = profile::command_line(&p, &self.shell);
        let (cwd, warning) = profile::start_dir(&p);
        match warning {
            Some(w) => self.banners.set("directory", w),
            None => self.banners.clear("directory"),
        }
        match Pane::start(id, &self.api, &cmd, Some(&cwd), cols, rows, self.notify.clone()) {
            Ok(mut pane) => {
                pane.screen.set_palette(Palette::from_profile(&p, self.dark()));
                pane.screen.set_look(Look::from_profile(&p));
                pane.profile = p.guid();
                self.panes.insert(id, pane);
                true
            }
            Err(e) => {
                self.banners.set("start", format!("Could not start {cmd}: {e}"));
                false
            }
        }
    }

    fn open_settings(&mut self, event_loop: &ActiveEventLoop, mode: Mode) {
        let slot = match mode {
            Mode::Settings => &self.settings,
            Mode::Profiles => &self.profiles_win,
        };
        if let Some(s) = slot {
            return s.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match SettingsWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, mode) {
            Ok(s) => match mode {
                Mode::Settings => self.settings = Some(s),
                Mode::Profiles => self.profiles_win = Some(s),
            },
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Saves after an edit in the settings window and applies it to every open pane.
    fn apply_edit(&mut self, edit: Edit) {
        match edit {
            Edit::None => return,
            Edit::Open(guid, how) => match how {
                // Separate windows arrive in milestone 7; until then a new window opens a tab.
                How::Tab | How::Window => self.new_tab(Some(&guid)),
                How::SplitRight => self.split(Axis::Row, Some(&guid)),
                How::SplitDown => self.split(Axis::Column, Some(&guid)),
            },
            Edit::Error(e) => self.banners.set("import", e),
            Edit::Changed(_) | Edit::Imported(_) => {
                // Saving waits for a pause in editing, so a color drag does not write the file every frame.
                self.save_due = Some(Instant::now() + SAVE_DELAY);
                self.refresh_panes();
                self.apply_fonts();
            }
        }
        self.relayout();
    }

    /// Rebuilds every pane's colors and cursor from its profile and the system appearance.
    fn refresh_panes(&mut self) {
        let dark = self.dark();
        for pane in self.panes.values_mut() {
            let p = self.store.get(&pane.profile).unwrap_or_else(|| self.store.default_profile());
            pane.screen.set_palette(Palette::from_profile(p, dark));
            pane.screen.set_look(Look::from_profile(p));
        }
        self.redraw();
    }

    fn save_now(&mut self) {
        self.save_due = None;
        match self.store.save() {
            Ok(()) => self.banners.clear("save"),
            Err(e) => self.banners.set("save", format!("Could not save settings: {e}")),
        }
    }

    /// Takes the renderer's fonts from the default profile. A font that is not
    /// installed is named in the banner.
    fn apply_fonts(&mut self) {
        let p = self.store.default_profile().clone();
        let mut normal = p.font("Normal Font");
        let mut alt = p.bool("Use Non-ASCII Font").then(|| p.font("Non Ascii Font").family);
        if let Some(g) = self.gpu.as_ref() {
            normal.family = g.renderer.resolve_family(&normal.family);
            alt = alt.map(|a| g.renderer.resolve_family(&a));
        }
        normal.size_pt = (normal.size_pt + self.font_offset).clamp(4.0, 128.0);
        let sig = format!("{normal:?} {alt:?} {} {} {}", p.float("Horizontal Spacing"), p.float("Vertical Spacing"), self.scale());
        // Re-measuring the font re-shapes all text, so only do it when a font setting changed.
        if sig == self.font_sig {
            return;
        }
        self.font_sig = sig;
        let Some(g) = self.gpu.as_mut() else { return };
        let px = normal.size_pt * 4.0 / 3.0 * g.window.scale_factor() as f32;
        g.renderer.set_fonts(&normal.family, alt.as_deref(), px, p.float("Horizontal Spacing") as f32, p.float("Vertical Spacing") as f32);
        let missing: Vec<String> = std::iter::once(normal.family).chain(alt).filter(|f| !g.renderer.has_family(f)).collect();
        if missing.is_empty() {
            self.banners.clear("font");
        } else {
            self.banners.set("font", format!("Font not installed: {}. Text uses a fallback font.", missing.join(", ")));
        }
    }

    /// Sizes the window to the default profile's Columns and Rows.
    fn size_window(&mut self) {
        let p = self.store.default_profile().clone();
        let (cw, ch) = self.cell();
        let pad = 2.0 * PAD * self.scale();
        let (w, h) = (p.int("Columns").max(10) as f32 * cw + pad, p.int("Rows").max(4) as f32 * ch + pad);
        if let Some(g) = self.gpu.as_ref() {
            let _ = g.window.request_inner_size(winit::dpi::PhysicalSize::new(w.ceil() as u32, h.ceil() as u32));
        }
    }

    fn new_tab(&mut self, guid: Option<&str>) {
        let area = self.content_area();
        if let Some(id) = self.start_pane(area, guid) {
            self.tabs.push(Tab { layout: Layout::new(id), focused: id, zoomed: None });
            self.active = self.tabs.len() - 1;
            self.relayout();
        }
    }

    fn split(&mut self, axis: Axis, guid: Option<&str>) {
        let Some(tab) = self.tabs.get(self.active) else { return };
        let target = tab.focused;
        let area = self.content_area();
        let Some((_, rect)) = tab.layout.rects(area, GAP).into_iter().find(|(id, _)| *id == target) else { return };
        let half = match axis {
            Axis::Row => Rect { w: rect.w / 2.0, ..rect },
            Axis::Column => Rect { h: rect.h / 2.0, ..rect },
        };
        if let Some(id) = self.start_pane(half, guid) {
            let tab = &mut self.tabs[self.active];
            tab.layout.split(target, axis, id);
            tab.focused = id;
            tab.zoomed = None;
            self.relayout();
        }
    }

    /// Closes a pane. The last pane closes its tab, and the last tab exits.
    fn close_pane(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        self.panes.remove(&id);
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) {
            let tab = &mut self.tabs[t];
            if tab.layout.remove(id) {
                if tab.focused == id {
                    tab.focused = tab.layout.panes()[0];
                }
            } else {
                self.tabs.remove(t);
                self.active = active_after_close(self.active, t, self.tabs.len());
            }
        }
        if self.tabs.is_empty() {
            event_loop.exit();
            return;
        }
        self.relayout();
    }

    fn relayout(&mut self) {
        let area = self.content_area();
        let (cw, ch) = self.cell();
        let _ = area;
        let sizes: Vec<(PaneId, (u16, u16))> = self
            .tabs
            .iter()
            .flat_map(|t| self.visible(t))
            .map(|(id, rect)| (id, self.grid_size(rect)))
            .collect();
        for (id, (cols, rows)) in sizes {
            if let Some(p) = self.panes.get_mut(&id) {
                p.resize(cols, rows);
                p.screen.set_cell_px(cw as u16, ch as u16);
            }
        }
        self.redraw();
    }

    fn redraw(&self) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }

    fn focused_pane(&mut self) -> Option<&mut Pane> {
        let id = self.tabs.get(self.active)?.focused;
        self.panes.get_mut(&id)
    }

    fn pane_rect(&self, id: PaneId) -> Option<Rect> {
        let tab = self.tabs.get(self.active)?;
        self.visible(tab).into_iter().find(|(p, _)| *p == id).map(|(_, r)| r)
    }

    fn pane_at(&self, x: f32, y: f32) -> Option<(PaneId, Rect)> {
        let tab = self.tabs.get(self.active)?;
        self.visible(tab).into_iter().find(|(_, r)| r.contains(x, y))
    }

    /// The cell under (x, y) in pane `rect`, and which half of the cell it is in.
    fn cell_at(&self, id: PaneId, rect: Rect, x: f32, y: f32) -> (u32, u32, Side) {
        let (cw, ch) = self.cell();
        let inner = self.inner(rect);
        let term = self.panes[&id].screen.term();
        let (col, row) = mouse::cell_at((x - inner.x) as f64, (y - inner.y) as f64, cw as f64, ch as f64, term.columns() as u32, term.screen_lines() as u32);
        let side = if (x - inner.x) - col as f32 * cw > cw / 2.0 { Side::Right } else { Side::Left };
        (col, row, side)
    }

    fn grid_point(&self, id: PaneId, col: u32, row: u32) -> Point {
        let offset = self.panes[&id].screen.term().grid().display_offset();
        viewport_to_point(offset, Point::new(row as usize, Column(col as usize)))
    }

    fn copy_selection(&mut self, id: PaneId) {
        let Some(text) = self.panes.get(&id).and_then(|p| p.screen.term().selection_to_string()) else { return };
        if let Some(cb) = &mut self.clipboard {
            if let Err(e) = cb.set_text(text) {
                self.banners.set("clipboard", format!("Copy failed: {e}"));
            }
        }
    }

    fn paste(&mut self) {
        let text = match self.clipboard.as_mut().map(|cb| cb.get_text()) {
            Some(Ok(t)) => t,
            Some(Err(e)) => {
                self.banners.set("clipboard", format!("Paste failed: {e}"));
                return self.redraw();
            }
            None => return,
        };
        if let Some(p) = self.focused_pane() {
            p.paste(&text);
        }
    }

    fn action(&mut self, action: Action, event_loop: &ActiveEventLoop) {
        let n = self.tabs.len();
        match action {
            Action::NewTab => self.new_tab(None),
            Action::ClosePane => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
                    self.close_pane(id, event_loop);
                }
            }
            Action::SplitRight => self.split(Axis::Row, None),
            Action::SplitDown => self.split(Axis::Column, None),
            Action::NextPane | Action::PrevPane => {
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    let panes = tab.layout.panes();
                    let i = panes.iter().position(|p| *p == tab.focused).unwrap_or(0);
                    let step = if action == Action::NextPane { 1 } else { panes.len() - 1 };
                    tab.focused = panes[(i + step) % panes.len()];
                }
            }
            Action::Focus(dir) => {
                let area = self.content_area();
                if let Some(tab) = self.tabs.get_mut(self.active) {
                    if let Some(id) = tab.layout.neighbor(tab.focused, dir, area) {
                        tab.focused = id;
                    }
                }
            }
            Action::NextTab if n > 0 => self.active = (self.active + 1) % n,
            Action::PrevTab if n > 0 => self.active = (self.active + n - 1) % n,
            Action::SelectTab(i) if i < n => self.active = i,
            Action::Copy => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.focused) {
                    self.copy_selection(id);
                }
            }
            Action::Paste => self.paste(),
            Action::ScrollPageUp => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageUp)),
            Action::ScrollPageDown => self.focused_pane().map_or((), |p| p.scroll(Scroll::PageDown)),
            Action::ScrollTop => self.focused_pane().map_or((), |p| p.scroll(Scroll::Top)),
            Action::ScrollBottom => self.focused_pane().map_or((), |p| p.scroll(Scroll::Bottom)),
            Action::OpenSettings => self.open_settings(event_loop, Mode::Settings),
            Action::OpenProfiles => self.open_settings(event_loop, Mode::Profiles),
            _ => {}
        }
        self.relayout();
    }

    fn key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) {
        if ev.state != ElementState::Pressed {
            return;
        }
        // Typing shows the cursor at once and restarts the blink.
        self.blink_on = true;
        self.last_blink = Instant::now();
        let base = to_key(&ev.key_without_modifiers(), None);
        if let Some(action) = base.as_ref().and_then(|b| keys::shortcut(b, self.mods)) {
            return self.action(action, event_loop);
        }
        let Some(key) = to_key(&ev.logical_key, ev.text.as_deref()) else { return };
        let mods = self.mods;
        let Some(pane) = self.focused_pane() else { return };
        let app_cursor = pane.screen.term().mode().contains(TermMode::APP_CURSOR);
        if let Some(bytes) = keys::press_bytes(&key, base.as_ref(), ev.text.as_deref(), mods, app_cursor) {
            pane.scroll(Scroll::Bottom);
            pane.input(&bytes);
        }
    }

    fn mouse_press(&mut self, button: MouseButton) {
        let (x, y) = self.cursor;
        if let Some(i) = tab_hit(&self.tab_rects, self.tabs.len(), x, y) {
            self.active = i;
            return self.redraw();
        }
        if button == MouseButton::Left {
            let area = self.content_area();
            let grab = 3.0 * self.scale();
            if let Some(d) = self.tabs.get(self.active).and_then(|t| {
                t.layout.dividers(area, GAP).into_iter().find(|d| {
                    Rect { x: d.rect.x - grab, y: d.rect.y - grab, w: d.rect.w + 2.0 * grab, h: d.rect.h + 2.0 * grab }.contains(x, y)
                })
            }) {
                self.drag = Drag::Divider { path: d.path, axis: d.axis, parent: d.parent };
                return;
            }
        }
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        self.tabs[self.active].focused = id;
        let (col, row, side) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let modes = self.panes[&id].screen.mouse_modes();
        let mb = match button {
            MouseButton::Left => Button::Left,
            MouseButton::Middle => Button::Middle,
            MouseButton::Right => Button::Right,
            _ => return,
        };
        if mouse::app_owns_mouse(modes, mods) {
            let ev = mouse::Event { button: mb, action: mouse::Action::Press, mods, col, row };
            if let Some(bytes) = mouse::encode(modes, ev) {
                self.panes.get_mut(&id).unwrap().input(&bytes);
            }
            self.drag = Drag::App { pane: id, button: mb };
            return self.redraw();
        }
        match mb {
            Button::Left => {
                let count = self.clicks.press(Instant::now(), col, row);
                let point = self.grid_point(id, col, row);
                let ty = input::selection_type(count, mods);
                self.panes.get_mut(&id).unwrap().screen.term_mut().selection = Some(Selection::new(ty, point, side));
                self.drag = Drag::Select { pane: id };
            }
            Button::Middle => self.paste(),
            Button::Right => self.show_context_menu(),
            _ => {}
        }
        self.redraw();
    }

    fn mouse_release(&mut self) {
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Select { pane } => {
                let empty = self.panes.get(&pane).and_then(|p| p.screen.term().selection_to_string()).map_or(true, |s| s.is_empty());
                if empty {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.screen.term_mut().selection = None;
                    }
                } else {
                    // iTerm2 copies a selection as soon as it is made.
                    self.copy_selection(pane);
                }
            }
            Drag::App { pane, button } => {
                let (x, y) = self.cursor;
                // Look the pane up by id: the button may come up outside it, and the app
                // must still get the release (cell_at clamps to the pane's edge).
                if let Some(rect) = self.pane_rect(pane) {
                    let (col, row, _) = self.cell_at(pane, rect, x, y);
                    let p = self.panes.get_mut(&pane).unwrap();
                    let modes = p.screen.mouse_modes();
                    let ev = mouse::Event { button, action: mouse::Action::Release, mods: self.mods, col, row };
                    if let Some(bytes) = mouse::encode(modes, ev) {
                        p.input(&bytes);
                    }
                }
            }
            _ => {}
        }
        self.redraw();
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        match &self.drag {
            Drag::Select { pane } => {
                let pane = *pane;
                let Some(rect) = self.pane_rect(pane) else { return };
                let (col, row, side) = self.cell_at(pane, rect, x, y);
                let point = self.grid_point(pane, col, row);
                if let Some(sel) = self.panes.get_mut(&pane).and_then(|p| p.screen.term_mut().selection.as_mut()) {
                    sel.update(point, side);
                }
                self.redraw();
            }
            Drag::Divider { path, axis, parent } => {
                let ratio = match axis {
                    Axis::Row => (x - parent.x) / parent.w,
                    Axis::Column => (y - parent.y) / parent.h,
                };
                let path = path.clone();
                self.tabs[self.active].layout.set_ratio(&path, ratio);
                self.relayout();
            }
            Drag::App { pane, button } => {
                let (pane, button) = (*pane, *button);
                self.send_motion(pane, Some(button), x, y);
            }
            Drag::None => {
                if let Some((id, _)) = self.pane_at(x, y) {
                    if self.panes[&id].screen.mouse_modes().tracking == Tracking::Any {
                        self.send_motion(id, None, x, y);
                    }
                }
            }
        }
    }

    fn send_motion(&mut self, pane: PaneId, button: Option<Button>, x: f32, y: f32) {
        let Some(rect) = self.pane_rect(pane) else { return };
        let (col, row, _) = self.cell_at(pane, rect, x, y);
        let p = self.panes.get_mut(&pane).unwrap();
        let modes = p.screen.mouse_modes();
        let ev = mouse::Event { button: button.unwrap_or(Button::None), action: mouse::Action::Motion, mods: self.mods, col, row };
        if let Some(bytes) = mouse::encode(modes, ev) {
            p.input(&bytes);
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let ch = self.cell().1;
        self.wheel_acc += match delta {
            MouseScrollDelta::LineDelta(_, y) => y * WHEEL_LINES,
            MouseScrollDelta::PixelDelta(p) => p.y as f32 / ch,
        };
        let lines = self.wheel_acc.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.wheel_acc -= lines as f32;
        let (x, y) = self.cursor;
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let p = self.panes.get_mut(&id).unwrap();
        let mode = *p.screen.term().mode();
        let modes = p.screen.mouse_modes();
        match input::wheel(modes, mods, mode.contains(TermMode::ALT_SCREEN), mode.contains(TermMode::APP_CURSOR), lines, col, row) {
            Wheel::Send(bytes) => p.input(&bytes),
            Wheel::Scroll(n) => p.scroll(Scroll::Delta(n)),
        }
        self.redraw();
    }

    fn build_frame(&mut self) -> Frame {
        let cell = self.cell();
        let palette_bg = self.panes.values().next().map_or(crate::palette::Palette::default().bg, |p| p.screen.palette.bg);
        let mut frame = Frame { clear: rgba(palette_bg, 1.0), ..Default::default() };
        let (w, _) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        self.tab_rects.clear();
        if self.tabs.len() > 1 {
            let titles: Vec<String> = self
                .tabs
                .iter()
                .map(|t| self.panes.get(&t.focused).and_then(|p| p.screen.title()).unwrap_or("Shell").to_string())
                .collect();
            let bar = Rect { x: 0.0, y: 0.0, w, h: self.tab_height() };
            self.tab_rects = scene::tab_bar(&mut frame, &titles, self.active, bar, cell);
        }
        let area = self.content_area();
        if let Some(tab) = self.tabs.get(self.active) {
            for (id, rect) in self.visible(tab) {
                if let Some(p) = self.panes.get(&id) {
                    scene::pane(&mut frame, &p.screen, self.inner(rect), cell, self.focused && id == tab.focused, self.blink_on);
                }
            }
            if tab.zoomed.is_none() {
                scene::dividers(&mut frame, &tab.layout.dividers(area, GAP));
            }
        }
        let error = self.banners.text().or_else(|| self.panes.values().find_map(|p| p.error.clone()));
        if let Some(text) = error {
            let (w, h) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
            scene::banner(&mut frame, &text, Rect { x: 0.0, y: 0.0, w, h }, cell);
        }
        frame
    }

    fn render(&mut self) {
        let frame = self.build_frame();
        let title = self
            .tabs
            .get(self.active)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| p.screen.title())
            .unwrap_or("Tandem")
            .to_string();
        let Some(gpu) = self.gpu.as_mut() else { return };
        gpu.window.set_title(&title);
        let texture = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => t,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => return,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                gpu.window.request_redraw();
                return;
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                match gpu.instance.create_surface(gpu.window.clone()) {
                    Ok(s) => gpu.surface = s,
                    Err(e) => self.banners.set("gpu", format!("Lost the window surface: {e}")),
                }
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.surface.configure(&gpu.device, &gpu.config);
                    gpu.window.request_redraw();
                }
                return;
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                self.banners.set("gpu", "GPU validation error");
                return;
            }
        };
        let view = texture.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let (w, h) = (gpu.config.width, gpu.config.height);
        if let Err(e) = gpu.renderer.draw(&frame, &view, w, h) {
            self.banners.set("gpu", e);
        }
        if let Some(gpu) = self.gpu.as_ref() {
            gpu.queue.present(texture);
        }
    }
}

fn to_key(key: &WKey, text: Option<&str>) -> Option<Key> {
    Some(match key {
        WKey::Named(n) => match n {
            NamedKey::Enter => Key::Enter,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Insert => Key::Insert,
            NamedKey::Delete => Key::Delete,
            NamedKey::Space => Key::Text(" ".into()),
            NamedKey::F1 => Key::F(1),
            NamedKey::F2 => Key::F(2),
            NamedKey::F3 => Key::F(3),
            NamedKey::F4 => Key::F(4),
            NamedKey::F5 => Key::F(5),
            NamedKey::F6 => Key::F(6),
            NamedKey::F7 => Key::F(7),
            NamedKey::F8 => Key::F(8),
            NamedKey::F9 => Key::F(9),
            NamedKey::F10 => Key::F(10),
            NamedKey::F11 => Key::F(11),
            NamedKey::F12 => Key::F(12),
            _ => return None,
        },
        WKey::Character(s) => Key::Text(s.to_string()),
        _ => return text.map(|t| Key::Text(t.to_string())),
    })
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("Tandem").with_inner_size(LogicalSize::new(1000.0, 650.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                message_box(&format!("Could not open a window: {e}"));
                return event_loop.exit();
            }
        };
        match init_gpu(window, event_loop) {
            Ok(gpu) => self.gpu = Some(gpu),
            Err(e) => {
                message_box(&format!("Could not start the GPU renderer: {e}"));
                return event_loop.exit();
            }
        }
        self.apply_fonts();
        self.size_window();
        self.new_tab(None);
        self.install_menus();
    }

    /// Runs the cursor blink when the focused pane's profile asks for it.
    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.save_due.is_some_and(|t| Instant::now() >= t) {
            self.save_now();
        }
        let blinks = self
            .tabs
            .get(self.active)
            .and_then(|t| self.panes.get(&t.focused))
            .and_then(|p| self.store.get(&p.profile))
            .is_some_and(|p| p.bool("Blinking Cursor"));
        if !blinks || !self.focused {
            if !self.blink_on {
                self.blink_on = true;
                self.redraw();
            }
            match self.save_due {
                Some(t) => event_loop.set_control_flow(ControlFlow::WaitUntil(t)),
                None => event_loop.set_control_flow(ControlFlow::Wait),
            }
            return;
        }
        if Instant::now() >= self.last_blink + BLINK {
            self.blink_on = !self.blink_on;
            self.last_blink = Instant::now();
            self.redraw();
        }
        let next = self.save_due.map_or(self.last_blink + BLINK, |t| t.min(self.last_blink + BLINK));
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Chord(c) => return self.run_chord(&c, event_loop),
            UserEvent::Menu(id) => return self.run_menu_id(&id, event_loop),
            UserEvent::Wake => {}
        }
        let mut changed = false;
        let mut exited = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            changed |= p.pump();
            if p.exited.is_some() {
                exited.push(*id);
            }
        }
        for id in exited {
            self.close_pane(id, event_loop);
        }
        if changed {
            self.sync_checks();
            self.redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        for profiles in [false, true] {
            let slot = if profiles { &mut self.profiles_win } else { &mut self.settings };
            if !slot.as_ref().is_some_and(|s| s.id() == id) {
                continue;
            }
            match event {
                WindowEvent::CloseRequested => *slot = None,
                WindowEvent::RedrawRequested => {
                    let edit = slot.as_mut().unwrap().redraw(&mut self.store);
                    self.apply_edit(edit);
                }
                other => {
                    slot.as_mut().unwrap().event(&other);
                }
            }
            return;
        }
        match event {
            WindowEvent::ThemeChanged(_) => self.refresh_panes(),
            WindowEvent::CloseRequested => {
                if self.save_due.is_some() {
                    self.save_now();
                }
                self.panes.clear();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                // Minimizing reports 0x0. Keep every pty at its size, or ConPTY reflows the screen.
                let Some((w, h)) = usable_size(size.width, size.height) else { return };
                if let Some(g) = self.gpu.as_mut() {
                    g.config.width = w;
                    g.config.height = h;
                    g.surface.configure(&g.device, &g.config);
                }
                self.relayout();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let _ = scale_factor;
                self.apply_fonts();
                self.relayout();
            }
            WindowEvent::Focused(f) => {
                self.focused = f;
                // Focus reporting (DECSET 1004): tell the app the window gained or lost focus.
                if let Some(p) = self.focused_pane() {
                    if p.screen.term().mode().contains(TermMode::FOCUS_IN_OUT) {
                        p.input(if f { b"\x1b[I" } else { b"\x1b[O" });
                    }
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.mods = Mods { shift: s.shift_key(), alt: s.alt_key(), ctrl: s.control_key(), cmd: s.super_key() };
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, event_loop),
            WindowEvent::CursorMoved { position, .. } => self.mouse_move(position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => self.mouse_press(button),
            WindowEvent::MouseInput { state: ElementState::Released, .. } => self.mouse_release(),
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }
}

fn init_gpu(window: Arc<Window>, event_loop: &ActiveEventLoop) -> Result<Gpu, String> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
        event_loop.owned_display_handle(),
    )));
    let surface = instance.create_surface(window.clone()).map_err(|e| e.to_string())?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .map_err(|e| e.to_string())?;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).map_err(|e| e.to_string())?;
    let caps = surface.get_capabilities(&adapter);
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
    surface.configure(&device, &config);
    let renderer = Renderer::new(&device, &queue, format, FONT, FONT_PX * window.scale_factor() as f32);
    Ok(Gpu { surface, adapter, device, queue, config, renderer, instance, window })
}

/// The active tab index after tab `closed` is removed, leaving `remaining` tabs.
fn active_after_close(active: usize, closed: usize, remaining: usize) -> usize {
    let active = if closed < active { active - 1 } else { active };
    active.min(remaining.saturating_sub(1))
}

/// The surface size to use, or None while the window is minimized (0 wide or high).
fn usable_size(w: u32, h: u32) -> Option<(u32, u32)> {
    (w > 0 && h > 0).then_some((w, h))
}

/// Moves the tab `new_tab` just added (the last one) to just after `active`, and returns
/// its new index. None when the tab count did not grow past `before`: nothing opened.
fn move_new_tab<T>(tabs: &mut Vec<T>, before: usize, active: usize) -> Option<usize> {
    if tabs.len() <= before {
        return None;
    }
    let tab = tabs.pop()?;
    let at = (active + 1).min(tabs.len());
    tabs.insert(at, tab);
    Some(at)
}

/// The tab under (x, y), if it still exists.
fn tab_hit(rects: &[Rect], tabs: usize, x: f32, y: f32) -> Option<usize> {
    rects.iter().position(|r| r.contains(x, y)).filter(|i| *i < tabs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_an_earlier_tab_keeps_the_same_tab_active() {
        // On tab 2 of 3 (index 1), tab index 0 closes: the user stays on the same tab, now index 0.
        assert_eq!(active_after_close(1, 0, 2), 0);
        // A later tab closing does not move the user.
        assert_eq!(active_after_close(0, 2, 2), 0);
        // The active tab itself closing moves to the one that took its place, or the last one.
        assert_eq!(active_after_close(2, 2, 2), 1);
        assert_eq!(active_after_close(0, 0, 0), 0);
    }

    #[test]
    fn a_new_tab_moves_next_to_the_active_one_only_if_it_opened() {
        let mut tabs = vec!["a", "b", "c", "new"];
        assert_eq!(move_new_tab(&mut tabs, 3, 1), Some(2));
        assert_eq!(tabs, ["a", "b", "new", "c"]);
        // The shell did not start: no tab was added, so nothing moves.
        let mut tabs = vec!["a", "b", "c"];
        assert_eq!(move_new_tab(&mut tabs, 3, 0), None);
        assert_eq!(tabs, ["a", "b", "c"]);
    }

    #[test]
    fn a_minimized_window_has_no_usable_size() {
        assert_eq!(usable_size(0, 0), None);
        assert_eq!(usable_size(800, 0), None);
        assert_eq!(usable_size(800, 600), Some((800, 600)));
    }

    #[test]
    fn a_stale_tab_rect_is_not_a_hit() {
        let r = |x: f32| Rect { x, y: 0.0, w: 10.0, h: 10.0 };
        let rects = [r(0.0), r(10.0), r(20.0)];
        assert_eq!(tab_hit(&rects, 3, 25.0, 5.0), Some(2));
        assert_eq!(tab_hit(&rects, 2, 25.0, 5.0), None);
    }
}

/// Messages in the banner, one slot per source, so one message cannot hide another
/// and a fixed problem can clear its own slot.
#[derive(Default)]
struct Banners {
    slots: std::collections::BTreeMap<&'static str, String>,
}

impl Banners {
    fn set(&mut self, slot: &'static str, text: impl Into<String>) {
        self.slots.insert(slot, text.into());
    }

    fn clear(&mut self, slot: &'static str) {
        self.slots.remove(slot);
    }

    fn text(&self) -> Option<String> {
        (!self.slots.is_empty()).then(|| self.slots.values().cloned().collect::<Vec<_>>().join("  |  "))
    }
}

#[cfg(test)]
mod banner_tests {
    use super::*;

    #[test]
    fn a_later_message_does_not_hide_an_earlier_one() {
        let mut b = Banners::default();
        b.set("settings", "Settings not loaded");
        b.set("font", "Font not installed: Monaco");
        let t = b.text().unwrap();
        assert!(t.contains("Settings not loaded") && t.contains("Monaco"), "{t}");
    }

    #[test]
    fn a_fixed_problem_clears_its_slot() {
        let mut b = Banners::default();
        b.set("font", "Font not installed");
        b.clear("font");
        assert_eq!(b.text(), None);
    }
}
