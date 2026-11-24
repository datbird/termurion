//! The About window: opened from the menu, closed by its buttons.

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::App;
use crate::about_ui::{AboutWindow, Choice};

impl App {
    /// About Termurion: a second request brings the open window forward.
    pub(super) fn show_about(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.about_window {
            w.focus();
            return;
        }
        let gpu = self.gpu.as_ref().map(|g| (&g.instance, &g.adapter, &g.device, &g.queue)).or(self.prefs_gpu.as_ref().map(|(i, a, d, q)| (i, a, d, q)));
        match gpu {
            Some((i, a, d, q)) => match AboutWindow::open(event_loop, i, a, d, q) {
                Ok(w) => self.about_window = Some(w),
                Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
            },
            None => self.banners.set("window", "Could not open the window: no graphics device yet".to_string()),
        }
    }

    /// Events for the About window. False for other windows.
    pub(super) fn about_window_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.about_window.as_mut().filter(|w| w.id() == window) else { return false };
        match event {
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
            }
        }
        match w.take_choice() {
            Some(Choice::OpenProject) => match super::semantic_ui::shell_open(crate::links::REPO, false) {
                Ok(()) => {}
                Err(e) => self.banners.set("action", format!("Could not open {}: {e}", crate::links::REPO)),
            },
            Some(Choice::Copy) => {
                let text = w.text();
                self.set_clipboard_text(text);
            }
            Some(Choice::Close) => self.about_window = None,
            None => {}
        }
        true
    }
}
