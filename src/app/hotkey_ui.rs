//! Hotkey windows in the window: registering the profiles' hotkeys, a press showing or
//! hiding a profile's dedicated window (with its siblings), the slide, auto-hide when focus
//! leaves, reopen when Tandem is active again, Pin Hotkey Window, and focus going back to
//! the app that was in front.

use std::time::{Duration, Instant};

use super::*;
use super::windows_ui::NewWindow;
use crate::hotkey::{self, Act, Prefs, Win};
use crate::hotkey_motion::{self, Motion};

/// A hotkey window's own state (kept in its `WinSlot`).
pub(super) struct HotkeyWin {
    /// The profile whose hotkey it belongs to.
    pub guid: String,
    /// Shown (not cloaked).
    pub shown: bool,
    pub roll: Option<Roll>,
    /// It hid because another app took focus (Reopen on activation brings it back).
    pub was_auto_hidden: bool,
    /// The window in front before it showed, to get focus back when the hotkey hides it.
    pub previous: Option<isize>,
}

pub(super) struct Roll {
    motion: Motion,
    start: Instant,
    hiding: bool,
    /// A hide by the hotkey returns focus to the previous window; an auto-hide does not.
    by_key: bool,
}

impl HotkeyWin {
    pub(super) fn new(guid: String) -> HotkeyWin {
        HotkeyWin { guid, shown: false, roll: None, was_auto_hidden: false, previous: None }
    }
}

/// The id of the app hotkey (Show/hide all windows); profile hotkeys count from 1.
pub(super) const APP_HOTKEY: u32 = 0;

fn cloak(hwnd: isize, on: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let v: i32 = on as i32;
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_CLOAK as u32, (&v as *const i32).cast(), 4) };
}

/// A window of Tandem is in front.
pub(super) fn tandem_in_front() -> bool {
    ours(foreground())
}

fn foreground() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

/// A window of this process (a terminal window, Settings, a dialog).
fn ours(hwnd: isize) -> bool {
    let mut pid = 0u32;
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd as _, &mut pid) };
    pid == std::process::id()
}

impl App {
    /// Every profile's hotkey settings.
    pub(super) fn hotkey_prefs(&self) -> Vec<(String, Prefs)> {
        self.store.profiles.iter().map(|p| (p.guid(), Prefs::from_profile(p))).collect()
    }

    /// Registers the profiles' hotkeys (and the app hotkey). Called once the keyboard hook
    /// runs and again when profiles or settings change. Failures are kept per profile for
    /// the Configure Hotkey Window sheet.
    pub(super) fn register_hotkeys(&mut self) {
        let mut chords = Vec::new();
        let mut taps = Vec::new();
        self.hotkey_ids.clear();
        if self.globals.bool("Hotkey") {
            if let Some(c) = hotkey::chord(&self.app_hotkey()) {
                chords.push((APP_HOTKEY, c));
            }
        }
        for (guid, prefs) in self.hotkey_prefs() {
            if !prefs.active() {
                continue;
            }
            self.hotkey_ids.push(guid);
            let id = self.hotkey_ids.len() as u32;
            chords.extend(prefs.shortcuts().into_iter().filter_map(hotkey::chord).map(|c| (id, c)));
            if let Some(m) = prefs.double_tap {
                taps.push((id, m));
            }
        }
        let secs = |k: &str, d: f64| Duration::from_secs_f64(self.globals.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(d).clamp(0.0, 5.0));
        let (min, max) = (secs("hotKeyDoubleTapMinDelay", 0.01), secs("hotKeyDoubleTapMaxDelay", 0.3));
        self.hotkey_errors.clear();
        match crate::winkeys::set_hotkeys(chords, taps, min, max) {
            Ok(results) => {
                for (id, r) in results {
                    if let Err(e) = r {
                        let owner = if id == APP_HOTKEY { "App Hotkey".to_string() } else { self.hotkey_ids.get(id as usize - 1).cloned().unwrap_or_default() };
                        self.hotkey_errors.insert(owner, e);
                    }
                }
                self.banners.clear("hotkeys");
            }
            Err(e) => self.banners.set("hotkeys", format!("Hotkeys are unavailable: {e}")),
        }
        if let Some(s) = self.settings.as_mut() {
            s.hotkeys.errors = self.hotkey_errors.clone();
        }
    }

    /// The app hotkey (Settings > Keys > Hotkey), iTerm2's global keys.
    pub(super) fn app_hotkey(&self) -> hotkey::Shortcut {
        let ch = self.globals.int("HotkeyChar");
        let text = char::from_u32(ch as u32).filter(|_| ch > 0).map(String::from).unwrap_or_default();
        hotkey::Shortcut { keycode: self.globals.int("HotkeyCode").clamp(0, u16::MAX as i64) as u16, flags: self.globals.int("HotkeyModifiers").max(0) as u64, chars: text.clone(), chars_ignoring: text }
    }

