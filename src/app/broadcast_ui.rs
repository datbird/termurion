//! Broadcast input in the window: the window's broadcast state as the model sees it, and
//! the one way user input reaches sessions (the sender's domain, or the sender alone).

use super::*;
use crate::broadcast::{self, Route, TabView, View, Write};

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
        let exited = self.panes.iter().filter(|(_, p)| p.exited.is_some()).map(|(id, _)| *id).collect();
        View { tabs, current_tab: self.active, current, exited }
    }

    /// Writes the model's tab flags back.
    pub(super) fn keep_broadcast_flags(&mut self, v: &View) {
        self.broadcast_tabs = self.tabs.iter().zip(&v.tabs).filter(|(_, tv)| tv.broadcasting).map(|(t, _)| t.id).collect();
    }

    /// User input from `sender`: to every session of its broadcast domain (the same bytes,
    /// as the sender encoded them), or to the sender alone.
    pub(super) fn user_write(&mut self, sender: PaneId, bytes: &[u8], what: Write) {
        let targets = match broadcast::route(what) {
            Route::Broadcast => self.broadcast.targets(&self.broadcast_view(), sender),
            Route::Local => vec![sender],
        };
        let gateways = self.tmux_gateways();
        let targets = broadcast::without_gateways(targets, sender, &gateways);
        for t in targets {
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
    }

    /// A paste from `sender`: bracketed by the sender's mode, then written like typing.
    pub(super) fn user_paste(&mut self, sender: PaneId, text: &str, what: Write) {
        let Some(bytes) = self.panes.get(&sender).map(|p| p.paste_payload(text)) else { return };
        self.user_write(sender, &bytes, what);
    }

    /// Shell > Broadcast Input's items. False for other selectors.
    pub(super) fn broadcast_command(&mut self, selector: &str) -> bool {
        let mut v = self.broadcast_view();
        let current = v.current;
        let suppressed = self.globals.bool("NoSyncSuppressBroadcastInputWarning");
        let owner = self.hwnd();
        // The warning (going from off to on). Windows' message box has no checkbox: the
        // Advanced setting "Suppress warning about broadcasting input" silences it.
        let mut confirm = |title: &str| suppressed || super::close_ui::confirm(owner, "Broadcast Input", title);
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
        self.keep_broadcast_flags(&v);
        self.menus_dirty = true;
        self.sync_checks();
        self.redraw();
        true
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
    /// question (or the remembered answer) decides whether all of them or only this one.
    pub(super) fn clear_or_reset(&mut self, clear: bool) {
        let Some(me) = self.focused_id() else { return };
        // The window's whole broadcast domain (iTerm2's broadcastSessions), sources or not.
        let targets = self.broadcast.domain_panes(&self.broadcast_view());
        let all = if targets.len() < 2 || !targets.contains(&me) {
            false
        } else {
            let key = if clear { "NoSyncClearAllBroadcast" } else { "NoSyncResetAllBroadcast" };
            match self.globals.get(key).and_then(|v| v.as_bool()) {
                Some(remembered) => remembered,
                None => {
                    let (heading, text, buttons) = if clear { broadcast::clear_question(targets.len()) } else { broadcast::reset_question(targets.len()) };
                    match yes_no_cancel(self.hwnd(), heading, &format!("{text}\n\nYes: {}. No: {}.", buttons[0], buttons[1])) {
                        Some(a) => a,
                        None => return,
                    }
                }
            }
        };
        for t in if all { targets } else { vec![me] } {
            if !clear {
                // A Filter or Zoom view would take the reset instead of the shell's screen.
                self.end_view(t);
            }
            if let Some(p) = self.panes.get_mut(&t) {
                if clear {
                    p.clear();
                } else {
                    p.screen.soft_reset();
                }
            }
        }
        self.redraw();
    }
}

/// Yes, No, or Cancel (None).
fn yes_no_cancel(owner: Option<isize>, title: &str, text: &str) -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IDNO, IDYES, MB_ICONQUESTION, MB_YESNOCANCEL, MessageBoxW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let owner = owner.map_or(std::ptr::null_mut(), |h| h as *mut core::ffi::c_void);
    match unsafe { MessageBoxW(owner, wide(text).as_ptr(), wide(title).as_ptr(), MB_YESNOCANCEL | MB_ICONQUESTION) } {
        IDYES => Some(true),
        IDNO => Some(false),
        _ => None,
    }
}
