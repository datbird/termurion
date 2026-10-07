//! Browser Instant Replay on the App (milestone 13x, Task 10; `BVC:204-340`): while a browser pane's
//! profile has Instant Replay on and the pane is shown, its view is captured as JPEG at up to
//! `WebInstantReplayFrameRate` into a per-pane ring bounded by the Instant Replay memory setting
//! (`IRMemory`) and, over all panes, by `web_replay::TOTAL_CAP`. Start Instant Replay opens the
//! playback window on a copy of the ring; Reveal in Explorer saves a Motion JPEG AVI in Tandem's own
//! folder. Nothing is written unless the person asks, a Dev Null pane is never recorded, and a page with
//! a visible password field is not captured. The capture itself is asynchronous in the browser
//! process; the app only starts it, so the UI thread never waits on a frame.

use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

use super::*;
use crate::web_replay::{self, Ring};
use crate::web_replay_ui::{PlaybackWindow, ReplayAct};

/// Where one pane's capture is.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    /// The password probe is out.
    Probing(Instant),
    /// The capture is out.
    Capturing(Instant),
}

/// One browser pane's recording.
pub(super) struct Recorder {
    ring: Ring,
    began: Instant,
    /// The pane's view is shown (set by the placement, so a hidden pane never wakes the loop).
    pub(super) shown: bool,
    stage: Stage,
    next: Instant,
}

impl Recorder {
    pub(super) fn new() -> Recorder {
        let now = Instant::now();
        Recorder { ring: Ring::new(0), began: now, shown: false, stage: Stage::Idle, next: now }
    }
}

/// The save in flight: the folder, the file and the error come back through the channel.
pub(super) type Saving = Receiver<Result<std::path::PathBuf, String>>;

impl App {
    /// The setting's rate: Hz, and the interval it makes (None for a rate that is no number: no capture).
    fn web_replay_interval(&self) -> Option<Duration> {
        web_replay::tick_interval(self.globals.number("WebInstantReplayFrameRate").unwrap_or(web_replay::DEFAULT_RATE))
    }

    /// Whether `pane` records now (see `web_replay::recording`).
    fn web_replay_wanted(&self, pane: PaneId) -> bool {
        let Some(p) = self.panes.get(&pane) else { return false };
        let on = self.store.get(&p.profile).is_some_and(|prof| prof.bool("Instant Replay"));
        let dev_null = p.browser_io().is_some_and(|io| io.dev_null);
        let shown = self.browser_views.get(&pane).is_some_and(|b| b.view.is_some() && b.replay.shown);
        web_replay::recording(on, dev_null, self.globals.int("IRMemory"), shown)
    }

