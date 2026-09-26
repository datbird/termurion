//! Instant Replay in the window: entering and stepping a pane's replay, the keys it takes,
//! what ends it, Clear Instant Replay, and the Instant Replay Control panel.
//! `PT` = `TerminalView/PseudoTerminal.m`, `PS` = `PTYSession/PTYSession.m`.

use super::*;
use crate::replay::{self, Close, IrKey, ReplayEvent};
use crate::replay_panel::{self, PanelAct};
use crate::replay_window::{PanelView, ReplayWindow};

impl App {
    /// The window shows the Instant Replay Control panel (`inInstantReplay`, `PT:10486-10489`).
    pub(super) fn in_instant_replay(&self) -> bool {
        self.replay_window.is_some()
    }

    /// `irAdvance:` (`PT:10473-10484`, `PS:1502-1532`): forward from the live view beeps;
    /// back from it enters replay (nothing when nothing was recorded) and steps; a replay
    /// steps, beeping at either end.
    pub(super) fn ir_advance(&mut self, dir: i32, event_loop: &ActiveEventLoop) {
        let Some(id) = self.focused_id().filter(|id| self.panes.contains_key(id)) else { return };
        if self.panes.get(&id).is_some_and(|p| p.replay.is_none()) {
            if dir > 0 {
                return find_ui::beep();
            }
            if !self.replay_session(id, event_loop) {
                return;
            }
        }
        let stepped = self.panes.get_mut(&id).is_some_and(|p| match p.replay.as_mut() {
            Some(r) => replay::step(r, &p.dvr, dir),
            None => false,
        });
        if !stepped {
            find_ui::beep();
        }
        self.show_frame(id);
        self.update_replay_panel();
    }

    /// `replaySession:` (`PT:10589-10608`): the pane shows its newest frame and the panel
    /// opens. False when nothing was recorded.
    pub(super) fn replay_session(&mut self, pane: PaneId, event_loop: &ActiveEventLoop) -> bool {
        let Some(r) = self.panes.get(&pane).and_then(|p| replay::enter(&p.dvr)) else { return false };
        // A Filter or Zoom view gives way (both are synthetic sessions in iTerm2).
        if self.panes.get(&pane).is_some_and(|p| p.live.is_some()) {
            self.end_view(pane);
        }
        if let Some(p) = self.panes.get_mut(&pane) {
            p.replay = Some(r);
        }
        self.show_frame(pane);
        if !self.in_instant_replay() {
            self.open_replay_panel(event_loop);
        }
        true
    }

    /// `setDvrFrame` (`PS:15314-15375`): the decoder's frame at the pane's size (no window
    /// resize), shown in place of its screen; Copy Mode ends.
    pub(super) fn show_frame(&mut self, pane: PaneId) {
        if !self.refit_frame(pane) {
            return;
        }
        self.leave_copy_mode(pane);
        self.redraw();
    }

    /// Shows the replay's frame as a view of the pane (`Pane::show_view`). The view is
    /// never fed bytes, so triggers, marks and shell integration never run on it. False
    /// when the pane has no frame to show (a stranded replay keeps what it shows).
    pub(super) fn refit_frame(&mut self, pane: PaneId) -> bool {
        let Some(p) = self.panes.get_mut(&pane) else { return false };
        let Some(dec) = p.replay.as_ref().and_then(|r| r.decoder.as_ref()).filter(|d| d.info.is_some()) else { return false };
        let own = p.own();
        let (cols, rows) = (own.term().columns(), own.term().screen_lines());
        let mut view = replay::frame_screen(dec, cols, rows, own.palette.clone(), Look::default());
        let (cw, ch) = own.cell_px();
        view.set_cell_px(cw, ch);
        p.show_view(view);
        true
    }

    /// Back to the live screen (`showLiveSession:inPlaceOf:`, `PT:10736-10765`).
    pub(super) fn end_replay(&mut self, pane: PaneId) {
        if let Some(p) = self.panes.get_mut(&pane) {
            if p.replay.take().is_some() {
                p.end_view();
            }
        }
        self.redraw();
    }

