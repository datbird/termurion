//! Broadcast input in the window: the window's broadcast state as the model sees it, and
//! the one way user input reaches sessions (the sender's domain, or the sender alone).

use super::*;
use crate::broadcast::{self, TabView, View, Write};

impl App {
    /// The stripe tile for this display's scale (made once per scale).
    pub(super) fn stripe_tile(&mut self) -> (u32, std::sync::Arc<Vec<u8>>) {
        let scale = self.scale();
        let n = (80.0 * scale.max(0.5)).round() as u32;
        match &self.stripes {
            Some((m, px)) if *m == n => (n, px.clone()),
            _ => {
                let (n, px) = crate::scene::stripe_tile(scale);
                let px = std::sync::Arc::new(px);
                self.stripes = Some((n, px.clone()));
                (n, px)
            }
        }
    }

    /// The current window as the broadcast model sees it.
    pub(super) fn broadcast_view(&self) -> View {
        let tabs = self.tabs.iter().map(|t| TabView { broadcasting: self.broadcast_tabs.contains(&t.id), panes: t.layout.panes() }).collect();
        let current = self.tabs.get(self.active).map_or(0, |t| t.focused);
        // A browser pane takes no typing: it is never a broadcast target.
        let exited = self.panes.iter().filter(|(_, p)| p.exited.is_some() || p.browser_io().is_some()).map(|(id, _)| *id).collect();
        View { tabs, current_tab: self.active, current, exited }
    }

    /// Writes the model's tab flags back.
    pub(super) fn keep_broadcast_flags(&mut self, v: &View) {
        self.broadcast_tabs = self.tabs.iter().zip(&v.tabs).filter(|(_, tv)| tv.broadcasting).map(|(t, _)| t.id).collect();
    }

    /// User input from `sender`: to every session of its broadcast domain (the same bytes,
    /// as the sender encoded them), or to the sender alone.
    pub(super) fn user_write(&mut self, sender: PaneId, bytes: &[u8], what: Write) {
        self.write_to_domain(sender, bytes, what, false);
    }

    /// Program-chosen text (Command Info, the navigator, the buttons): as `user_write`, but a
    /// target that is ended or reading a password is skipped. Returns how many were skipped.
    pub(super) fn program_write(&mut self, sender: PaneId, bytes: &[u8], what: Write) -> usize {
        self.write_to_domain(sender, bytes, what, true)
    }

    fn write_to_domain(&mut self, sender: PaneId, bytes: &[u8], what: Write, program: bool) -> usize {
        let gateways = self.tmux_gateways();
        let mut targets = broadcast::write_targets(&self.broadcast, &self.broadcast_view(), sender, what, &gateways);
        let mut skipped = 0;
        if program {
            let panes = &self.panes;
            (targets, skipped) = broadcast::program_targets(targets, |t| {
                panes.get(&t).map(|p| broadcast::TargetState { exited: p.exited.is_some(), ended: p.ended, password: p.at_password_prompt() })
            });
        }
        for t in targets {
            // The person's own typing makes the line theirs: a model's held text no longer
            // describes it (Task 10 fix round 2).
            if crate::orch_gate::person_writes(what) {
                self.orch_held.remove(&t);
            }
            // A tmux gateway reads typing as its Command Menu's keys; reports and other
            // bytes would land in tmux's command stream, so they are dropped there.
            if self.is_tmux_gateway(t) {
                if broadcast::typed(what) {
                    self.tmux_gateway_input(t, bytes, what == Write::Key);
                }
                continue;
            }
            if let Some(p) = self.panes.get_mut(&t) {
                p.input(bytes);
            }
        }
        // tmux panes' typing goes out as send-keys now.
        self.tmux_send_input();
        skipped
    }

