//! The paste progress view on the App (`PasteView.xib`, `PasteViewController.m`, `iTermPasteHelper.m`):
//! drawn at the top right of a pane whose paste shows progress, with Cancel and, while the paste
//! waits for the prompt, the keyboard button that lets keystrokes through. Keys typed during a
//! paste are held until it ends (`PTYTextView.m:7510`), or sent at once with passthrough on. The
//! words and rules are `paste_view`.

use super::*;
use crate::paste_view as pv;

impl App {
    /// Draws the view for each visible pane of `tab` that is pasting with progress and no status bar
    /// progress component; returns what a click can hit.
    pub(super) fn paste_view_draw(&self, frame: &mut scene::Frame, tab: &Tab) -> Vec<(PaneId, scene::PasteHits)> {
        let now = Instant::now();
        let mut hits = Vec::new();
        for (id, rect) in self.visible(tab) {
            let Some((q, _, shows)) = self.pastes.get(&id) else { continue };
            let Some((sent, total)) = q.progress().filter(|_| *shows && !self.bar_shown_for(id)) else { continue };
            let state = self.paste_view.get(&id).copied().unwrap_or_default();
            let hint = state.look(now);
            let fraction = sent as f32 / total.max(1) as f32;
            let h = scene::paste_view(frame, rect, self.pane_cell(id), self.scale(), self.dropdown_views.old_style(), pv::PASTING, fraction, q.waiting_for_prompt(), state.passthrough, hint);
            hits.push((id, h));
        }
        hits
    }

    /// The pane's paste is waiting for the shell prompt.
    fn paste_waiting(&self, pane: PaneId) -> bool {
        self.pastes.get(&pane).is_some_and(|(q, _, _)| q.waiting_for_prompt())
    }

    /// A left press on the view: Cancel stops the paste, the keyboard button toggles passthrough
    /// (queued keys go at once when it turns on, `pasteViewManagerDidSetKeystrokePassthrough:`).
    pub(super) fn paste_view_press(&mut self, x: f32, y: f32) -> bool {
        let Some((pane, hit)) = self.paste_hits.iter().chain(self.paste_status_hits.iter()).find(|(_, h)| h.view.contains(x, y)).map(|(p, h)| (*p, *h)) else { return false };
        if hit.cancel.contains(x, y) {
            self.cancel_paste(pane);
        } else if hit.keyboard.is_some_and(|k| k.contains(x, y)) {
            let on = self.paste_view.entry(pane).or_default().toggle(Instant::now());
            if on {
                self.flush_queued_keys = true;
            }
        }
        self.redraw();
        true
    }

    /// The text the pointer's hover shows over the keyboard button.
    pub(super) fn paste_view_tip(&self, x: f32, y: f32) -> Option<&'static str> {
        self.paste_hits.iter().chain(self.paste_status_hits.iter()).find(|(_, h)| h.keyboard.is_some_and(|k| k.contains(x, y))).map(|_| pv::KEYSTROKES_TOOLTIP)
    }

    /// A key while the focused pane pastes: Esc or Ctrl+C abort a paste waiting for the prompt;
    /// other keys are held until the paste ends (and show the hint while it waits). True when taken.
    pub(super) fn paste_view_key(&mut self, ev: &KeyEvent) -> bool {
        let Some(pane) = self.focused_id() else { return false };
        let pasting = self.pastes.get(&pane).is_some_and(|(q, _, _)| q.busy());
        if !pasting || ev.state != ElementState::Pressed {
            return false;
        }
        let waiting = self.paste_waiting(pane);
        let passthrough = self.paste_view.get(&pane).is_some_and(|s| s.passthrough);
        let escape = ev.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Escape);
        let ctrl_c = self.mods.ctrl && !self.mods.alt && !self.mods.shift && matches!(&ev.key_without_modifiers(), winit::keyboard::Key::Character(c) if c.eq_ignore_ascii_case("c"));
        if pv::aborts(waiting, passthrough, escape, ctrl_c) {
            self.cancel_paste(pane);
            return true;
        }
        if !pv::queues_keys(pasting, passthrough) {
            return false;
        }
        self.paste_view.entry(pane).or_default().typed(Instant::now(), waiting);
        self.queued_keys.entry(pane).or_default().push(ev.clone());
        self.redraw();
        true
    }

    /// Forgets what a finished paste left (`_keystrokePassthrough = NO` for the next one) and sends
    /// the keys it held. Called with the loop's event source, since a held key is a key press.
    pub(super) fn paste_view_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        let now = Instant::now();
        let flush = std::mem::take(&mut self.flush_queued_keys);
        let ready: Vec<PaneId> = self
            .queued_keys
            .keys()
            .copied()
            .filter(|p| {
                let busy = self.pastes.get(p).is_some_and(|(q, _, _)| q.busy());
                !busy || flush || self.paste_view.get(p).is_some_and(|s| s.passthrough)
            })
            .collect();
        for pane in ready {
            let keys = self.queued_keys.remove(&pane).unwrap_or_default();
            if self.focused_id() == Some(pane) {
                for ev in &keys {
                    self.key(ev, event_loop);
                }
            }
        }
        let idle: Vec<PaneId> = self.paste_view.keys().copied().filter(|p| !self.pastes.get(p).is_some_and(|(q, _, _)| q.busy()) && !self.queued_keys.contains_key(p)).collect();
        for p in idle {
            self.paste_view.remove(&p);
        }
        for (p, s) in self.paste_view.iter_mut() {
            if !self.pastes.get(p).is_some_and(|(q, _, _)| q.waiting_for_prompt()) {
                s.stopped_waiting(now);
            }
        }
        // The hint animates: a frame about every 16 ms while it is on screen.
        let animating = self.paste_view.values().any(|s| s.hint_shown(now));
        if !self.paste_hits.is_empty() || !self.paste_status_hits.is_empty() || animating {
            self.redraw();
        }
        animating.then(|| now + std::time::Duration::from_millis(16))
    }
}
