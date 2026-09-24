//! The mark commands (Edit > Marks and Annotations, Clear to Last Mark, the two Select
//! commands), mark alerts, and the Set Named Mark prompt.

use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;

use super::App;
use crate::layout::PaneId;
use crate::marks;
use crate::prompt_ui::{PromptResult, PromptWindow};

/// Mark commands this module runs.
pub(super) const SELECTORS: [&str; 13] = [
    "saveScrollPosition:",
    "addNamedMark:",
    "jumpToSavedScrollPosition:",
    "nextMark:",
    "previousMark:",
    "clearToLastMark:",
    "selectOutputOfLastCommand:",
    "selectCurrentCommand:",
    "toggleAlertOnNextMark:",
    "toggleAlertOnMarksInOffscreenSessions:",
    "enableMarkAlertShowsModalAlert:",
    "enableMarkAlertPostsNotification:",
    "enableMarkAlertRingsBell:",
];

/// Menu items with a check mark: (selector, Mark Alert Action value or "").
pub(super) const CHECKS: [(&str, &str); 5] = [
    ("toggleAlertOnNextMark:", ""),
    ("toggleAlertOnMarksInOffscreenSessions:", ""),
    ("enableMarkAlertShowsModalAlert:", "Modal Alert"),
    ("enableMarkAlertPostsNotification:", "Post Notification"),
    ("enableMarkAlertRingsBell:", "Ring Bell"),
];

/// The display offset that puts grid line `line` at the top of the view (or centers it).
pub(super) fn offset_for(line: i32, rows: usize, history: usize, center: bool) -> usize {
    let top = if center { line - rows as i32 / 2 } else { line };
    (-top).clamp(0, history as i32) as usize
}

impl App {
    fn focused_id_for_marks(&self) -> Option<PaneId> {
        self.tabs.get(self.active).map(|t| t.focused)
    }

