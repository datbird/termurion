//! The Cockpit in the app (Window > Cockpit): opening it, hiding it with the app, and
//! keeping its frame, split and group mode.

use super::*;
use crate::cockpit::{self, Act, Frame, GroupMode, View};
use crate::cockpit_ui::CockpitWindow;

/// A drag emits many moves; the globals file is written at most this often.
const SAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The panel's state at launch: the saved group mode and list height.
pub(super) fn initial_view(g: &Globals) -> View {
    let split = g.string(cockpit::SPLIT_KEY).trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0);
    View { mode: GroupMode::from_raw(g.int(cockpit::GROUP_KEY)), list_height: split, ..Default::default() }
}

impl App {
    /// Window > Cockpit (`showAndFocusCommand`): opens the one window where it was last, or
    /// shows it again, and puts the caret in the command bar.
    pub(super) fn reveal_cockpit(&mut self, event_loop: &ActiveEventLoop) {
        self.cockpit_view.focus_command = true;
        if let Some(w) = self.cockpit.as_mut() {
            w.show_and_focus();
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let work: Vec<cockpit::Area> = super::requests_ui::work_areas().into_iter().map(|(x, y, w, h)| cockpit::Area { x: x as f32, y: y as f32, w: w as f32, h: h as f32 }).collect();
        let frame = cockpit::place(Frame::parse(&self.globals.string(cockpit::FRAME_KEY)), &work);
        let dark = self.dark();
        match CockpitWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, frame, dark) {
            Ok(mut w) => {
                if let Some(hwnd) = w.hwnd() {
                    // The key hook takes Win chords only for registered windows.
                    crate::winkeys::add_cockpit_window(hwnd);
                }
                w.show_and_focus();
                self.cockpit = Some(w);
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Hidden while another app is in front, back when Tandem is (`hidesOnDeactivate`); the
    /// Cockpit's own window counts as Tandem's.
    pub(super) fn cockpit_follows_app(&mut self) {
        let in_front = super::hotkey_ui::tandem_in_front();
        if let Some(w) = self.cockpit.as_mut() {
            w.set_shown(in_front);
        }
    }

    /// Events for the Cockpit window. False when `window` is not it.
    pub(super) fn cockpit_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.cockpit.as_mut() else { return false };
        if w.id() != window {
            return false;
        }
        match event {
            WindowEvent::CloseRequested => self.cockpit_acts(vec![Act::Closed]),
            WindowEvent::RedrawRequested => {
                let acts = w.redraw(&mut self.cockpit_view);
                self.cockpit_acts(acts);
            }
            other => {
                w.event(other);
            }
        }
        true
    }

    /// Each pass: the tip line expires on its own, and a pending save of the frame is written.
    pub(super) fn cockpit_tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        if self.cockpit_save_due.is_some_and(|t| now >= t) {
            self.cockpit_save_due = None;
            self.save_globals();
        }
        if self.cockpit_view.tip.expire(now) {
            if let Some(w) = self.cockpit.as_ref() {
                w.request_redraw();
            }
        }
        [self.cockpit_save_due, self.cockpit_view.tip.next_change()].into_iter().flatten().min()
    }

    /// What a redraw asked for: the window closes (the view keeps its state), or the frame or
    /// split is stored, written to the settings file at most once a second.
    pub(super) fn cockpit_acts(&mut self, acts: Vec<Act>) {
        for act in acts {
            match act {
                Act::Closed => {
                    if let Some(hwnd) = self.cockpit.as_ref().and_then(|w| w.hwnd()) {
                        crate::winkeys::remove_cockpit_window(hwnd);
                    }
                    self.cockpit = None;
                    self.cockpit_save_due = None;
                    self.save_globals();
                }
                Act::Moved(f) => self.cockpit_store(cockpit::FRAME_KEY, f.text()),
                Act::Split(h) => self.cockpit_store(cockpit::SPLIT_KEY, h.to_string()),
            }
        }
    }

    fn cockpit_store(&mut self, key: &str, text: String) {
        self.globals.set(key, toml::Value::String(text));
        if self.cockpit_save_due.is_none() {
            self.cockpit_save_due = Some(Instant::now() + SAVE_EVERY);
        }
    }
}
