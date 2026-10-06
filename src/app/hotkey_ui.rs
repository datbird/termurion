//! Hotkey windows in the window: registering the profiles' hotkeys, a press showing or
//! hiding a profile's dedicated window (with its siblings), the slide, auto-hide when focus
//! leaves, reopen when Tandem is active again, Pin Hotkey Window, and focus going back to
//! the app that was in front.

use std::time::{Duration, Instant};

use super::*;
use super::windows_ui::NewWindow;
use crate::hotkey::{self, Act, Prefs, Win};
use crate::hotkey_motion::{self, FadePath, Motion};

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
    /// The opacity the renderer draws the frame at on an alpha surface (1.0 outside a fade).
    pub fade: f32,
}

pub(super) struct Roll {
    motion: Motion,
    /// How the window fades during the roll, None when it only slides.
    fade: Option<FadePath>,
    start: Instant,
    hiding: bool,
    /// A hide by the hotkey returns focus to the previous window; an auto-hide does not.
    by_key: bool,
}

impl HotkeyWin {
    /// A fade is running (a present error now stops fading).
    pub(super) fn fading(&self) -> bool {
        self.roll.as_ref().is_some_and(|r| r.fade.is_some())
    }

    pub(super) fn new(guid: String) -> HotkeyWin {
        HotkeyWin { guid, shown: false, roll: None, was_auto_hidden: false, previous: None, fade: 1.0 }
    }
}

/// The id of the app hotkey (Show/hide all windows); profile hotkeys count from 1.
pub(super) const APP_HOTKEY: u32 = 0;
/// The id of the Open Quickly hotkey (Settings > Keys > Hotkey).
pub(super) const OPEN_QUICKLY_HOTKEY: u32 = u32::MAX;

fn cloak(hwnd: isize, on: bool) {
    use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    let v: i32 = on as i32;
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_CLOAK as u32, (&v as *const i32).cast(), 4) };
}

/// Sets the window's opacity for a fade: `WS_EX_LAYERED` is added for the animation and
/// removed again at full opacity, so the swap chain keeps its normal path outside a fade.
/// False when Windows refused the attribute (the fade then stops).
pub(super) fn set_window_alpha(hwnd: isize, alpha: f32) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GetWindowLongPtrW, IsWindow, LWA_ALPHA, SetLayeredWindowAttributes, SetWindowLongPtrW, WS_EX_LAYERED};
    unsafe {
        if IsWindow(hwnd as _) == 0 {
            return false;
        }
        let ex = GetWindowLongPtrW(hwnd as _, GWL_EXSTYLE);
        let layered = ex & WS_EX_LAYERED as isize != 0;
        if alpha >= 1.0 {
            if layered {
                SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, ex & !(WS_EX_LAYERED as isize));
            }
            return true;
        }
        if !layered {
            SetWindowLongPtrW(hwnd as _, GWL_EXSTYLE, ex | WS_EX_LAYERED as isize);
        }
        SetLayeredWindowAttributes(hwnd as _, 0, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, LWA_ALPHA) != 0
    }
}

/// A window of Tandem is in front.
pub(super) fn tandem_in_front() -> bool {
    ours(foreground())
}

pub(super) fn foreground() -> isize {
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize }
}

