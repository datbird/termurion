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

    /// Tandem's own overlays inside the terminal rectangle, by kind (B5): the front info bar
    /// of each visible pane, portholes, block pills, session pickers, AI indicators and the
    /// find bar of the focused pane.
    fn ui_rects(&self) -> mouse::UiRects {
        let mut ui = mouse::UiRects::default();
        let cell = self.cell();
        if let Some(tab) = self.tabs.get(self.active) {
            for (id, rect) in self.visible(tab) {
                if let Some(bar) = self.bars.get(&id).and_then(|b| b.front()) {
                    ui.info_bars.push(crate::infobar::layout(bar, rect, cell).bar);
                }
            }
        }
        ui.portholes = self.porthole_hits.iter().map(|h| h.3).collect();
        ui.pills = self.block_buttons.iter().map(|b| b.0).collect();
        ui.pickers = self.picker_hits.iter().map(|h| h.1).collect();
        ui.indicators = self.chat_indicator_hits.iter().map(|h| h.1).collect();
        if self.focused_id().is_some_and(|id| self.finds.get(&id).is_some_and(|f| f.open)) {
            ui.find.extend(self.find_bar_rect);
            ui.find.extend(self.find_mode_rect);
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
            first_mouse_blocks: false,
            ui_over: matches!(kind, mouse::Kind::Click | mouse::Kind::Hover) && self.tandem_ui_at(x, y),
        };
        mouse::should_report(self.modes(pane), mods, kind, gates)
    }

    /// Every mouse report goes to its pane through here.
    pub(super) fn write_mouse_report(&mut self, pane: PaneId, bytes: &[u8]) {
        if let Some(p) = self.panes.get_mut(&pane) {
            p.input(bytes);
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
                let left = wheel::natural_swap(n > 0.0, ev.precise, touchpad_natural(), affects);
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
        let end = rest[1..].find("\n    fn ").or_else(|| rest[1..].find("\n    pub(super) fn ")).map_or(rest.len(), |e| e + 1);
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
    fn alt_click_reads_report_gate() {
        let b = body(MOD, "alt_click_moves_cursor");
        assert!(b.contains("report_gate_with("), "alt-click moves the cursor only where the click would not be reported");
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
        for needle in ["infobar::layout", "porthole_hits", "block_buttons", "picker_hits", "chat_indicator_hits", "find_bar_rect", "find_mode_rect"] {
            assert!(b.contains(needle), "{needle} is not read");
        }
        assert!(body(UI, "tandem_ui_at").contains("inline_button_at"), "inline buttons too");
    }
}