    /// A key while the panel shows (`maybeHandleInstantReplayKeyEvent:`,
    /// `PS:12795-12891`): runs before every other key handler, and every key but a Win
    /// chord is taken, its release too, so none reaches the session or a tool.
    pub(super) fn replay_key(&mut self, ev: &KeyEvent, event_loop: &ActiveEventLoop) -> bool {
        if ev.state != ElementState::Pressed {
            return self.replay_keys.release(&ev.physical_key);
        }
        if !self.in_instant_replay() {
            self.replay_keys.press(ev.physical_key, false);
            return false;
        }
        // A modifier alone is no keystroke in iTerm2 (`flagsChanged:`): taken quietly.
        if toolbelt_tools_ui::is_modifier(&ev.logical_key) {
            self.replay_keys.press(ev.physical_key, true);
            return true;
        }
        let act = match keystroke::key_of(&ev.logical_key, ev.text.as_deref()) {
            Some(key) => replay::ir_key(&key, self.mods.shift, self.mods.cmd),
            None if self.mods.cmd => IrKey::Pass,
            None => IrKey::Beep,
        };
        let taken = act != IrKey::Pass;
        self.replay_keys.press(ev.physical_key, taken);
        match act {
            IrKey::Pass => {}
            IrKey::Beep => find_ui::beep(),
            // Esc is the Cancel button's key equivalent while it shows (`IRX:92-99`);
            // key equivalents run before the session's keyDown.
            IrKey::Exit if self.replay_window.as_ref().is_some_and(|w| w.panel.export != replay_panel::Export::Normal) => {
                if let Some(w) = self.replay_window.as_mut() {
                    w.panel.cancel();
                }
                self.update_replay_panel();
            }
            IrKey::Exit => {
                self.close_instant_replay(true, event_loop);
            }
            IrKey::Step { dir, count } => {
                for _ in 0..count {
                    self.ir_advance(dir, event_loop);
                }
            }
        }
        taken
    }

    /// `closeInstantReplay:orTerminateSession:` (`PT:10528-10539`) for the focused pane.
    pub(super) fn close_instant_replay(&mut self, or_terminate: bool, event_loop: &ActiveEventLoop) -> bool {
        let pane = self.focused_id();
        self.close_ir_of(pane, or_terminate, event_loop)
    }

    /// A live session's recording closes the panel; a loaded (read-only) recording closes
    /// its pane when `or_terminate`, else nothing happens.
    fn close_ir_of(&mut self, pane: Option<PaneId>, or_terminate: bool, event_loop: &ActiveEventLoop) -> bool {
        if !self.replays_read_only(pane) {
            self.close_replay_panel(pane, event_loop);
            true
        } else if let (true, Some(id)) = (or_terminate, pane) {
            self.close_pane(id, event_loop);
            true
        } else {
            false
        }
    }

    /// The pane shows a frame of a loaded recording (`liveSession.screen.dvr.readOnly`).
    fn replays_read_only(&self, pane: Option<PaneId>) -> bool {
        pane.and_then(|id| self.panes.get(&id)).is_some_and(|p| p.replay.is_some() && p.dvr.read_only)
    }

    /// The panel closes (`closeInstantReplayWindow`), and its `windowWillClose:`
    /// (`IRW:143-145`) puts `pane`'s live screen back; a loaded recording's pane closes
    /// (`PT:10450-10458`).
    pub(super) fn close_replay_panel(&mut self, pane: Option<PaneId>, event_loop: &ActiveEventLoop) {
        self.replay_window = None;
        let Some(id) = pane else { return };
        if self.replays_read_only(Some(id)) {
            self.close_pane(id, event_loop);
        } else {
            self.end_replay(id);
        }
    }

    /// What an event in the window does to Instant Replay (`replay::on_event`). `pane` is
    /// the pane the closers act on: the one that was active before a change of pane.
    pub(super) fn replay_event(&mut self, e: ReplayEvent, pane: Option<PaneId>, event_loop: &ActiveEventLoop) -> Close {
        let close = replay::on_event(self.in_instant_replay(), e);
        match close {
            Close::Nothing | Close::Refuse => {}
            Close::OpenPanel => {
                if !self.in_instant_replay() {
                    self.open_replay_panel(event_loop);
                }
                self.update_replay_panel();
            }
            // A closing window takes its panel along (`PT:4910-4914`); its sessions go next.
            Close::ClosePanel if e == ReplayEvent::WindowClosed => self.replay_window = None,
            // A tab change closes the panel itself (`setInstantReplayBarVisible:`).
            Close::ClosePanel if matches!(e, ReplayEvent::TabShown { .. }) => {
                self.close_replay_panel(pane, event_loop);
            }
            Close::ClosePanel => {
                self.close_ir_of(pane, false, event_loop);
            }
        }
        close
    }