    /// Every pass of the loop: starts the next capture of each recording pane when it is due, forgets
    /// a capture that never answered, applies the memory bounds, and collects a finished save. The
    /// time to look again, only while some pane is recording (no idle wake).
    pub(super) fn web_replay_tick(&mut self) -> Option<Instant> {
        self.web_replay_saved();
        let now = Instant::now();
        let interval = self.web_replay_interval();
        let cap = web_replay::cap_bytes(self.globals.int("IRMemory"));
        let mut due: Option<Instant> = None;
        let panes: Vec<PaneId> = self.browser_views.keys().copied().collect();
        for pane in panes {
            let wanted = self.web_replay_wanted(pane);
            let profile_on = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).is_some_and(|prof| prof.bool("Instant Replay"));
            let Some(bv) = self.browser_views.get_mut(&pane) else { continue };
            let r = &mut bv.replay;
            // A profile with it off (or a setting of 0) keeps nothing (iTerm2 drops the builder).
            if !profile_on || cap == 0 {
                r.ring.set_max(0);
                r.stage = Stage::Idle;
                continue;
            }
            r.ring.set_max(cap);
            match r.stage {
                Stage::Probing(t) | Stage::Capturing(t) if now >= t + web_replay::PENDING_LIMIT => r.stage = Stage::Idle,
                Stage::Probing(t) | Stage::Capturing(t) => {
                    // The answer wakes the loop; the limit is the only thing to wait for.
                    if wanted {
                        let at = t + web_replay::PENDING_LIMIT;
                        due = Some(due.map_or(at, |d| d.min(at)));
                    }
                    continue;
                }
                Stage::Idle => {}
            }
            if !wanted {
                continue;
            }
            if interval.is_none() {
                continue;
            }
            if now < r.next {
                due = Some(due.map_or(r.next, |d| d.min(r.next)));
                continue;
            }
            // A capture is slower than the rate on most machines, so the rate is an upper bound: the
            // next one waits for this one's answer and then for the interval.
            if let Some(v) = &bv.view {
                v.execute_script(web_replay::PROBE_JS, web_replay::PROBE_TOKEN);
                r.stage = Stage::Probing(now);
            }
        }
        self.web_replay_trim();
        due
    }

    /// The sum over every pane's ring stays under the total cap (the pane whose oldest frame is oldest
    /// shrinks first).
    fn web_replay_trim(&mut self) {
        let origin = self.browser_views.values().map(|b| b.replay.began).min();
        let Some(origin) = origin else { return };
        let mut rings: Vec<(&mut Ring, Duration)> = self.browser_views.values_mut().map(|b| (&mut b.replay.ring, b.replay.began.duration_since(origin))).collect();
        web_replay::trim_total(&mut rings, web_replay::TOTAL_CAP);
    }

    /// A script result for a recording: the probe's answer starts the capture; the capture's answer
    /// goes into the ring.
    pub(super) fn web_replay_script_done(&mut self, pane: PaneId, token: u64, json: &Result<String, String>) {
        let interval = self.web_replay_interval().unwrap_or(Duration::from_secs(1));
        let wanted = self.web_replay_wanted(pane);
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        let r = &mut bv.replay;
        let now = Instant::now();
        match (token, r.stage) {
            (web_replay::PROBE_TOKEN, Stage::Probing(_)) => {
                if wanted && web_replay::probe_allows(json) {
                    if let Some(v) = &bv.view {
                        v.capture(web_replay::CAPTURE_TOKEN, true);
                        r.stage = Stage::Capturing(now);
                        return;
                    }
                }
                r.stage = Stage::Idle;
                r.next = now + interval;
            }
            (web_replay::CAPTURE_TOKEN, Stage::Capturing(_)) => {
                r.stage = Stage::Idle;
                r.next = now + interval;
                use base64::Engine as _;
                if let Some(bytes) = json.as_ref().ok().and_then(|b| base64::engine::general_purpose::STANDARD.decode(b.trim()).ok()) {
                    let at = now.duration_since(r.began);
                    r.ring.push(at, bytes);
                }
            }
            _ => {}
        }
    }

    /// The pane's view is shown or hidden (called by the placement).
    pub(super) fn web_replay_shown(&mut self, pane: PaneId, shown: bool) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.replay.shown = shown;
        }
    }

    /// Start Instant Replay on a browser pane (`startInstantReplay`, `BVC:258-285`): the playback
    /// window on the frames kept so far. With none, a beep.
    pub(super) fn web_replay_start(&mut self, pane: PaneId, event_loop: &ActiveEventLoop) {
        let frames: Vec<(Duration, Vec<u8>)> = self.browser_views.get(&pane).map(|b| b.replay.ring.frames().to_vec()).unwrap_or_default();
        let size = frames.last().and_then(|f| web_replay::jpeg_size(&f.1)).unwrap_or((800, 600));
        let Some(g) = self.gpu.as_ref() else { return };
        self.web_replay = None;
        match PlaybackWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, frames, size) {
            Ok(w) => {
                w.focus();
                self.web_replay = Some(w);
            }
            Err(_) => find_ui::beep(),
        }
    }

    /// Edit > Clear Instant Replay on a browser pane (`clearInstantReplay`, `BVC:263-266`).
    pub(super) fn web_replay_clear(&mut self, pane: PaneId) {
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.replay.ring.clear();
        }
    }

    /// The playback window's events (the window chain's `web_replay_event`).
    pub(super) fn web_replay_event(&mut self, id: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.web_replay.as_mut().filter(|w| w.id() == id) else { return false };
        match w.event(event) {
            Some(ReplayAct::Closed) => self.web_replay = None,
            Some(ReplayAct::Reveal) => self.web_replay_reveal(),
            None => {}
        }
        true
    }

    /// Reveal in Explorer: the movie is built and written on a worker (never the UI thread) into
    /// Tandem's own folder under a random name, and selected in Explorer when it is done.
    fn web_replay_reveal(&mut self) {
        let Some(w) = self.web_replay.as_mut() else { return };
        if w.saving {
            return;
        }
        let (frames, size) = w.movie_input();
        w.saving = true;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(save_movie(&frames, size));
        });
        self.web_replay_save = Some(rx);
    }

    /// A finished save: the file selected in Explorer, or the failure's warning.
    fn web_replay_saved(&mut self) {
        let Some(rx) = &self.web_replay_save else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the save stopped".to_string()),
        };
        self.web_replay_save = None;
        if let Some(w) = self.web_replay.as_mut() {
            w.saving = false;
        }
        match result {
            Ok(path) => {
                let text = path.to_string_lossy().replace('/', "\\");
                let Some(arg) = crate::semantic::explorer_select_arg(&text) else { return };
                let mut cmd = std::process::Command::new("explorer.exe");
                {
                    use std::os::windows::process::CommandExt;
                    cmd.raw_arg(arg);
                }
                if let Err(e) = cmd.spawn() {
                    self.recording_warning(web_replay::MOVIE_HEADING, web_replay::movie_failed(&e.to_string()));
                }
            }
            Err(e) => self.recording_warning(web_replay::MOVIE_HEADING, web_replay::movie_failed(&e)),
        }
    }
}

