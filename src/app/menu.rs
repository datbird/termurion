//! iTerm2's menu bar and right-click menu, and running its menu commands.

use std::collections::HashMap;

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use alacritty_terminal::vte::ansi::{CharsetIndex, Handler, StandardCharset};
use muda::{CheckMenuItem, ContextMenu, IsMenuItem, Menu, MenuItem, MenuTheme, PredefinedMenuItem, Submenu};
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Fullscreen;

use super::*;
use crate::commands::{self, Status};
use crate::layout::{Dir, visible_rects};
use crate::menus::{self, Chord, Item};

/// iTerm2's Session > Terminal State toggles: label, escape to turn on, escape to turn
/// off, and the mode that shows the check mark.
const TOGGLES: [(&str, &str, &str, TermMode); 6] = [
    ("Alternate Screen", "\x1b[?1049h", "\x1b[?1049l", TermMode::ALT_SCREEN),
    ("Focus Reporting", "\x1b[?1004h", "\x1b[?1004l", TermMode::FOCUS_IN_OUT),
    ("Mouse Reporting", "\x1b[?1000h", "\x1b[?1000l\x1b[?1002l\x1b[?1003l", TermMode::MOUSE_MODE),
    ("Paste Bracketing", "\x1b[?2004h", "\x1b[?2004l", TermMode::BRACKETED_PASTE),
    ("Application Cursor", "\x1b[?1h", "\x1b[?1l", TermMode::APP_CURSOR),
    ("Application Keypad", "\x1b=", "\x1b>", TermMode::APP_KEYPAD),
];

/// Session > Terminal State's key reporting radio group.
const KEY_MODES: [(&str, crate::keyproto::KeyMode); 5] = [
    ("Standard Key Reporting Mode", crate::keyproto::KeyMode::Standard),
    ("modifyOtherKeys Mode 1", crate::keyproto::KeyMode::Mok1),
    ("modifyOtherKeys Mode 2", crate::keyproto::KeyMode::Mok2),
    ("CSI u Mode", crate::keyproto::KeyMode::CsiU),
    ("Raw Key Reporting Mode", crate::keyproto::KeyMode::Raw),
];

/// Session > Terminal State's kitty keyboard flags.
const KITTY_FLAGS: [(&str, u8); 5] = [
    ("Disambiguate Escape", crate::keyproto::DISAMBIGUATE),
    ("Report All Event Types", crate::keyproto::EVENT_TYPES),
    ("Report Alternate Keys", crate::keyproto::ALTERNATE_KEYS),
    ("Report All Keys as Escape Codes", crate::keyproto::ALL_AS_ESCAPES),
    ("Report Associated Text", crate::keyproto::ASSOCIATED_TEXT),
];

/// Session > Terminal State > Emulation Level.
const LEVELS: [(&str, u16); 5] = [("VT100", 100), ("VT200", 200), ("VT300", 300), ("VT400", 400), ("VT500", 500)];

/// How far one Move Divider command moves a divider, as a share of its split.
const NUDGE: f32 = 0.05;

/// Windows shows "&" as a mnemonic marker, so a literal "&" is written "&&".
fn menu_text(item: &Item) -> String {
    let label = item.label().replace('&', "&&");
    match item.chord().filter(menus::reaches_tandem) {
        Some(c) => format!("{label}\t{}", menus::chord_label(&c)),
        None => label,
    }
}

pub(super) struct MenuBar {
    _menu: Menu,
    checks: HashMap<String, CheckMenuItem>,
    /// Edit > Named Marks, rebuilt as marks change, and what it lists now.
    pub(super) named: Option<Submenu>,
    pub(super) named_items: Vec<(u32, String)>,
    /// The focused pane and its named marks when the menu was last built.
    pub(super) named_key: (Option<PaneId>, Vec<(u32, Option<String>)>),
    /// The Profiles menu, its fixed items, and the profiles listed after them.
    pub(super) profiles: Option<(Submenu, usize)>,
    pub(super) profile_items: Vec<(String, String)>,
}