    /// A change of tab or of active pane in the current window since last looked
    /// (`TAB:907-909`, `PT:7520-7528`).
    pub(super) fn replay_follow_focus(&mut self, event_loop: &ActiveEventLoop) {
        let now = self.tabs.get(self.active).map(|t| (t.id, t.focused));
        let Some(before) = std::mem::replace(&mut self.replay_seen, now) else { return };
        let Some(now) = now else { return };
        if before.0 != now.0 {
            let replaying = self.panes.get(&now.1).is_some_and(|p| p.replay.is_some());
            self.replay_event(ReplayEvent::TabShown { replaying }, Some(now.1), event_loop);
        } else if before.1 != now.1 && self.panes.get(&before.1).is_some_and(|p| p.replay.is_some()) {
            // Only a replaying pane that stops being active closes it (`[activeSession_ dvr]`).
            self.replay_event(ReplayEvent::OtherPaneActive, Some(before.1), event_loop);
        }
    }

    /// `showHideInstantReplay` (`PT:10511-10526`): the panel, placed by
    /// `originForAccessoryOfSize:`, with the recording's tick marks as they are now.
    fn open_replay_panel(&mut self, event_loop: &ActiveEventLoop) {
        let ticks = self.focused_id().and_then(|id| self.panes.get(&id)).map(|p| p.dvr.timestamps()).unwrap_or_default();
        let Some(g) = self.gpu.as_ref() else { return };
        match ReplayWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, ticks) {
            Ok(mut w) => {
                if let Some(at) = self.replay_panel_origin(w.outer_size()) {
                    w.place(at);
                }
                self.replay_window = Some(w);
                self.replay_seen = self.tabs.get(self.active).map(|t| (t.id, t.focused));
            }
            Err(e) => self.banners.set("window", format!("Could not open the window: {e}")),
        }
    }

    /// Where the panel goes (`replay_panel::origin`), in screen pixels: the window's frame,
    /// its monitor's work area, and the bottom of the tab strip (the top of the room the
    /// sessions share).
    fn replay_panel_origin(&self, size: (f32, f32)) -> Option<(f32, f32)> {
        let g = self.gpu.as_ref()?;
        let pos = g.window.outer_position().ok()?;
        let outer = g.window.outer_size();
        let inner = g.window.inner_position().ok()?;
        let (work, _) = self.monitor_rects()?;
        let window = Rect { x: pos.x as f32, y: pos.y as f32, w: outer.width as f32, h: outer.height as f32 };
        let tabs_bottom = inner.y as f32 + self.window_room().y;
        Some(replay_panel::origin(size, window, work, tabs_bottom))
    }

    /// The panel shows the focused pane's recording as it is now (`updateInstantReplayView`).
    pub(super) fn update_replay_panel(&mut self) {
        if let Some(w) = &self.replay_window {
            w.request_redraw();
        }
    }

    /// `instantReplaySeekTo:` (`PT:10461-10466`): the slider enters replay first when live,
    /// then seeks to the first frame at or after its time.
    pub(super) fn replay_seek(&mut self, f: f32, event_loop: &ActiveEventLoop) {
        let Some(id) = self.focused_id().filter(|id| self.panes.contains_key(id)) else { return };
        if self.panes.get(&id).is_some_and(|p| p.replay.is_none()) && !self.replay_session(id, event_loop) {
            return;
        }
        if let Some(p) = self.panes.get_mut(&id) {
            if let Some(r) = p.replay.as_mut() {
                replay::seek_fraction(r, &p.dvr, f);
            }
        }
        self.show_frame(id);
        self.update_replay_panel();
    }

    /// Edit > Clear Instant Replay (`PS:1490-1500`): during a replay its decoder goes and
    /// nothing is cleared (kept: the session's own recorder is not the one it asks); else
    /// the recorder starts over unless it is a loaded recording.
    pub(super) fn clear_instant_replay(&mut self) {
        let Some(p) = self.focused_id().and_then(|id| self.panes.get_mut(&id)) else { return };
        let replaying = p.replay.is_some();
        if let Some(r) = p.replay.as_mut() {
            r.decoder = None;
        }
        if p.dvr.can_clear(replaying) {
            p.dvr.clear();
        }
        self.update_replay_panel();
    }

    /// The panel's view of the focused pane's recording (`IRW:301-362`).
    fn replay_panel_view(&self) -> Option<PanelView> {
        let w = self.replay_window.as_ref()?;
        let p = self.focused_id().and_then(|id| self.panes.get(&id))?;
        let (first, last) = (p.dvr.first_timestamp(), p.dvr.last_timestamp());
        let cur = replay::current(p.replay.as_ref());
        let current = if cur < 0 { replay_panel::LIVE.to_string() } else { time_label(cur) };
        // The band measures against the range the ticks were made from (`_firstTimestamp`
        // and `_span`, set when the panel opened).
        let ticks = &w.panel.ticks_at_open;
        let (t0, t1) = (ticks.first().copied().unwrap_or(0), ticks.last().copied().unwrap_or(0));
        Some(PanelView {
            earliest: time_label(first),
            current,
            slider: replay_panel::slider(cur, first, last).unwrap_or(w.slider),
            band: w.panel.band(cur, t0, t1),
            buttons: w.panel.buttons(),
            current_ts: cur,
        })
    }

    /// The panel's own events. Its window belongs to one terminal window, which becomes
    /// current first.
    pub(super) fn replay_window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: &WindowEvent) -> bool {
        if !self.replay_window.as_ref().is_some_and(|w| w.id() == id) {
            match self.window_of_replay_panel(id) {
                Some(owner) if self.enter(owner) => {}
                _ => return false,
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                let pane = self.focused_id();
                self.close_replay_panel(pane, event_loop);
            }
            WindowEvent::RedrawRequested => {
                let Some(view) = self.replay_panel_view() else { return true };
                let acts = self.replay_window.as_mut().map(|w| w.redraw(&view)).unwrap_or_default();
                for act in acts {
                    match act {
                        PanelAct::Seek(f) => self.replay_seek(f, event_loop),
                        PanelAct::Beep => find_ui::beep(),
                        // Session > Log > Export Recording writes the range (the next task).
                        PanelAct::Export { .. } => {}
                    }
                    self.update_replay_panel();
                }
            }
            other => {
                if let Some(w) = self.replay_window.as_mut() {
                    w.event(other);
                }
            }
        }
        true
    }
}

