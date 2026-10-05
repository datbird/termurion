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
        let switches = self.alt_switches(id);
        let strings_set = !self.globals.string("AlternateMouseScrollStringForUp").is_empty() || !self.globals.string("AlternateMouseScrollStringForDown").is_empty();
        let horizontal_reports = self.globals.bool("ReportHorizontalScrollEvents");
        let prefs = wheel::Prefs::from_globals(&self.globals);
        let affects = self.globals.bool("NaturalScrollingAffectsHorizontalMouseReporting");
        let (cw, ch) = self.cell();
        let scale = self.scale() as f64;
        let cell_pt = (cw as f64 / scale, ch as f64 / scale);
        let Some(p) = self.panes.get_mut(&id) else { return };
        let mode = *p.screen.term().mode();
        let reporting = mouse::app_owns_mouse(modes, mods);
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
                p.input(&bytes);
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
        let Some(p) = self.panes.get_mut(&id) else { return };
        if reporting {
            // Positive x moves the content right: the view scrolls left.
            let bytes = if vertical {
                let n = p.wheel.report.take(dy, ev.precise, cell_pt.1, &prefs);
                input::wheel_reports_with(modes, mods, n as i32, col, row)
            } else if horizontal_reports {
                let n = p.wheel.horizontal.take(dx, ev.precise, cell_pt.0, &prefs);
                let left = wheel::natural_swap(n > 0.0, ev.precise, touchpad_natural(), affects);
                let button = if left { Button::WheelLeft } else { Button::WheelRight };
                mouse::encode(modes, mouse::Event { button, action: mouse::Action::Press, mods, col, row }).unwrap_or_default().repeat(n.abs() as usize)
            } else {
                Vec::new()
            };
            if !bytes.is_empty() {
                p.input(&bytes);
            }
        } else if vertical {
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