    /// The hotkey window of a profile, if one was made.
    fn hotkey_window_of(&self, guid: &str) -> Option<WindowId> {
        if self.hotkey.as_ref().is_some_and(|h| h.guid == guid) {
            return self.current_window();
        }
        self.windows.parked().find(|(_, s)| s.hotkey.as_ref().is_some_and(|h| h.guid == guid)).and_then(|(_, s)| s.gpu.as_ref().map(|g| g.window.id()))
    }

    /// Every hotkey window, shown or hidden.
    pub(super) fn hotkey_windows(&self) -> Vec<WindowId> {
        let mut out: Vec<WindowId> = self.windows.parked().filter(|(_, s)| s.hotkey.is_some()).filter_map(|(_, s)| s.gpu.as_ref().map(|g| g.window.id())).collect();
        if self.hotkey.is_some() {
            out.extend(self.current_window());
        }
        out
    }

    /// A registered hotkey was pressed.
    pub(super) fn hotkey_pressed(&mut self, id: u32) {
        if id == APP_HOTKEY {
            self.app_hotkey_pressed();
            return;
        }
        let Some(guid) = self.hotkey_ids.get(id as usize - 1).cloned() else { return };
        let all = self.hotkey_prefs();
        let mut group = vec![guid.clone()];
        group.extend(hotkey::siblings(&guid, &all));
        let fg = foreground();
        let mut wins = Vec::new();
        for g in &group {
            let pinned = all.iter().find(|(x, _)| x == g).is_some_and(|(_, p)| !p.auto_hides);
            let w = match self.hotkey_window_of(g) {
                Some(w) => {
                    self.enter(w);
                    let h = self.hotkey.as_ref().expect("a hotkey window");
                    // Never split a group mid-slide.
                    if h.roll.is_some() {
                        return;
                    }
                    Win { open: h.shown, focused: h.shown && self.hwnd() == Some(fg), pinned }
                }
                None => Win { open: false, focused: false, pinned },
            };
            wins.push(w);
        }
        for (g, act) in group.iter().zip(hotkey::press(&wins)) {
            match act {
                Act::Show => self.show_hotkey_window(g, fg),
                Act::Hide => {
                    if let Some(w) = self.hotkey_window_of(g) {
                        self.enter(w);
                        self.roll_out(true);
                    }
                }
                Act::Focus => {
                    if let Some(w) = self.hotkey_window_of(g) {
                        self.focus_terminal_window(w);
                    }
                }
                Act::Nothing => {}
            }
        }
    }

    fn show_hotkey_window(&mut self, guid: &str, fg: isize) {
        let previous = (!ours(fg)).then_some(fg);
        match self.hotkey_window_of(guid) {
            Some(w) => {
                self.enter(w);
                if let Some(h) = self.hotkey.as_mut() {
                    h.previous = previous;
                    h.was_auto_hidden = false;
                }
                self.roll_in();
            }
            // Made on the next turn of the event loop, then rolled in.
            None => self.pending_windows.push(NewWindow::Hotkey(guid.to_string(), previous)),
        }
    }

    /// Shows the current (hotkey) window: from its hidden place for the edge styles that
    /// slide, at once otherwise.
    pub(super) fn roll_in(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let guid = match &self.hotkey {
            Some(h) if !h.shown && h.roll.is_none() => h.guid.clone(),
            _ => return,
        };
        let prefs = self.store.get(&guid).map(Prefs::from_profile);
        let floats = prefs.as_ref().is_some_and(|p| p.floats);
        let animates = prefs.as_ref().is_some_and(|p| p.animates);
        // Placed while cloaked, so nothing flashes at the old place.
        cloak(hwnd, true);
        let Some(g) = &self.gpu else { return };
        g.window.set_visible(true);
        g.window.set_minimized(false);
        g.window.set_window_level(if floats { winit::window::WindowLevel::AlwaysOnTop } else { winit::window::WindowLevel::Normal });
        let shown = Self::outer_rect(g);
        let motion = self.slide_for(shown, animates).map(|hidden| Motion { from: hidden, to: (shown.x, shown.y), duration: self.hotkey_duration() });
        if let Some(m) = &motion {
            g.window.set_outer_position(winit::dpi::PhysicalPosition::new(m.from.0.round() as i32, m.from.1.round() as i32));
        }
        cloak(hwnd, false);
        g.window.focus_window();
        let now = Instant::now();
        if let Some(h) = self.hotkey.as_mut() {
            h.shown = true;
            h.roll = motion.map(|motion| Roll { motion, start: now, hiding: false, by_key: true });
        }
        if let Some(w) = self.current_window() {
            self.last_window = Some(w);
        }
        self.redraw();
    }