/// Menu ids of the Profiles menu's profile entries.
pub(super) const PROFILE_ID: &str = "profile-open:";

impl App {
    pub(super) fn hwnd(&self) -> Option<isize> {
        let g = self.gpu.as_ref()?;
        match g.window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
            _ => None,
        }
    }

    /// Builds the menu bar from iTerm2's menus, the right-click menu, and the Win chord hook.
    pub(super) fn install_menus(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let menu = Menu::new();
        let mut subs: HashMap<Vec<String>, Submenu> = HashMap::new();
        let mut checks = HashMap::new();
        for item in menus::items() {
            let (parent, entry): (&[String], Box<dyn IsMenuItem>) = if item.separator {
                (&item.path[..], Box::new(PredefinedMenuItem::separator()))
            } else if item.submenu {
                let s = Submenu::new(item.label().replace('&', "&&"), true);
                subs.insert(item.path.clone(), s.clone());
                (&item.path[..item.path.len() - 1], Box::new(s))
            } else {
                let enabled = !matches!(commands::status(item), Status::Later(_)) && !commands::DISABLED_IN_SESSION.contains(&item.selector.as_str());
                let parent = &item.path[..item.path.len() - 1];
                let alert_check = super::marks_ui::CHECKS.iter().any(|(s, _)| *s == item.selector) || super::seltools_ui::CHECKED.contains(&item.selector.as_str());
                let state_check = parent.iter().any(|p| p == "Terminal State") && item.label() != "Reset";
                if alert_check || state_check {
                    let c = CheckMenuItem::with_id(item.id(), menu_text(item), enabled, false, None);
                    checks.insert(item.label().to_string(), c.clone());
                    (parent, Box::new(c))
                } else {
                    (parent, Box::new(MenuItem::with_id(item.id(), menu_text(item), enabled, None)))
                }
            };
            let appended = if parent.is_empty() {
                menu.append(entry.as_ref())
            } else if let Some(s) = subs.get(parent) {
                s.append(entry.as_ref())
            } else {
                continue;
            };
            if let Err(e) = appended {
                self.banners.set("menu", format!("Menu item {}: {e}", item.id()));
            }
        }
        if let Err(e) = unsafe { menu.init_for_hwnd_with_theme(hwnd, MenuTheme::Auto) } {
            self.banners.set("menu", format!("Could not attach the menu bar: {e}"));
        }

        let proxy = self.proxy.clone();
        muda::MenuEvent::set_event_handler(Some(move |e: muda::MenuEvent| {
            let _ = proxy.lock().unwrap().send_event(UserEvent::Menu(e.id.0));
        }));
        let proxy = self.proxy.clone();
        let send = Box::new(move |c| {
            let _ = proxy.lock().unwrap().send_event(UserEvent::Chord(c));
        });
        let snapshot = self.route_snapshot.clone();
        let bound = Box::new(move |c: &Chord| crate::keyroute::win_chord_bound(&snapshot.read().unwrap(), c));
        let exempt_map = self.exempt_map.clone();
        let exempt = Box::new(move |vk: u16, m: crate::mouse::Mods| {
            let Some(key) = crate::winkeys::vk_to_key(vk) else { return false };
            let chord = Chord { key, cmd: m.cmd, alt: m.alt, shift: m.shift, ctrl: m.ctrl };
            crate::keyroute::press_from_chord(&chord).stroke.find(&exempt_map.read().unwrap(), false).is_some()
        });
        self.refresh_route();
        if let Err(e) = crate::winkeys::install(hwnd, send, bound, self.remap_tags.clone(), exempt) {
            self.banners.set("keys", format!("Win chords are unavailable: {e}"));
        }
        let named = subs.get(&vec!["Edit".to_string(), "Named Marks".to_string()]).cloned();
        let profiles = subs.get(&vec!["Profiles".to_string()]).cloned().map(|s| {
            let fixed = s.items().len();
            (s, fixed)
        });
        self.menu_bar = Some(MenuBar { _menu: menu, checks, named, named_items: Vec::new(), named_key: (None, vec![(0, None)]), profiles, profile_items: Vec::new() });
        self.sync_checks();
    }

    /// iTerm2's right-click menu, after `extra` items (label, what it does) and a separator.
    pub(super) fn show_context_menu_with(&mut self, extra: &[(String, super::semantic_ui::CtxItem)]) {
        let Some(hwnd) = self.hwnd() else { return };
        let context = Menu::new();
        let mut results = Vec::new();
        for (label, item) in extra {
            results.push(context.append(&MenuItem::with_id(item.id(), label.replace('&', "&&"), true, None)));
        }
        if !extra.is_empty() {
            results.push(context.append(&PredefinedMenuItem::separator()));
        }
        for id in commands::CONTEXT {
            results.push(match menus::by_id(id) {
                None => context.append(&PredefinedMenuItem::separator()),
                Some(item) => {
                    let enabled = !matches!(commands::status(item), Status::Later(_));
                    context.append(&MenuItem::with_id(item.id(), menu_text(item), enabled, None))
                }
            });
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Context menu: {e}"));
        }
        unsafe { context.show_context_menu_for_hwnd(hwnd, None) };
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(context);
    }

    /// A porthole's language list as a popup: `items` in order, None for a separator. The
    /// picks come back as `porthole-lang:<index>`.
    pub(super) fn show_porthole_languages(&mut self, items: &[Option<String>]) {
        let Some(hwnd) = self.hwnd() else { return };
        let menu = Menu::new();
        let mut index = 0;
        let mut results = Vec::new();
        for item in items {
            results.push(match item {
                Some(name) => {
                    let r = menu.append(&MenuItem::with_id(format!("porthole-lang:{index}"), name.replace('&', "&&"), true, None));
                    index += 1;
                    r
                }
                None => menu.append(&PredefinedMenuItem::separator()),
            });
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Language menu: {e}"));
        }
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
    }

    /// Check marks follow the focused pane's terminal modes.
    pub(super) fn sync_checks(&mut self) {
        let mode = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).map(|p| *p.screen.term().mode());
        let Some(bar) = &self.menu_bar else { return };
        for (label, _, _, flag) in TOGGLES {
            if let Some(c) = bar.checks.get(label) {
                c.set_checked(mode.is_some_and(|m| m.intersects(flag)));
            }
        }
        let screen = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).map(|p| p.own());
        let kb = screen.map(|s| s.keyboard());
        let check = |label: &str, on: bool| {
            if let Some(c) = bar.checks.get(label) {
                c.set_checked(on);
            }
        };
        for (label, m) in KEY_MODES {
            check(label, kb.is_some_and(|k| k.mode == m));
        }
        for (label, bit) in KITTY_FLAGS {
            check(label, kb.is_some_and(|k| k.kitty & bit != 0));
        }
        for (label, level) in LEVELS {
            check(label, screen.is_some_and(|s| s.level() == level));
        }
        check("Literal Controls", screen.is_some_and(|s| s.keys.literal));
        let alert_next = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).is_some_and(|p| p.own().marks.alert_next);
        let action = self.globals.string("Mark Alert Action");
        self.refresh_named_marks();
        self.refresh_profiles_menu();
        let Some(bar) = &self.menu_bar else { return };
        for selector in super::seltools_ui::CHECKED {
            let Some(item) = menus::items().iter().find(|i| i.selector == *selector) else { continue };
            if let Some(c) = bar.checks.get(item.label()) {
                c.set_checked(self.is_checked(selector));
            }
        }
        for (selector, value) in super::marks_ui::CHECKS {
            let Some(item) = menus::items().iter().find(|i| i.selector == selector) else { continue };
            let on = match selector {
                "toggleAlertOnNextMark:" => alert_next,
                "toggleAlertOnMarksInOffscreenSessions:" => self.globals.bool("Alert On Marks in Offscreen Sessions"),
                _ => action == value,
            };
            if let Some(c) = bar.checks.get(item.label()) {
                c.set_checked(on);
            }
        }
    }

    /// A Win chord the hook took: key routing first (a mapping beats a menu command).
    pub(super) fn run_chord(&mut self, chord: &Chord, event_loop: &ActiveEventLoop) {
        self.banners.clear("action");
        self.dispatch(crate::keyroute::press_from_chord(chord), None, None, Some(chord), event_loop);
    }

    pub(super) fn run_menu_id(&mut self, id: &str, event_loop: &ActiveEventLoop) {
        if let Some(guid) = id.strip_prefix(PROFILE_ID) {
            return self.open_profile_from_menu(guid);
        }
        if self.find_mode_picked(id) || self.context_picked(id) || self.named_mark_picked(id) || self.porthole_language_picked(id) {
            return;
        }
        if let Some(item) = menus::by_id(id) {
            self.run_command(item, event_loop);
        }
    }

    /// The focused pane's saved profile (a session copy's original): what new tabs,
    /// splits, and Edit Session use.
    fn focused_guid(&self) -> Option<String> {
        let t = self.tabs.get(self.active)?;
        self.panes.get(&t.focused).map(|p| self.store.shared_guid(&p.profile))
    }

    pub(super) fn focused_id(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    /// Runs one iTerm2 menu command. Commands owned by a later milestone do nothing.
    pub(super) fn run_command(&mut self, item: &'static Item, event_loop: &ActiveEventLoop) {
        if matches!(commands::status(item), Status::Later(_)) {
            return;
        }
        let label = item.label();
        let window = self.gpu.as_ref().map(|g| g.window.clone());
        if self.run_find_command(&item.selector, label) || self.run_seltools_command(&item.selector) || self.run_copymode_command(&item.selector) || self.run_fold_command(&item.selector) || self.run_view_command(&item.selector) || self.run_note_command(&item.selector, event_loop) {
            return self.relayout();
        }
        // Commands that change the terminal itself act on the pane's own screen.
        let sel = item.selector.as_str();
        if sel.starts_with("terminalState") || sel.starts_with("reset") || sel.starts_with("clear") {
            if let Some(id) = self.focused_id() {
                self.end_view(id);
            }
        }
        if self.run_trigger_command_menu(&item.selector, event_loop) {
            return self.relayout();
        }
        if self.run_mark_command(&item.selector, event_loop) {
            self.sync_checks();
            return self.relayout();
        }
        match item.selector.as_str() {
            "showAbout:" => info_box(&format!("Tandem {}\nAn iTerm2-style terminal for Windows.", env!("CARGO_PKG_VERSION"))),
            "showPrefWindow:" => self.open_settings(event_loop, Mode::Settings),
            "makeScreenshot:" => self.make_screenshot(event_loop),
            "renderSelection:" => self.render_selection(),
            "findGlobally:" => self.open_global_find(event_loop),
            "editCurrentSession:" => {
                let guid = self.focused_guid();
                self.open_settings(event_loop, Mode::Settings);
                if let (Some(s), Some(g)) = (self.settings.as_mut(), guid) {
                    s.select(&g);
                }
            }
            "showBookmarkWindow:" | "openSplitHorizontallySheet:" | "openSplitVerticallySheet:" => {
                self.open_settings(event_loop, Mode::Profiles)
            }
            "terminate:" => self.quit(event_loop),
            "moveTabToNewWindow:" => self.move_tab_to_new_window(),
            "moveSessionToWindow:" => self.move_session_to_window(),
            "mergeAllWindows:" => self.merge_all_windows(),
            "duplicateWindow:" => self.duplicate_window(),
            "closeTerminalWindow:" => {
                if let Some(id) = self.current_window() {
                    self.close_window(id, event_loop);
                }
            }
            "hide:" | "performMiniaturize:" => {
                if let Some(w) = &window {
                    w.set_minimized(true);
                }
            }
            "performZoom:" => {
                if let Some(w) = &window {
                    w.set_maximized(!w.is_maximized());
                }
            }
            "toggleFullScreenMode:" => toggle_fullscreen(window.as_deref()),
            "setWindowStyle:" => {
                if let Some((id, _)) = crate::frames::MENU.iter().find(|(_, n)| *n == label) {
                    self.apply_window_style(*id, true);
                }
            }
            "arrangeInFront:" => {
                if let Some(w) = &window {
                    w.focus_window();
                }
            }
            "newSession:" => self.new_tab(None),
            "newWindow:" => self.new_window(event_loop, None),
            "newSessionAlternate:" => {
                let (before, active) = (self.tabs.len(), self.active);
                self.new_tab(None);
                if let Some(at) = super::move_new_tab(&mut self.tabs, before, active) {
                    self.active = at;
                }
            }
            "newWindowWithSameProfile:" => {
                let g = self.focused_guid();
                self.new_window(event_loop, g.as_deref());
            }
            "newSessionWithSameProfile:" | "duplicateTab:" | "duplicateSession:" => {
                let g = self.focused_guid();
                self.new_tab(g.as_deref());
            }
            "splitVertically:" => {
                let g = self.focused_guid();
                self.split(Axis::Row, g.as_deref());
            }
            "splitHorizontally:" => {
                let g = self.focused_guid();
                self.split(Axis::Column, g.as_deref());
            }
            "runCoprocess:" => self.open_coprocess_prompt(event_loop),
            "stopCoprocess:" => {
                if let Some(id) = self.focused_id() {
                    self.stop_coprocess(id);
                }
            }
            "closeCurrentSession:" => {
                if let Some(id) = self.focused_id().filter(|id| self.confirm_close(close_ui::Closing::Session(*id))) {
                    self.close_pane(id, event_loop);
                }
            }
            "closeCurrentTab:" => {
                if let Some(t) = self.tabs.get(self.active).filter(|_| self.confirm_close(close_ui::Closing::Tab(self.active))) {
                    for id in t.layout.panes() {
                        self.close_pane(id, event_loop);
                    }
                }
            }
            "restartSession:" => {
                if let Some(id) = self.focused_id() {
                    self.restart_pane(id);
                }
            }
            "copy:" => {
                if let Some(id) = self.focused_id() {
                    self.copy_selection(id);
                }
            }
            "print:" => {
                let what = match label {
                    "Selection" => crate::print::What::Selection,
                    "Buffer" => crate::print::What::Buffer,
                    _ => crate::print::What::Screen,
                };
                if let Some(id) = self.focused_id() {
                    self.print(id, what);
                }
            }
            "paste:" => self.paste(),
            "pasteSelection:" => {
                let text = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).and_then(|p| p.screen.term().selection_to_string());
                if let (Some(text), Some(p)) = (text, self.focused_pane()) {
                    p.paste(&text);
                }
            }
            "selectAll:" => {
                if let Some(p) = self.focused_pane() {
                    let term = p.screen.term_mut();
                    let top = Point::new(Line(-(term.grid().history_size() as i32)), Column(0));
                    let bottom = Point::new(Line(term.screen_lines() as i32 - 1), Column(term.columns() - 1));
                    let mut sel = Selection::new(SelectionType::Lines, top, Side::Left);
                    sel.update(bottom, Side::Right);
                    term.selection = Some(sel);
                }
            }
            "clearBuffer:" => self.focused_pane().map_or((), |p| p.clear()),
            "clearScrollbackBuffer:" => self.focused_pane().map_or((), |p| p.screen.clear(true)),
            "reset:" | "terminalStateReset:" => self.focused_pane().map_or((), |p| p.screen.soft_reset()),
            "resetCharset:" => {
                if let Some(p) = self.focused_pane() {
                    let term = p.screen.term_mut();
                    term.configure_charset(CharsetIndex::G0, StandardCharset::Ascii);
                    term.set_active_charset(CharsetIndex::G0);
                }
            }
            "terminalStateToggleLiteralMode:" => {
                if let Some(p) = self.focused_pane() {
                    p.screen.keys.literal = !p.screen.keys.literal;
                }
            }
            "terminalStateSetEmulationLevel:" => {
                if let (Some((_, level)), Some(p)) = (LEVELS.iter().find(|l| l.0 == label), self.focused_pane()) {
                    p.screen.set_level(*level);
                }
            }
            s if s.starts_with("terminalStateToggle") && KEY_MODES.iter().any(|m| m.0 == label) => {
                if let (Some((_, mode)), Some(p)) = (KEY_MODES.iter().find(|m| m.0 == label), self.focused_pane()) {
                    p.screen.set_key_mode(*mode);
                }
            }
            s if s.starts_with("terminalStateToggle") && KITTY_FLAGS.iter().any(|m| m.0 == label) => {
                if let (Some((_, bit)), Some(p)) = (KITTY_FLAGS.iter().find(|m| m.0 == label), self.focused_pane()) {
                    p.screen.toggle_kitty_flag(*bit);
                }
            }
            s if s.starts_with("terminalStateToggle") => {
                if let Some((_, on, off, flag)) = TOGGLES.iter().find(|t| t.0 == label) {
                    if let Some(p) = self.focused_pane() {
                        let now = p.screen.term().mode().intersects(*flag);
                        p.screen.feed(if now { off.as_bytes() } else { on.as_bytes() });
                    }
                }
            }
            "biggerFont:" => self.font_offset += 1.0,
            "smallerFont:" => self.font_offset = (self.font_offset - 1.0).max(-8.0),
            "returnToDefaultSize:" => self.font_offset = 0.0,
            "maximizePane:" => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.zoomed = if t.zoomed.is_some() { None } else { Some(t.focused) };
                }
            }
            "nextTab:" | "previousTab:" => {
                let n = self.tabs.len();
                if n > 0 {
                    self.active = if item.selector == "nextTab:" { (self.active + 1) % n } else { (self.active + n - 1) % n };
                }
            }
            "moveTabLeft:" | "moveTabRight:" => {
                let n = self.tabs.len();
                if n > 1 {
                    let to = if item.selector == "moveTabRight:" { (self.active + 1) % n } else { (self.active + n - 1) % n };
                    self.tabs.swap(self.active, to);
                    self.active = to;
                }
            }
            "selectPaneUp:" | "selectPaneDown:" | "selectPaneLeft:" | "selectPaneRight:" => {
                let dir = match item.selector.as_str() {
                    "selectPaneUp:" => Dir::Up,
                    "selectPaneDown:" => Dir::Down,
                    "selectPaneLeft:" => Dir::Left,
                    _ => Dir::Right,
                };
                let area = self.content_area();
                if let Some(t) = self.tabs.get_mut(self.active) {
                    if let Some(id) = t.layout.neighbor(t.focused, dir, area) {
                        t.focused = id;
                        t.zoomed = None;
                    }
                }
            }
            "nextPane:" | "previousPane:" => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    let panes = t.layout.panes();
                    let i = panes.iter().position(|p| *p == t.focused).unwrap_or(0);
                    let step = if item.selector == "nextPane:" { 1 } else { panes.len() - 1 };
                    t.focused = panes[(i + step) % panes.len()];
                    t.zoomed = None;
                }
            }
            "movePaneDividerUp:" | "movePaneDividerDown:" | "movePaneDividerLeft:" | "movePaneDividerRight:" => {
                let dir = match item.selector.as_str() {
                    "movePaneDividerUp:" => Dir::Up,
                    "movePaneDividerDown:" => Dir::Down,
                    "movePaneDividerLeft:" => Dir::Left,
                    _ => Dir::Right,
                };
                if let Some(t) = self.tabs.get_mut(self.active) {
                    let focused = t.focused;
                    // iTerm2 moves the divider on that side; failing that, the one on the other side.
                    let opposite = match dir {
                        Dir::Up => Dir::Down,
                        Dir::Down => Dir::Up,
                        Dir::Left => Dir::Right,
                        Dir::Right => Dir::Left,
                    };
                    if !t.layout.nudge(focused, dir, NUDGE) {
                        let _ = t.layout.nudge(focused, opposite, -NUDGE);
                    }
                }
            }
            "arrangeSplitPanesEvenly:" => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.layout.even();
                }
            }
            "saveContents:" | "saveDocumentAs:" => self.save_text(item.selector == "saveDocumentAs:"),
            "installShellIntegration:" => self.install_shell_integration(),
            "showHelp:" => {
                let _ = std::process::Command::new("explorer.exe").arg("https://iterm2.com/documentation.html").spawn();
            }
            _ => {}
        }
        self.apply_fonts();
        self.sync_checks();
        self.relayout();
    }

    fn save_text(&mut self, selection_only: bool) {
        let Some(p) = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)) else { return };
        let text = if selection_only { p.screen.term().selection_to_string() } else { Some(p.screen.all_text()) };
        let Some(text) = text.filter(|t| !t.is_empty()) else { return };
        let name = if selection_only { "selection.txt" } else { "session.txt" };
        if let Some(path) = rfd::FileDialog::new().set_file_name(name).save_file() {
            match std::fs::write(&path, text) {
                Ok(()) => self.banners.clear("save-text"),
                Err(e) => self.banners.set("save-text", format!("Could not save {}: {e}", path.display())),
            }
        }
    }

    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        if !self.confirm_close(close_ui::Closing::Window) {
            return;
        }
        self.save_all_window_frames();
        if let Some(h) = self.hwnd() {
            crate::notify::remove(h);
        }
        if self.save_due.is_some() {
            self.save_now();
        }
        self.panes.clear();
        event_loop.exit();
    }

    /// The rectangles the active tab shows (only the zoomed pane when one is zoomed).
    pub(super) fn visible(&self, tab: &Tab) -> Vec<(PaneId, Rect)> {
        visible_rects(&tab.layout, tab.zoomed, self.content_area(), GAP)
    }
}