/// `stringForTimestamp:` (`IRW:279-299`) with the Windows locale's short date and its
/// time with seconds (the medium style).
fn time_label(ts_us: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64);
    replay_panel::time_label(ts_us, day_of(now), &|t| format_local(t, true), &|t| format_local(t, false), &day_of)
}

/// Microseconds since 1970 as local time.
fn local_time(ts_us: i64) -> windows_sys::Win32::Foundation::SYSTEMTIME {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns steps from 1601.
    let ticks = (ts_us.max(0) as u64 / 1_000_000 + 11_644_473_600) * 10_000_000;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let mut utc = unsafe { std::mem::zeroed() };
    let mut here = unsafe { std::mem::zeroed() };
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc);
        SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut here);
    }
    here
}

/// The local day, as (year, month * 32 + day).
fn day_of(ts_us: i64) -> (i32, u32) {
    let t = local_time(ts_us);
    (t.wYear as i32, t.wMonth as u32 * 32 + t.wDay as u32)
}

fn format_local(ts_us: i64, date: bool) -> String {
    use windows_sys::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx};
    let t = local_time(ts_us);
    let mut buf = [0u16; 128];
    let n = unsafe {
        if date {
            GetDateFormatEx(std::ptr::null(), DATE_SHORTDATE, &t, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32, std::ptr::null())
        } else {
            GetTimeFormatEx(std::ptr::null(), 0, &t, std::ptr::null(), buf.as_mut_ptr(), buf.len() as i32)
        }
    };
    String::from_utf16_lossy(&buf[..(n.max(1) as usize - 1)])
}
