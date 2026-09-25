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


/// How far one Move Divider command moves a divider, as a share of its split.
const NUDGE: f32 = 0.05;

/// Windows shows "&" as a mnemonic marker, so a literal "&" is written "&&".
fn menu_text(item: &Item) -> String {
    let label = item.label().replace('&', "&&");
    match item.chord() {
        Some(c) => format!("{label}\t{}", menus::chord_label(&c)),
        None => label,
    }
}

pub(super) struct MenuBar {
    _menu: Menu,
    context: Menu,
    checks: HashMap<String, CheckMenuItem>,
}

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
                let enabled = !matches!(commands::status(item), Status::Later(_));
                let parent = &item.path[..item.path.len() - 1];
                let alert_check = super::marks_ui::CHECKS.iter().any(|(s, _)| *s == item.selector);
                if alert_check || (parent.last().is_some_and(|p| p == "Terminal State") && TOGGLES.iter().any(|t| t.0 == item.label())) {
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

        let context = Menu::new();
        for id in commands::CONTEXT {
            let result = match menus::by_id(id) {
                None => context.append(&PredefinedMenuItem::separator()),
                Some(item) => {
                    let enabled = !matches!(commands::status(item), Status::Later(_));
                    context.append(&MenuItem::with_id(item.id(), menu_text(item), enabled, None))
                }
            };
            if let Err(e) = result {
                self.banners.set("menu", format!("Context menu: {e}"));
            }
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
        self.menu_bar = Some(MenuBar { _menu: menu, context, checks });
        self.sync_checks();
    }

    pub(super) fn show_context_menu(&self) {
        if let (Some(bar), Some(hwnd)) = (&self.menu_bar, self.hwnd()) {
            unsafe { bar.context.show_context_menu_for_hwnd(hwnd, None) };
        }
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
        let alert_next = self.tabs.get(self.active).and_then(|t| self.panes.get(&t.focused)).is_some_and(|p| p.screen.marks.alert_next);
        let action = self.globals.string("Mark Alert Action");
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
        if self.find_mode_picked(id) {
            return;
        }
        if let Some(item) = menus::by_id(id) {
            self.run_command(item, event_loop);
        }
    }

    fn focused_guid(&self) -> Option<String> {
        let t = self.tabs.get(self.active)?;
        self.panes.get(&t.focused).map(|p| p.profile.clone())
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
        if self.run_find_command(&item.selector, label) {
            return self.relayout();
        }
        if self.run_mark_command(&item.selector, event_loop) {
            self.sync_checks();
            return self.relayout();
        }
        match item.selector.as_str() {
            "showAbout:" => info_box(&format!("Tandem {}\nAn iTerm2-style terminal for Windows.", env!("CARGO_PKG_VERSION"))),
            "showPrefWindow:" => self.open_settings(event_loop, Mode::Settings),
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
            "terminate:" | "closeTerminalWindow:" => self.quit(event_loop),
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
                if let Some(w) = &window {
                    match label {
                        "Full Screen" => w.set_fullscreen(Some(Fullscreen::Borderless(None))),
                        "Maximized" => {
                            w.set_fullscreen(None);
                            w.set_maximized(true);
                        }
                        _ => {
                            w.set_fullscreen(None);
                            w.set_maximized(false);
                        }
                    }
                }
            }
            "arrangeInFront:" => {
                if let Some(w) = &window {
                    w.focus_window();
                }
            }
            "newSession:" | "newWindow:" => self.new_tab(None),
            "newSessionAlternate:" => {
                let (before, active) = (self.tabs.len(), self.active);
                self.new_tab(None);
                if let Some(at) = super::move_new_tab(&mut self.tabs, before, active) {
                    self.active = at;
                }
            }
            "newSessionWithSameProfile:" | "newWindowWithSameProfile:" | "duplicateTab:" | "duplicateSession:" => {
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
            "closeCurrentSession:" => {
                if let Some(id) = self.focused_id() {
                    self.close_pane(id, event_loop);
                }
            }
            "closeCurrentTab:" => {
                if let Some(t) = self.tabs.get(self.active) {
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