#[cfg(test)]
mod install_tests {
    use super::*;

    #[test]
    fn the_pwsh_install_command_allows_scripts_and_quotes_the_path() {
        let c = pwsh_install_command(std::path::Path::new(r"C:\Users\o'brien\$x\tandem_shell_integration.ps1"));
        assert!(c.contains("(Get-ExecutionPolicy) -in 'Restricted','AllSigned'"), "{c}");
        assert!(c.contains("Set-ExecutionPolicy -Scope CurrentUser RemoteSigned -Force"), "{c}");
        assert!(c.contains(r"Add-Content $PROFILE '. ''C:\Users\o''''brien\$x\tandem_shell_integration.ps1'''"), "{c}");
        assert!(c.ends_with('\r'));
    }

    #[test]
    fn the_pwsh_install_command_adds_one_line_to_the_profile() {
        let dir = std::env::temp_dir().join(format!("tandem-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("tandem_shell_integration.ps1");
        std::fs::write(&script, "$global:TandemLoaded = 1").unwrap();
        let profile = dir.join("profile.ps1");
        let cmd = pwsh_install_command(&script).trim_end().to_string();
        let run = format!("$PROFILE = '{}'; {cmd}; {cmd}; if ($global:TandemLoaded) {{ 'loaded' }}", profile.display());
        let out = std::process::Command::new("powershell.exe").args(["-NoProfile", "-NoLogo", "-Command", &run]).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("loaded"), "{}", String::from_utf8_lossy(&out.stderr));
        let text = std::fs::read_to_string(&profile).unwrap();
        assert_eq!(text.matches("tandem_shell_integration.ps1").count(), 1, "{text}");
    }

    #[test]
    fn install_command_depends_on_the_shell() {
        assert_eq!(install_plan("pwsh.exe -NoLogo"), Plan::Pwsh);
        assert_eq!(install_plan("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"), Plan::Pwsh);
        assert_eq!(install_plan("wsl.exe -d Ubuntu"), Plan::Curl);
        assert_eq!(install_plan("C:\\Program Files\\Git\\bin\\bash.exe --login"), Plan::Curl);
        assert_eq!(install_plan("cmd.exe"), Plan::Unsupported);
    }
}

/// How Install Shell Integration sets up a shell.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Plan {
    /// Tandem's PowerShell script, loaded from $PROFILE.
    Pwsh,
    /// iTerm2's own installer, for bash, zsh, and fish (WSL, Git Bash, ssh).
    Curl,
    /// cmd.exe and others have no prompt hooks.
    Unsupported,
}

