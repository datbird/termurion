//! Instant Replay in the window: entering and stepping a pane's replay, the keys it takes,
//! what ends it, Clear Instant Replay, and the Instant Replay Control panel.
//! `PT` = `TerminalView/PseudoTerminal.m`, `PS` = `PTYSession/PTYSession.m`.

use super::*;
use crate::recording;
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
        self.ir_advance_by(dir, 1, event_loop);
    }

    /// `irAdvance:` `count` times (Shift+Left/Right steps 15), beeping per step, with the
    /// frame built once at the end.
    fn ir_advance_by(&mut self, dir: i32, count: u32, event_loop: &ActiveEventLoop) {
        let Some(id) = self.focused_id().filter(|id| self.panes.contains_key(id)) else { return };
        if self.panes.get(&id).is_some_and(|p| p.replay.is_none()) {
            if dir > 0 {
                return find_ui::beep();
            }
            if !self.replay_session(id, event_loop) {
                return;
            }
        }
        for _ in 0..count {
            let stepped = self.panes.get_mut(&id).is_some_and(|p| match p.replay.as_mut() {
                Some(r) => replay::step(r, &p.dvr, dir),
                None => false,
            });
            if !stepped {
                find_ui::beep();
            }
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

    /// Panes leaving their window (Move Tab or Move Session to New Window, Merge All
    /// Windows) or restarting end their replay first, and the panel closes when it was the
    /// focused pane's: the replay does not follow the pane.
    pub(super) fn end_replays(&mut self, panes: &[PaneId]) {
        let focused = self.focused_id();
        for &id in panes {
            if !self.panes.get(&id).is_some_and(|p| p.replay.is_some()) {
                continue;
            }
            if focused == Some(id) {
                self.replay_window = None;
            }
            self.end_replay(id);
        }
    }

    /// Bury Session is refused for `pane`: its tab holds a replaying pane, or it is a
    /// loaded recording's (a synthetic session).
    pub(super) fn bury_refused(&self, pane: PaneId) -> bool {
        let tab = self.tabs.iter().find(|t| t.layout.panes().contains(&pane));
        let replaying = tab.is_some_and(|t| t.layout.panes().iter().any(|id| self.panes.get(id).is_some_and(|p| p.replay.is_some())));
        replaying || self.panes.get(&pane).is_some_and(|p| p.dvr.read_only)
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
            // Esc leaves replay in every export state: the panel is never key (`IRW:80-86`),
            // so its Cancel button's Esc equivalent never fires (`PS:12863-12866`).
            IrKey::Exit => {
                self.close_instant_replay(true, event_loop);
            }
            IrKey::Step { dir, count } => self.ir_advance_by(dir, count, event_loop),
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
        match ReplayWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, ticks, crate::tab_theme::forced_dark(self.tab_style())) {
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
        let tabs_bottom = inner.y as f32 + self.window_area().y;
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
                        // The range from Set Start to Set End, from the live session's
                        // recorder (`PT:10442-10444`).
                        PanelAct::Export { from, to } => {
                            if let Some(id) = self.focused_id() {
                                self.export_recording(id, from, to);
                            }
                        }
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

impl App {
    /// Session > Log > Export Recording (`PT:13809-13811`): the focused session's whole
    /// recording. Always enabled, as iTerm2's `DVR.empty` is never set (`D:89`), so an empty
    /// recorder writes an empty recording. During a replay iTerm2's current session is the
    /// replay view, which has no recorder (`PS:1471`): the dialog shows, then "Error
    /// encoding recording." (kept).
    pub(super) fn export_recording_menu(&mut self) {
        let Some(id) = self.focused_id().filter(|id| self.panes.contains_key(id)) else { return };
        if self.panes.get(&id).is_some_and(|p| p.replay.is_some()) {
            if export_dialog().is_some() {
                self.recording_warning(recording::WARNING, recording::ENCODE_FAILED.to_string());
            }
            return;
        }
        let (first, last) = self.panes.get(&id).map_or((0, 0), |p| (p.dvr.first_timestamp(), p.dvr.last_timestamp()));
        self.export_recording(id, first, last);
    }

    /// `exportRecording:from:to:window:` (`RC:137-208`): the save dialog, then the file
    /// with the pane's recording from `from` to `to` and its profile without the private
    /// keys. A write error shows "The recording could not be saved." with the error text.
    pub(super) fn export_recording(&mut self, pane: PaneId, from: i64, to: i64) {
        let Some(path) = export_dialog() else { return };
        let Some(p) = self.panes.get(&pane) else { return };
        // A range copy stamps its frames now (`D:215-245`); stamps never repeat.
        let mut last = 0;
        let mut now = || {
            last = unix_us().max(last + 1);
            last
        };
        let dvr = p.dvr.to_json(from, to, &mut now);
        if dvr.is_null() {
            return self.recording_warning(recording::WARNING, recording::ENCODE_FAILED.to_string());
        }
        let profile = self.store.get(&p.profile).unwrap_or_else(|| self.store.default_profile());
        let profile = match serde_json::to_value(&profile.values) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        let guid = uuid::Uuid::new_v4().to_string().to_uppercase();
        let bytes = recording::encode(dvr, recording::redact(&profile, &guid));
        if let Err(e) = crate::localfs::judge_io(&path).and_then(|p| crate::localfs::write(&p, bytes)) {
            self.recording_warning(recording::SAVE_FAILED, e.to_string());
        }
    }

    /// Session > Log > Import Recording (`RC:23-135`): the open dialog for `.itr` files
    /// unless `path` names one. A file that does not load shows its warning. One that does
    /// opens in a new tab at the front of the current window, with a session-local copy
    /// of its profile, in a pane with no shell at its first frame's size, holding the
    /// read-only recorder; replay starts on the next pass (`RC:124-126`).
    pub(super) fn import_recording(&mut self, path: Option<std::path::PathBuf>) {
        let path = path.or_else(|| {
            let mut d = rfd::FileDialog::new().add_filter(recording::FILTER, &[recording::EXTENSION]);
            if let Some(home) = home_folder() {
                d = d.set_directory(home);
            }
            d.pick_file()
        });
        let Some(path) = path else { return };
        let loaded = crate::localfs::judge_io(&path).and_then(|p| crate::localfs::read(&p)).map_err(|e| recording::LoadError::Read(e.to_string())).and_then(|b| recording::load(&b));
        let (dvr, values) = match loaded {
            Ok(l) => l,
            Err(e) => {
                let (heading, text) = recording::warning(&e);
                return self.recording_warning(heading, text);
            }
        };
        // Keys the file lacks read their defaults. A fresh Guid keeps two imports of one
        // file apart.
        let mut profile = crate::profile::Profile::default();
        for (k, v) in values {
            if let Ok(v) = serde_json::from_value::<toml::Value>(v) {
                profile.set(&k, v);
            }
        }
        profile.set(crate::profile::GUID, toml::Value::String(uuid::Uuid::new_v4().to_string().to_uppercase()));
        self.store.sessions.push(crate::session_profile::SessionProfile { profile: profile.clone(), overridden: Default::default() });
        let (cols, rows) = match dvr.frame(dvr.ring.first) {
            Some(f) => (f.info.cols as u16, f.info.rows as u16),
            None => {
                let cell = self.profile_cell(&profile);
                let area = self.content_area();
                self.grid_size_in(area, cell, self.pane_room_for(self.next_id, &profile))
            }
        };
        let id = self.next_id;
        self.next_id += 1;
        self.install_pane(id, Pane::recording(id, cols, rows), &profile, false);
        if let Some(p) = self.panes.get_mut(&id) {
            p.dvr = dvr;
        }
        self.tabs.insert(0, Tab::new(self.next_tab_id, Layout::new(id), id));
        self.next_tab_id += 1;
        self.active = 0;
        // After any pinned tabs.
        self.settle_tabs();
        self.relayout();
        self.recording_replay = Some(id);
    }

    /// Queues a warning with OK only (`iTermWarning`, `kiTermWarningTypePersistent`).
    fn recording_warning(&mut self, heading: &'static str, text: String) {
        self.recording_warnings.push((None, heading, text));
        self.redraw();
    }

    /// Opens the warnings queued since the last pass and starts a loaded recording's replay
    /// (`replaySession:`; nothing when it holds no frame).
    pub(super) fn open_recording_windows(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(id) = self.recording_replay.take() {
            if self.enter_pane_window(id) && self.panes.contains_key(&id) {
                self.replay_session(id, event_loop);
            }
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let mut failed = None;
        for (slot, heading, text) in self.recording_warnings.iter_mut().filter(|(w, _, _)| w.is_none()) {
            match crate::prompt_ui::ConfirmWindow::warning(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, heading, text) {
                Ok(w) => *slot = Some(w),
                Err(e) => failed = Some(e),
            }
        }
        if let Some(e) = failed {
            self.recording_warnings.retain(|(w, _, _)| w.is_some());
            self.banners.set("window", format!("Could not open the window: {e}"));
        }
    }

    /// Events for a recording warning; OK or closing it dismisses it. False for other
    /// windows.
    pub(super) fn recording_warning_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        use crate::prompt_ui::ConfirmResult;
        let Some(i) = self.recording_warnings.iter().position(|(w, _, _)| w.as_ref().is_some_and(|w| w.id() == window)) else { return false };
        let Some(w) = self.recording_warnings[i].0.as_mut() else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => ConfirmResult::Cancel,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                ConfirmResult::Open
            }
        };
        if !matches!(answer, ConfirmResult::Open) {
            self.recording_warnings.remove(i);
        }
        true
    }
}

/// The save dialog (`RC:144-151`): "Recording", `.itr`, the home folder.
fn export_dialog() -> Option<std::path::PathBuf> {
    let mut d = rfd::FileDialog::new().set_file_name(recording::DEFAULT_NAME).add_filter(recording::FILTER, &[recording::EXTENSION]);
    if let Some(home) = home_folder() {
        d = d.set_directory(home);
    }
    d.save_file()
}

fn home_folder() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE").map(std::path::PathBuf::from)
}

fn unix_us() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_micros() as i64)
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
