//! Announcement bars in panes: queuing, drawing, and what their buttons do.

use super::*;
use crate::infobar::{self, Bar};

/// How many questions may wait in one pane.
const BARS_PER_PANE: usize = 4;

impl App {
    /// Queues a bar on a pane (once per question) and redraws.
    pub(super) fn ask(&mut self, pane: PaneId, id: &str, text: &str, buttons: &[&str], important: bool) -> bool {
        let bar = Bar { id: id.to_string(), text: text.to_string(), buttons: buttons.iter().map(|b| b.to_string()).collect(), important };
        // Output cannot pile up questions: past a few waiting, new ones are dropped.
        let asked = self.bars.entry(pane).or_default().push_capped(bar, BARS_PER_PANE).is_ok();
        self.redraw();
        asked
    }

    /// Draws a pane's front bar and returns the box it drew (the overlay the events read).
    pub(super) fn draw_info_bar(&self, frame: &mut Frame, pane: PaneId, rect: Rect, cell: (f32, f32)) -> Option<Rect> {
        let bar = self.bars.get(&pane).and_then(|b| b.front())?;
        Some(scene::info_bar(frame, bar, &infobar::layout(bar, rect, cell), cell))
    }

    /// A click on a bar: runs its button (or closes it). True when the click was on one.
    pub(super) fn info_bar_press(&mut self, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let Some(tab) = self.tabs.get(self.active) else { return false };
        for (id, rect) in self.visible(tab) {
            let Some(bar) = self.bars.get(&id).and_then(|b| b.front()) else { continue };
            // The bar is laid out in its pane's cell (`build_frame`).
            let cell = self.pane_cell(id);
            if !infobar::layout(bar, rect, cell).bar.contains(x, y) {
                continue;
            }
            if let Some(pressed) = infobar::hit(bar, rect, cell, x, y) {
                let bar = self.bars.get_mut(&id).and_then(|b| b.pop());
                if let Some(bar) = bar {
                    self.bar_answered(id, &bar.id, pressed.map(|i| bar.buttons[i].as_str()), event_loop);
                }
                self.redraw();
            }
            return true;
        }
        false
    }

    /// What a bar's answer does. `button` is None when the bar was closed.
    pub(super) fn bar_answered(&mut self, pane: PaneId, id: &str, button: Option<&str>, event_loop: &ActiveEventLoop) {
        if !self.host_state_bar(pane, id, button) && !self.paste_bar(pane, id, button) && !self.shell_history_bar(pane, id, button) && !self.clipboard_bar(pane, id, button, event_loop) && !self.porthole_link_bar(pane, id, button) && !self.trigger_bar(pane, id, button) && !self.bell_bar(pane, id, button) && !self.coprocess_bar(pane, id, button) && !self.status_bar(pane, id, button) && !self.captured_bar(pane, id, button) && !self.git_bar(pane, id, button) && !self.session_end_bar(pane, id, button, event_loop) && !self.tmux_bar(pane, id, button, event_loop) && !self.host_offer_bar(pane, id, button) && !self.frustration_bar(pane, id, button) && !self.alt_scroll_bar(pane, id, button) && !self.arrangement_bar(pane, id, button, event_loop) && !self.shell_version_bar(pane, id, button) && !self.claude_watcher_bar(pane, id, button) {
            self.prompted_bar(pane, id, button);
        }
    }
}