pub(super) fn install_plan(cmdline: &str) -> Plan {
    let c = cmdline.to_lowercase();
    if c.contains("pwsh") || c.contains("powershell") {
        Plan::Pwsh
    } else if ["wsl", "bash", "zsh", "fish", "ssh"].iter().any(|s| c.contains(s)) {
        Plan::Curl
    } else {
        Plan::Unsupported
    }
}

/// The command typed into PowerShell: allow local scripts for this user if the policy
/// blocks them, add one line that loads the script to $PROFILE, and load it now. Paths
/// go in single quotes, so nothing in them is expanded.
pub(super) fn pwsh_install_command(script: &std::path::Path) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let path = q(&script.display().to_string());
    let line = q(&format!(". {path}"));
    format!(
        "if ((Get-ExecutionPolicy) -in 'Restricted','AllSigned') {{ Set-ExecutionPolicy -Scope CurrentUser RemoteSigned -Force }}; if (!(Test-Path $PROFILE)) {{ New-Item -ItemType File -Force $PROFILE | Out-Null }}; if (!(Select-String -Quiet -SimpleMatch 'tandem_shell_integration.ps1' $PROFILE)) {{ Add-Content $PROFILE {line} }}; . {path}\r"
    )
}

const PWSH_SCRIPT: &str = include_str!("../../resources/shell_integration/tandem_shell_integration.ps1");

