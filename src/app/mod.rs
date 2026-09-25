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
use winit::keyboard::{KeyCode, KeyLocation, ModifiersKeyState, PhysicalKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use winit::window::{Window, WindowId};

use crate::conpty::{self, Api, Source};
use crate::globals::{self, Globals};
use crate::input::{self, Clicks, Wheel};
use crate::keyroute::{self, Route};
use crate::keys::{self, Action, AltSide, Key, KeyOptions};
use crate::keystroke;
use crate::mru::Mru;
use crate::pointer::{self, Down, Up};
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

mod actions;
mod annotations_ui;
mod clipboard_ui;
mod copymode_ui;
mod files_ui;
mod find_ui;
mod infobar_ui;
mod keyproto_ui;
mod fold_ui;
mod labels_ui;
mod semantic_ui;
mod session_ui;
mod seltools_ui;
mod view_ui;
mod marks_ui;
mod menu;
mod requests_ui;

pub enum UserEvent {
    /// A pane has output, or its shell exited.
    Wake,
    /// A Win chord from the keyboard hook.
    Chord(crate::menus::Chord),
    /// A menu item was clicked (its id).
    Menu(String),
}

struct Tab {
    /// Stable across moves, for the most-recently-used order.
    id: u64,
    layout: Layout,
    focused: PaneId,
    /// Maximize Active Pane: this pane fills the tab.
    zoomed: Option<PaneId>,
}

/// A press whose pointer binding runs when the button comes up.
struct PointerDown {
    button: u8,
    clicks: u8,
    pane: PaneId,
    cell: (u32, u32),
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
    /// Keys > Remap Modifiers, read by the keyboard hook's thread.
    remap_tags: Arc<std::sync::RwLock<crate::remap::Tags>>,
    /// What the keyboard hook needs to decide which Win chords Tandem takes.
    route_snapshot: Arc<std::sync::RwLock<keyroute::Snapshot>>,
    /// Global bindings to Do Not Remap Modifiers, for the hook.
    exempt_map: Arc<std::sync::RwLock<toml::Table>>,
    globals: Globals,
    /// The Leader was pressed; the next key carries the Leader flag.
    leader_on: bool,
    mru: Mru,
    next_tab_id: u64,
    /// The modifiers of a Cycle Tabs chord; letting go of them ends the cycle.
    mru_mods: Option<Mods>,
    alt_side: AltSide,
    pointer_clicks: pointer::Clicks,
    /// The last press for the pointer multi-click count: when, button, where, count.
    pointer_last: Option<(Instant, u8, (f32, f32), u8)>,
    /// A press that runs a pointer binding on release.
    pointer_down: Option<PointerDown>,
    /// Toggle Mouse Reporting: a pane's own choice over its profile's.
    mouse_override: HashMap<PaneId, bool>,
    /// An Alt single click in a pane at a cell, which may move the cursor on release.
    alt_click: Option<(PaneId, (u32, u32))>,
    wheel_acc_x: f32,
    /// Mark navigation: the mark each pane is at.
    mark_pos: HashMap<PaneId, u32>,
    /// Each pane's find bar and matches.
    finds: HashMap<PaneId, find_ui::FindState>,
    /// Where the focused pane's find bar and its mode label were last drawn.
    find_bar_rect: Option<Rect>,
    find_mode_rect: Option<Rect>,
    /// The Win-click target under the pointer while Win is held.
    hover: Option<(PaneId, crate::find::Match)>,
    /// The annotation or session note window: its pane and note (None for the session note).
    note_win: Option<(crate::note_ui::NoteWindow, PaneId, Option<u32>)>,
    /// Annotation navigation: the row each pane is at.
    note_pos: HashMap<PaneId, i32>,
    /// View > Show Timestamps and Show Cursor Guide, per pane, over the profile's choice.
    show_stamps: HashMap<PaneId, bool>,
    show_guide: HashMap<PaneId, bool>,
    /// Find Cursor is up until then (longer while Win is held).
    find_cursor_until: Option<Instant>,
    /// The pointer is at the right edge of a pane with Hover timestamps.
    stamp_near: bool,
    /// When an animated image next changes frame.
    anim_due: Option<Instant>,
    /// Copy Mode, per pane.
    copy_modes: HashMap<PaneId, crate::copymode::State>,
    /// Labels on results (Find URLs and the like), waiting for a key.
    shortcuts: Option<labels_ui::Shortcuts>,
    /// The last pop-up menu, kept until its pick arrives.
    popup_menu: Option<muda::Menu>,
    /// What the right-click menu's own items act on.
    ctx_pending: Option<semantic_ui::CtxPending>,
    /// The Find Globally window.
    global_find: Option<crate::global_find_ui::GlobalFindWindow>,
    /// The Set Named Mark window, and the pane it names a mark in.
    prompt: Option<(crate::prompt_ui::PromptWindow, PaneId)>,
    api: Api,
    shell: String,
    gpu: Option<Gpu>,
    tabs: Vec<Tab>,
    active: usize,
    panes: HashMap<PaneId, Pane>,
    next_id: PaneId,
    mods: Mods,
    /// Announcement bars waiting in each pane.
    bars: HashMap<PaneId, crate::infobar::Bars>,
    /// RequestAttention: pane flashes and fireworks (start time, and the cursor cell).
    flashes: HashMap<PaneId, Instant>,
    fireworks: HashMap<PaneId, (Instant, (usize, usize))>,
    /// SetColors `tab`: the tab color a pane's program chose.
    tab_colors: HashMap<PaneId, alacritty_terminal::vte::ansi::Rgb>,
    /// When the last StealFocus question closed.
    steal_asked: Option<Instant>,
    /// When a pane flash or fireworks needs the next frame.
    attention_due: Option<Instant>,
    /// A short message at the bottom of the window, and when it goes away.
    toast: Option<(String, Instant)>,
    /// OSC 52 reads waiting for the user's answer, with the selection parameter to echo.
    pending_reads: HashMap<PaneId, String>,
    /// Keys whose press went to a program, so their release may too.
    sent_keys: keyproto_ui::Held,
    /// The physical key of the press being routed now.
    typing: Option<PhysicalKey>,
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
    let gpath = globals::default_path();
    let globals = match Globals::load(&gpath) {
        Ok(g) => g,
        Err(e) => {
            let side = gpath.with_extension("toml.recovered");
            banners.set("globals", format!("Settings not loaded ({e}). Changes save to {}.", side.display()));
            Globals::load(&side).unwrap_or_else(|_| Globals::empty(&side))
        }
    };
    let mut app = App {
        notify,
        proxy,
        menu_bar: None,
        font_offset: 0.0,
        remap_tags: Arc::new(std::sync::RwLock::new(crate::remap::Tags::from_globals(&globals))),
        route_snapshot: Arc::new(std::sync::RwLock::new(keyroute::Snapshot::default())),
        exempt_map: Arc::new(std::sync::RwLock::new(toml::Table::new())),
        globals,
        leader_on: false,
        mru: Mru::default(),
        next_tab_id: 1,
        mru_mods: None,
        alt_side: AltSide::Left,
        pointer_clicks: pointer::Clicks::default(),
        pointer_last: None,
        pointer_down: None,
        mouse_override: HashMap::new(),
        alt_click: None,
        wheel_acc_x: 0.0,
        mark_pos: HashMap::new(),
        finds: HashMap::new(),
        find_bar_rect: None,
        find_mode_rect: None,
        hover: None,
        popup_menu: None,
        shortcuts: None,
        copy_modes: HashMap::new(),
        show_stamps: HashMap::new(),
        show_guide: HashMap::new(),
        find_cursor_until: None,
        stamp_near: false,
        anim_due: None,
        note_win: None,
        note_pos: HashMap::new(),
        ctx_pending: None,
        prompt: None,
        global_find: None,
        api,
        shell: default_shell(),
        gpu: None,
        tabs: Vec::new(),
        active: 0,
        panes: HashMap::new(),
        next_id: 1,
        mods: Mods::default(),
        sent_keys: keyproto_ui::Held::default(),
        bars: HashMap::new(),
        pending_reads: HashMap::new(),
        toast: None,
        flashes: HashMap::new(),
        fireworks: HashMap::new(),
        tab_colors: HashMap::new(),
        attention_due: None,
        steal_asked: None,
        typing: None,
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
        let (cwd, warning) = profile::start_dir_with(&p, self.previous_dir().as_deref());
        match warning {
            Some(w) => self.banners.set("directory", w),
            None => self.banners.clear("directory"),
        }
        match Pane::start(id, &self.api, &cmd, Some(&cwd), cols, rows, self.notify.clone()) {
            Ok(mut pane) => {
                pane.restyle(Palette::from_profile(&p, self.dark()), Look::from_profile(&p));
                pane.screen.place_prompt_first_column = p.bool("Place Prompt at First Column");
                pane.profile = p.guid();
                pane.set_buffering(p.bool("Buffer Input by Default"));
                self.panes.insert(id, pane);
                true
            }
            Err(e) => {
                self.banners.set("start", format!("Could not start {cmd}: {e}"));
                false
            }
        }
    }

    /// The focused session's directory, from shell integration, when it is on this machine
    /// (iTerm2 prefers the local directory for a new session).
    fn previous_dir(&self) -> Option<std::path::PathBuf> {
        let t = self.tabs.get(self.active)?;
        let shell = &self.panes.get(&t.focused)?.own().shell;
        let here = std::env::var("COMPUTERNAME").unwrap_or_default();
        if shell.host.as_ref().is_some_and(|h| !h.eq_ignore_ascii_case(&here) && !h.eq_ignore_ascii_case("localhost")) {
            return None;
        }
        profile::local_path(shell.cwd.as_deref()?)
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
            Edit::GlobalsChanged => self.save_globals(),
            Edit::Changed(_) | Edit::Imported(_) => {
                if matches!(edit, Edit::Imported(_)) {
                    self.save_globals();
                }
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
        // Session copies follow edits to their saved profiles first.
        self.store.merge_sessions();
        let dark = self.dark();
        for pane in self.panes.values_mut() {
            let p = self.store.get(&pane.profile).unwrap_or_else(|| self.store.default_profile());
            pane.restyle(Palette::from_profile(p, dark), Look::from_profile(p));
            pane.own_mut().place_prompt_first_column = p.bool("Place Prompt at First Column");
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
            self.tabs.push(Tab { id: self.next_tab_id, layout: Layout::new(id), focused: id, zoomed: None });
            self.next_tab_id += 1;
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
        if let Some(p) = self.panes.remove(&id) {
            self.forget_session_profile(&p.profile);
        }
        self.finds.remove(&id);
        self.copy_modes.remove(&id);
        self.note_pos.remove(&id);
        self.show_stamps.remove(&id);
        self.show_guide.remove(&id);
        self.bars.remove(&id);
        self.pending_reads.remove(&id);
        self.flashes.remove(&id);
        self.fireworks.remove(&id);
        self.tab_colors.remove(&id);
        if self.note_win.as_ref().is_some_and(|(_, p, _)| *p == id) {
            self.note_win = None;
        }
        if let Some(t) = self.tabs.iter().position(|t| t.layout.panes().contains(&id)) {
            let tab = &mut self.tabs[t];
            if tab.layout.remove(id) {
                if tab.focused == id {
                    tab.focused = tab.layout.panes()[0];
                }
            } else {
                let gone = self.tabs.remove(t);
                self.mru.forget(gone.id);
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
        if let Some(t) = self.tabs.get(self.active) {
            self.mru.touch(t.id);
        }
        self.refresh_route();
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
                p.set_cell_px(cw as u16, ch as u16);
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

    fn focused_profile(&self) -> Option<&Profile> {
        let t = self.tabs.get(self.active)?;
        self.panes.get(&t.focused).and_then(|p| self.store.get(&p.profile))
    }

    /// Runs `f` with the key routing inputs: the focused profile's map, the global map,
    /// the Leader, and the number shortcut settings.
    fn with_route<R>(&self, f: impl FnOnce(&keyroute::Context) -> R) -> R {
        let profile_map = self.focused_profile().and_then(|p| p.get("Keyboard Map")).and_then(|v| v.as_table().cloned()).unwrap_or_default();
        let global_map = self.globals.table("GlobalKeyMap");
        let ctx = keyroute::Context {
            profile_map: &profile_map,
            global_map: &global_map,
            language_agnostic: self.globals.bool("LanguageAgnosticKeyBindings"),
            leader: keystroke::parse(&self.globals.string("Leader")),
            leader_on: self.leader_on,
            switch_tab: self.globals.int("SwitchTabModifier"),
            switch_pane: self.globals.int("SwitchPaneModifier"),
            switch_window: self.globals.int("SwitchWindowModifier"),
        };
        f(&ctx)
    }

    /// Hands the keyboard hook what it needs after a focus, profile, or settings change.
    fn refresh_route(&self) {
        *self.route_snapshot.write().unwrap() = self.with_route(keyroute::Snapshot::new);
        let exempt: toml::Table = self
            .globals
            .table("GlobalKeyMap")
            .into_iter()
            .filter(|(_, v)| crate::keyaction::Binding::from_value(v).is_some_and(|b| b.action == 22))
            .collect();
        *self.exempt_map.write().unwrap() = exempt;
        *self.remap_tags.write().unwrap() = crate::remap::Tags::from_globals(&self.globals);
    }

    fn save_globals(&mut self) {
        match self.globals.save() {
            Ok(()) => self.banners.clear("globals"),
            Err(e) => self.banners.set("globals", format!("Could not save settings: {e}")),
        }
        self.refresh_route();
    }

    /// Mouse reporting is allowed for a pane: its Toggle Mouse Reporting choice, else its
    /// profile's "Mouse Reporting".
    fn mouse_allowed(&self, id: PaneId) -> bool {
        self.mouse_override.get(&id).copied().unwrap_or_else(|| {
            self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_none_or(|p| p.bool("Mouse Reporting"))
        })
    }

    /// The pane's mouse modes, or none when mouse reporting is off for it.
    fn modes(&self, id: PaneId) -> mouse::Modes {
        match self.panes.get(&id) {
            Some(p) if self.mouse_allowed(id) => p.screen.mouse_modes(),
            _ => mouse::Modes::default(),
        }
    }

    fn pointer_prefs(&self) -> pointer::Prefs {
        pointer::Prefs { pass_on_control_click: self.globals.bool("PassOnControlClick"), report_right_click: self.globals.bool("ReportRightClick") }
    }

    /// The multi-click count for a press of `button` here: 1 to 4.
    fn os_clicks(&mut self, button: u8) -> u8 {
        let now = Instant::now();
        let (x, y) = self.cursor;
        let near = 4.0 * self.scale();
        let count = match self.pointer_last {
            Some((t, b, (px, py), n)) if b == button && now.duration_since(t) <= input::MULTI_CLICK && (x - px).abs() <= near && (y - py).abs() <= near => (n + 1).min(4),
            _ => 1,
        };
        self.pointer_last = Some((now, button, (x, y), count));
        count
    }

    /// What a key sends to the terminal, with the focused profile's Keys options.
    fn key_bytes(&self, key: &Key, base: Option<&Key>, text: Option<&str>, keypad: bool) -> Option<Vec<u8>> {
        let t = self.tabs.get(self.active)?;
        let pane = self.panes.get(&t.focused)?;
        let mode = *pane.own().term().mode();
        let opts = self.store.get(&pane.profile).map_or(KeyOptions { left_option: 2, right_option: 2, option_is_alt: true, app_keypad_allowed: false }, KeyOptions::from_profile);
        keys::press_bytes_with(key, base, text, self.mods, self.alt_side, mode.contains(TermMode::APP_CURSOR), mode.contains(TermMode::APP_KEYPAD), keypad, &opts)
    }

    fn key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) {
        if self.protocol_key_event(ev) || ev.state != ElementState::Pressed {
            return;
        }
        // Typing shows the cursor at once and restarts the blink.
        self.blink_on = true;
        self.last_blink = Instant::now();
        self.banners.clear("action");
        if self.shortcut_key(ev) || self.find_key(ev) || self.copy_mode_key(ev) {
            return;
        }
        let base = keystroke::key_of(&ev.key_without_modifiers(), None);
        let Some(key) = keystroke::key_of(&ev.logical_key, ev.text.as_deref()) else { return };
        let Some(stroke) = keystroke::from_key_event(ev, self.mods) else { return };
        let keypad = ev.location == KeyLocation::Numpad;
        let code = match ev.physical_key {
            PhysicalKey::Code(c) => Some(c),
            _ => None,
        };
        let digit = if self.globals.bool("UseVirtualKeyCodesForDetectingDigits") {
            code.and_then(us_digit)
        } else {
            match &key {
                Key::Text(s) => s.chars().next().and_then(|c| c.to_digit(10)).map(|d| d as u8),
                _ => None,
            }
        };
        let fallback = self.protocol_bytes(ev, &key, base.as_ref()).or_else(|| self.key_bytes(&key, base.as_ref(), ev.text.as_deref(), keypad));
        self.typing = Some(ev.physical_key);
        self.dispatch(keyroute::Press { stroke, digit }, fallback, Some((key, base)), None, event_loop);
        self.typing = None;
    }

    /// Runs a key press: the Leader, number shortcuts, key mappings, then the terminal
    /// (or, for a Win chord from the hook, its menu command).
    pub(super) fn dispatch(
        &mut self,
        press: keyroute::Press,
        fallback: Option<Vec<u8>>,
        typed: Option<(Key, Option<Key>)>,
        chord: Option<&crate::menus::Chord>,
        event_loop: &ActiveEventLoop,
    ) {
        let was_leader = self.leader_on;
        let route = self.with_route(|c| keyroute::route(c, &press));
        self.leader_on = false;
        match route {
            Route::LeaderOn => self.leader_on = true,
            // One window until milestone 7: window 1 is this one.
            Route::SelectWindow(_) => {}
            Route::SelectPane(d) => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    let panes = t.layout.panes();
                    let i = if d == 9 { panes.len() - 1 } else { d as usize - 1 };
                    if let Some(id) = panes.get(i) {
                        t.focused = *id;
                        t.zoomed = None;
                    }
                }
            }
            Route::SelectTab(d) => {
                let i = if d == 9 { self.tabs.len().saturating_sub(1) } else { d as usize - 1 };
                if i < self.tabs.len() {
                    self.active = i;
                }
            }
            Route::Binding(b) => self.run_key_action(&b, fallback, chord, event_loop),
            Route::Terminal => {
                if let Some(c) = chord {
                    if let Some(item) = crate::commands::command_for_chord(c) {
                        self.run_command(item, event_loop);
                    }
                } else if let Some((key, base)) = typed {
                    self.to_terminal(&key, base.as_ref(), fallback, event_loop);
                    if was_leader {
                        self.refresh_route();
                    }
                    return;
                }
            }
        }
        self.relayout();
    }

    /// A key with no binding: the Windows extras, movement-key scrolling, else the pane.
    fn to_terminal(&mut self, key: &Key, base: Option<&Key>, bytes: Option<Vec<u8>>, event_loop: &ActiveEventLoop) {
        if let Some(action) = base.and_then(|b| keys::shortcut(b, self.mods)) {
            return self.action(action, event_loop);
        }
        // Filter and Zoom views are read-only: a key ends the view (Esc only that).
        if let Some(id) = self.focused_id().filter(|id| self.panes.get(id).is_some_and(|p| p.live.is_some())) {
            self.end_view(id);
            if matches!(key, Key::Escape) {
                return;
            }
        }
        let mods = self.mods;
        let movement = self.focused_profile().is_some_and(|p| p.bool("Movement Keys Scroll Outside Interactive Apps"));
        let focused = self.focused_id();
        let Some(pane) = self.focused_pane() else { return };
        if movement {
            let alt_screen = pane.screen.term().mode().contains(TermMode::ALT_SCREEN);
            if let Some(scroll) = keys::movement_scroll(key, mods, alt_screen, pane.exited.is_some()) {
                pane.scroll(scroll);
                return self.redraw();
            }
        }
        if let Some(bytes) = bytes {
            pane.scroll(Scroll::Bottom);
            pane.input(&bytes);
            if let (Some(k), Some(id)) = (self.typing.take(), focused) {
                self.sent_keys.press(k, id);
            }
        }
    }

    fn mouse_press(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) {
        let (x, y) = self.cursor;
        if let Some(i) = tab_hit(&self.tab_rects, self.tabs.len(), x, y) {
            self.active = i;
            return self.redraw();
        }
        if button == MouseButton::Left && self.info_bar_press(x, y, event_loop) {
            return;
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
        if button == MouseButton::Left && self.find_press(x, y) {
            return;
        }
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let b = button_number(button);
        // A left click always focuses its pane; right and middle only when the setting says so.
        if b == 0 || self.globals.bool("FocusOnRightOrMiddleClick") {
            self.tabs[self.active].focused = id;
            self.refresh_route();
        }
        let (col, row, side) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let modes = self.modes(id);
        let mb = match b {
            0 => Some(Button::Left),
            1 => Some(Button::Right),
            2 => Some(Button::Middle),
            _ => None,
        };
        let reportable = mb.is_some() && mouse::app_owns_mouse(modes, mods);
        let app_tracking = modes.tracking != Tracking::Off;
        let os = self.os_clicks(b);
        let clicks = self.pointer_clicks.press(b, os);
        let map = self.globals.table("PointerActions");
        let down = pointer::on_down(b, clicks, mods, reportable, app_tracking, &map, &self.pointer_prefs());
        if down == Down::Bound {
            self.pointer_down = Some(PointerDown { button: b, clicks, pane: id, cell: (col, row) });
            return self.redraw();
        }
        // Win-click opens the file or URL under the pointer (Semantic History). Windows
        // would open the Start menu when Win comes up, so that is masked.
        if b == 0 && mods.cmd && !mods.ctrl && self.globals.bool("CommandSelection") {
            #[cfg(windows)]
            crate::winkeys::mask_start_menu();
            let point = self.grid_point(id, col, row);
            if self.open_target(id, point, false) {
                return self.redraw();
            }
        }
        if let (true, Some(mb)) = (reportable, mb) {
            let ev = mouse::Event { button: mb, action: mouse::Action::Press, mods, col, row };
            if let Some(bytes) = mouse::encode(modes, ev) {
                self.panes.get_mut(&id).unwrap().input(&bytes);
            }
            self.drag = Drag::App { pane: id, button: mb };
            return self.redraw();
        }
        match b {
            0 => {
                let count = self.clicks.press(Instant::now(), col, row);
                let point = self.grid_point(id, col, row);
                let plain = !mods.shift && !mods.alt && !mods.ctrl && !mods.cmd;
                if count == 1 && plain && (self.click_unfold(id, point) || self.click_note(id, point, event_loop)) {
                    return;
                }
                let pick = input::click_pick(count, mods, self.globals.bool("DoubleClickPerformsSmartSelection"));
                let smart = pick == input::Pick::Smart && self.smart_select(id, point, false).is_some();
                if !smart {
                    let ty = match pick {
                        input::Pick::Type(t) => t,
                        input::Pick::Smart => input::SelectionType::Semantic,
                    };
                    self.panes.get_mut(&id).unwrap().screen.term_mut().selection = Some(Selection::new(ty, point, side));
                }
                if smart || pick == input::Pick::Type(input::SelectionType::Semantic) {
                    self.clip_selection_to_dividers(id, point);
                }
                if smart {
                    // iTerm2 copies a selection as soon as it is made.
                    self.copy_selection(id);
                }
                // A Smart Selection is whole; moving the pointer does not reshape it.
                self.drag = if smart { Drag::None } else { Drag::Select { pane: id } };
                let alt_only = mods.alt && !mods.shift && !mods.ctrl && !mods.cmd;
                self.alt_click = (count == 1 && alt_only).then_some((id, (col, row)));
            }
            // With no binding and no reporting app, a right click opens the menu (iTerm2's native behavior).
            1 => {
                let point = self.grid_point(id, col, row);
                self.show_context_menu_at(Some((id, point)));
            }
            _ => {}
        }
        self.redraw();
    }

    /// Alt-click moves the cursor (Pointer > Alt-click moves cursor), unless the app
    /// takes clicks or the view is scrolled back.
    fn alt_click_moves_cursor(&mut self, pane: PaneId, target: (u32, u32)) {
        if !self.globals.bool("OptionClickMovesCursor") || self.modes(pane).tracking != Tracking::Off {
            return;
        }
        let Some(p) = self.panes.get_mut(&pane) else { return };
        let term = p.screen.term();
        if term.grid().display_offset() != 0 {
            return;
        }
        let c = term.grid().cursor.point;
        let mode = *term.mode();
        let bytes = actions::option_click_arrows((c.column.0 as u32, c.line.0.max(0) as u32), target, term.columns() as u32, mode.contains(TermMode::ALT_SCREEN), mode.contains(TermMode::APP_CURSOR));
        p.input(&bytes);
    }

    fn mouse_release(&mut self, button: MouseButton, event_loop: &ActiveEventLoop) {
        let b = button_number(button);
        if let Some(pd) = self.pointer_down.take() {
            if pd.button == b {
                let modes = self.modes(pd.pane);
                let reportable = b <= 2 && mouse::app_owns_mouse(modes, self.mods);
                let map = self.globals.table("PointerActions");
                if let Up::Perform(binding) = pointer::on_up(b, pd.clicks, self.mods, reportable, modes.tracking != Tracking::Off, &map, &self.pointer_prefs()) {
                    self.run_pointer_action(&binding, pd.pane, pd.cell, event_loop);
                }
                self.relayout();
                return;
            }
            self.pointer_down = Some(pd);
        }
        let alt_click = self.alt_click.take();
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::Select { pane } => {
                let empty = self.panes.get(&pane).and_then(|p| p.screen.term().selection_to_string()).map_or(true, |s| s.is_empty());
                if empty {
                    if let Some(p) = self.panes.get_mut(&pane) {
                        p.screen.term_mut().selection = None;
                    }
                    if let Some((p, cell)) = alt_click.filter(|(p, _)| *p == pane) {
                        self.alt_click_moves_cursor(p, cell);
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
                    let modes = self.modes(pane);
                    let p = self.panes.get_mut(&pane).unwrap();
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
        self.update_hover();
        // Hover timestamps show while the pointer is at a pane's right edge.
        let near = self.near_stamp_edge();
        if near != self.stamp_near {
            self.stamp_near = near;
            self.redraw();
        }
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
                    if self.globals.bool("FocusFollowsMouse") && self.tabs.get(self.active).is_some_and(|t| t.focused != id) {
                        self.tabs[self.active].focused = id;
                        self.refresh_route();
                        self.redraw();
                    }
                    if self.modes(id).tracking == Tracking::Any {
                        self.send_motion(id, None, x, y);
                    }
                }
            }
        }
    }

    fn send_motion(&mut self, pane: PaneId, button: Option<Button>, x: f32, y: f32) {
        let Some(rect) = self.pane_rect(pane) else { return };
        let (col, row, _) = self.cell_at(pane, rect, x, y);
        let modes = self.modes(pane);
        let p = self.panes.get_mut(&pane).unwrap();
        let ev = mouse::Event { button: button.unwrap_or(Button::None), action: mouse::Action::Motion, mods: self.mods, col, row };
        if let Some(bytes) = mouse::encode(modes, ev) {
            p.input(&bytes);
        }
    }

    fn wheel(&mut self, delta: MouseScrollDelta) {
        let (cw, ch) = self.cell();
        let (dx, dy) = match delta {
            MouseScrollDelta::LineDelta(x, y) => (x * WHEEL_LINES, y * WHEEL_LINES),
            MouseScrollDelta::PixelDelta(p) => (p.x as f32 / cw, p.y as f32 / ch),
        };
        self.wheel_acc += dy;
        self.wheel_acc_x += dx;
        self.wheel_horizontal();
        let lines = self.wheel_acc.trunc() as i32;
        if lines == 0 {
            return;
        }
        self.wheel_acc -= lines as f32;
        let (x, y) = self.cursor;
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let mods = self.mods;
        let modes = self.modes(id);
        let p = self.panes.get_mut(&id).unwrap();
        let mode = *p.screen.term().mode();
        match input::wheel(modes, mods, mode.contains(TermMode::ALT_SCREEN), mode.contains(TermMode::APP_CURSOR), lines, col, row) {
            Wheel::Send(bytes) => p.input(&bytes),
            Wheel::Scroll(n) => p.scroll(Scroll::Delta(n)),
        }
        self.redraw();
    }

    /// Horizontal wheel movement goes to a mouse-reporting app as buttons 66 and 67 when
    /// "Mouse reporting supports horizontal scrolling" is on. Otherwise it does nothing.
    fn wheel_horizontal(&mut self) {
        let steps = self.wheel_acc_x.trunc() as i32;
        if steps == 0 {
            return;
        }
        self.wheel_acc_x -= steps as f32;
        if !self.globals.bool("ReportHorizontalScrollEvents") {
            return;
        }
        let (x, y) = self.cursor;
        let Some((id, rect)) = self.pane_at(x, y) else { return };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let modes = self.modes(id);
        if !mouse::app_owns_mouse(modes, self.mods) {
            return;
        }
        // Positive x moves the content right: the view scrolls left.
        let button = if steps > 0 { Button::WheelLeft } else { Button::WheelRight };
        let ev = mouse::Event { button, action: mouse::Action::Press, mods: self.mods, col, row };
        if let Some(one) = mouse::encode(modes, ev) {
            self.panes.get_mut(&id).unwrap().input(&one.repeat(steps.unsigned_abs() as usize));
        }
    }

    fn build_frame(&mut self) -> Frame {
        let mut attention: Option<Instant> = None;
        let cell = self.cell();
        let palette_bg = self.panes.values().next().map_or(crate::palette::Palette::default().bg, |p| p.screen.palette.bg);
        let mut frame = Frame { clear: rgba(palette_bg, 1.0), ..Default::default() };
        let (w, _) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        self.tab_rects.clear();
        if self.tabs.len() > 1 {
            let titles: Vec<scene::TabLook> = self.tabs.iter().map(|t| self.tab_look(t)).collect();
            let bar = Rect { x: 0.0, y: 0.0, w, h: self.tab_height() };
            self.tab_rects = scene::tab_bar(&mut frame, &titles, self.active, bar, cell);
        }
        let area = self.content_area();
        if let Some(tab) = self.tabs.get(self.active) {
            for (id, rect) in self.visible(tab) {
                if let Some(p) = self.panes.get(&id) {
                    let shortcuts = self.shortcuts.as_ref().filter(|s| s.pane == id);
                    let labeled: Vec<crate::find::Match> = shortcuts.map(|s| s.items.iter().map(|(_, m, _)| *m).collect()).unwrap_or_default();
                    let labels: Vec<(crate::find::Pos, String)> = shortcuts.map(|s| s.items.iter().map(|(l, m, _)| (m.start, l.clone())).collect()).unwrap_or_default();
                    // Find highlights show only while the find bar is open.
                    let matches = if labeled.is_empty() { self.finds.get(&id).filter(|f| f.open).map(|f| f.results.matches.as_slice()).unwrap_or_default() } else { labeled.as_slice() };
                    let hover = self.hover.filter(|(h, _)| *h == id).map(|(_, m)| m);
                    let notes: Vec<(crate::find::Match, Option<String>)> = self.note_cells(id).into_iter().map(|(m, _, t)| (m, t)).collect();
                    let hl = scene::Highlights { matches, color: p.screen.palette.match_bg, hover, link: p.screen.palette.link, labels: &labels, notes: &notes };
                    scene::pane_with(&mut frame, &p.screen, self.inner(rect), cell, self.focused && id == tab.focused, self.blink_on, Some(&hl));
                }
                self.draw_view_extras(&mut frame, id, self.inner(rect), cell);
                if let Some((row, col, selecting)) = self.copy_cursor_on_screen(id) {
                    scene::copy_cursor(&mut frame, self.inner(rect), cell, row, col, selecting);
                }
                if self.panes.get(&id).is_some_and(|p| p.buffer.on) {
                    scene::buffering_sign(&mut frame, self.inner(rect), cell);
                }
                self.draw_info_bar(&mut frame, id, rect, cell);
                if let Some(t) = self.draw_attention(&mut frame, id, self.inner(rect), cell) {
                    attention = Some(attention.map_or(t, |a: Instant| a.min(t)));
                }
                let show = self.panes.get(&id).and_then(|p| self.store.get(&p.profile)).is_none_or(|p| p.bool("Show Mark Indicators"));
                if let Some(p) = self.panes.get(&id) {
                    let bars: Vec<(usize, crate::marks::MarkColor)> = p
                        .screen
                        .visible_marks()
                        .into_iter()
                        .filter(|(_, m)| show || m.name.is_some())
                        .map(|(row, m)| (row, crate::marks::color(&m)))
                        .collect();
                    let pad = PAD * self.scale();
                    scene::mark_bars(&mut frame, &bars, self.inner(rect), cell.1, pad);
                }
            }
            if tab.zoomed.is_none() {
                scene::dividers(&mut frame, &tab.layout.dividers(area, GAP));
            }
            self.find_bar_rect = None;
            self.find_mode_rect = None;
            let bar_pane = self.visible(tab).into_iter().find(|(id, _)| *id == tab.focused);
            if let (Some((_, rect)), Some(f)) = (bar_pane, self.finds.get(&tab.focused).filter(|f| f.open)) {
                let count = if f.filter { "Filter".to_string() } else { f.results.label() };
                let mode = self.find_mode();
                let bar = scene::FindBar { query: &f.query, count: &count, mode: mode.label(), error: f.error.is_some() };
                let mode_rect = scene::find_bar(&mut frame, &bar, rect, cell);
                self.find_bar_rect = frame.quads.last().map(|q| q.rect);
                self.find_mode_rect = Some(mode_rect);
            }
        }
        let (ww, wh) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        self.draw_find_cursor(&mut frame, Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell);
        if let Some((text, _)) = self.toast.as_ref().filter(|(_, until)| Instant::now() < *until) {
            scene::toast(&mut frame, text, Rect { x: 0.0, y: 0.0, w: ww, h: wh }, cell);
        }
        self.attention_due = attention;
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
            .and_then(|p| p.own().title())
            .unwrap_or("Tandem")
            .to_string();
        // iTerm2 shows the Leader as sparkles on the cursor; Tandem says so in the title.
        let title = if self.leader_on { format!("[Leader] {title}") } else { title };
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

/// iTerm2's button numbers: 0 left, 1 right, 2 middle, then the others.
fn button_number(b: MouseButton) -> u8 {
    match b {
        MouseButton::Left => 0,
        MouseButton::Right => 1,
        MouseButton::Middle => 2,
        MouseButton::Back => 3,
        MouseButton::Forward => 4,
        MouseButton::Other(n) => n.min(255) as u8,
    }
}

/// The digit a physical key has on a U.S. keyboard (Emulate U.S. Keyboard).
fn us_digit(code: KeyCode) -> Option<u8> {
    use KeyCode::*;
    Some(match code {
        Digit0 | Numpad0 => 0,
        Digit1 | Numpad1 => 1,
        Digit2 | Numpad2 => 2,
        Digit3 | Numpad3 => 3,
        Digit4 | Numpad4 => 4,
        Digit5 | Numpad5 => 5,
        Digit6 | Numpad6 => 6,
        Digit7 | Numpad7 => 7,
        Digit8 | Numpad8 => 8,
        Digit9 | Numpad9 => 9,
        _ => return None,
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
        let find_due = self.run_due_searches();
        let spotlight = self.tick_find_cursor();
        let animation = self.next_animation_frame();
        let sync = self.flush_due_syncs();
        let toast = self.toast_due();
        if self.attention_due.is_some_and(|t| Instant::now() >= t) {
            self.prune_attention();
            self.redraw();
        }
        let wake = [self.save_due, find_due, spotlight, animation, sync, toast, self.attention_due].into_iter().flatten().min();
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
            match wake {
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
        let next = wake.map_or(self.last_blink + BLINK, |t| t.min(self.last_blink + BLINK));
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Chord(c) => {
                if let Some(s) = self.settings.as_mut().filter(|s| s.recording()) {
                    return s.record_chord(&c);
                }
                return self.run_chord(&c, event_loop);
            }
            UserEvent::Menu(id) => return self.run_menu_id(&id, event_loop),
            UserEvent::Wake => {}
        }
        let mut changed = false;
        let mut exited = Vec::new();
        let now = Instant::now();
        self.sync_output_policy();
        for (id, p) in self.panes.iter_mut() {
            let got = p.pump();
            if got {
                if let Some(f) = self.finds.get_mut(id) {
                    f.output_changed(now);
                }
            }
            changed |= got;
            if p.exited.is_some() {
                exited.push(*id);
            }
        }
        for id in exited {
            self.close_pane(id, event_loop);
        }
        if changed {
            self.take_downloads();
            self.take_requests();
            self.check_marks();
            self.sync_checks();
            self.redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.prompt_event(id, &event) || self.global_find_event(id, &event) || self.note_event(id, &event) {
            return;
        }
        for profiles in [false, true] {
            let slot = if profiles { &mut self.profiles_win } else { &mut self.settings };
            if !slot.as_ref().is_some_and(|s| s.id() == id) {
                continue;
            }
            match event {
                WindowEvent::CloseRequested => {
                    *slot = None;
                    crate::winkeys::set_recording(false);
                }
                WindowEvent::RedrawRequested => {
                    let edit = slot.as_mut().unwrap().redraw(&mut self.store, &mut self.globals);
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
                // Keys held while the window loses focus send no release to Tandem.
                if !f {
                    self.sent_keys.clear();
                }
                // iTerm2 drops a pending Leader when the app goes to the background.
                if !f && self.leader_on {
                    self.leader_on = false;
                    self.refresh_route();
                }
                // Focus reporting (DECSET 1004): tell the app the window gained or lost focus.
                if let Some(p) = self.focused_pane() {
                    if p.screen.term().mode().contains(TermMode::FOCUS_IN_OUT) {
                        p.focus_report(f);
                    }
                }
                self.redraw();
            }
            WindowEvent::ModifiersChanged(m) => {
                let s = m.state();
                self.mods = Mods { shift: s.shift_key(), alt: s.alt_key(), ctrl: s.control_key(), cmd: s.super_key() };
                self.update_hover();
                if s.alt_key() {
                    self.alt_side = if m.ralt_state() == ModifiersKeyState::Pressed { AltSide::Right } else { AltSide::Left };
                }
                // Cycle Tabs ends when the chord's modifiers come up.
                if let Some(c) = self.mru_mods {
                    if !((c.ctrl && s.control_key()) || (c.alt && s.alt_key()) || (c.cmd && s.super_key())) {
                        self.mru_mods = None;
                        self.mru.commit();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.key(&event, event_loop),
            WindowEvent::CursorMoved { position, .. } => self.mouse_move(position.x as f32, position.y as f32),
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => self.mouse_press(button, event_loop),
            WindowEvent::MouseInput { state: ElementState::Released, button, .. } => self.mouse_release(button, event_loop),
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
    fn steal_focus_asks_once() {
        use crate::screen::Request;
        let now = Instant::now();
        let many: Vec<(PaneId, Request)> = (0..1000).map(|_| (1, Request::StealFocus)).chain([(2, Request::StealFocus), (1, Request::ReportCellSize)]).collect();
        let kept = clipboard_ui::thin_requests(many.clone(), None, now);
        assert_eq!(kept.iter().filter(|(p, r)| *p == 1 && *r == Request::StealFocus).count(), 1);
        assert_eq!(kept.iter().filter(|(p, r)| *p == 2 && *r == Request::StealFocus).count(), 1);
        assert!(kept.contains(&(1, Request::ReportCellSize)), "other requests stay");
        let quiet = clipboard_ui::thin_requests(many, Some(now), now + std::time::Duration::from_millis(500));
        assert!(!quiet.iter().any(|(_, r)| *r == Request::StealFocus), "none right after a question");
    }

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
