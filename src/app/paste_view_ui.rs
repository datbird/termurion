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
            let under = self.panes.get(&id).map_or(crate::palette::rgb(0, 0, 0), |p| p.screen.palette.bg);
            let anim = self.paste_anim.get(&id).copied().unwrap_or_else(|| pv::Anim::new(now));
            if let Some((q, _, shows)) = self.pastes.get(&id) {
                if let Some((sent, total)) = q.progress().filter(|_| *shows && !self.bar_shown_for(id)) {
                    let state = self.paste_view.get(&id).copied().unwrap_or_default();
                    let hint = state.look(now);
                    let fraction = sent as f32 / total.max(1) as f32;
                    // A view back for a new paste before the tick restarts its animation starts over.
                    let look = if anim.closed.is_some() { pv::opening(0.0) } else { anim.look(now).unwrap_or(pv::AT_REST) };
                    let h = scene::paste_view(frame, rect, self.pane_cell(id), self.scale(), self.dropdown_views.old_style(), pv::PASTING, fraction, q.waiting_for_prompt(), state.passthrough, hint, look, under);
                    hits.push((id, h));
                    continue;
                }
            }
            // The paste ended: the view fades out where it was, with nothing to click.
            if anim.closed.is_some() {
                if let Some(look) = anim.look(now) {
                    scene::paste_view(frame, rect, self.pane_cell(id), self.scale(), self.dropdown_views.old_style(), pv::PASTING, anim.fraction, false, false, None, look, under);
                }
            }
        }
        hits
    }

    /// Keeps each pane's view animation: it starts when the view first shows, fades out when the
    /// paste ends, and goes when the fade is done. True while one is moving.
    fn paste_anim_tick(&mut self, now: Instant) -> bool {
        let mut live: Vec<(PaneId, f32)> = Vec::new();
        for (id, (q, _, shows)) in self.pastes.iter() {
            if let Some((sent, total)) = q.progress().filter(|_| *shows && !self.bar_shown_for(*id)) {
                live.push((*id, sent as f32 / total.max(1) as f32));
            }
        }
        for (id, fraction) in &live {
            self.paste_anim.entry(*id).or_insert_with(|| pv::Anim::new(now)).step(now, true, *fraction);
        }
        self.paste_anim.retain(|id, a| live.iter().any(|(l, _)| l == id) || a.step(now, false, 0.0));
        self.paste_anim.values().any(|a| a.animating(now))
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
        self.queued_keys.entry(pane).or_default().push(pv::Held { key: ev.clone(), mods: self.mods, phys_mods: self.phys_mods });
        self.redraw();
        true
    }

    /// Sends keys a paste held to the pane they were typed in, each with the modifiers it had then
    /// (iTerm2 writes them to that session). A pane that is gone drops them. The key path acts on the
    /// focused pane, so the pane is made the focused one for the replay and put back after, unless a
    /// key changed the focus itself.
    fn replay_held_keys(&mut self, pane: PaneId, keys: Vec<pv::Held<KeyEvent, Mods>>, event_loop: &ActiveEventLoop) {
        let Some(at) = self.tabs.iter().position(|t| t.layout.panes().contains(&pane)) else { return };
        let (tab_id, was_active, was_focused) = (self.tabs[at].id, self.active, self.tabs[at].focused);
        let (mods, phys) = (self.mods, self.phys_mods);
        for held in &keys {
            self.active = at;
            self.tabs[at].focused = pane;
            self.mods = held.mods;
            self.phys_mods = held.phys_mods;
            self.key(&held.key, event_loop);
            if self.tabs.get(at).is_none_or(|t| t.id != tab_id) {
                break;
            }
        }
        self.mods = mods;
        self.phys_mods = phys;
        if let Some(t) = self.tabs.get_mut(at).filter(|t| t.id == tab_id && t.focused == pane) {
            t.focused = was_focused;
            if self.active == at && was_active < self.tabs.len() {
                self.active = was_active;
            }
        }
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
            self.replay_held_keys(pane, keys, event_loop);
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
        let moving = self.paste_anim_tick(now);
        let animating = moving || self.paste_view.values().any(|s| s.hint_shown(now));
        if !self.paste_hits.is_empty() || !self.paste_status_hits.is_empty() || animating {
            self.redraw();
        }
        animating.then(|| now + std::time::Duration::from_millis(16))
    }
}