/// Builds the AVI and writes it to a new owner-only file in Tandem's Replay folder (P16): a random name,
/// never an existing entry, never a link. The folder is swept at the next launch and at exit.
fn save_movie(frames: &[(Duration, Vec<u8>)], size: (u32, u32)) -> Result<std::path::PathBuf, String> {
    use std::io::Write;
    let avi = web_replay::mjpeg_avi(frames, size.0, size.1);
    if avi.is_empty() {
        return Err("there is nothing to save, or the movie is too large".to_string());
    }
    let base = crate::webhost_win::local_appdata().ok_or("the local data folder is not known")?;
    let folder = web_replay::replay_folder(&base);
    crate::webhost_win::prepare_folder(&folder)?;
    let dir = crate::localfs::LocalPath::judge(&folder).ok_or("the Replay folder is on a share or behind a link")?;
    let name = format!("Tandem Instant Replay {}.avi", uuid::Uuid::new_v4().simple());
    let file = dir.unfollowed(&name).ok_or("the file name was refused")?;
    let mut f = crate::localfs::create_owner_only(&file).map_err(|e| e.to_string())?;
    f.write_all(&avi).and_then(|_| f.flush()).map_err(|e| {
        let _ = crate::localfs::remove_unfollowed(&file);
        e.to_string()
    })?;
    Ok(file.path().to_path_buf())
}

#[cfg(test)]
mod tests {
    #[test]
    fn browser_replay_is_wired_as_bounded() {
        let src = include_str!("web_replay_app.rs");
        let prod = &src[..src.find("#[cfg(test)]").unwrap()];
        let tick = &prod[prod.find("pub(super) fn web_replay_tick").unwrap()..prod.find("fn web_replay_trim").unwrap()];
        // Capture only for a pane `recording` allows (Dev Null, hidden and 0 MB never), after the probe.
        assert!(prod.contains("web_replay::recording(on, dev_null"));
        assert!(tick.contains("self.web_replay_wanted(pane)") && tick.contains("PROBE_JS"));
        assert!(tick.contains("self.web_replay_trim()"), "the sum over panes is bounded every pass");
        // No file is written by the capture path: only the save (a worker) touches the disk.
        let before_save = &prod[..prod.find("fn save_movie").unwrap()];
        assert!(!before_save.contains("create_owner_only") && !before_save.contains("std::fs"));
        let save = &prod[prod.find("fn save_movie").unwrap()..];
        assert!(save.contains("create_owner_only") && save.contains("LocalPath::judge") && save.contains("replay_folder"));
        assert!(!save.contains("temp_dir") && !save.contains("%TEMP%"), "never a predictable name in a shared folder");
        assert!(prod.contains("std::thread::spawn"), "the movie is built off the UI thread");
        // The ring is cleared by Clear Instant Replay and wired to the menu.
        let replay_ui = include_str!("replay_ui.rs");
        assert!(replay_ui.contains("self.web_replay_start(id, event_loop)") && replay_ui.contains("self.web_replay_clear(id)"));
        // Hidden panes are not captured: the placement says so.
        let browser = include_str!("browser_app.rs");
        assert!(browser.contains("bv.replay.shown = true;") && browser.contains("self.web_replay_shown(pane, false);"));
    }
}