    /// Hides the current (hotkey) window. `by_key`: the hotkey hid it, so focus goes back to
    /// the window that was in front before it showed.
    pub(super) fn roll_out(&mut self, by_key: bool) {
        let guid = match &self.hotkey {
            Some(h) if h.shown && h.roll.is_none() => h.guid.clone(),
            _ => return,
        };
        let animates = self.store.get(&guid).is_some_and(|p| Prefs::from_profile(p).animates);
        let Some(g) = &self.gpu else { return };
        let shown = Self::outer_rect(g);
        match self.slide_for(shown, animates) {
            Some(hidden) => {
                let motion = Motion { from: (shown.x, shown.y), to: hidden, duration: self.hotkey_duration() };
                if let Some(h) = self.hotkey.as_mut() {
                    h.roll = Some(Roll { motion, start: Instant::now(), hiding: true, by_key });
                }
            }
            None => self.finish_hide(by_key, None),
        }
    }

    /// The hidden origin to slide from or to, or None to show and hide at once.
    fn slide_for(&self, shown: Rect, animates: bool) -> Option<(f32, f32)> {
        if !animates || !hotkey_motion::slides(self.win_style) {
            return None;
        }
        let monitors: Vec<Rect> = super::requests_ui::work_areas().into_iter().map(|(x, y, w, h)| Rect { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }).collect();
        let center = (shown.x + shown.w / 2.0, shown.y + shown.h / 2.0);
        let own = monitors.iter().copied().find(|m| center.0 >= m.x && center.0 < m.x + m.w && center.1 >= m.y && center.1 < m.y + m.h)?;
        let (x, y) = hotkey_motion::hidden_origin(self.win_style, shown, own);
        hotkey_motion::slide_allowed(Rect { x, y, ..shown }, own, &monitors).then_some((x, y))
    }

    fn hotkey_duration(&self) -> Duration {
        Duration::from_secs_f64(self.globals.get("hotkeyTermAnimationDuration").and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(0.25).clamp(0.0, 5.0))
    }

