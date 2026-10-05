//! The wheel path of one pane and the alternate-scroll question (milestone 13j): which of
//! arrows, a swallowed event, mouse reports or the history a wheel event does, and the bar
//! that offers to turn arrows on. Only the person's wheel and keys reach it: arrows and the
//! inferrer never see an API call, a program's output, or a pane the pointer is not over.

use super::*;
use crate::alt_infer::{self, Change};
use crate::wheel::{self, AltScroll, AltSwitches};

impl App {
    /// The switches that can turn alternate scroll on for `id` right now.
    fn alt_switches(&self, id: PaneId) -> AltSwitches {
        let Some(p) = self.panes.get(&id) else {
            return AltSwitches { advanced: false, profile_auto: false, profile_vertical_only: false, term_1007: false };
        };
        let profile = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile());
        AltSwitches {
            advanced: self.globals.bool("AlternateMouseScroll"),
            profile_auto: profile.bool("Automatically Enable Alternate Mouse Scroll"),
            profile_vertical_only: profile.bool("Restrict Alternate Mouse Scroll to Vertical"),
            term_1007: p.screen.alternate_scroll_mode(),
        }
    }

    /// Make Text Bigger (1) and Make Text Smaller (-1): the menu and the pinch. The shared
    /// font offset moves one step (clamped as the menu always did; `apply_fonts` holds the
    /// upper clamp). The caller lays out. 13k reads the font metrics this changes.
    pub(super) fn change_font_size(&mut self, dir: i32) {
        let offset = if dir > 0 { self.font_offset + 1.0 } else { (self.font_offset - 1.0).max(-8.0) };
        self.set_font_offset(offset);
    }

    /// Ctrl+wheel as the pinch gesture (`PseudoTerminal.m:1751-1773`): true when the event was
    /// the pinch's. One notch is one font step. Only `wheel` calls it, after every UI wheel
    /// handler and the overlay hold, over a pane (P15).
    pub(super) fn pinch(&mut self, ev: wheel::Event) -> bool {
        if !wheel::is_pinch(self.mods, self.globals.bool("PinchToChangeFontSizeDisabled")) {
            return false;
        }
        let step = self.magnifier.add(wheel::pinch_amount(ev.dy, WHEEL_LINES as f64), Instant::now());
        if step != 0 {
            self.change_font_size(step);
            self.relayout();
            self.redraw();
        }
        true
    }

    /// The pointer entered this window (`WindowEvent::CursorEntered`, the window already
    /// current): with focus follows mouse it makes the window key while Tandem is active, or
    /// takes the foreground for it when `StealKeyFocus` says so (`FFM:107-165`). Never from a
    /// Tandem dialog, a password prompt or a waiting approval, and never for a hidden hotkey
    /// window. The pane under the pointer is focused by `mouse_move` as before.
    pub(super) fn ffm_enter(&mut self) {
        let ffm = self.globals.bool("FocusFollowsMouse");
        if !ffm || !matches!(self.drag, Drag::None) {
            return;
        }
        let now = pointer_screen();
        // The refusal point (the pointer where Tandem became active) holds until it moves.
        let moved = self.ffm_saved.is_none_or(|s| Some(s) != now);
        if moved {
            self.ffm_saved = None;
        }
        let enter = mouse::ffm_on_enter(ffm, self.globals.bool("StealKeyFocus"), self.app_active, moved);
        let Some(hwnd) = self.hwnd() else { return };
        let front = hotkey_ui::foreground();
        if front == hwnd || self.windows.current().is_some_and(|k| self.hidden_hotkey_key(k)) {
            return;
        }
        if !mouse::ffm_may_act(enter, self.ffm_front(front), self.focused_password_prompt(), !self.orch_waits.is_empty()) {
            return;
        }
        match enter {
            mouse::Enter::MakeKey => {
                if let Some(id) = self.current_window() {
                    self.focus_terminal_window(id);
                }
            }
            mouse::Enter::Steal => {
                // Windows may refuse and flash the taskbar button instead; that is its rule.
                use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
                unsafe { SetForegroundWindow(hwnd as _) };
            }
            mouse::Enter::Nothing => {}
        }
    }

    /// Plain focus follows mouse inside a window: the pane under the pointer takes focus.
    pub(super) fn ffm_pane_focus(&mut self, id: PaneId) {
        if self.globals.bool("FocusFollowsMouse") && self.tabs.get(self.active).is_some_and(|t| t.focused != id) {
            self.tabs[self.active].focused = id;
            self.refresh_route();
            self.redraw();
        }
    }

    /// Tandem lost the foreground: the pointer is remembered (`FFMC:99-102`).
    pub(super) fn ffm_became_inactive(&mut self) {
        self.ffm_saved = pointer_screen();
    }

    /// Tandem became active (`FFMC:40-94`): with Aggressive Focus Follows Mouse the Tandem
    /// window under the pointer comes forward and its pane takes focus, unless the pointer is
    /// where it was when Tandem resigned; otherwise the pointer is the refusal point.
    pub(super) fn ffm_became_active(&mut self) {
        let ffm = self.globals.bool("FocusFollowsMouse");
        let Some(now) = pointer_screen() else { return };
        let go = mouse::ffm_on_activate(ffm, self.globals.bool("AggressiveFocusFollowsMouse"), self.ffm_saved, now);
        if !go {
            if ffm {
                self.ffm_saved = Some(now);
            }
            return;
        }
        self.ffm_saved = None;
        let front = hotkey_ui::foreground();
        if self.ffm_front(front) != mouse::Front::Terminal || self.focused_password_prompt() || !self.orch_waits.is_empty() {
            return;
        }
        let Some((id, hwnd)) = self.terminal_window_under(now) else { return };
        if hwnd != front {
            self.focus_terminal_window(id);
        }
        self.focus_pane_at_screen(hwnd, now);
    }

    /// What is in front, as focus follows mouse needs it.
    fn ffm_front(&self, front: isize) -> mouse::Front {
        if !hotkey_ui::ours(front) {
            mouse::Front::Elsewhere
        } else if self.is_terminal_hwnd(front) {
            mouse::Front::Terminal
        } else {
            mouse::Front::OtherTandem
        }
    }

    fn is_terminal_hwnd(&self, h: isize) -> bool {
        self.hwnd() == Some(h) || self.windows.parked().any(|(_, s)| s.gpu.as_ref().and_then(|g| menu::hwnd_of(&g.window)) == Some(h))
    }

    /// A focused pane of any window sits at a password prompt.
    fn focused_password_prompt(&self) -> bool {
        let mut tabs = self.tabs.iter().chain(self.windows.parked().flat_map(|(_, s)| s.tabs.iter()));
        tabs.any(|t| self.panes.get(&t.focused).is_some_and(|p| p.at_password_prompt()))
    }

    /// The Tandem terminal window under the screen point: its id and handle.
    fn terminal_window_under(&self, screen: (f64, f64)) -> Option<(winit::window::WindowId, isize)> {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, WindowFromPoint, GA_ROOT};
        let under = unsafe { GetAncestor(WindowFromPoint(POINT { x: screen.0 as i32, y: screen.1 as i32 }), GA_ROOT) } as isize;
        if under == 0 {
            return None;
        }
        let here = self.gpu.as_ref().filter(|g| menu::hwnd_of(&g.window) == Some(under)).map(|g| g.window.id());
        here.or_else(|| self.windows.parked().find_map(|(_, s)| s.gpu.as_ref().filter(|g| menu::hwnd_of(&g.window) == Some(under)).map(|g| g.window.id()))).map(|id| (id, under))
    }

    /// Focuses the pane of the (current) window under the screen point, as focus follows mouse.
    fn focus_pane_at_screen(&mut self, hwnd: isize, screen: (f64, f64)) {
        use windows_sys::Win32::Foundation::{HWND, POINT};
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        let mut p = POINT { x: screen.0 as i32, y: screen.1 as i32 };
        if unsafe { ScreenToClient(hwnd as HWND, &mut p) } == 0 {
            return;
        }
        if let Some((id, _)) = self.pane_at(p.x as f32, p.y as f32) {
            self.ffm_pane_focus(id);
        }
    }

    /// Every Tandem overlay inside the terminal rectangle or over the window, in ONE list (B5):
    /// the front info bar of each visible pane, portholes, block pills, session pickers, AI
    /// indicators, the find bar of the focused pane, the completion list, each shown composer
    /// and its popup, the toast and the error banner. A new overlay adds its `Overlay` kind
    /// here; `tandem_ui_at` and the report gate read only this list.
    fn ui_rects(&self) -> mouse::UiRects {
        use mouse::Overlay;
        let mut ui = mouse::UiRects::default();
        let cell = self.cell();
        if let Some(tab) = self.tabs.get(self.active) {
            for (id, rect) in self.visible(tab) {
                if let Some(bar) = self.bars.get(&id).and_then(|b| b.front()) {
                    ui.push(Overlay::InfoBar, crate::infobar::layout(bar, rect, cell).bar);
                }
                if let Some(r) = self.completion_rect(id, rect, cell) {
                    ui.push(Overlay::Completion, r);
                }
                if self.composer_shown(id) {
                    ui.push(Overlay::Composer, self.composer_layout(id, rect).panel);
                    if let Some(r) = self.composer_popup_rect(id, rect) {
                        ui.push(Overlay::ComposerPopup, r);
                    }
                }
            }
        }
        // The pick's looks are drawn inside `pick_rects`, so those are what hold the events.
        for r in self.pane_move_hit_rects() {
            ui.push(Overlay::PaneMove, r);
        }
        for h in &self.porthole_hits {
            ui.push(Overlay::Porthole, h.3);
        }
        for b in &self.block_buttons {
            ui.push(Overlay::Pill, b.0);
        }
        for h in &self.picker_hits {
            ui.push(Overlay::Picker, h.1);
        }
        for h in &self.chat_indicator_hits {
            ui.push(Overlay::Indicator, h.1);
        }
        if self.focused_id().is_some_and(|id| self.finds.get(&id).is_some_and(|f| f.open)) {
            for r in self.find_bar_rect.into_iter().chain(self.find_mode_rect) {
                ui.push(Overlay::Find, r);
            }
        }
        let (ww, wh) = self.gpu.as_ref().map_or((800.0, 600.0), |g| (g.config.width as f32, g.config.height as f32));
        let window = Rect { x: 0.0, y: 0.0, w: ww, h: wh };
        // The toast and the banner are not interactive, but a click through them is not the program's either.
        if let Some((text, _)) = self.toast.as_ref().filter(|(_, until)| Instant::now() < *until) {
            ui.push(Overlay::Toast, scene::toast_rect(text, window, cell));
        }
        if self.banners.text().is_some() || self.panes.values().any(|p| p.error.is_some()) {
            ui.push(Overlay::Banner, scene::banner_rect(window, cell));
        }
        ui
    }

    /// A button a program drew on a line of its output (Tandem acts on it, not the program).
    fn inline_button_at(&self, x: f32, y: f32) -> bool {
        let Some((id, rect)) = self.pane_at(x, y) else { return false };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let Some(p) = self.panes.get(&id) else { return false };
        let line = row as i32 - p.screen.term().grid().display_offset() as i32;
        crate::blocks::button_hit(&p.screen.buttons_on_line(line), col as usize).is_some()
    }

    /// Takes the first-mouse flag of the window in front of the event: true once after a
    /// client-area click that activated it (`firstmouse_win`).
    pub(super) fn first_mouse(&mut self) -> bool {
        #[cfg(windows)]
        {
            self.hwnd().is_some_and(crate::firstmouse_win::take)
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// The point is on one of Tandem's overlays inside a pane: no program ever sees a click,
    /// drag or hover there, whatever the button (B5).
    pub(super) fn tandem_ui_at(&self, x: f32, y: f32) -> bool {
        self.ui_rects().hit(x, y) || self.inline_button_at(x, y)
    }

    /// The one answer to "would this event reach the program" (B7): the pane's modes (reporting
    /// on, restricted to the alternate screen, Alt), the profile's click and wheel gates, the
    /// horizontal setting, and for a press or hover the overlay under the pointer. A drag or
    /// release continues a click the program already saw, so an overlay it ends over does not
    /// hold it back.
    pub(super) fn report_gate(&self, pane: PaneId, kind: mouse::Kind) -> bool {
        self.report_gate_with(pane, kind, self.mods)
    }

    /// `report_gate` for the modifiers `mods` instead of the ones held (Alt-click asks whether
    /// the click would have gone to the program without Alt).
    pub(super) fn report_gate_with(&self, pane: PaneId, kind: mouse::Kind, mods: mouse::Mods) -> bool {
        let profile = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).unwrap_or_else(|| self.store.default_profile());
        let (x, y) = self.cursor;
        let gates = mouse::Gates {
            clicks: profile.bool("Mouse Reporting allow clicks and drags"),
            wheel: profile.bool("Mouse Reporting allow mouse wheel"),
            horizontal: self.globals.bool("ReportHorizontalScrollEvents"),
            first_mouse_blocks: self.first_mouse_down && !self.globals.bool("ReportOnFirstMouse"),
            ui_over: !matches!(kind, mouse::Kind::Drag) && self.tandem_ui_at(x, y),
        };
        mouse::should_report(self.modes(pane), mods, kind, gates)
    }

    /// Every mouse report goes to its pane through here.
    pub(super) fn write_mouse_report(&mut self, pane: PaneId, bytes: &[u8]) {
        // `PS:14033-14041`: watch for the echo unless detection is off, the question is
        // silenced, or its bar is already up.
        let watch = self.globals.bool("AutodetectMouseReportingStuck")
            && !self.globals.bool("NoSyncNeverAskAboutMouseReportingFrustration")
            && !self.bars.get(&pane).is_some_and(|b| b.has(super::host_offers_ui::bar_id(super::host_offers_ui::Offer::Mouse)));
        if let Some(p) = self.panes.get_mut(&pane) {
            if watch {
                p.stuck.get_or_insert_with(Default::default).armed(bytes, Instant::now());
            }
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.input(bytes);
        }
    }

    /// A pane whose output echoed a mouse report back: reporting is on for a program that has
    /// gone (`PS:14102-14120`). The saved answer decides, else 13b's offer asks. Each notice is
    /// taken here, and the detector is spent per match, so one occurrence asks once.
    pub(super) fn stuck_tick(&mut self) {
        use super::host_offers_ui::{Offer, bar_id, offer_buttons, offer_key, offer_text};
        let seen: Vec<PaneId> = self.panes.iter_mut().filter_map(|(id, p)| std::mem::take(&mut p.stuck_seen).then_some(*id)).collect();
        if self.globals.bool("NoSyncNeverAskAboutMouseReportingFrustration") {
            return;
        }
        for id in seen {
            if !self.mouse_allowed(id) || self.modes(id).tracking == mouse::Tracking::Off {
                continue;
            }
            match crate::stuck_mouse::next(self.globals.optional_bool(offer_key(Offer::Mouse))) {
                crate::stuck_mouse::Next::TurnOff => self.turn_off(id, Offer::Mouse),
                crate::stuck_mouse::Next::Offer => {
                    if !self.bars.get(&id).is_some_and(|b| b.has(bar_id(Offer::Mouse))) {
                        self.ask(id, bar_id(Offer::Mouse), offer_text(Offer::Mouse), offer_buttons(Offer::Mouse), false);
                    }
                }
                crate::stuck_mouse::Next::Nothing => {}
            }
        }
    }

    /// The whole wheel path for one pane at a cell: `dy` lines up and `dx` steps left (both
    /// positive), after the accumulators. A vertical event is one with `|dx| < |dy|`
    /// (`NSEvent+iTerm.m:197-199`, strict).
    pub(super) fn route_wheel(&mut self, id: PaneId, col: u32, row: u32, ev: wheel::Event) {
        let (dx, dy) = (ev.dx, ev.dy);
        if dy == 0.0 && dx == 0.0 {
            return;
        }
        let vertical = dx.abs() < dy.abs();
        let mods = self.mods;
        let modes = self.modes(id);
        let reporting = self.report_gate(id, mouse::Kind::Wheel { horizontal: !vertical });
        let switches = self.alt_switches(id);
        let strings_set = !self.globals.string("AlternateMouseScrollStringForUp").is_empty() || !self.globals.string("AlternateMouseScrollStringForDown").is_empty();
        let horizontal_reports = self.globals.bool("ReportHorizontalScrollEvents");
        let prefs = wheel::Prefs::from_globals(&self.globals);
        let affects = self.globals.bool("NaturalScrollingAffectsHorizontalMouseReporting");
        let (proportional, double) = (self.globals.bool("ProportionalScrollWheelReporting"), self.globals.bool("DoubleReportScrollWheel"));
        let (cw, ch) = self.cell();
        let scale = self.scale() as f64;
        let cell_pt = (cw as f64 / scale, ch as f64 / scale);
        let Some(p) = self.panes.get_mut(&id) else { return };
        let mode = *p.screen.term().mode();
        let writable = p.exited.is_none() && !p.gateway;
        let decision = wheel::alternate(mode.contains(TermMode::ALT_SCREEN), writable, reporting, mods.alt, vertical, switches, strings_set);
        match decision {
            AltScroll::Arrows { .. } => {
                let delta = if vertical { p.wheel.report.take(dy, ev.precise, cell_pt.1, &prefs) } else { p.wheel.horizontal.take(dx, ev.precise, cell_pt.0, &prefs) };
                if delta == 0.0 {
                    return;
                }
                let bytes = input::alt_arrows(delta >= 0.0, vertical, mods, mode.contains(TermMode::APP_CURSOR), wheel::arrow_count(delta));
                p.screen.term_mut().selection = None;
                // Arrows are keys: they follow broadcast input as iTerm2's do (reports stay local).
                self.user_write(id, &bytes, crate::broadcast::Write::WheelArrows);
                return self.redraw();
            }
            AltScroll::Swallow => {
                p.screen.term_mut().selection = None;
                return self.redraw();
            }
            AltScroll::NoAndInfer => {
                let change = self.alt_infer.entry(id).or_default().scroll(dy, dx);
                self.alt_infer_changed(id, change);
            }
            AltScroll::No => {}
        }
        if reporting {
            let Some(p) = self.panes.get_mut(&id) else { return };
            // Positive x moves the content right: the view scrolls left.
            let bytes = if vertical {
                let chosen = wheel::report_delta(p.wheel.report.take(dy, ev.precise, cell_pt.1, &prefs), cell_pt.1, prefs.fast_trackpad);
                let steps = if chosen == 0.0 { 0 } else { wheel::report_steps(chosen, proportional, double) as i32 };
                input::wheel_reports_with(modes, mods, if chosen > 0.0 { steps } else { -steps }, col, row)
            } else {
                let n = wheel::report_delta(p.wheel.horizontal.take(dx, ev.precise, cell_pt.0, &prefs), cell_pt.0, prefs.fast_trackpad);
                let steps = if n == 0.0 { 0 } else { wheel::report_steps(n, proportional, double) as i32 };
                let natural = wheel::needs_touchpad_setting(ev.precise, affects) && touchpad_natural();
                let left = wheel::natural_swap(n > 0.0, ev.precise, natural, affects);
                input::horizontal_wheel_reports(modes, mods, steps, left, horizontal_reports, col, row)
            };
            if !bytes.is_empty() {
                self.write_mouse_report(id, &bytes);
            }
        } else if vertical {
            let Some(p) = self.panes.get_mut(&id) else { return };
            let lines = p.wheel.scroll.take(dy, ev.precise, cell_pt.1, &prefs);
            if lines != 0.0 {
                p.scroll(Scroll::Delta(lines as i32));
            }
        }
        self.redraw();
    }

    /// What the inferrer said: ask the question once, or take it back.
    fn alt_infer_changed(&mut self, id: PaneId, change: Change) {
        match change {
            Change::None => {}
            Change::Infer(true) => {
                if !self.globals.bool(alt_infer::NEVER_KEY) {
                    self.ask(id, alt_infer::ASK_ID, alt_infer::ASK_TEXT, &[alt_infer::YES, alt_infer::DONT_ASK], true);
                }
            }
            Change::Infer(false) => {
                if let Some(bars) = self.bars.get_mut(&id) {
                    bars.retain(|b| b.id != alt_infer::ASK_ID);
                }
                self.redraw();
            }
        }
    }

    /// A key the person pressed in the focused pane: up and down arrows (any modifiers) are
    /// `Some(true)` and `Some(false)`, every other key `None`.
    pub(super) fn alt_infer_key(&mut self, ev: &KeyEvent) {
        let Some(id) = self.focused_id() else { return };
        let arrow = match ev.logical_key {
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowUp) => Some(true),
            winit::keyboard::Key::Named(winit::keyboard::NamedKey::ArrowDown) => Some(false),
            _ => None,
        };
        let Some(inf) = self.alt_infer.get_mut(&id) else { return };
        let change = inf.key(arrow);
        self.alt_infer_changed(id, change);
    }

    /// A click or drag in the pane under the pointer (`nonScrollWheelEvent:`).
    pub(super) fn alt_infer_non_scroll(&mut self, id: PaneId) {
        let Some(inf) = self.alt_infer.get_mut(&id) else { return };
        let change = inf.non_scroll();
        self.alt_infer_changed(id, change);
    }

    /// The keyboard moved to another pane (`firstResponderDidChange`): the old pane's guess ends.
    pub(super) fn alt_infer_focus_moved(&mut self) {
        let now = self.focused_id();
        if now == self.alt_infer_focus {
            return;
        }
        if let Some(old) = std::mem::replace(&mut self.alt_infer_focus, now) {
            if let Some(inf) = self.alt_infer.get_mut(&old) {
                let change = inf.reset();
                self.alt_infer_changed(old, change);
            }
        }
    }

    /// The question's answer. False for other bars. Yes turns the Advanced switch on; Don't
    /// Ask Again is remembered (`iTermNaggingController.m:408-420`); a close does nothing.
    pub(super) fn alt_scroll_bar(&mut self, _pane: PaneId, id: &str, button: Option<&str>) -> bool {
        if id != alt_infer::ASK_ID {
            return false;
        }
        match button {
            Some(b) if b == alt_infer::YES => {
                self.globals.set("AlternateMouseScroll", toml::Value::Boolean(true));
                self.save_globals();
            }
            Some(b) if b == alt_infer::DONT_ASK => {
                self.globals.set(alt_infer::NEVER_KEY, toml::Value::Boolean(true));
                self.save_globals();
            }
            _ => {}
        }
        true
    }
}

