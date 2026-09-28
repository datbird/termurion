//! iTerm2's menu bar and right-click menu, and running its menu commands.

use std::collections::HashMap;

use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::vte::ansi::{CharsetIndex, Handler, StandardCharset};
use muda::{CheckMenuItem, ContextMenu, Icon, IconMenuItem, IsMenuItem, Menu, MenuItem, MenuTheme, PredefinedMenuItem, Submenu};
use winit::event_loop::ActiveEventLoop;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Fullscreen;

use super::*;
use crate::commands::{self, Status};
use crate::layout::{Dir, visible_rects};
use crate::marks::TOGGLES;
use crate::menus::{self, Chord, Item};

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

/// Windows shows "&" as a mnemonic marker, so a literal "&" is written "&&". Mac words read
/// as Windows ones ("Finder" is "Explorer").
fn menu_text(item: &Item) -> String {
    let label = menus::shown_label(item.label()).replace('&', "&&");
    match item.chord().filter(menus::reaches_tandem) {
        Some(c) => format!("{label}\t{}", menus::chord_label(&c)),
        None => label,
    }
}

pub(super) struct MenuBar {
    /// The window's menu bar (hidden in full screen, `App::sync_menu_hidden`).
    pub(super) menu: Menu,
    checks: HashMap<String, CheckMenuItem>,
    /// Edit > Named Marks, rebuilt as marks change, and what it lists now.
    pub(super) named: Option<Submenu>,
    pub(super) named_items: Vec<(u32, String)>,
    /// The focused pane and its named marks when the menu was last built.
    pub(super) named_key: (Option<PaneId>, Vec<(u32, Option<String>)>),
    /// The Profiles menu, its fixed items, and the profiles listed after them.
    pub(super) profiles: Option<(Submenu, usize)>,
    pub(super) profile_items: Vec<(String, String)>,
    /// Window > Arrangements > Restore Window Arrangement and ... as Tabs, and what they list.
    pub(super) restore: Option<Submenu>,
    pub(super) restore_tabs: Option<Submenu>,
    pub(super) arrangement_items: Vec<(String, bool)>,
    pub(super) arrangements_filled: bool,
    /// Session > Buried Sessions and what it lists: (menu id, label); an empty id is a line.
    pub(super) buried: Option<Submenu>,
    pub(super) buried_items: Vec<(String, String)>,
    /// Edit > Snippets and Edit > Actions, and what they list.
    pub(super) snippets: Option<Submenu>,
    pub(super) snippet_items: Option<Vec<crate::snippets::MenuEntry>>,
    pub(super) actions: Option<Submenu>,
    pub(super) action_items: Vec<(String, String)>,
    /// View > Toolbelt and its fixed items, then its tool items (by tool name) after them.
    pub(super) toolbelt: Option<(Submenu, usize)>,
    pub(super) toolbelt_checks: Vec<(String, CheckMenuItem)>,
    /// Scripts > Manage > Install Python Runtime, retitled "Check for Updated Runtime" once uv
    /// is installed.
    pub(super) install_runtime: Option<MenuItem>,
    /// iTerm2 > Suppressed Alerts, retitled with the count and time (`retitle_suppressed_menu`).
    pub(super) suppressed: Option<MenuItem>,
    /// The Scripts menu and its fixed items (Manage and the line), then the scripts after them.
    pub(super) scripts: Option<(Submenu, usize)>,
    pub(super) script_entries: Vec<super::scripts_app::MenuEntry>,
    /// The Window menu and its fixed items, then a line and the windows after them, and what
    /// it lists: (registry key, text, checked).
    pub(super) window_list: Option<(Submenu, usize)>,
    pub(super) window_items: Vec<(u64, String, bool)>,
}

/// Menu ids of the Profiles menu's profile entries.
pub(super) const PROFILE_ID: &str = "profile-open:";

