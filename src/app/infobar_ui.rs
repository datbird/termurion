//! Announcement bars in panes: queuing, drawing, and what their buttons do.

use super::*;
use crate::infobar::{self, Bar};

impl App {
    /// Queues a bar on a pane (once per question) and redraws.
    pub(super) fn ask(&mut self, pane: PaneId, id: &str, text: &str, buttons: &[&str], important: bool) {
        let bar = Bar { id: id.to_string(), text: text.to_string(), buttons: buttons.iter().map(|b| b.to_string()).collect(), important };
        self.bars.entry(pane).or_default().push(bar);
        self.redraw();
    }

    /// Draws a pane's front bar.
    pub(super) fn draw_info_bar(&self, frame: &mut Frame, pane: PaneId, rect: Rect, cell: (f32, f32)) {
        if let Some(bar) = self.bars.get(&pane).and_then(|b| b.front()) {
            scene::info_bar(frame, bar, &infobar::layout(bar, rect, cell), cell);
        }
    }

    /// A click on a bar: runs its button (or closes it). True when the click was on one.
    pub(super) fn info_bar_press(&mut self, x: f32, y: f32, event_loop: &ActiveEventLoop) -> bool {
        let cell = self.cell();
        let Some(tab) = self.tabs.get(self.active) else { return false };
        for (id, rect) in self.visible(tab) {
            let Some(bar) = self.bars.get(&id).and_then(|b| b.front()) else { continue };
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
        if !self.clipboard_bar(pane, id, button, event_loop) {
            self.prompted_bar(pane, id, button);
        }
    }
}