    /// Scrolls pane `id` so grid line `line` is at the top (or centered).
    fn show_line(&mut self, id: PaneId, line: i32, center: bool) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let grid = p.screen.term().grid();
        let want = offset_for(line, grid.screen_lines(), grid.history_size(), center) as i32;
        let now = grid.display_offset() as i32;
        p.scroll(Scroll::Delta(want - now));
    }

    fn select(&mut self, id: PaneId, (a, b): (Point, Point)) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let mut sel = Selection::new(SelectionType::Simple, a, Side::Left);
        sel.update(b, Side::Right);
        p.screen.term_mut().selection = Some(sel);
    }

    fn top_visible_line(&self, id: PaneId) -> i32 {
        self.panes.get(&id).map_or(0, |p| -(p.screen.term().grid().display_offset() as i32))
    }

    fn step_mark(&mut self, id: PaneId, forward: bool) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let marks = p.screen.marks_in_order();
        let lines: Vec<i32> = marks.iter().map(|(l, _)| *l).collect();
        let from = self.mark_pos.get(&id).and_then(|m| marks.iter().find(|(_, x)| x.id == *m)).map(|(l, _)| *l);
        let Some(line) = marks::next_mark(&lines, from, forward) else { return };
        let mark = marks.iter().find(|(l, _)| *l == line).map(|(_, m)| m.clone()).unwrap();
        self.mark_pos.insert(id, mark.id);
        if mark.command.is_empty() {
            self.show_line(id, line, true);
        } else {
            // A command mark goes to the top, with its command selected.
            self.show_line(id, line, false);
            let range = mark.command_range.and_then(|(a, b)| {
                let p = self.panes.get(&id)?;
                Some((p.screen.resolve(a)?, p.screen.resolve(b)?))
            });
            if let Some((a, b)) = range {
                let b = if b.column.0 > 0 { Point::new(b.line, b.column - 1) } else { b };
                self.select(id, (a, b));
            }
        }
    }

    /// Runs one mark command. False when `selector` is not one.
    pub(super) fn run_mark_command(&mut self, selector: &str, event_loop: &ActiveEventLoop) -> bool {
        if !SELECTORS.contains(&selector) {
            return false;
        }
        let Some(id) = self.focused_id_for_marks() else { return true };
        match selector {
            "saveScrollPosition:" => {
                let line = self.top_visible_line(id);
                if let Some(p) = self.panes.get_mut(&id) {
                    let m = p.screen.set_manual_mark(line, None);
                    self.mark_pos.insert(id, m);
                }
            }
            "addNamedMark:" => self.open_named_mark_prompt(id, event_loop),
            "jumpToSavedScrollPosition:" => {
                let last = self.panes.get_mut(&id).and_then(|p| p.screen.marks_in_order().last().map(|(l, m)| (*l, m.id)));
                if let Some((line, m)) = last {
                    self.mark_pos.insert(id, m);
                    self.show_line(id, line, false);
                }
            }
            "nextMark:" => self.step_mark(id, true),
            "previousMark:" => self.step_mark(id, false),
            "clearToLastMark:" => {
                if let Some(p) = self.panes.get_mut(&id) {
                    let lines: Vec<i32> = p.screen.marks_in_order().iter().map(|(l, _)| *l).collect();
                    let cursor = p.screen.term().grid().cursor.point.line.0;
                    let start = marks::clear_start(&lines, p.screen.current_prompt_line(), cursor);
                    p.screen.clear_from(start);
                    p.scroll(Scroll::Bottom);
                }
            }
            "selectOutputOfLastCommand:" => {
                if let Some(r) = self.panes.get(&id).and_then(|p| p.screen.last_output_range()) {
                    self.select(id, r);
                    self.copy_selection(id);
                }
            }
            "selectCurrentCommand:" => {
                if let Some(r) = self.panes.get(&id).and_then(|p| p.screen.last_command_range()) {
                    self.select(id, r);
                    self.copy_selection(id);
                }
            }
            "toggleAlertOnNextMark:" => {
                if let Some(p) = self.panes.get_mut(&id) {
                    p.screen.marks.alert_next = !p.screen.marks.alert_next;
                }
            }
            "toggleAlertOnMarksInOffscreenSessions:" => {
                let now = self.globals.bool("Alert On Marks in Offscreen Sessions");
                self.globals.set("Alert On Marks in Offscreen Sessions", toml::Value::Boolean(!now));
                self.save_globals();
            }
            other => {
                if let Some((_, value)) = CHECKS.iter().find(|(s, v)| *s == other && !v.is_empty()) {
                    self.globals.set("Mark Alert Action", toml::Value::String(value.to_string()));
                    self.save_globals();
                }
            }
        }
        true
    }

    fn open_named_mark_prompt(&mut self, id: PaneId, event_loop: &ActiveEventLoop) {
        if let Some((w, _)) = &self.prompt {
            return w.focus();
        }
        let Some(g) = self.gpu.as_ref() else { return };
        match PromptWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, "Set Named Mark", "Enter Mark Name", &crate::prompt_ui::local_stamp()) {
            Ok(w) => self.prompt = Some((w, id)),
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Handles an event for the Set Named Mark window. False when `window` is not it.
    pub(super) fn prompt_event(&mut self, window: winit::window::WindowId, event: &WindowEvent) -> bool {
        let Some((w, pane)) = self.prompt.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        let pane = *pane;
        match event {
            WindowEvent::CloseRequested => self.prompt = None,
            WindowEvent::RedrawRequested => match w.redraw() {
                PromptResult::Open => {}
                PromptResult::Cancel => self.prompt = None,
                PromptResult::Ok(name) => {
                    self.prompt = None;
                    // An empty name cancels, as in iTerm2.
                    if !name.trim().is_empty() {
                        let line = self.top_visible_line(pane);
                        if let Some(p) = self.panes.get_mut(&pane) {
                            let m = p.screen.set_manual_mark(line, Some(name));
                            self.mark_pos.insert(pane, m);
                        }
                        self.redraw();
                    }
                }
            },
            other => {
                w.event(other);
            }
        }
        true
    }

    /// After new output: a new mark resets mark navigation, and alerts when asked to.
    pub(super) fn check_marks(&mut self) {
        let active: Vec<PaneId> = self.tabs.get(self.active).map(|t| t.layout.panes()).unwrap_or_default();
        let offscreen = self.globals.bool("Alert On Marks in Offscreen Sessions");
        let mut alerts = Vec::new();
        for (id, p) in self.panes.iter_mut() {
            if !p.screen.take_new_mark() {
                continue;
            }
            if let Some((_, m)) = p.screen.marks_in_order().last() {
                self.mark_pos.insert(*id, m.id);
            }
            if std::mem::take(&mut p.screen.marks.alert_next) || (offscreen && !active.contains(id)) {
                let title = p.screen.title().unwrap_or("Shell").to_string();
                alerts.push((title, p.screen.previous_code().unwrap_or(0) != 0));
            }
        }
        for (title, failed) in alerts {
            self.mark_alert(&title, failed);
        }
    }

    fn mark_alert(&mut self, title: &str, failed: bool) {
        use windows_sys::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONHAND, MB_OK};
        match self.globals.string("Mark Alert Action").as_str() {
            "Modal Alert" => super::menu::info_box(&format!("Mark set in session \"{title}\".")),
            "Ring Bell" => unsafe {
                MessageBeep(if failed { MB_ICONHAND } else { MB_OK });
            },
            _ => {
                let result = self.hwnd().ok_or_else(|| "no window".to_string()).and_then(|h| crate::notify::show(h, "Mark Set", &format!("Session {title} had a mark set.")));
                if let Err(e) = result {
                    self.banners.set("notify", format!("Notification failed: {e}"));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_scroll_to_the_top_or_the_middle() {
        assert_eq!(offset_for(-30, 20, 100, false), 30);
        assert_eq!(offset_for(-30, 20, 100, true), 40);
        assert_eq!(offset_for(5, 20, 100, false), 0);
        assert_eq!(offset_for(-300, 20, 100, false), 100);
    }
}