/// A window of this process (a terminal window, Settings, a dialog).
pub(super) fn ours(hwnd: isize) -> bool {
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
        // Registered whenever it is set (`iTermOpenQuicklyHotKeyProvider`: no leader, no switch).
        if let Some(c) = crate::settings_hotkey::quickly_hotkey(&self.globals).as_ref().and_then(hotkey::chord) {
            chords.push((OPEN_QUICKLY_HOTKEY, c));
        }
        // Profile ids stay below the session ids.
        let mut too_many = false;
        for (guid, prefs) in self.hotkey_prefs() {
            if !prefs.active() {
                continue;
            }
            if self.hotkey_ids.len() + 1 >= crate::session_hotkeys::SESSION_HOTKEY_BASE as usize {
                too_many = true;
                break;
            }
            self.hotkey_ids.push(guid);
            let id = self.hotkey_ids.len() as u32;
            chords.extend(prefs.shortcuts().into_iter().filter_map(hotkey::chord).map(|c| (id, c)));
            if let Some(m) = prefs.double_tap {
                taps.push((id, m));
            }
        }
        // The sessions' hotkeys the person set or allowed, after every other hotkey (those win a
        // clash), at most 16 chords; a refused one is shown in its row.
        let reserved: Vec<hotkey::Chord> = chords.iter().map(|(_, c)| *c).collect();
        let live: Vec<(PaneId, hotkey::Shortcut)> = self.session_hotkeys.live().iter().filter(|(p, _)| self.panes.contains_key(p)).cloned().collect();
        let plan = crate::session_hotkeys::plan(&live, &reserved);
        self.session_hotkey_ids.clear();
        for (i, (chord, panes)) in plan.groups.into_iter().enumerate() {
            chords.push((crate::session_hotkeys::SESSION_HOTKEY_BASE + i as u32, chord));
            self.session_hotkey_ids.push(panes);
        }
        let secs = |k: &str, d: f64| Duration::from_secs_f64(self.globals.get(k).and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64))).unwrap_or(d).clamp(0.0, 5.0));
        let (min, max) = (secs("hotKeyDoubleTapMinDelay", 0.01), secs("hotKeyDoubleTapMaxDelay", 0.3));
        self.hotkey_errors.clear();
        for (pane, why) in plan.refused {
            self.hotkey_errors.insert(crate::session_hotkeys::error_key(pane), why.text().to_string());
        }
        match crate::winkeys::set_hotkeys(chords, taps, min, max) {
            Ok(results) => {
                for (id, r) in results {
                    if let Err(e) = r {
                        let owner = match id {
                            APP_HOTKEY => "App Hotkey".to_string(),
                            OPEN_QUICKLY_HOTKEY => "Open Quickly Hotkey".to_string(),
                            // A refused session chord is shown on each session that uses it.
                            id if crate::session_hotkeys::is_session_id(id) => {
                                for key in crate::session_hotkeys::error_keys(id, &self.session_hotkey_ids) {
                                    self.hotkey_errors.insert(key, e.clone());
                                }
                                continue;
                            }
                            _ => self.hotkey_ids.get((id as usize).wrapping_sub(1)).cloned().unwrap_or_default(),
                        };
                        self.hotkey_errors.insert(owner, e);
                    }
                }
                self.banners.clear("hotkeys");
            }
            Err(e) => self.banners.set("hotkeys", format!("Hotkeys are unavailable: {e}")),
        }
        if too_many {
            self.banners.set("hotkeys", "Too many profiles have a hotkey: the rest are not registered.");
        }
        for s in [self.settings.as_mut(), self.profiles_win.as_mut()].into_iter().flatten() {
            s.hotkeys.errors = self.hotkey_errors.clone();
        }
        self.release_stale_hotkey_windows();
    }

    /// A hotkey window whose profile lost its hotkey (turned off, or deleted) becomes a
    /// regular window: shown, with a taskbar button, so it can be reached and closed.
    fn release_stale_hotkey_windows(&mut self) {
        let back = self.current_window();
        let all = self.hotkey_prefs();
        for w in self.hotkey_windows() {
            self.enter(w);
            let Some(guid) = self.hotkey.as_ref().map(|h| h.guid.clone()) else { continue };
            if hotkey::stale_windows(&[guid], &all).is_empty() {
                continue;
            }
            self.hotkey = None;
            if let (Some(hwnd), Some(g)) = (self.hwnd(), &self.gpu) {
                // The taskbar button comes back through `presence_tick`.
                g.window.set_window_level(winit::window::WindowLevel::Normal);
                g.window.set_visible(true);
                cloak(hwnd, false);
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }

    /// The app hotkey (Settings > Keys > Hotkey), iTerm2's global keys.
    pub(super) fn app_hotkey(&self) -> hotkey::Shortcut {
        let ch = self.globals.int("HotkeyChar");
        let text = char::from_u32(ch as u32).filter(|_| ch > 0).map(String::from).unwrap_or_default();
        hotkey::Shortcut { keycode: self.globals.int("HotkeyCode").clamp(0, u16::MAX as i64) as u16, flags: self.globals.int("HotkeyModifiers").max(0) as u64, chars: text.clone(), chars_ignoring: text }
    }

    /// The hotkey window of a profile, if one was made.
    pub(super) fn hotkey_window_of(&self, guid: &str) -> Option<WindowId> {
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
    /// A press, with every hotkey it matched (siblings share a chord or a double tap).
    pub(super) fn hotkey_pressed(&mut self, ids: &[u32], event_loop: &ActiveEventLoop) {
        // A session's hotkey brings its window forward, so it is routed first and never meets the
        // tail below that returns to the window before.
        let route = crate::session_hotkeys::route(ids);
        for index in &route.sessions {
            self.session_hotkey_pressed(*index);
        }
        if !route.tail_applies() {
            return;
        }
        let ids = &route.others[..];
        if ids.contains(&APP_HOTKEY) {
            self.app_hotkey_pressed();
        }
        // The palette alone comes forward; Tandem follows only when a row is committed.
        if ids.contains(&OPEN_QUICKLY_HOTKEY) {
            self.open_quickly(event_loop);
        }
        let pressed: Vec<String> = ids.iter().filter(|id| **id != APP_HOTKEY && **id != OPEN_QUICKLY_HOTKEY).filter_map(|id| self.hotkey_ids.get(*id as usize - 1).cloned()).collect();
        let all = self.hotkey_prefs();
        let back = self.current_window();
        for group in hotkey::groups_for(&pressed, &all) {
            self.press_group(&group, &all);
        }
        // A hotkey window that showed is current; otherwise the window before comes back.
        if let (Some(b), false) = (back, self.hotkey.as_ref().is_some_and(|h| h.shown)) {
            self.enter(b);
        }
    }

    /// One press for one sibling group (iTerm2's hotKeyPressedWithSiblings).
    fn press_group(&mut self, group: &[String], all: &[(String, Prefs)]) {
        let fg = foreground();
        let mut wins = Vec::new();
        for g in group {
            let pinned = all.iter().find(|(x, _)| x == g).is_some_and(|(_, p)| !p.auto_hides);
            let w = match self.hotkey_window_of(g) {
                Some(w) => {
                    self.enter(w);
                    let h = self.hotkey.as_ref().expect("a hotkey window");
                    // Never split a group mid-slide.
                    if h.roll.is_some() {
                        return;
                    }
                    // Minimized counts as hidden: the press brings it back.
                    let minimized = self.gpu.as_ref().is_some_and(|g| g.window.is_minimized().unwrap_or(false));
                    let open = h.shown && !minimized;
                    Win { open, focused: open && self.hwnd() == Some(fg), pinned }
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

    pub(super) fn show_hotkey_window(&mut self, guid: &str, fg: isize) {
        let previous = (!ours(fg)).then_some(fg);
        match self.hotkey_window_of(guid) {
            Some(w) => {
                self.enter(w);
                if let Some(h) = self.hotkey.as_mut() {
                    h.previous = previous;
                    h.was_auto_hidden = false;
                    // A minimized window shows again (it may still count as shown).
                    h.shown = h.shown && !self.gpu.as_ref().is_some_and(|g| g.window.is_minimized().unwrap_or(false));
                }
                self.roll_in();
            }
            // Already on its way (a second press before the event loop made it).
            None if self.pending_windows.iter().any(|w| matches!(w, NewWindow::Hotkey(g, _) if g == guid)) => {}
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
        // A hotkey window shows on the desktop the person is on (iTerm2 opens it in the active
        // space; its Space only matters for All Desktops). It keeps following only while the
        // profile's Space is All Desktops; the saved state does not carry the flag.
        self.win_follow_desktop = self.store.get(&guid).is_some_and(|p| crate::desktops::follows(p.int("Space")));
        self.follow_now();
        let Some(g) = &self.gpu else { return };
        g.window.set_window_level(if floats { winit::window::WindowLevel::AlwaysOnTop } else { winit::window::WindowLevel::Normal });
        // The profile's Screen: the window shows on that monitor, in its style's frame there.
        self.move_to_preferred_screen(&guid);
        let Some(g) = &self.gpu else { return };
        let shown = Self::outer_rect(g);
        let fade = self.fade_path_now(animates);
        let slide = self.slide_for(shown, animates);
        let duration = self.hotkey_duration();
        // A fade with no slide (left, normal, centered...) runs from the shown place to itself.
        let motion = (slide.is_some() || fade.is_some()).then(|| Motion { from: slide.unwrap_or((shown.x, shown.y)), to: (shown.x, shown.y), duration, alpha: (if fade.is_some() { 0.0 } else { 1.0 }, 1.0) });
        if let Some(m) = &motion {
            g.window.set_outer_position(winit::dpi::PhysicalPosition::new(m.from.0.round() as i32, m.from.1.round() as i32));
        }
        // The first frame is already transparent: nothing flashes at full opacity.
        match fade {
            Some(FadePath::Layered) => {
                if !set_window_alpha(hwnd, 0.0) {
                    self.fade_failed = true;
                }
            }
            Some(FadePath::Composed) => {
                if let Some(h) = self.hotkey.as_mut() {
                    h.fade = 0.0;
                }
            }
            None => {}
        }
        // A refused attribute ends fading for the run: the window shows at once (a slide still runs).
        let (motion, fade) = if self.fade_failed && fade.is_some() {
            self.end_fade();
            (slide.map(|from| Motion { from, to: (shown.x, shown.y), duration, alpha: (1.0, 1.0) }), None)
        } else {
            (motion, fade)
        };
        if fade == Some(FadePath::Composed) {
            // Drawn at opacity 0 before the window shows, so the last frame it hid with does not flash.
            if let Some(h) = self.hotkey.as_mut() {
                h.shown = true;
                h.roll = motion.map(|motion| Roll { motion, fade, start: Instant::now(), hiding: false, by_key: true });
            }
            self.render();
        }
        let Some(g) = &self.gpu else { return };
        cloak(hwnd, false);
        g.window.focus_window();
        let now = Instant::now();
        if let Some(h) = self.hotkey.as_mut() {
            h.shown = true;
            h.roll = motion.map(|motion| Roll { motion, fade, start: now, hiding: false, by_key: true });
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
        let fade = self.fade_path_now(animates);
        let slide = self.slide_for(shown, animates);
        if slide.is_none() && fade.is_none() {
            return self.finish_hide(by_key, None);
        }
        let motion = Motion { from: (shown.x, shown.y), to: slide.unwrap_or((shown.x, shown.y)), duration: self.hotkey_duration(), alpha: (1.0, if fade.is_some() { 0.0 } else { 1.0 }) };
        if let Some(h) = self.hotkey.as_mut() {
            h.fade = 1.0;
            h.roll = Some(Roll { motion, fade, start: Instant::now(), hiding: true, by_key });
        }
    }

    /// How the current window fades, or None to show and hide it at once: "Animate hotkey window"
    /// on, the style fades, the probe passed for the surface it presents on, and no present has
    /// failed this run.
    fn fade_path_now(&self, animates: bool) -> Option<FadePath> {
        if !animates || !hotkey_motion::fades(self.win_style) {
            return None;
        }
        let alpha_surface = self.win_alpha_surface == crate::transparency::SurfaceKind::Alpha;
        // An alpha-surface window that is not drawing see-through has an opaque alpha mode: scaling
        // its target would darken it, not fade it.
        if alpha_surface && !self.see_through_now() {
            return None;
        }
        hotkey_motion::fade_path(alpha_surface, hotkey_motion::FADE_LAYERED, hotkey_motion::FADE_COMPOSED, self.fade_failed)
    }

    /// Puts the current window back to full opacity (the layered style off, the renderer's fade 1).
    fn end_fade(&mut self) {
        if let Some(hwnd) = self.hwnd() {
            set_window_alpha(hwnd, 1.0);
        }
        if let Some(h) = self.hotkey.as_mut() {
            h.fade = 1.0;
        }
    }

    /// The window was recreated (its surface kind changed) while it faded: the fade ends and the
    /// window is shown at once; a slide still finishes.
    pub(super) fn end_fade_on_recreate(&mut self) {
        if let Some(r) = self.hotkey.as_mut().and_then(|h| h.roll.as_mut()) {
            r.fade = None;
            r.motion.alpha = (1.0, 1.0);
        }
        self.end_fade();
    }

    /// Moves the current window onto its profile's Screen (-1: the one with the pointer) and
    /// frames it there by its style.
    /// True when the window was moved (and so framed by its style).
    pub(super) fn move_to_preferred_screen(&mut self, guid: &str) -> bool {
        use crate::placement;
        let monitors: Vec<Rect> = placement::primary_first(super::requests_ui::work_areas().into_iter().map(|(x, y, w, h)| Rect { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }).collect());
        let Some(g) = &self.gpu else { return false };
        let r = Self::outer_rect(g);
        let inside = |m: &Rect, x: f32, y: f32| x >= m.x && x < m.x + m.w && y >= m.y && y < m.y + m.h;
        let window_monitor = monitors.iter().position(|m| inside(m, r.x + r.w / 2.0, r.y + r.h / 2.0)).unwrap_or(0);
        let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        let cursor_monitor = if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } != 0 { monitors.iter().position(|m| inside(m, pt.x as f32, pt.y as f32)).unwrap_or(window_monitor) } else { window_monitor };
        let pref = self.store.get(guid).and_then(|p| p.get("Screen")).and_then(|v| v.as_integer()).unwrap_or(-1);
        let work = placement::screen_for(pref, &monitors, window_monitor, cursor_monitor);
        if inside(&work, r.x + r.w / 2.0, r.y + r.h / 2.0) {
            return false;
        }
        g.window.set_outer_position(winit::dpi::PhysicalPosition::new(work.x.round() as i32, work.y.round() as i32));
        let style = self.win_style;
        if style != crate::frames::NORMAL {
            self.apply_window_style(style, false);
        }
        true
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
        // Full opacity again, while it is cloaked: the next show starts from its own first frame.
        self.end_fade();
        if let Some(g) = &self.gpu {
            if let Some((x, y)) = back {
                g.window.set_outer_position(winit::dpi::PhysicalPosition::new(x.round() as i32, y.round() as i32));
            }
            // Cloaking does not take focus away: hiding the window makes Windows activate the
            // next one, so typing never goes into an invisible shell.
            g.window.set_visible(false);
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
        // Only windows that are sliding (checked without swapping every window in).
        let mut moving: Vec<WindowId> = self.windows.parked().filter(|(_, s)| s.hotkey.as_ref().is_some_and(|h| h.roll.is_some())).filter_map(|(_, s)| s.gpu.as_ref().map(|g| g.window.id())).collect();
        if self.hotkey.as_ref().is_some_and(|h| h.roll.is_some()) {
            moving.extend(self.current_window());
        }
        if moving.is_empty() {
            return None;
        }
        let back = self.current_window();
        let mut next = None;
        for w in moving {
            self.enter(w);
            let Some(roll) = self.hotkey.as_ref().and_then(|h| h.roll.as_ref()) else { continue };
            let (motion, hiding, by_key, fade) = (roll.motion, roll.hiding, roll.by_key, roll.fade);
            let (mut pos, alpha, mut done) = motion.at(roll.start.elapsed());
            let origin = motion.from;
            // A fade that failed ends at once: shown or hidden where it was going.
            if fade.is_some() && self.fade_failed {
                (pos, done) = (motion.to, true);
                self.end_fade();
            } else if fade == Some(FadePath::Layered) {
                let hwnd = self.hwnd();
                // The last step of a show is full opacity (the style comes off); a hide ends at 0.
                if hwnd.is_none_or(|h| !set_window_alpha(h, alpha)) {
                    self.fade_failed = true;
                    (pos, done) = (motion.to, true);
                    self.end_fade();
                }
            } else if fade == Some(FadePath::Composed) {
                if let Some(h) = self.hotkey.as_mut() {
                    h.fade = alpha;
                }
                self.redraw();
            }
            let (x, y) = pos;
            if let Some(g) = &self.gpu {
                let (cx, cy) = (x.round() as i32, y.round() as i32);
                if g.window.outer_position().map_or(true, |p| (p.x, p.y) != (cx, cy)) {
                    g.window.set_outer_position(winit::dpi::PhysicalPosition::new(cx, cy));
                }
            }
            if !done {
                next = Some(Instant::now() + Duration::from_millis(8));
            } else if hiding {
                self.finish_hide(by_key, Some(origin));
            } else {
                self.end_fade();
                if let Some(h) = self.hotkey.as_mut() {
                    h.roll = None;
                }
                self.redraw();
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
        let back = self.current_window();
        let mut shown = false;
        for w in self.hotkey_windows() {
            self.enter(w);
            let reopen = self.hotkey.as_ref().filter(|h| h.was_auto_hidden && !h.shown).is_some_and(|h| self.store.get(&h.guid).is_some_and(|p| Prefs::from_profile(p).reopens));
            if reopen {
                if let Some(h) = self.hotkey.as_mut() {
                    h.was_auto_hidden = false;
                }
                self.roll_in();
                shown = true;
            }
        }
        if let (Some(b), false) = (back, shown) {
            self.enter(b);
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

    /// Brings the current window forward; a hidden hotkey window shows the way its hotkey
    /// shows it (nothing may focus it while it is cloaked).
    pub(super) fn bring_forward(&mut self) {
        if self.hotkey.as_ref().is_some_and(|h| !h.shown) {
            self.roll_in();
            return;
        }
        if let Some(g) = &self.gpu {
            g.window.set_minimized(false);
            g.window.focus_window();
        }
    }

    /// Puts a window on top of Tandem's others and makes it the current one without
    /// activating Tandem, as iTerm2's `makeKeyAndOrderFront:` orders a window front within the
    /// app (Activate's `order_window_front`, `H:3072-3079`); a minimized window is restored
    /// without activation. A hidden hotkey window shows the way its hotkey shows it.
    pub(super) fn order_front(&mut self, id: WindowId) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{HWND_TOP, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SetWindowPos, ShowWindow};
        if !self.enter(id) {
            return;
        }
        self.last_window = Some(id);
        if self.hotkey.as_ref().is_some_and(|h| !h.shown) {
            self.roll_in();
            return;
        }
        let Some(h) = self.hwnd() else { return };
        if self.gpu.as_ref().is_some_and(|g| g.window.is_minimized() == Some(true)) {
            unsafe { ShowWindow(h as _, SW_SHOWNOACTIVATE) };
        }
        unsafe { SetWindowPos(h as _, HWND_TOP, 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE) };
    }

    /// Whether the current window is a pinned hotkey window (the menu check).
    pub(super) fn hotkey_pinned(&self) -> bool {
        self.hotkey.as_ref().and_then(|h| self.store.get(&h.guid)).is_some_and(|p| !p.bool("HotKey Window AutoHides"))
    }

    /// Whether the pin indicator shows on this window's panes: the Advanced switch is on and
    /// this is a hotkey window whose profile does not auto-hide (`PS:14295-14304`).
    pub(super) fn in_pinned_hotkey_window(&self) -> bool {
        self.globals.bool("ShowPinnedIndicator") && self.hotkey_pinned()
    }

    /// A window the switcher and Next Window skip: a hidden hotkey window.
    pub(super) fn hidden_hotkey_key(&self, k: u64) -> bool {
        if self.windows.current() == Some(k) {
            return self.hotkey.as_ref().is_some_and(|h| !h.shown);
        }
        self.windows.parked().any(|(kk, s)| *kk == k && s.hotkey.as_ref().is_some_and(|h| !h.shown))
    }

    /// A window Next and Previous Window pass by: a hidden hotkey window, or an open one while
    /// Hotkey windows are excluded from Cycle Through Windows (P6).
    pub(super) fn cycling_skips_key(&self, k: u64) -> bool {
        let state = if self.windows.current() == Some(k) { self.hotkey.as_ref().map(|h| h.shown) } else { self.windows.parked().find(|(kk, _)| **kk == k).and_then(|(_, s)| s.hotkey.as_ref().map(|h| h.shown)) };
        crate::presence::cycling_skips(state, self.globals.bool(crate::presence::EXCLUDED_FROM_CYCLING_KEY))
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
                if let Some(b) = self.last_window {
                    self.enter(b);
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

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    #[test]
    fn window_alpha_adds_and_removes_the_layered_style() {
        unsafe {
            let class: Vec<u16> = "TandemAlphaTest\0".encode_utf16().collect();
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(DefWindowProcW), lpszClassName: class.as_ptr(), ..std::mem::zeroed() });
            // A tool window that is never shown, off every monitor.
            let h = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), class.as_ptr(), WS_POPUP, -30000, -30000, 16, 16, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            assert!(!h.is_null());
            let layered = || GetWindowLongPtrW(h, GWL_EXSTYLE) & WS_EX_LAYERED as isize != 0;
            assert!(!layered());
            assert!(set_window_alpha(h as isize, 0.5));
            assert!(layered(), "a fade adds the layered style");
            let (mut key, mut alpha, mut flags) = (0u32, 0u8, 0u32);
            GetLayeredWindowAttributes(h, &mut key, &mut alpha, &mut flags);
            assert_eq!((alpha, flags), (128, LWA_ALPHA));
            // Later steps of a fade are the same call; a desktop-less ssh session (0) refuses a second
            // `SetLayeredWindowAttributes` on a window (error 31), so they are checked on the checklist.
            assert!(set_window_alpha(h as isize, 1.0));
            assert!(!layered(), "full opacity removes the style");
            assert!(set_window_alpha(h as isize, 1.0), "removing twice is fine");
            DestroyWindow(h);
            assert!(!set_window_alpha(h as isize, 0.5), "a destroyed window refuses");
        }
    }
}