    /// Shell > Broadcast Input's items. False for other selectors.
    /// Turning broadcasting on asks the warning (`BroadcastInput`) first, whose OK
    /// (`broadcast_granted`) turns it on.
    pub(super) fn broadcast_command(&mut self, selector: &str) -> bool {
        let mut v = self.broadcast_view();
        let current = v.current;
        // The warning (going from off to on), whose text the model gives.
        let mut asked = None;
        let mut confirm = |title: &str| {
            asked = Some(title.to_string());
            false
        };
        match selector {
            "disableBroadcasting:" => {
                self.broadcast.disable(&mut v);
            }
            "enableSendInputToAllTabs:" => {
                self.broadcast.all_tabs(&mut v, &mut confirm);
            }
            "enableSendInputToAllPanes:" => {
                self.broadcast.current_tab(&mut v, &mut confirm);
            }
            "toggleBroadcastingToCurrentSession:" => {
                if v.exited.contains(&current) {
                    return true;
                }
                self.broadcast.toggle_session(&mut v, current);
            }
            "toggleBroadcastSource:" => {
                if v.exited.contains(&current) || !self.broadcast.receives(&v, current) {
                    return true;
                }
                self.broadcast.toggle_source(&v, current);
            }
            "toggleUseBackgroundPatternIndicator:" => {
                let on = !self.globals.bool("Use background pattern indicator");
                self.globals.set("Use background pattern indicator", toml::Value::Boolean(on));
                self.save_globals();
            }
            _ => return false,
        }
        self.broadcast_changed(&v);
        // After the state is kept: a silenced warning answers at once.
        if let Some(text) = asked {
            let selector = if selector == "enableSendInputToAllTabs:" { "enableSendInputToAllTabs:" } else { "enableSendInputToAllPanes:" };
            self.ask_scripts(broadcast_input_ask(&text, current, selector));
        }
        true
    }

    /// The broadcast warning answered OK: the item `selector` turns broadcasting on in the
    /// window and tab of `anchor`, the pane it was asked from (gone: nothing happens), while
    /// broadcasting there is still off.
    pub(super) fn broadcast_granted(&mut self, anchor: PaneId, selector: &str) {
        if !self.enter_pane_window(anchor) {
            return;
        }
        let Some(mut v) = self.broadcast_view().at(anchor) else { return };
        if self.broadcast.grant(&mut v, selector == "enableSendInputToAllTabs:") {
            self.broadcast_changed(&v);
        }
    }

    /// The model's state kept, and the menus and window follow it.
    fn broadcast_changed(&mut self, v: &View) {
        self.keep_broadcast_flags(v);
        self.menus_dirty = true;
        self.sync_checks();
        self.redraw();
    }

    /// The context menu's Toggle Broadcasting Input, on the pane that was clicked.
    pub(super) fn toggle_broadcast_pane(&mut self, pane: PaneId) {
        let mut v = self.broadcast_view();
        self.broadcast.toggle_session(&mut v, pane);
        self.keep_broadcast_flags(&v);
        self.sync_checks();
        self.redraw();
    }

    /// The menu checks for Shell > Broadcast Input.
    pub(super) fn broadcast_checked(&self, selector: &str) -> Option<bool> {
        let v = self.broadcast_view();
        Some(match selector {
            "disableBroadcasting:" => self.broadcast.mode(&v) == broadcast::Mode::Off,
            "enableSendInputToAllTabs:" => self.broadcast.mode(&v) == broadcast::Mode::AllTabs,
            "enableSendInputToAllPanes:" => self.broadcast.mode(&v) == broadcast::Mode::AllPanes,
            "toggleUseBackgroundPatternIndicator:" => self.globals.bool("Use background pattern indicator"),
            "toggleBroadcastSource:" => self.broadcast.sources.contains(&v.current) && self.broadcast.receives(&v, v.current),
            _ => return None,
        })
    }

    /// Clear Buffer (`clear`) or Reset (`!clear`): with two or more broadcast sessions, a
    /// question (or its remembered answer) decides whether all of them or only this one
    /// (`PT:13675-13716`).
    pub(super) fn clear_or_reset(&mut self, clear: bool) {
        let Some(me) = self.focused_id() else { return };
        // The window's whole broadcast domain (iTerm2's broadcastSessions), sources or not.
        let targets = self.broadcast.domain_panes(&self.broadcast_view());
        if targets.len() < 2 || !targets.contains(&me) {
            return self.clear_or_reset_answered(clear, me, vec![me]);
        }
        let spec = broadcast_spec(clear, targets.len());
        self.ask_scripts(super::scripts_alerts::Ask::new(spec, super::scripts_alerts::Then::Broadcast { clear, me, targets }));
    }

    /// Clears (`clear`) or resets the panes `targets`, in the window of `me`, the pane it
    /// was asked from.
    pub(super) fn clear_or_reset_answered(&mut self, clear: bool, me: PaneId, targets: Vec<PaneId>) {
        if targets.is_empty() || !self.enter_pane_window(me) {
            return;
        }
        let jiggle = self.globals.bool("JiggleTTYSizeOnClearBuffer");
        for t in targets {
            // A replaying pane's buffer is not touched (its replay view has no shell).
            if self.panes.get(&t).is_some_and(|p| p.replay.is_some()) {
                continue;
            }
            if !clear {
                // A Filter or Zoom view would take the reset instead of the shell's screen.
                self.end_view(t);
            }
            if let Some(p) = self.panes.get_mut(&t) {
                if clear {
                    crate::pane::clear_then_jiggle(p, jiggle);
                } else {
                    p.screen.soft_reset();
                }
            }
        }
        self.redraw();
    }
}