/// A window's Win32 handle.
pub(super) fn hwnd_of(window: &Window) -> Option<isize> {
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

impl App {
    /// This window's Win32 handle.
    pub(super) fn hwnd(&self) -> Option<isize> {
        hwnd_of(&self.gpu.as_ref()?.window)
    }

    /// Builds the menu bar from iTerm2's menus, the right-click menu, and the Win chord hook.
    pub(super) fn install_menus(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let menu = Menu::new();
        let mut subs: HashMap<Vec<String>, Submenu> = HashMap::new();
        let mut checks = HashMap::new();
        let mut install_runtime = None;
        let mut suppressed = None;
        for item in menus::items() {
            let (parent, entry): (&[String], Box<dyn IsMenuItem>) = if item.separator {
                (&item.path[..], Box::new(PredefinedMenuItem::separator()))
            } else if item.submenu {
                let s = Submenu::new(item.label().replace('&', "&&"), true);
                subs.insert(item.path.clone(), s.clone());
                (&item.path[..item.path.len() - 1], Box::new(s))
            } else {
                let enabled = Self::menu_enabled(item);
                let parent = &item.path[..item.path.len() - 1];
                let alert_check = super::marks_ui::CHECKS.iter().any(|(s, _)| *s == item.selector) || super::seltools_ui::CHECKED.contains(&item.selector.as_str()) || matches!(item.selector.as_str(), "toggleMultiLinePasteWarning:" | "promptToConvertTabsToSpacesWhenPasting:");
                let state_check = parent.iter().any(|p| p == "Terminal State") && item.label() != "Reset";
                if alert_check || state_check {
                    let c = CheckMenuItem::with_id(item.id(), menu_text(item), enabled, false, None);
                    checks.insert(item.label().to_string(), c.clone());
                    (parent, Box::new(c))
                } else {
                    let m = MenuItem::with_id(item.id(), menu_text(item), enabled, None);
                    if item.selector == "installPythonRuntime:" {
                        install_runtime = Some(m.clone());
                    }
                    if item.selector == "showSuppressedAlerts:" {
                        suppressed = Some(m.clone());
                    }
                    (parent, Box::new(m))
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
        let hotkeys = self.proxy.clone();
        crate::winkeys::set_hotkey_sink(Box::new(move |ids| {
            let _ = hotkeys.lock().unwrap().send_event(UserEvent::Hotkey(ids));
        }));
        match crate::winkeys::install(hwnd, send, bound, self.remap_tags.clone(), exempt) {
            Err(e) => self.banners.set("keys", format!("Win chords are unavailable: {e}")),
            Ok(()) if !self.hotkeys_registered => {
                self.hotkeys_registered = true;
                self.register_hotkeys();
            }
            Ok(()) => {}
        }
        let toolbelt = subs.get(&["View", "Toolbelt"].map(str::to_string).to_vec()).cloned().map(|s| {
            let fixed = s.items().len();
            (s, fixed)
        });
        let toolbelt_checks = toolbelt.clone().map(|(s, _)| self.append_toolbelt_menu(&s)).unwrap_or_default();
        let named = subs.get(&vec!["Edit".to_string(), "Named Marks".to_string()]).cloned();
        let profiles = subs.get(&vec!["Profiles".to_string()]).cloned().map(|s| {
            let fixed = s.items().len();
            (s, fixed)
        });
        let scripts = subs.get(&vec!["Scripts".to_string()]).cloned().map(|s| {
            let fixed = s.items().len();
            (s, fixed)
        });
        let window_list = subs.get(&vec!["Window".to_string()]).cloned().map(|s| {
            let fixed = s.items().len();
            (s, fixed)
        });
        self.menu_bar = Some(MenuBar { menu, checks, named, named_items: Vec::new(), named_key: (None, vec![(0, None)]), profiles, profile_items: Vec::new(), restore: subs.get(&["Window", "Arrangements", "Restore Window Arrangement"].map(str::to_string).to_vec()).cloned(), restore_tabs: subs.get(&["Window", "Arrangements", "Restore Window Arrangement as Tabs"].map(str::to_string).to_vec()).cloned(), arrangement_items: Vec::new(), arrangements_filled: false, buried: subs.get(&["Session", "Buried Sessions"].map(str::to_string).to_vec()).cloned(), buried_items: vec![(String::from("-"), String::new())], snippets: subs.get(&["Edit", "Snippets"].map(str::to_string).to_vec()).cloned(), snippet_items: None, actions: subs.get(&["Edit", "Actions"].map(str::to_string).to_vec()).cloned(), action_items: vec![(String::from("-"), String::from("-"))], toolbelt, toolbelt_checks, install_runtime, suppressed, scripts, script_entries: Vec::new(), window_list, window_items: Vec::new() });
        self.runtime_menu_title();
        self.retitle_suppressed_menu();
        self.sync_checks();
    }

    /// iTerm2's right-click menu, after `extra` items (label, what it does) and a separator,
    /// with the script providers' items (label, what it does) before Split Pane.
    pub(super) fn show_context_menu_with(&mut self, extra: &[(String, super::semantic_ui::CtxItem)], providers: &[(String, super::semantic_ui::CtxItem)]) {
        let Some(hwnd) = self.hwnd() else { return };
        let context = Menu::new();
        let mut results = Vec::new();
        let own = |(label, item): &(String, super::semantic_ui::CtxItem)| MenuItem::with_id(item.id(), label.replace('&', "&&"), true, None);
        for slot in context_layout(extra.len(), providers.len()) {
            results.push(match slot {
                Slot::Separator => context.append(&PredefinedMenuItem::separator()),
                Slot::Extra(i) => context.append(&own(&extra[i])),
                Slot::Provider(i) => context.append(&own(&providers[i])),
                Slot::Command(id) => match menus::by_id(id) {
                    None => context.append(&PredefinedMenuItem::separator()),
                    Some(item) => {
                        let enabled = !matches!(commands::status(item), Status::Later(_));
                        context.append(&MenuItem::with_id(item.id(), menu_text(item), enabled, None))
                    }
                },
            });
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Context menu: {e}"));
        }
        unsafe { context.show_context_menu_for_hwnd(hwnd, None) };
        // The pick arrives after the menu closes, so the menu lives until the next one.
        self.popup_menu = Some(context);
    }

    /// The Window menu's window list, in every window's menu bar: a line after the fixed
    /// items, then each terminal window by number, the focused one checked (`PT:3069-3084`,
    /// `:6480-6485`).
    pub(super) fn refresh_window_list(&mut self) {
        let focused = self.last_window.map(super::windows_ui::key);
        let items = window_list(&self.window_menu_entries(), focused, self.globals.bool("IncludeShortcutInWindowsMenu"), self.globals.int("SwitchWindowModifier"));
        let mut failed = None;
        for bar in self.menu_bars_mut() {
            if bar.window_items == items {
                continue;
            }
            let Some((sub, fixed)) = &bar.window_list else { continue };
            while sub.remove_at(*fixed).is_some() {}
            if !items.is_empty() {
                let _ = sub.append(&PredefinedMenuItem::separator());
            }
            for (k, text, checked) in &items {
                let entry = CheckMenuItem::with_id(format!("{}{k}", crate::wintitle::LIST_PREFIX), text.replace('&', "&&"), true, *checked, None);
                if let Err(e) = sub.append(&entry) {
                    failed = Some(e);
                }
            }
            bar.window_items = items.clone();
        }
        if let Some(e) = failed {
            self.banners.set("menu", format!("Window menu: {e}"));
        }
    }

    /// A window picked from the Window menu comes forward (un-minimized; a hidden hotkey
    /// window shows as its hotkey shows it). False for other ids.
    fn window_list_picked(&mut self, id: &str) -> bool {
        let Some(k) = id.strip_prefix(crate::wintitle::LIST_PREFIX).and_then(|k| k.parse::<u64>().ok()) else { return false };
        if let Some(w) = self.window_id_of_key(k) {
            self.focus_terminal_window(w);
        }
        // The pick toggled its item's check: every list is rebuilt from the windows.
        for bar in self.menu_bars_mut() {
            bar.window_items.clear();
        }
        self.refresh_window_list();
        true
    }

    /// Edit Session for the session `pane` (`PT editSession:makeKey:`): Settings opened at the
    /// profile it uses; Settings alone without a session.
    pub(super) fn edit_session(&mut self, pane: Option<PaneId>, event_loop: &ActiveEventLoop) {
        let guid = pane.and_then(|id| self.panes.get(&id)).map(|p| self.store.shared_guid(&p.profile));
        let tmux = pane.is_some_and(|id| self.panes.get(&id).is_some_and(|p| p.tmux_io().is_some()));
        self.open_settings(event_loop, Mode::Settings);
        if let (Some(s), Some(g)) = (self.settings.as_mut(), guid) {
            s.select(&g);
            // iTerm2's `tmux:self.isTmuxClient` (PTYSession.m:12333-12338).
            s.set_tmux_session(tmux);
        }
    }

    /// A context menu built from `entries` (`tab_menu::Entry`) as a popup at the pointer;
    /// `what` names it in an error banner. A label's "&" is doubled so Windows shows it.
    pub(super) fn show_entries(&mut self, entries: &[crate::tab_menu::Entry], what: &str) {
        let Some(hwnd) = self.hwnd() else { return };
        let menu = Menu::new();
        let mut results = Vec::new();
        for e in entries {
            let item = entry_item(e, &mut results);
            results.push(menu.append(item.as_ref()));
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("{what}: {e}"));
        }
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
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

    /// The tabs that do not fit the bar, as a popup from its overflow button: each tab's title
    /// as plain text with its number, the selected tab checked (`PSM _setupCells:`). The picks
    /// come back as `tab-overflow:<index>`.
    pub(super) fn show_tab_overflow(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let html = self.globals.bool("HTMLTabTitles");
        let menu = Menu::new();
        let mut results = Vec::new();
        for &i in &self.tab_bar.hidden {
            let Some(t) = self.tabs.get(i) else { continue };
            let look = self.tab_look(t);
            let mut title = crate::tab_layout::plain_title(&crate::tab_layout::limit_title(&look.title), html);
            // The number is 0 with `HideTabNumber` (`TAB:1303-1305`).
            if look.number > 0 && !self.globals.bool("HideTabNumber") {
                title.push_str(&format!(" ({})", look.number));
            }
            results.push(menu.append(&CheckMenuItem::with_id(format!("tab-overflow:{i}"), title.replace('&', "&&"), true, i == self.active, None)));
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Tab menu: {e}"));
        }
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
    }

    /// The Tab Color picker as a popup at the pointer: for a tab "Default" (no color) first,
    /// then a 16 by 16 swatch per color labeled "#rrggbb", then "Choose Color\u{2026}"; a group
    /// always has a color, so it has no "Default". The picks come back as `color-pick:<n>`.
    /// 13d's tab and tab group menus open it.
    pub(super) fn open_color_picker(&mut self, target: ColorTarget, swatches: Vec<crate::palette::Rgb>) {
        let Some(hwnd) = self.hwnd() else { return };
        let menu = Menu::new();
        let mut results = Vec::new();
        for (n, (label, color, _)) in picker_items(&swatches, matches!(target, ColorTarget::Tab(_))).into_iter().enumerate() {
            let id = format!("color-pick:{n}");
            results.push(match color {
                Some(c) => menu.append(&IconMenuItem::with_id(id, label, true, swatch_icon(c), None)),
                None => menu.append(&MenuItem::with_id(id, label, true, None)),
            });
        }
        if let Some(Err(e)) = results.into_iter().find(Result::is_err) {
            self.banners.set("menu", format!("Tab Color menu: {e}"));
        }
        self.color_pick = Some((target, swatches));
        unsafe { menu.show_context_menu_for_hwnd(hwnd, None) };
        self.popup_menu = Some(menu);
    }

    /// A pick from the Tab Color picker: "Default" clears the color, a swatch sets it, and
    /// "Choose Color\u{2026}" asks Windows' color dialog (the swatches as its custom colors;
    /// Cancel changes nothing).
    fn color_picked(&mut self, id: &str) -> bool {
        let Some(n) = id.strip_prefix("color-pick:").and_then(|n| n.parse::<usize>().ok()) else { return false };
        let Some((target, swatches)) = self.color_pick.take() else { return true };
        let Some((_, color, kind)) = picker_items(&swatches, matches!(target, ColorTarget::Tab(_))).into_iter().nth(n) else { return true };
        match kind {
            PickKind::Default | PickKind::Swatch => self.apply_picked_color(target, color),
            PickKind::Choose => {
                let current = match &target {
                    ColorTarget::Tab(id) => self.tabs.iter().find(|t| t.id == *id).and_then(|t| self.tab_look(t).color),
                    ColorTarget::Group(g) => self.tabs.iter().find_map(|t| t.group.as_ref().filter(|x| x.id == *g)).map(|x| x.color),
                };
                if let Some(c) = self.hwnd().and_then(|h| choose_color(h, current, &swatches)) {
                    self.apply_picked_color(target, Some(c));
                }
            }
        }
        true
    }

    /// Colors what the picker was opened for: a tab (nothing when it has closed since), or a
    /// tab group.
    pub(super) fn apply_picked_color(&mut self, target: ColorTarget, color: Option<crate::palette::Rgb>) {
        match target {
            ColorTarget::Tab(id) => {
                if let Some(tab) = self.tabs.iter().position(|t| t.id == id) {
                    self.set_tab_color_for(tab, color);
                }
            }
            // A group always has a color: the picker offers no "Default" for it.
            ColorTarget::Group(g) => {
                if let Some(c) = color {
                    self.set_group_color(&g, c);
                }
            }
        }
    }

    /// A tab picked from the overflow popup is selected.
    fn tab_overflow_picked(&mut self, id: &str) -> bool {
        let Some(i) = id.strip_prefix("tab-overflow:").and_then(|i| i.parse::<usize>().ok()) else { return false };
        if i < self.tabs.len() {
            self.select_tab(i);
            self.redraw();
        }
        true
    }

    /// Whether the menu bar's check item for this label is checked (false when the item has
    /// no check mark).
    pub(super) fn menu_checked(&self, label: &str) -> bool {
        self.menu_bar.as_ref().and_then(|b| b.checks.get(label)).is_some_and(|c| c.is_checked())
    }

    /// Whether a menu bar item is enabled: the rule `install_menus` builds the bar with.
    pub(super) fn menu_enabled(item: &Item) -> bool {
        let sel = item.selector.as_str();
        !matches!(commands::status(item), Status::Later(_)) && !commands::DISABLED_IN_SESSION.contains(&sel) && !commands::DISABLED_UNDER_UV.contains(&sel)
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
        self.refresh_scripts_menu();
        self.refresh_arrangement_menus();
        self.refresh_buried_menu();
        self.refresh_snippet_menus();
        self.sync_toolbelt_checks();
        self.refresh_window_list();
        self.menus_dirty = false;
        let Some(bar) = &self.menu_bar else { return };
        for selector in super::seltools_ui::CHECKED {
            let Some(item) = menus::items().iter().find(|i| i.selector == *selector) else { continue };
            if let Some(c) = bar.checks.get(item.label()) {
                c.set_checked(self.is_checked(selector));
            }
        }
        for item in menus::items().iter().filter(|i| matches!(i.selector.as_str(), "toggleMultiLinePasteWarning:" | "promptToConvertTabsToSpacesWhenPasting:")) {
            if let (Some(c), Some(on)) = (bar.checks.get(item.label()), self.paste_checked(item.label())) {
                c.set_checked(on);
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
        // Win+Z undoes in the Snippets or Actions tool while it has the keyboard.
        if self.toolbelt_chord(chord) {
            return;
        }
        // A composer's own Win chords (Win+W closes a dropdown; Win+Y, Win+Alt+V, the marks,
        // the history).
        if self.composer_chord(chord) {
            return;
        }
        self.banners.clear("action");
        self.dispatch(crate::keyroute::press_from_chord(chord), None, None, Some(chord), event_loop);
    }

    pub(super) fn run_menu_id(&mut self, id: &str, event_loop: &ActiveEventLoop) {
        if self.tab_menu_picked(id, event_loop) {
            return;
        }
        if self.buried_picked(id) || self.script_picked(id) {
            return;
        }
        if self.arrangement_picked(id) || self.window_list_picked(id) {
            return;
        }
        if let Some(guid) = id.strip_prefix(PROFILE_ID) {
            return self.open_profile_from_menu(guid);
        }
        if self.snippet_or_action_picked(id, event_loop) || self.toolbelt_picked(id, event_loop) {
            return;
        }
        if self.find_mode_picked(id) || self.context_picked(id) || self.named_mark_picked(id) || self.porthole_language_picked(id) || self.tab_overflow_picked(id) || self.color_picked(id) || self.status_bar_picked(id, event_loop) {
            return;
        }
        if let Some(item) = menus::by_id(id) {
            self.run_command(item, event_loop);
        }
    }

    /// The focused pane's saved profile (a session copy's original): what new tabs,
    /// splits, and Edit Session use.
    pub(super) fn focused_guid(&self) -> Option<String> {
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
        if self.paste_command(&item.selector, label) {
            return self.sync_checks();
        }
        if self.run_find_command(&item.selector, label) || self.run_seltools_command(&item.selector) || self.run_copymode_command(&item.selector) || self.run_fold_command(&item.selector) || self.run_view_command(&item.selector) || self.run_note_command(&item.selector, event_loop) {
            return self.relayout();
        }
        // Commands that change the terminal itself act on the pane's own screen, and do
        // nothing on a replaying pane (iTerm2 sends them to the shell-less replay view).
        let sel = item.selector.as_str();
        if crate::replay::blocks_command(sel) {
            if let Some(id) = self.focused_id() {
                if self.panes.get(&id).is_some_and(|p| p.replay.is_some()) {
                    return;
                }
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
            "openQuickly:" => self.open_quickly(event_loop),
            "installPythonRuntime:" => self.install_python_runtime(),
            "revealScriptsInFinder:" => self.reveal_scripts_folder(),
            "newPythonScript:" => self.new_python_script(event_loop),
            "openREPL:" => self.open_repl(),
            "importScript:" => self.import_scripts(),
            "exportScript:" => self.export_scripts(event_loop),
            "openDependencyEditor:" => self.open_dependency_editor(event_loop),
            "openScriptConsole:" => self.open_script_console(event_loop),
            "showSuppressedAlerts:" => self.show_suppressed_alerts(event_loop),
            "editCurrentSession:" => self.edit_session(self.focused_id(), event_loop),
            "showBookmarkWindow:" | "openSplitHorizontallySheet:" | "openSplitVerticallySheet:" => {
                self.open_settings(event_loop, Mode::Profiles)
            }
            "terminate:" => self.quit(event_loop),
            "moveTabToNewWindow:" if self.layout_allowed(crate::winmodel::LayoutOp::MoveTab) => self.move_tab_to_new_window(),
            "moveSessionToWindow:" if self.layout_allowed(crate::winmodel::LayoutOp::MovePane) => self.move_session_to_window(),
            "moveSessionToTab:" if self.layout_allowed(crate::winmodel::LayoutOp::MovePane) => self.move_session_to_tab(),
            "mergeAllWindows:" if self.layout_allowed(crate::winmodel::LayoutOp::MoveTab) => self.merge_all_windows(),
            "moveTabToNewWindow:" | "moveSessionToWindow:" | "mergeAllWindows:" => {}
            "arrangeHorizontally:" => self.arrange_windows_horizontally(),
            s if self.broadcast_command(s) => {}
            "togglePinHotkeyWindow:" => {
                if let Some(g) = self.hotkey.as_ref().map(|h| h.guid.clone()) {
                    self.toggle_pin_hotkey(&g);
                }
            }
            "toggleSizeLocked:" => self.toggle_lock(false),
            "toggleLayoutLocked:" => self.toggle_lock(true),
            "decreaseHeight:" => self.resize_window_by_cell(crate::winmodel::Resize::Height(-1)),
            "increaseHeight:" => self.resize_window_by_cell(crate::winmodel::Resize::Height(1)),
            "decreaseWidth:" => self.resize_window_by_cell(crate::winmodel::Resize::Width(-1)),
            "increaseWidth:" => self.resize_window_by_cell(crate::winmodel::Resize::Width(1)),
            "editWindowTitle:" => self.open_title_prompt(event_loop, false),
            "editTabTitle:" => self.open_title_prompt(event_loop, true),
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
            "toggleFullScreenMode:" => {
                let entering = window.as_ref().is_some_and(|w| w.fullscreen().is_none());
                toggle_fullscreen(window.as_deref());
                // Entering full screen flashes the tab bar (`PseudoTerminal+WindowStyle.m:857`).
                if entering {
                    self.flash_tab_bar();
                }
            }
            "toggleFullScreenTabBar:" => self.toggle_full_screen_tab_bar(),
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
            // New Tab appends; from a group's last tab it joins that group (`AD newTabAtIndex:`).
            "newSession:" => {
                let (before, active) = (self.tabs.len(), self.active);
                let group = self.tabs.get(active).and_then(|t| t.group.as_ref()).map(|g| g.id.clone());
                self.new_tab(None);
                if self.tabs.len() > before {
                    self.new_tab_joins_group(self.tabs.len() - 1, active, group);
                }
            }
            "newWindow:" => {
                if self.tmux_new(true, true).is_none() {
                    self.new_window(event_loop, None);
                }
            }
            "detachTmux:" => self.tmux_request_detach(),
            // A replay view cannot be buried (`textViewCanBury`, `PS:14820-14822`): refused
            // while the tab holds a replaying pane or the focused pane is a loaded recording.
            "bury:" => {
                if let Some(id) = self.focused_id().filter(|id| !self.bury_refused(*id)) {
                    self.bury_pane(id);
                }
            }
            "openDashboard:" => self.open_tmux_dashboard(event_loop),
            "toggleTmuxPausePane:" => self.tmux_toggle_pause(),
            "forceDetachTmux:" => self.tmux_force_detach(),
            "newTmuxWindow:" => {
                self.tmux_new(true, false);
            }
            "newTmuxTab:" => {
                self.tmux_new(false, false);
            }
            "newSessionAlternate:" => {
                let (before, active) = (self.tabs.len(), self.active);
                let group = self.tabs.get(active).and_then(|t| t.group.as_ref()).map(|g| g.id.clone());
                self.new_tab(None);
                if let Some(at) = super::move_new_tab(&mut self.tabs, before, active) {
                    self.active = at;
                    self.new_tab_joins_group(at, active, group);
                }
            }
            "newWindowWithSameProfile:" => {
                let g = self.focused_guid();
                self.new_window(event_loop, g.as_deref());
            }
            "duplicateTab:" => self.duplicate_tab(self.active),
            "newSessionWithSameProfile:" | "duplicateSession:" => {
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
                if let Some(id) = self.focused_id().filter(|_| self.layout_allowed(crate::winmodel::LayoutOp::ClosePane)).filter(|id| self.confirm_close(close_ui::Closing::Session(*id))) {
                    self.close_session_undoable(id, event_loop);
                }
            }
            "closeCurrentTab:" => self.close_tab_at(self.active, event_loop),
            "undoCloseSession:" => self.undo_close(),
            "saveWindowArrangement:" => self.open_save_arrangement(event_loop, true),
            "saveCurrentWindowAsArrangement:" => self.open_save_arrangement(event_loop, false),
            "importWindowArrangement:" => self.load_arrangement_file(),
            "changeProfileInArrangement:" => self.open_settings_page(event_loop, crate::settings_ui::Page::Arrangements),
            // A session whose shell has exited restarts at once; a running one asks first
            // (`restartSessionWithConfirmation:`), once: iTerm2's question is modal, so a
            // second pick while the pane's question waits does nothing. The API's
            // RestartSession never asks.
            "restartSession:" => {
                if let Some(id) = self.focused_id() {
                    if restart_asks(self.panes.get(&id).is_some_and(|p| p.exited.is_some())) {
                        if !self.alert_waits(|then| *then == super::scripts_alerts::Then::Restart(id)) {
                            self.ask_scripts(super::scripts_alerts::Ask::new(restart_spec(), super::scripts_alerts::Then::Restart(id)));
                        }
                    } else {
                        self.restart_pane(id);
                    }
                }
            }
            "copy:" => {
                if self.composer_copy_menu() {
                    return;
                }
                if let Some(id) = self.focused_id() {
                    self.copy_selection(id);
                    // Copy after a drag a program took as mouse reports (iTerm2's Cmd-C).
                    self.frustration_copy(id);
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
            "paste:" => {
                if !self.composer_paste_menu() {
                    self.paste();
                }
            }
            "pasteOptions:" => {
                if self.composer_advanced_paste_menu() {
                    return;
                }
                if let Some(id) = self.focused_id() {
                    self.open_advanced_paste(event_loop, id, None);
                }
            }
            "openPasteHistory:" => self.open_paste_history(event_loop),
            "openCommandHistory:" => self.open_command_history(event_loop),
            // View > Show Composer: disabled (does nothing) while the session wants an Auto
            // Composer (`PT:14153`).
            "compose:" => {
                if let Some(id) = self.focused_id().filter(|id| !self.should_show_auto_composer(*id)) {
                    self.compose(id);
                }
            }
            "toggleAutoComposer:" => self.toggle_auto_composer(),
            "openDirectories:" => self.open_recent_dirs(event_loop),
            "openAutocomplete:" => self.open_autocomplete(),
            "toggleShowClippings:" => self.toggle_clippings(),
            "toggleToolbeltVisibility:" => self.toggle_toolbelt(),
            "setDefaultToolbeltWidth:" => self.set_default_toolbelt_width(),
            "toggleAutoCommandHistory:" => {
                let on = !self.globals.bool("AutoCommandHistory");
                self.globals.set("AutoCommandHistory", toml::Value::Boolean(on));
                self.save_globals();
                if !on {
                    self.completion = None;
                }
                self.menus_dirty = true;
            }
            "pasteSelection:" => {
                let text = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).and_then(|p| p.screen.term().selection_to_string());
                if let (Some(text), Some(id)) = (text, self.focused_id()) {
                    self.user_paste(id, &text, crate::broadcast::Write::Paste);
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
            "clearBuffer:" => self.clear_or_reset(true),
            "clearInstantReplay:" => self.clear_instant_replay(),
            "exportRecording:" => self.export_recording_menu(),
            "loadRecording:" => self.import_recording(None),
            // Window > Password Manager: no account, the window's current session
            // (`openPasswordManager:`, `AD:2815-2819`).
            "openPasswordManager:" => self.open_password_manager(None, None),
            // Disabled while the focused pane replays (`PT:13940-13941`).
            "irPrev:" => {
                if self.focused_id().and_then(|id| self.panes.get(&id)).is_some_and(|p| p.replay.is_none()) {
                    self.ir_advance(-1, event_loop);
                }
            }
            "clearScrollbackBuffer:" => self.focused_pane().map_or((), |p| p.screen.clear(true)),
            "reset:" => self.clear_or_reset(false),
            "terminalStateReset:" => self.focused_pane().map_or((), |p| p.screen.soft_reset()),
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
            "maximizePane:" if self.tabs.get(self.active).is_some_and(|t| self.tmux_of(t.focused).is_some()) => {
                let f = self.tabs[self.active].focused;
                self.tmux_zoom(f);
            }
            "maximizePane:" => {
                if let Some(t) = self.tabs.get_mut(self.active) {
                    t.zoomed = if t.zoomed.is_some() { None } else { Some(t.focused) };
                }
            }
            "nextTab:" | "previousTab:" => {
                let n = self.tabs.len();
                if n > 0 {
                    self.select_tab(if item.selector == "nextTab:" { (self.active + 1) % n } else { (self.active + n - 1) % n });
                }
            }
            "moveTabLeft:" => self.move_active_tab(-1),
            "moveTabRight:" => self.move_active_tab(1),
            "nextTabGroup:" => self.select_tab_group(1),
            "previousTabGroup:" => self.select_tab_group(-1),
            "addTabToNewGroupAction:" => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.id) {
                    self.add_tab_to_new_group(event_loop, id);
                }
            }
            "removeTabFromGroupAction:" => {
                if let Some(id) = self.tabs.get(self.active).map(|t| t.id) {
                    self.remove_tab_from_group(id);
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
        if !self.prefs_exit_pending && self.confirm_close(close_ui::Closing::App) {
            self.quit_confirmed(event_loop);
        }
    }

    /// Quit once confirmed (the restart after Import All asks nothing).
    pub(super) fn quit_confirmed(&mut self, event_loop: &ActiveEventLoop) {
        // Already ending, waiting for the settings folder's quit question.
        if self.prefs_exit_pending {
            return;
        }
        self.save_pending_settings();
        // Before any window closes, so the saved state holds every window.
        self.save_state_for_quit();
        self.flush_notes();
        let keep = self.keep_jobs_on_quit();
        self.save_all_window_frames();
        self.clear_undo();
        if let Some(h) = self.hwnd() {
            crate::notify::remove(h);
        }
        if self.save_due.is_some() {
            self.save_now();
        }
        self.leave_host(keep);
        self.panes.clear();
        self.focus_order.clear();
        if self.prefs_before_exit() {
            self.catalog_flush();
            event_loop.exit();
        }
    }

    /// The rectangles the active tab shows (only the zoomed pane when one is zoomed).
    /// Each shown pane's terminal rectangle (a Clippings panel takes the right of it).
    pub(super) fn visible(&self, tab: &Tab) -> Vec<(PaneId, Rect)> {
        visible_rects(&tab.layout, tab.zoomed, self.content_area_for(tab), GAP).into_iter().map(|(id, r)| (id, self.clip_split(id, self.pane_bar_split(tab, id, r).0).0)).collect()
    }
}

/// One entry of the right-click menu: the nth extra item, the nth script provider item, a
/// menu command by id, or a separator.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Slot {
    Extra(usize),
    Provider(usize),
    Command(&'static str),
    Separator,
}

/// The right-click menu's order: `extra` items and a separator, then `commands::CONTEXT` with
/// the `providers` items and a separator after its first separator (iTerm2 puts them after
/// its custom actions, before Split Pane, `CMH:572-580`).
pub(super) fn context_layout(extra: usize, providers: usize) -> Vec<Slot> {
    let mut slots: Vec<Slot> = (0..extra).map(Slot::Extra).collect();
    if extra > 0 {
        slots.push(Slot::Separator);
    }
    let mut placed = providers == 0;
    for id in commands::CONTEXT {
        if id.is_empty() {
            slots.push(Slot::Separator);
            if !placed {
                slots.extend((0..providers).map(Slot::Provider));
                slots.push(Slot::Separator);
                placed = true;
            }
        } else {
            slots.push(Slot::Command(id));
        }
    }
    slots
}

/// What the Tab Color picker colors: a tab by its stable `Tab.id`, or a tab group by its id.
/// 13d's menus build them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ColorTarget {
    Tab(u64),
    Group(String),
}

/// A context menu entry as a muda item: a check item when it has a check mark, a submenu with
/// its entries (their append results go to `results`), or a separator.
fn entry_item(e: &crate::tab_menu::Entry, results: &mut Vec<muda::Result<()>>) -> Box<dyn IsMenuItem> {
    use crate::tab_menu::Entry;
    match e {
        Entry::Item { id, label, enabled, checked: Some(on) } => Box::new(CheckMenuItem::with_id(id.as_str(), label.replace('&', "&&"), *enabled, *on, None)),
        Entry::Item { id, label, enabled, checked: None } => Box::new(MenuItem::with_id(id.as_str(), label.replace('&', "&&"), *enabled, None)),
        Entry::Submenu { label, entries } => {
            let sub = Submenu::new(label.replace('&', "&&"), true);
            for child in entries {
                let item = entry_item(child, results);
                results.push(sub.append(item.as_ref()));
            }
            Box::new(sub)
        }
        Entry::Separator => Box::new(PredefinedMenuItem::separator()),
    }
}

/// What a Tab Color picker entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PickKind {
    /// No color.
    Default,
    /// Its color.
    Swatch,
    /// Windows' color dialog.
    Choose,
}

/// The Tab Color picker's entries in order, as (label, color, kind): "Default" first when
/// `with_default`, a "#rrggbb" entry per swatch, then "Choose Color\u{2026}".
pub(super) fn picker_items(swatches: &[crate::palette::Rgb], with_default: bool) -> Vec<(String, Option<crate::palette::Rgb>, PickKind)> {
    let mut items = Vec::new();
    if with_default {
        items.push((requests_ui::NO_COLOR.to_string(), None, PickKind::Default));
    }
    items.extend(swatches.iter().map(|c| (format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b), Some(*c), PickKind::Swatch)));
    items.push((requests_ui::CHOOSE_COLOR.to_string(), None, PickKind::Choose));
    items
}

/// A 16 by 16 square of the color, for a picker entry.
fn swatch_icon(c: crate::palette::Rgb) -> Option<Icon> {
    Icon::from_rgba([c.r, c.g, c.b, 0xff].repeat(16 * 16), 16, 16).ok()
}

/// Windows' color dialog, fully open, starting at `initial` with `swatches` (the first 16)
/// as its custom colors. None on Cancel.
fn choose_color(hwnd: isize, initial: Option<crate::palette::Rgb>, swatches: &[crate::palette::Rgb]) -> Option<crate::palette::Rgb> {
    use windows_sys::Win32::UI::Controls::Dialogs::{CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW, ChooseColorW};
    let colorref = |c: &crate::palette::Rgb| c.r as u32 | (c.g as u32) << 8 | (c.b as u32) << 16;
    let mut custom = [0x00ff_ffffu32; 16];
    for (slot, c) in custom.iter_mut().zip(swatches) {
        *slot = colorref(c);
    }
    let mut cc: CHOOSECOLORW = unsafe { std::mem::zeroed() };
    cc.lStructSize = std::mem::size_of::<CHOOSECOLORW>() as u32;
    cc.hwndOwner = hwnd as _;
    cc.lpCustColors = custom.as_mut_ptr();
    cc.Flags = CC_FULLOPEN;
    if let Some(c) = initial {
        cc.rgbResult = colorref(&c);
        cc.Flags |= CC_RGBINIT;
    }
    if unsafe { ChooseColorW(&mut cc) } == 0 {
        return None;
    }
    let v = cc.rgbResult;
    Some(crate::palette::rgb(v as u8, (v >> 8) as u8, (v >> 16) as u8))
}

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn context_layout_places_providers_before_split_pane() {
        use Slot::{Command, Extra, Provider, Separator};
        let today: Vec<Slot> = commands::CONTEXT.iter().map(|id| if id.is_empty() { Separator } else { Command(id) }).collect();
        // No providers: today's layout, the extra items and a separator first when there are some.
        assert_eq!(context_layout(0, 0), today);
        let mut with_extra = vec![Extra(0), Extra(1), Separator];
        with_extra.extend(today.iter().cloned());
        assert_eq!(context_layout(2, 0), with_extra);
        // Providers go after Copy and Paste's separator, with a separator after them.
        let mut want = vec![Extra(0), Separator, Command("Edit > Copy"), Command("Edit > Paste"), Separator, Provider(0), Provider(1), Separator];
        want.extend(today[3..].iter().cloned());
        assert_eq!(want[8], Command("Shell > Split Vertically with Current Profile"));
        assert_eq!(context_layout(1, 2), want);
    }
}

#[cfg(test)]
mod color_picker_tests {
    use super::*;
    use crate::palette::rgb;
    use crate::tab_layout::DEFAULT_COLORS;

    #[test]
    fn picker_follows_iterm2() {
        let swatches = [
            ("#fb6b62", rgb(0xfb, 0x6b, 0x62)),
            ("#f6ac47", rgb(0xf6, 0xac, 0x47)),
            ("#f0dc4f", rgb(0xf0, 0xdc, 0x4f)),
            ("#b5d749", rgb(0xb5, 0xd7, 0x49)),
            ("#5fa3f8", rgb(0x5f, 0xa3, 0xf8)),
            ("#c18ed9", rgb(0xc1, 0x8e, 0xd9)),
            ("#787878", rgb(0x78, 0x78, 0x78)),
        ];
        let rows: Vec<(String, Option<crate::palette::Rgb>, PickKind)> = swatches.iter().map(|(l, c)| (l.to_string(), Some(*c), PickKind::Swatch)).collect();
        // A tab: "Default" (no color) first, the swatches, then "Choose Color...".
        let mut want = vec![("Default".to_string(), None, PickKind::Default)];
        want.extend(rows.iter().cloned());
        want.push(("Choose Color\u{2026}".to_string(), None, PickKind::Choose));
        assert_eq!(picker_items(&DEFAULT_COLORS, true), want);
        // A group always has a color: no "Default".
        let mut want = rows.clone();
        want.push(("Choose Color\u{2026}".to_string(), None, PickKind::Choose));
        assert_eq!(picker_items(&DEFAULT_COLORS, false), want);
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
    fn install_command_follows_iterm2() {
        assert_eq!(curl_command(true), "curl -L https://iterm2.com/shell_integration/install_shell_integration_and_utilities.sh | bash\r");
        assert_eq!(curl_command(false), "curl -L https://iterm2.com/shell_integration/install_shell_integration.sh | bash\r");
        assert_eq!(INSTALL_UTILITIES_KEY, "NoSyncInstallUtilities");
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

/// Where "Also install iTerm2 Utilities" is remembered (`iTermShellIntegrationFirstPageViewController.m:19`).
pub(super) const INSTALL_UTILITIES_KEY: &str = "NoSyncInstallUtilities";

/// The one-liner typed for `Plan::Curl`: with the utilities, iTerm2's
/// `install_shell_integration_and_utilities.sh` (`iTermShellIntegrationDownloadAndRunViewController.m:28-33`).
pub(super) fn curl_command(utilities: bool) -> &'static str {
    if utilities {
        "curl -L https://iterm2.com/shell_integration/install_shell_integration_and_utilities.sh | bash\r"
    } else {
        "curl -L https://iterm2.com/shell_integration/install_shell_integration.sh | bash\r"
    }
}

/// Tandem's box for `Plan::Curl`: "Also install iTerm2 Utilities?" as Yes/No/Cancel.
const CURL_TEXT: &str = "Tandem will type iTerm2's installer into this session. It adds iTerm2's script to your shell's startup file.\n\nAlso install iTerm2 Utilities? Utilities include scripts to download files, display images inline, and more.\n\nYes types:\ncurl -L https://iterm2.com/shell_integration/install_shell_integration_and_utilities.sh | bash\n\nNo types:\ncurl -L https://iterm2.com/shell_integration/install_shell_integration.sh | bash";

/// Yes, No or Cancel for `text`, the default button Yes when `yes_first`, else No: Some(true),
/// Some(false), None.
fn ask_utilities(text: &str, yes_first: bool) -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDNO, IDYES, MB_DEFBUTTON1, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNOCANCEL, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let default = if yes_first { MB_DEFBUTTON1 } else { MB_DEFBUTTON2 };
    match unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Install Shell Integration").as_ptr(), MB_YESNOCANCEL | MB_ICONQUESTION | default) } {
        IDYES => Some(true),
        IDNO => Some(false),
        _ => None,
    }
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
    pub(super) fn install_shell_integration(&mut self) {
        if let Some(pane) = self.focused_id() {
            self.install_shell_integration_in(pane);
        }
    }

    /// Install Shell Integration for `pane`: types the installer for its profile's shell.
    pub(super) fn install_shell_integration_in(&mut self, pane: PaneId) {
        let Some(guid) = self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile)) else { return };
        let cmdline = crate::profile::command_line(&self.profile(Some(&guid)), &self.shell, None);
        let typed = match install_plan(&cmdline) {
            Plan::Unsupported => {
                return info_box("This shell has no prompt hooks, so it cannot have shell integration. Use PowerShell (pwsh), or bash, zsh, or fish in WSL.");
            }
            Plan::Curl => {
                let yes_first = self.globals.get(INSTALL_UTILITIES_KEY).and_then(|v| v.as_bool()).unwrap_or(true);
                let Some(utilities) = ask_utilities(CURL_TEXT, yes_first) else { return };
                self.globals.set(INSTALL_UTILITIES_KEY, toml::Value::Boolean(utilities));
                self.save_globals();
                curl_command(utilities).to_string()
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
        if let Some(p) = self.panes.get_mut(&pane) {
            p.scroll(Scroll::Bottom);
            p.input(typed.as_bytes());
        }
    }
}

pub(super) fn toggle_fullscreen(window: Option<&Window>) {
    if let Some(w) = window {
        w.set_fullscreen(if w.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) });
    }
}

pub(super) fn info_box(text: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONINFORMATION, MB_OK, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe { MessageBoxW(std::ptr::null_mut(), wide(text).as_ptr(), wide("Tandem").as_ptr(), MB_OK | MB_ICONINFORMATION) };
}

/// Whether Restart Session asks first: only while the session's shell runs (`PT:2851`).
pub(super) fn restart_asks(exited: bool) -> bool {
    !exited
}

/// The Window menu's window list (`PT:3069-3084`): (registry key, text, checked) for each
/// window by number. `entries` are (registry key, window number, the window's title as the
/// OS shows it without the label, its title without the number); `focused` is checked.
pub(super) fn window_list(entries: &[(u64, usize, String, String)], focused: Option<u64>, include: bool, tag: i64) -> Vec<(u64, String, bool)> {
    let mut sorted: Vec<&(u64, usize, String, String)> = entries.iter().collect();
    sorted.sort_by_key(|e| e.1);
    sorted.into_iter().map(|(k, n, text, title)| (*k, crate::wintitle::menu_entry(text, title, include, tag, *n), focused == Some(*k))).collect()
}

/// "Restart session?" (`PT:2855-2866`): the object API with no cancel label, so Cancel may be
/// remembered, as in iTerm2.
pub(super) fn restart_spec() -> crate::warnings::Spec {
    use crate::api_auth::{CANCEL, OK};
    crate::warnings::Spec {
        id: Some("NoSyncSuppressRestartSessionConfirmationAlert".to_string()),
        kind: crate::warnings::Kind::Permanent,
        heading: "Restart session?".to_string(),
        text: "Running jobs will be killed.".to_string(),
        labels: vec![OK.to_string(), CANCEL.to_string()],
        cancel: None,
        never_remember: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use crate::warnings::{Kind, checkbox_label};

    /// `restartSessionWithConfirmation:` (`PT:2848-2870`): built with the object API, which
    /// sets no cancel label, so Cancel may be remembered and two buttons count (Review Focus 2).
    #[test]
    fn restart_confirmation_follows_iterm2() {
        let s = super::restart_spec();
        assert_eq!((s.heading.as_str(), s.text.as_str()), ("Restart session?", "Running jobs will be killed."));
        assert_eq!(s.labels, ["OK", "Cancel"]);
        assert_eq!(s.id.as_deref(), Some("NoSyncSuppressRestartSessionConfirmationAlert"));
        assert_eq!((s.kind, s.cancel), (Kind::Permanent, None));
        assert!(s.never_remember.is_empty());
        assert!(s.rememberable(1), "Cancel may be remembered");
        assert_eq!(checkbox_label(&s), Some("Remember my choice"));
        assert!(!super::restart_asks(true), "an exited session restarts at once");
        assert!(super::restart_asks(false));
    }

    /// The Window menu's window list (`PT:3069-3084`): by window number, the current one
    /// checked.
    #[test]
    fn window_list_entries() {
        let entries = [(7, 1, "b".to_string(), "b".to_string()), (3, 0, "a".to_string(), "a".to_string())];
        let list = super::window_list(&entries, Some(3), true, 6);
        assert_eq!(list, [(3, "a \u{2014} Alt+Win+1".to_string(), true), (7, "b \u{2014} Alt+Win+2".to_string(), false)]);
        let list = super::window_list(&entries, Some(7), false, 6);
        assert_eq!(list, [(3, "a".to_string(), false), (7, "b".to_string(), true)]);
    }
}