/// The pointer's screen point as floats.
fn pointer_screen() -> Option<(f64, f64)> {
    tab_drag_ui::cursor_screen().map(|(x, y)| (x as f64, y as f64))
}

/// The touchpad's natural scrolling (false off Windows).
fn touchpad_natural() -> bool {
    #[cfg(windows)]
    return wheel::touchpad_natural();
    #[cfg(not(windows))]
    false
}

#[cfg(test)]
mod tests {
    /// The text of `fn name` in `src`: from its signature to the next method of the same impl.
    fn body(src: &'static str, name: &str) -> &'static str {
        let production = src.split("\n#[cfg(test)]\nmod tests").next().unwrap();
        let start = production.find(&format!("fn {name}(")).unwrap_or_else(|| panic!("fn {name} missing"));
        let rest = &production[start..];
        let end = [rest[1..].find("\n    fn "), rest[1..].find("\n    pub(super) fn ")].into_iter().flatten().min().map_or(rest.len(), |e| e + 1);
        &rest[..end]
    }

    const MOD: &str = include_str!("mod.rs");
    const UI: &str = include_str!("mouse_ui.rs");

    /// B7: every reader of "would this event be reported" asks `report_gate`, one test each.
    #[test]
    fn press_reads_report_gate() {
        let b = body(MOD, "mouse_press");
        assert!(b.contains("report_gate("), "mouse_press");
        assert!(!b.contains("app_owns_mouse("), "mouse_press keeps its own copy of the rule");
    }

    #[test]
    fn release_pointer_action_reads_report_gate() {
        let b = body(MOD, "mouse_release");
        assert!(b.matches("report_gate(").count() >= 2, "on_up and the app drag's frustration both read it");
        assert!(!b.contains("app_owns_mouse("));
    }

    #[test]
    fn drag_frustration_reads_report_gate() {
        let b = body(MOD, "mouse_move");
        assert!(b.contains("report_gate("));
        assert!(!b.contains("app_owns_mouse("));
    }

    #[test]
    fn hover_motion_reads_report_gate() {
        assert!(body(MOD, "mouse_move").contains("Kind::Hover"));
        assert!(body(MOD, "send_motion").contains("report_gate("));
    }

    #[test]
    fn wheel_reads_report_gate() {
        let b = body(UI, "route_wheel");
        assert!(b.contains("report_gate("));
        assert!(!b.contains("app_owns_mouse("));
    }

    /// The stuck-reporting watch is armed in `write_mouse_report` under its two settings and the
    /// open bar, asks through 13b's host offer (B2, no second bar), and runs after the pump.
    #[test]
    fn stuck_detection_is_wired_to_the_host_offer() {
        let w = body(UI, "write_mouse_report");
        for needle in ["AutodetectMouseReportingStuck", "NoSyncNeverAskAboutMouseReportingFrustration", "bar_id(", ".armed("] {
            assert!(w.contains(needle), "{needle}");
        }
        let t = body(UI, "stuck_tick");
        for needle in ["mouse_allowed(", "Tracking::Off", "offer_key(", "offer_text(", "self.turn_off("] {
            assert!(t.contains(needle), "{needle}");
        }
        let second_bar = format!("fn {}_bar", "stuck");
        assert!(!MOD.contains(&second_bar) && !UI.contains(&second_bar));
        assert!(body(MOD, "user_event").contains("self.stuck_tick()"));
    }

    /// Fix round 2, 1: the wheel over an overlay stops before the program's wheel path (no
    /// report and no alternate-scroll arrows), in `App::wheel` after the UI wheel handlers.
    #[test]
    fn the_wheel_over_an_overlay_reaches_no_program() {
        let b = body(MOD, "wheel");
        let ui = b.find("tandem_ui_at(").expect("wheel asks the overlay list");
        assert!(b.find("porthole_wheel(").unwrap() < ui && ui < b.find("route_wheel(").unwrap());
    }

    /// Fix round 2, 2 and structural: the Move Pane pick joins the one list from `pick_rects`
    /// (the rects `draw_pane_move` paints), and the completion hit uses the box the draw uses.
    #[test]
    fn overlay_hits_are_the_drawn_rects() {
        let u = body(UI, "ui_rects");
        assert!(u.contains("Overlay::PaneMove") && u.contains("pane_move_hit_rects"));
        let c = include_str!("completion_ui.rs");
        let production = c.split("\n#[cfg(test)]").next().unwrap();
        assert!(!MOD.contains("draw_completion(&mut frame, id, self.inner(rect)"), "the draw takes the pane rect");
        assert!(production.contains("self.inner(rect)"), "the shared box applies the inset once");
    }

    /// Task 6 review, Important 1 and 2: `mouse_press` follows `press_follow_up`.
    #[test]
    fn mouse_press_follows_press_follow_up() {
        let b = body(MOD, "mouse_press");
        assert!(b.contains("press_follow_up("));
    }

    /// Every mouse report is written through `write_mouse_report` (Task 7 arms the detector there).
    #[test]
    fn every_mouse_report_goes_through_write_mouse_report() {
        for (src, name) in [(MOD, "mouse_press"), (MOD, "mouse_release"), (MOD, "send_motion"), (UI, "route_wheel")] {
            let b = body(src, name);
            assert!(b.contains("write_mouse_report("), "{name}");
            assert!(!b.contains(".input(&bytes)"), "{name} writes straight to the pane");
        }
    }

    /// B5: the overlay swallow sits before the pane section, and every overlay list feeds it.
    #[test]
    fn overlays_are_swallowed_before_the_pane_section() {
        let b = body(MOD, "mouse_press");
        let swallow = b.find("tandem_ui_at(").expect("mouse_press swallows presses on overlays");
        let pane = b.find("let Some((id, rect)) = self.pane_at").unwrap();
        assert!(swallow < pane);
    }

    #[test]
    fn tandem_ui_at_reads_every_overlay_list() {
        let b = body(UI, "ui_rects");
        for kind in crate::mouse::Overlay::ALL {
            assert!(b.contains(&format!("Overlay::{kind:?}")), "{kind:?} is not pushed into the one list");
        }
        for needle in ["infobar::layout", "porthole_hits", "block_buttons", "picker_hits", "chat_indicator_hits", "find_bar_rect", "find_mode_rect", "completion_rect", "composer_layout", "composer_popup_rect", "toast_rect", "banner_rect"] {
            assert!(b.contains(needle), "{needle} is not read");
        }
        assert!(body(UI, "tandem_ui_at").contains("inline_button_at"), "inline buttons too");
        assert!(!body(UI, "tandem_ui_at").contains("completion") && !body(UI, "tandem_ui_at").contains("composer"), "no per-site special cases");
    }

    /// Review fix 1: the overlay under the pointer holds every kind of event but a drag's end.
    #[test]
    fn the_gate_asks_the_overlay_list_for_every_kind() {
        let b = body(UI, "report_gate_with");
        assert!(b.contains("!matches!(kind, mouse::Kind::Drag)") && b.contains("self.tandem_ui_at(x, y)"));
        assert!(!b.contains("Kind::Click | mouse::Kind::Hover"));
    }

    /// Review fix 2: Alt-click reads the raw mode, not the gate.
    #[test]
    fn alt_click_reads_the_raw_mouse_mode() {
        let b = body(MOD, "alt_click_moves_cursor");
        assert!(b.contains("alt_click_may_move_cursor(") && b.contains("mouse_modes()"));
        assert!(!b.contains("report_gate"), "profile gates and the toggle do not matter");
    }

    /// The `body` helper stops at the nearer next method, private or `pub(super)`.
    #[test]
    fn body_stops_at_the_nearest_method() {
        let b = body(UI, "tandem_ui_at");
        assert!(!b.contains("fn report_gate_with"), "{b}");
    }

    /// R3, B6: the flag is taken at the very top of `mouse_press`, `first_click` runs only after
    /// `pane_at` (after every UI hit test), and the report gate reads `first_mouse_down`.
    #[test]
    fn first_mouse_is_applied_after_the_ui_hit_tests() {
        let b = body(MOD, "mouse_press");
        let take = b.find("self.first_mouse()").expect("the flag is taken in mouse_press");
        let clear = b.find("self.first_mouse_down = false").expect("the next press clears the old flag");
        let find = b.find("self.find_press(").expect("find_press");
        let pane_at = b.find("self.pane_at(x, y)").expect("pane_at");
        let apply = b.find("first_click(").expect("first_click");
        let cell = b.find("self.cell_at(").expect("cell_at");
        let title = b.find("self.title_press(").expect("title_press");
        assert!(take < b.find("self.composer_press(").unwrap(), "taken before any hit test");
        assert!(clear < b.find("self.composer_press(").unwrap());
        assert!(title < apply && find < apply && pane_at < apply, "a title, find or divider press is never first mouse");
        assert!(apply < cell, "before the focus and cell block");
        assert!(b.contains("FirstClick::ActivateOnly") && b.contains("FirstClick::FocusOnly"));
        assert!(b.contains("self.first_mouse_down = counts"), "Normal stores the returned flag");
    }

    #[test]
    fn report_gate_reads_the_first_mouse_flag() {
        let b = body(UI, "report_gate_with");
        assert!(b.contains("first_mouse_blocks: self.first_mouse_down && !self.globals.bool(\"ReportOnFirstMouse\")"));
    }

    #[test]
    fn first_mouse_down_is_per_window() {
        let w = include_str!("windows_ui.rs");
        assert!(w.contains("swap(&mut self.first_mouse_down, &mut s.first_mouse_down)"));
        assert!(w.matches("first_mouse_down: false").count() >= 2, "WinSlot::new and new_empty");
        assert!(MOD.contains("first_mouse_down: bool"));
    }

    #[test]
    fn every_window_is_watched_for_first_mouse() {
        let w = include_str!("windows_ui.rs");
        assert!(body(w, "open_window_with").contains("firstmouse_win::watch"), "windows opened later");
        assert!(MOD.contains("firstmouse_win::watch"), "the first window, in resumed");
        assert!(include_str!("transparency_ui.rs").contains("firstmouse_win::watch"), "a recreated window");
        assert!(body(w, "drop_current_window").contains("firstmouse_win::forget"));
    }
    /// Task 8: Ctrl+wheel is the pinch after every UI wheel handler and the overlay hold,
    /// over a pane only (P15).
    #[test]
    fn pinch_runs_after_the_ui_wheel_handlers() {
        let b = body(MOD, "wheel");
        let ui = b.find("self.porthole_wheel(").expect("the UI handlers");
        let held = b.find("self.tandem_ui_at(").expect("the overlay hold");
        let pane = b.find("self.pane_at(x, y)").expect("pane_at");
        let pinch = b.find("self.pinch(").expect("pinch is called from wheel");
        assert!(ui < pinch && held < pinch, "never over a bar or an overlay");
        assert!(pane < pinch, "only over a pane");
        assert!(body(UI, "pinch").contains("is_pinch(") && body(UI, "pinch").contains("PinchToChangeFontSizeDisabled"), "Ctrl alone and the Advanced switch");
        assert!(body(UI, "pinch").contains("change_font_size("), "the existing zoom path");
        assert!(body(UI, "change_font_size").contains("set_font_offset("));
    }

    /// B3: the activation edge is the existing `app_active` edge; no second field, no Focused
    /// heuristic. The pointer is saved on the falling edge.
    #[test]
    fn activation_edge_is_the_app_active_edge() {
        let b = body(MOD, "about_to_wait");
        let edge = b.find("let active = hotkey_ui::tandem_in_front()").unwrap();
        let set = b.find("self.app_active = active").unwrap();
        let (up, down) = (b.find("self.ffm_became_active()").expect("rising edge"), b.find("self.ffm_became_inactive()").expect("falling edge"));
        assert!(edge < up && up < set && edge < down && down < set, "both before the flag is overwritten");
        assert_eq!(MOD.matches("app_active: bool").count(), 1);
        assert!(MOD.contains("ffm_saved: Option<(f64, f64)>") && MOD.contains("magnifier: crate::wheel::Magnifier"));
        let w = include_str!("windows_ui.rs");
        assert!(!w.contains("ffm_saved") && !w.contains("magnifier"), "both are app-wide, not per window");
    }

    /// The pointer entering a window is a `CursorEntered` event; the plain-FFM pane focus in
    /// `mouse_move` is the one `ffm_enter` also serves.
    #[test]
    fn cursor_entered_runs_ffm_enter() {
        assert!(MOD.contains("WindowEvent::CursorEntered"));
        let enter = body(UI, "ffm_enter");
        for needle in ["ffm_on_enter(", "ffm_may_act(", "focus_terminal_window(", "SetForegroundWindow", "hidden_hotkey_key", "StealKeyFocus"] {
            assert!(enter.contains(needle), "{needle}");
        }
        assert!(!enter.contains("Drag::App"), "a held press keeps its pane");
        let act = body(UI, "ffm_became_active");
        assert!(act.contains("ffm_on_activate(") && act.contains("AggressiveFocusFollowsMouse"));
        assert!(!body(MOD, "mouse_move").contains("self.tabs[self.active].focused = id"), "the pane focus moved into one helper");
    }

    /// The menu's bigger and smaller text use `change_font_size`, one clamp.
    #[test]
    fn menu_font_arms_use_change_font_size() {
        let m = include_str!("menu.rs");
        assert!(m.contains("\"biggerFont:\" => self.change_font_size(1)"));
        assert!(m.contains("\"smallerFont:\" => self.change_font_size(-1)"));
    }
}