/// The Clear (`clear`) or Reset question for `n` broadcast sessions: all, the current one only,
/// or Cancel; permanently silenceable under `NoSyncClearAllBroadcast` or
/// `NoSyncResetAllBroadcast` (the silence flag; the answer is in `_selection`).
pub(super) fn broadcast_spec(clear: bool, n: usize) -> crate::warnings::Spec {
    let (heading, text, buttons) = if clear { broadcast::clear_question(n) } else { broadcast::reset_question(n) };
    let id = if clear { "NoSyncClearAllBroadcast" } else { "NoSyncResetAllBroadcast" };
    super::scripts_alerts::ask(heading, &text, &buttons, Some(id), crate::warnings::Kind::Permanent, super::scripts_alerts::Then::Nothing).spec
}

/// The panes a Clear or Reset answer acts on: every broadcast session (0), the one it was
/// asked from (1), or none (Cancel).
pub(super) fn broadcast_targets(index: usize, me: PaneId, targets: Vec<PaneId>) -> Vec<PaneId> {
    match index {
        0 => targets,
        1 => vec![me],
        _ => Vec::new(),
    }
}

/// The warning before broadcasting input, `text` from the model (`warning_title`), asked from
/// pane `anchor` for the menu item `selector`: no heading, OK and Cancel, permanently
/// silenceable (`iTermBroadcastInputHelper.m:218-222`).
pub(super) fn broadcast_input_ask(text: &str, anchor: PaneId, selector: &'static str) -> super::scripts_alerts::Ask {
    use crate::api_auth::{CANCEL, OK};
    super::scripts_alerts::ask("", text, &[OK, CANCEL], Some("NoSyncSuppressBroadcastInputWarning"), crate::warnings::Kind::Permanent, super::scripts_alerts::Then::BroadcastInput(anchor, selector))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warnings::{Kind, Preempt, checkbox_label, preempt};

    /// Clear and Reset with broadcast (`PT:13675-13716`): all, current only or Cancel,
    /// permanently silenceable; `NoSyncClearAllBroadcast` is the silence flag (Corrections, R3).
    #[test]
    fn broadcast_questions_follow_iterm2() {
        let c = broadcast_spec(true, 3);
        let (heading, text, _) = broadcast::clear_question(3);
        assert_eq!((c.heading.as_str(), c.text.as_str()), (heading, text.as_str()));
        assert_eq!(c.id.as_deref(), Some("NoSyncClearAllBroadcast"));
        assert_eq!(c.labels, ["Clear All", "Clear Current Session Only", "Cancel"]);
        assert_eq!((c.kind, c.cancel), (Kind::Permanent, Some(2)));
        assert_eq!(checkbox_label(&c), Some("Remember my choice"));
        let r = broadcast_spec(false, 3);
        let (heading, text, buttons) = broadcast::reset_question(3);
        assert_eq!((r.heading.as_str(), r.text.as_str(), r.labels.clone()), (heading, text.as_str(), buttons.map(String::from).to_vec()));
        assert_eq!(r.id.as_deref(), Some("NoSyncResetAllBroadcast"));
        assert_eq!(broadcast_targets(0, 2, vec![1, 2, 3]), [1, 2, 3]);
        assert_eq!(broadcast_targets(1, 2, vec![1, 2, 3]), [2]);
        assert!(broadcast_targets(2, 2, vec![1, 2, 3]).is_empty());
        let mut g = crate::globals::Globals::empty(std::path::Path::new("broadcast-questions-test.toml"));
        assert_eq!(preempt(&c, &g, 0.0, false), Preempt::Show);
        g.set("NoSyncClearAllBroadcast", toml::Value::Boolean(false));
        assert_eq!(preempt(&c, &g, 0.0, false), Preempt::Show, "false is not silenced, not current only");
        g.set("NoSyncClearAllBroadcast", toml::Value::Boolean(true));
        assert_eq!(preempt(&c, &g, 0.0, false), Preempt::Answer(0), "silenced with no selection: Clear All");
        g.set("NoSyncClearAllBroadcast_selection", toml::Value::Integer(1));
        assert_eq!(preempt(&c, &g, 0.0, false), Preempt::Answer(1));
    }
}