fn ask(text: &str) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_ICONQUESTION, MB_YESNO, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Install Shell Integration").as_ptr(), MB_YESNO | MB_ICONQUESTION) == IDYES }
}

impl App {
    /// iTerm2 > Install Shell Integration, for the focused pane's shell.
    fn install_shell_integration(&mut self) {
        let Some(guid) = self.focused_guid() else { return };
        let cmdline = crate::profile::command_line(&self.profile(Some(&guid)), &self.shell);
        let typed = match install_plan(&cmdline) {
            Plan::Unsupported => {
                return info_box("This shell has no prompt hooks, so it cannot have shell integration. Use PowerShell (pwsh), or bash, zsh, or fish in WSL.");
            }
            Plan::Curl => {
                if !ask("Tandem will type iTerm2's installer into this session:\n\ncurl -L https://iterm2.com/shell_integration/install_shell_integration.sh | bash\n\nIt adds iTerm2's script to your shell's startup file. Continue?") {
                    return;
                }
                "curl -L https://iterm2.com/shell_integration/install_shell_integration.sh | bash\r".to_string()
            }
            Plan::Pwsh => {
                let dir = std::env::var_os("APPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir).join("Tandem").join("shell_integration");
                let path = dir.join("tandem_shell_integration.ps1");
                if !ask(&format!("Tandem will save its PowerShell script to\n{}\nand add a line that loads it to your PowerShell profile ($PROFILE).\n\nIf PowerShell's execution policy blocks scripts (Restricted or AllSigned), Tandem also sets it to RemoteSigned for your user only. Continue?", path.display())) {
                    return;
                }
                if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, PWSH_SCRIPT)) {
                    return self.banners.set("install", format!("Could not save {}: {e}", path.display()));
                }
                pwsh_install_command(&path)
            }
        };
        if let Some(pane) = self.focused_pane() {
            pane.scroll(Scroll::Bottom);
            pane.input(typed.as_bytes());
        }
    }
}

fn toggle_fullscreen(window: Option<&Window>) {
    if let Some(w) = window {
        w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) });
    }
}

pub(super) fn info_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_OK | MB_ICONINFORMATION) };
}