    /// Cloaks the current hotkey window and, after a hide by the hotkey, gives focus back.
    /// `back` is where a slide left it: the shown place, so the next show starts right.
    fn finish_hide(&mut self, by_key: bool, back: Option<(f32, f32)>) {
        let Some(hwnd) = self.hwnd() else { return };
        cloak(hwnd, true);
        if let (Some(g), Some((x, y))) = (&self.gpu, back) {
            g.window.set_outer_position(winit::dpi::PhysicalPosition::new(x.round() as i32, y.round() as i32));
        }
        let previous = self.hotkey.as_mut().and_then(|h| {
            h.shown = false;
            h.roll = None;
            h.previous.take()
        });
        if by_key {
            if let Some(p) = previous.filter(|p| unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindow(*p as _) } != 0) {
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(p as _) };
            }
        }
    }

    /// Moves every sliding hotkey window; the next wake while any still moves.
    pub(super) fn tick_hotkeys(&mut self) -> Option<Instant> {
        let back = self.current_window();
        let mut next = None;
        for w in self.hotkey_windows() {
            self.enter(w);
            let Some(roll) = self.hotkey.as_ref().and_then(|h| h.roll.as_ref()) else { continue };
            let ((x, y), done) = roll.motion.at(roll.start.elapsed());
            let (hiding, by_key, origin) = (roll.hiding, roll.by_key, roll.motion.from);
            if let Some(g) = &self.gpu {
                g.window.set_outer_position(winit::dpi::PhysicalPosition::new(x.round() as i32, y.round() as i32));
            }
            if !done {
                next = Some(Instant::now() + Duration::from_millis(8));
            } else if hiding {
                self.finish_hide(by_key, Some(origin));
            } else if let Some(h) = self.hotkey.as_mut() {
                h.roll = None;
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
        next
    }

    /// The current hotkey window lost focus: an auto-hiding one hides unless focus went to
    /// one of Tandem's own non-terminal windows (Settings, a dialog) or to a sibling.
    pub(super) fn hotkey_focus_lost(&mut self) {
        let guid = match &self.hotkey {
            Some(h) if h.shown && h.roll.is_none() => h.guid.clone(),
            _ => return,
        };
        if !self.store.get(&guid).is_some_and(|p| Prefs::from_profile(p).auto_hides) {
            return;
        }
        let fg = foreground();
        let fg_terminal = self.terminal_window_of_hwnd(fg);
        if ours(fg) {
            let Some(other) = fg_terminal else { return };
            let siblings = hotkey::siblings(&guid, &self.hotkey_prefs());
            let sibling = self.hotkey_windows().into_iter().any(|w| w == other) && {
                let me = self.current_window();
                self.enter(other);
                let s = self.hotkey.as_ref().is_some_and(|h| siblings.contains(&h.guid));
                if let Some(m) = me {
                    self.enter(m);
                }
                s
            };
            if sibling || Some(other) == self.current_window() {
                return;
            }
            self.roll_out(false);
        } else {
            self.roll_out(false);
            if let Some(h) = self.hotkey.as_mut() {
                h.was_auto_hidden = true;
            }
        }
    }

    /// The terminal window with this OS handle.
    fn terminal_window_of_hwnd(&self, hwnd: isize) -> Option<WindowId> {
        let of = |g: &Gpu| match winit::raw_window_handle::HasWindowHandle::window_handle(g.window.as_ref()).ok().map(|h| h.as_raw()) {
            Some(winit::raw_window_handle::RawWindowHandle::Win32(h)) if h.hwnd.get() == hwnd => Some(g.window.id()),
            _ => None,
        };
        self.gpu.as_ref().and_then(of).or_else(|| self.windows.parked().find_map(|(_, s)| s.gpu.as_ref().and_then(of)))
    }

    /// Tandem became active again: hotkey windows that hid because another app took focus
    /// come back when their profile says so (Reopen on activation).
    pub(super) fn hotkey_reactivated(&mut self) {
        for w in self.hotkey_windows() {
            self.enter(w);
            let reopen = self.hotkey.as_ref().filter(|h| h.was_auto_hidden && !h.shown).is_some_and(|h| self.store.get(&h.guid).is_some_and(|p| Prefs::from_profile(p).reopens));
            if reopen {
                if let Some(h) = self.hotkey.as_mut() {
                    h.was_auto_hidden = false;
                }
                self.roll_in();
            }
        }
    }

    /// Window > Pin Hotkey Window and the Toggle Pin Hotkey Window key action: the profile's
    /// Pin setting (it is a profile setting, so every window of the profile follows).
    pub(super) fn toggle_pin_hotkey(&mut self, guid: &str) {
        let Some(p) = self.store.get_mut(guid) else { return };
        let pinned = !p.bool("HotKey Window AutoHides");
        p.set("HotKey Window AutoHides", toml::Value::Boolean(pinned));
        if let Err(e) = self.store.save() {
            self.banners.set("settings", format!("Settings not saved: {e}"));
        }
        self.menus_dirty = true;
        self.sync_checks();
    }

    /// Whether the current window is a pinned hotkey window (the menu check).
    pub(super) fn hotkey_pinned(&self) -> bool {
        self.hotkey.as_ref().and_then(|h| self.store.get(&h.guid)).is_some_and(|p| !p.bool("HotKey Window AutoHides"))
    }

    /// A window the switcher and Next Window skip: a hidden hotkey window.
    pub(super) fn hidden_hotkey_key(&self, k: u64) -> bool {
        if self.windows.current() == Some(k) {
            return self.hotkey.as_ref().is_some_and(|h| !h.shown);
        }
        self.windows.parked().any(|(kk, s)| *kk == k && s.hotkey.as_ref().is_some_and(|h| !h.shown))
    }

    /// The app hotkey: Show/hide all windows. Windows has no "hide app", so the terminal
    /// windows are minimized (and shown hotkey windows hidden), then restored.
    fn app_hotkey_pressed(&mut self) {
        let hotkeys = self.hotkey_windows();
        let regular: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).filter(|w| !hotkeys.contains(w)).collect();
        match hotkey::app_press(tandem_in_front(), regular.len()) {
            hotkey::AppAct::HideAll => {
                self.app_hidden.clear();
                for w in regular {
                    self.enter(w);
                    if let Some(g) = &self.gpu {
                        if !g.window.is_minimized().unwrap_or(false) {
                            g.window.set_minimized(true);
                            self.app_hidden.push(w);
                        }
                    }
                }
                for w in hotkeys {
                    self.enter(w);
                    self.roll_out(false);
                }
            }
            hotkey::AppAct::ShowAll => {
                let back = std::mem::take(&mut self.app_hidden);
                for w in &back {
                    if self.enter(*w) {
                        if let Some(g) = &self.gpu {
                            g.window.set_minimized(false);
                        }
                    }
                }
                if let Some(w) = self.last_window.filter(|w| regular.contains(w)).or_else(|| regular.last().copied()) {
                    self.focus_terminal_window(w);
                }
            }
            hotkey::AppAct::OpenWindow => self.pending_windows.push(NewWindow::Profile(None)),
        }
    }
}
