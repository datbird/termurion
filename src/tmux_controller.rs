//! The tmux session behind one gateway (iTerm2's TmuxController, the window-opening half):
//! which windows are open with which panes, the attach list and its replies, the per-pane
//! captures, and the notifications that change windows. It says what the window side must
//! do as `Action`s; it owns no panes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::tmux_gateway::{Flags, Gateway, Tag};
use crate::tmux_layout::{self, Node};
use crate::tmux_proto::Note;
use crate::tmux_state;
use crate::tmux_windows::{self, Affinities, OpenIn, Place, Window};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Primary,
    Alt,
    State,
    Pending,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Req {
    ShowSize,
    SessionGuid,
    Hidden,
    Affinities,
    ListSessions,
    ListWindows,
    /// `list-windows -F "#{window_id}"` for a tab reorder: the wanted order.
    Reorder(Vec<u32>),
    /// `new-window -PF` from Tandem: the tmux window whose Tandem window it joins.
    NewWindow(Option<u32>),
    /// `refresh-client -A '%P:pause'` for these panes.
    PausePanes(Vec<u32>),
    /// The Dashboard's list of one session's windows.
    SessionWindows(u32),
    /// `display -p` for a window tmux added.
    DisplayOne,
    /// A capture for one opener (a window being opened, or one layout change's new panes).
    Capture { opener: u64, pane: u32, part: Part },
    /// The short list-windows after a client resize.
    Relist,
    Ignore,
}

/// A pane ready to show: tmux's size, the bytes that rebuild it (fed quietly), and the
/// output tmux had not finished (fed as output).
#[derive(Clone, Debug, PartialEq)]
pub struct PaneOpen {
    pub pane: u32,
    pub w: u32,
    pub h: u32,
    pub restore: Vec<u8>,
    pub pending: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Open { window: Window, node: Node, panes: Vec<PaneOpen>, place: Place },
    /// A new layout: `added` panes open, `removed` panes close, `kept` panes (in layout
    /// order) keep their screens and name the tab.
    /// `zoomed`: tmux's Z flag, when the notification or list carried flags.
    Relayout { window: u32, node: Node, added: Vec<PaneOpen>, removed: Vec<u32>, kept: Vec<u32>, zoomed: Option<bool> },
    Close(u32),
    Rename(u32, String),
    /// The session changed: every tmux tab of this gateway closes.
    CloseAll,
    Notify(&'static str, &'static str),
    Abort(String),
    Output(u32, Vec<u8>),
    SelectPane(u32, u32),
    /// A pane paused: by tmux (its notification) or by the person (Pause Pane).
    Paused { pane: u32, by_tmux: bool },
    /// Panes unpaused: their contents are replaced by these.
    Unpaused(Vec<PaneOpen>),
    /// The pause warning bar of a pane.
    PauseWarning(u32, crate::tmux_pause::Bar),
    /// Every window of the attach has opened (iTerm2's tmuxDidOpenInitialWindows).
    InitialWindowsOpened,
    /// The attach found hidden windows or too many (the Dashboard may open).
    Attached { too_many: bool, hidden: bool },
    /// The session's `@iterm2_id`: another gateway of this Tandem may already show it.
    SessionGuid(String),
    SelectWindow(u32),
}

#[derive(Default)]
struct Captured {
    primary: Vec<u8>,
    alt: Vec<u8>,
    state: String,
    pending: Vec<u8>,
}

struct Opener {
    window: Window,
    node: Node,
    /// Panes being captured (all of a new window, or the added ones of a layout change).
    panes: Vec<u32>,
    waiting: usize,
    captured: HashMap<u32, Captured>,
    initial: bool,
    anonymous: bool,
    /// Unpausing: the captures replace the panes' contents.
    unpause: bool,
    /// A layout change of an open window: the panes it removes and keeps.
    update: Option<(Vec<u32>, Vec<u32>, Option<bool>)>,
}

/// Preferences the controller reads.
#[derive(Clone, Debug)]
pub struct Prefs {
    pub open_in: OpenIn,
    pub limit: i64,
    pub anonymous_location: i64,
    pub prevent_tab: bool,
    pub max_history: u32,
    /// tmuxVariableWindowSizesSupported.
    pub variable_sizes: bool,
    /// TmuxPauseModeAgeLimit, TmuxUnpauseAutomatically, TmuxWarnBeforePausing.
    pub pause_age: f64,
    pub unpause_automatically: bool,
    pub warn_before_pausing: bool,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { open_in: OpenIn::NativeWindows, limit: 10, anonymous_location: 1, prevent_tab: false, max_history: 1000, variable_sizes: true, pause_age: 120.0, unpause_automatically: false, warn_before_pausing: true }
    }
}

pub struct Controller {
    pub session: Option<(u32, String)>,
    pub sessions: Vec<(u32, String)>,
    tags: HashMap<Tag, Req>,
    next_tag: Tag,
    version: Option<f64>,
    known: bool,
    want_open: bool,
    pub tab_bug: bool,
    pub status_bar: bool,
    /// The client size in cells, when tmux has none saved (`@iterm2_size`).
    pub client: (u32, u32),
    hidden: BTreeSet<u32>,
    affinities: Affinities,
    pub prefs: Prefs,
    /// Windows and layout changes waiting for captures, by opener id (iTerm2 gives each
    /// change its own TmuxWindowOpener).
    openers: BTreeMap<u64, Opener>,
    next_opener: u64,
    /// Open windows and their panes, first to last.
    pub windows: BTreeMap<u32, Vec<u32>>,
    /// Border status per window ("top", "bottom", "off").
    border: HashMap<u32, String>,
    /// A new `@iterm2_id` this controller set.
    pub session_guid: Option<String>,
    /// The client size last sent (fixed-size mode).
    last_client: Option<(u32, u32)>,
    /// Each window's size last sent (variable-size mode).
    window_sizes: HashMap<u32, (u32, u32)>,
    /// Client resizes whose relist has not come back (only the last one is read).
    outstanding: u32,
    /// Windows Tandem asked for: not anonymous when tmux adds them.
    intentional: BTreeSet<u32>,
    /// The last `@affinities` command sent (iTerm2 skips an unchanged one).
    last_affinities: Option<String>,
    /// Pause mode (tmux 3.2 and later).
    pause_mode: bool,
    monitor: Option<crate::tmux_pause::Monitor>,
    paused: BTreeSet<u32>,
    warnings: HashMap<u32, crate::tmux_pause::Warning>,
    /// Panes tmux paused that unpause by themselves, and when.
    auto_unpause: Vec<(u32, f64)>,
    /// Seconds on the app's clock, for latency samples.
    clock: f64,
    /// Each open window's layout, for the sizes of a recapture.
    nodes: HashMap<u32, Node>,
    /// The Dashboard's windows of each session it looked at.
    pub session_windows: HashMap<u32, Vec<crate::tmux_dashboard::WindowRow>>,
    /// Lists waiting their 1.5 s: sessions (after %sessions-changed) and one session's windows.
    sessions_due: Option<f64>,
    windows_due: Vec<(u32, f64)>,
    /// Initial windows still capturing.
    initial_pending: usize,
}

impl Controller {
    pub fn new(first_tag: Tag) -> Controller {
        Controller {
            session: None,
            sessions: Vec::new(),
            tags: HashMap::new(),
            next_tag: first_tag,
            version: None,
            known: false,
            want_open: false,
            tab_bug: false,
            status_bar: false,
            client: (80, 24),
            hidden: BTreeSet::new(),
            affinities: Affinities::default(),
            prefs: Prefs::default(),
            openers: BTreeMap::new(),
            next_opener: 0,
            windows: BTreeMap::new(),
            border: HashMap::new(),
            session_guid: None,
            last_client: None,
            window_sizes: HashMap::new(),
            outstanding: 0,
            intentional: BTreeSet::new(),
            last_affinities: None,
            pause_mode: false,
            monitor: None,
            paused: BTreeSet::new(),
            warnings: HashMap::new(),
            auto_unpause: Vec::new(),
            clock: 0.0,
            nodes: HashMap::new(),
            session_windows: HashMap::new(),
            sessions_due: None,
            windows_due: Vec::new(),
            initial_pending: 0,
        }
    }

    fn tag(&mut self, req: Req) -> Tag {
        let t = self.next_tag;
        self.next_tag += 1;
        self.tags.insert(t, req);
        t
    }

    fn v(&self) -> f64 {
        self.version.unwrap_or(0.0)
    }

    /// The window that shows `pane`.
    pub fn window_of(&self, pane: u32) -> Option<u32> {
        self.windows.iter().find(|(_, ps)| ps.contains(&pane)).map(|(w, _)| *w)
    }

    /// The tmux tabs' sizes in cells (by tmux window) after Tandem laid them out: per
    /// window on tmux 2.9 and later, else one client size, the smallest.
    pub fn tabs_resized(&mut self, tabs: &[(u32, (u32, u32))], gw: &mut Gateway) {
        let Some((s, _)) = self.session.clone() else { return };
        if !self.known || tabs.is_empty() {
            return;
        }
        if crate::tmux_sizes::variable(self.version, self.prefs.variable_sizes) {
            let keyed: Vec<(String, (u32, u32))> = tabs.iter().map(|(w, size)| (w.to_string(), *size)).collect();
            let v = self.v();
            let cmds = crate::tmux_sizes::window_size_commands(&keyed, &mut self.window_sizes, v);
            if !cmds.is_empty() {
                gw.send_list(cmds.into_iter().map(|c| (c, None, Flags { tolerate: true, ..Default::default() })).collect(), false);
            }
            return;
        }
        let sizes: Vec<(u32, u32)> = tabs.iter().map(|(_, s)| *s).collect();
        let Some(size) = crate::tmux_sizes::fixed_client_size(&sizes) else { return };
        if self.last_client == Some(size) {
            return;
        }
        self.last_client = Some(size);
        self.window_sizes.clear();
        let list = crate::tmux_sizes::client_size_commands(s, size.0, size.1, self.status_bar, self.version)
            .into_iter()
            .enumerate()
            .map(|(i, (cmd, tolerate))| {
                let tag = match i {
                    0 => Some(self.tag(Req::Ignore)),
                    2 => Some(self.tag(Req::Relist)),
                    _ => None,
                };
                (cmd, tag, Flags { tolerate, ..Default::default() })
            })
            .collect();
        self.outstanding += 1;
        gw.send_list(list, false);
    }

    /// A command, then the short list whose layouts follow it (iTerm2's edits that relist).
    fn with_relist(&mut self, cmd: String, gw: &mut Gateway) {
        let t = self.tag(Req::Relist);
        self.outstanding += 1;
        gw.send_list(vec![(cmd, None, Flags::default()), (crate::tmux_sizes::short_list(self.v()), Some(t), Flags::default())], false);
    }

    /// A divider moved `cells` (plus is right or down) before `pane`, the first pane right of
    /// or below it.
    pub fn resize_pane(&mut self, pane: u32, cells: i32, horizontal: bool, gw: &mut Gateway) {
        if cells != 0 {
            self.with_relist(crate::tmux_edits::resize_pane(pane, cells, horizontal), gw);
        }
    }

    pub fn zoom(&mut self, pane: u32, gw: &mut Gateway) {
        self.with_relist(crate::tmux_edits::zoom(pane), gw);
    }

    pub fn swap(&mut self, a: u32, b: u32, gw: &mut Gateway) {
        self.with_relist(crate::tmux_edits::swap_panes(a, b), gw);
    }

    /// New Tmux Tab (`affinity`: the tmux window whose Tandem window gets the tab) or New
    /// Tmux Window. With variable sizes the client first takes the new window's size.
    pub fn new_window(&mut self, affinity: Option<u32>, dir: &crate::tmux_edits::Dir, size: Option<(u32, u32)>, gw: &mut Gateway) {
        let mut list = Vec::new();
        if let Some((w, h)) = size.filter(|_| crate::tmux_sizes::variable(self.version, self.prefs.variable_sizes)) {
            self.window_sizes.clear();
            let h = crate::tmux_windows::client_height(h, self.status_bar, self.version);
            list.push((format!("refresh-client -C {w},{h}"), None, Flags { tolerate: true, ..Default::default() }));
        }
        let recycling = self.version.is_some_and(|v| v >= 1.9);
        let t = self.tag(Req::NewWindow(affinity));
        list.push((crate::tmux_edits::new_window(None, dir, recycling), Some(t), Flags::default()));
        gw.send_list(list, false);
    }

    /// Hide: the window stays in tmux, is saved in `@hidden`, and its tab closes.
    pub fn hide(&mut self, window: u32, gw: &mut Gateway) -> Vec<Action> {
        let Some((s, _)) = self.session.clone() else { return Vec::new() };
        self.hidden.insert(window);
        gw.send(&crate::tmux_persist::hidden_command(s, &self.hidden), None, Flags::default());
        match self.windows.remove(&window) {
            Some(_) => vec![Action::Close(window)],
            None => Vec::new(),
        }
    }

    pub fn kill_window(&mut self, window: u32, gw: &mut Gateway) {
        gw.send(&crate::tmux_edits::kill_window(window), None, Flags { tolerate: true, laggy: true, ..Default::default() });
    }

    /// Which tmux windows share a Tandem window, saved for the next attach (skipped while
    /// windows are still opening, and when unchanged).
    pub fn save_affinities(&mut self, groups: &[crate::tmux_persist::Group], gw: &mut Gateway) {
        let Some((s, _)) = self.session.clone() else { return };
        if !self.openers.is_empty() {
            return;
        }
        let value = crate::tmux_persist::affinities_value(groups);
        let cmd = crate::tmux_persist::affinities_command(s, &value);
        if self.last_affinities.as_deref() == Some(cmd.as_str()) {
            return;
        }
        self.affinities = Affinities::parse(&value);
        self.last_affinities = Some(cmd.clone());
        gw.send(&cmd, None, Flags::default());
    }

    /// The Dashboard wants a session's windows: listed 1.5 s from now, once per request.
    pub fn dashboard_windows(&mut self, session: u32, now: f64) {
        if !self.windows_due.iter().any(|(s, _)| *s == session) {
            self.windows_due.push((session, now + 1.5));
        }
    }

    /// When the next Dashboard list is due (the app wakes for it).
    pub fn next_due(&self) -> Option<f64> {
        self.sessions_due.into_iter().chain(self.windows_due.iter().map(|(_, t)| *t)).reduce(f64::min)
    }

    /// The Dashboard's window list of the attached session goes stale: list it again.
    fn windows_changed(&mut self) {
        if let Some((s, _)) = self.session {
            if self.session_windows.contains_key(&s) {
                let now = self.clock;
                self.dashboard_windows(s, now);
            }
        }
    }

    /// Sends the lists that are due (the Dashboard's; iTerm2's timers).
    pub fn poll(&mut self, now: f64, gw: &mut Gateway) {
        self.clock = self.clock.max(now);
        if self.sessions_due.is_some_and(|t| t <= now) {
            self.sessions_due = None;
            self.list_sessions(gw);
        }
        let due: Vec<u32> = self.windows_due.iter().filter(|(_, t)| *t <= now).map(|(s, _)| *s).collect();
        self.windows_due.retain(|(_, t)| *t > now);
        let format = tmux_windows::detailed_format(self.v());
        for s in due {
            let t = self.tag(Req::SessionWindows(s));
            gw.send(&crate::tmux_dashboard::list_windows(s, &format), Some(t), Flags { tolerate: true, ..Default::default() });
        }
    }

    pub fn list_sessions(&mut self, gw: &mut Gateway) {
        let t = self.tag(Req::ListSessions);
        gw.send(crate::tmux_dashboard::list_sessions(), Some(t), Flags::default());
    }

    /// A Dashboard command with no reply to read (rename, attach, and the like).
    pub fn command(&mut self, cmd: &str, gw: &mut Gateway) {
        gw.send(cmd, None, Flags::default());
    }

    /// Kill Session and New Session list the sessions again afterwards.
    pub fn command_then_list(&mut self, cmd: &str, laggy: bool, gw: &mut Gateway) {
        gw.send(cmd, None, Flags { tolerate: true, laggy, ..Default::default() });
        self.list_sessions(gw);
    }

    pub fn hidden(&self) -> &BTreeSet<u32> {
        &self.hidden
    }

    /// Opens windows from the Dashboard (iTerm2's openWindowWithId intentional): out of
    /// `@hidden`, then listed and opened; `tabs`: all in one Tandem window.
    pub fn open_windows(&mut self, windows: &[u32], tabs: bool, gw: &mut Gateway) {
        let Some((s, _)) = self.session.clone() else { return };
        for w in windows {
            self.hidden.remove(w);
        }
        gw.send(&crate::tmux_persist::hidden_command(s, &self.hidden), None, Flags::default());
        if tabs {
            if let Some(first) = windows.first() {
                for w in &windows[1..] {
                    self.affinities.set_equal(&w.to_string(), &first.to_string());
                }
            }
        }
        let format = tmux_windows::detailed_format(self.v());
        for w in windows {
            self.intentional.insert(*w);
            let t = self.tag(Req::DisplayOne);
            gw.send(&format!("display -p -F {format} -t @{w}"), Some(t), Flags { tolerate: true, ..Default::default() });
        }
    }

    pub fn pause_mode(&self) -> bool {
        self.pause_mode
    }

    pub fn is_paused(&self, pane: u32) -> bool {
        self.paused.contains(&pane)
    }

    /// The app's clock in seconds (latency samples are timed with it).
    pub fn set_clock(&mut self, now: f64) {
        self.clock = now;
    }

    /// Shell > tmux > Pause Pane (iTerm2's pausePanes): only in pause mode.
    pub fn pause_panes(&mut self, panes: &[u32], gw: &mut Gateway) {
        if !self.pause_mode || panes.is_empty() {
            return;
        }
        let t = self.tag(Req::PausePanes(panes.to_vec()));
        gw.send(&crate::tmux_pause::pause_command(panes), Some(t), Flags::default());
    }

    /// Unpause: each pane is captured again (with `continue`) and its contents replaced.
    pub fn unpause(&mut self, panes: &[u32], gw: &mut Gateway) {
        for &p in panes {
            if !self.paused.remove(&p) {
                continue;
            }
            let Some(window) = self.window_of(p) else { continue };
            let node = self.nodes.get(&window).cloned().unwrap_or(Node::Leaf { w: 80, h: 24, x: 0, y: 0, pane: Some(p) });
            let id = self.next_opener;
            self.next_opener += 1;
            let waiting = self.capture(id, window, &[p], false, false, gw);
            let w = Window { id: window, ..Default::default() };
            self.openers.insert(id, Opener { window: w, node, panes: vec![p], waiting, captured: HashMap::new(), initial: false, anonymous: false, unpause: true, update: None });
        }
    }

    /// TmuxPauseModeAgeLimit changed: tmux gets the new age (iTerm2 sends it again).
    pub fn set_age_limit(&mut self, age: f64, gw: &mut Gateway) {
        if (age - self.prefs.pause_age).abs() < f64::EPSILON {
            return;
        }
        self.prefs.pause_age = age;
        if let Some(m) = &mut self.monitor {
            m.age_limit = age;
        }
        if self.pause_mode {
            gw.send(&crate::tmux_pause::enable_command(age), None, Flags::default());
        }
    }

    /// Once a second: automatic unpauses that are due, and the pause warnings.
    pub fn tick(&mut self, now: f64, gw: &mut Gateway) -> Vec<Action> {
        self.clock = now;
        let due: Vec<u32> = self.auto_unpause.iter().filter(|(_, t)| *t <= now).map(|(p, _)| *p).collect();
        self.auto_unpause.retain(|(_, t)| *t > now);
        if !due.is_empty() {
            self.unpause(&due, gw);
        }
        let mut out = Vec::new();
        let Some(m) = &mut self.monitor else { return out };
        for (pane, ttl, red) in m.update(now) {
            if self.window_of(pane).is_none() {
                continue;
            }
            let paused = self.paused.contains(&pane);
            if let Some(bar) = self.warnings.entry(pane).or_default().update(ttl, red, paused, self.prefs.warn_before_pausing) {
                out.push(Action::PauseWarning(pane, bar));
            }
        }
        out
    }

    /// Tabs moved: tmux's windows take the tabs' order (iTerm2's setPartialWindowIdOrder).
    pub fn reorder(&mut self, wanted: Vec<u32>, gw: &mut Gateway) {
        let t = self.tag(Req::Reorder(wanted));
        gw.send("list-windows -F \"#{window_id}\"", Some(t), Flags::default());
    }

    /// Tandem closed this pane (tmux still has it, until milestone 8c sends kill-pane): the
    /// next layout that names it opens it again.
    pub fn forget_pane(&mut self, pane: u32) {
        for ps in self.windows.values_mut() {
            ps.retain(|p| *p != pane);
        }
    }

    /// iTerm2's didGuessVersion: windows wait for the version.
    pub fn version_known(&mut self, min: Option<f64>, tab_bug: bool, status_bar: bool, gw: &mut Gateway) {
        self.version = min;
        self.tab_bug = tab_bug;
        self.status_bar = status_bar;
        self.known = true;
        // iTerm2's enablePauseModeIfPossible.
        if min.is_some_and(|v| v >= 3.2) && !self.pause_mode {
            self.pause_mode = true;
            self.monitor = Some(crate::tmux_pause::Monitor::new(self.prefs.pause_age));
            gw.send(&crate::tmux_pause::enable_command(self.prefs.pause_age), None, Flags::default());
        }
        if std::mem::take(&mut self.want_open) {
            self.open_initial(gw);
        }
    }

    /// iTerm2's sessionChangedTo: close everything, then open the new session's windows.
    pub fn session_changed(&mut self, id: u32, name: &str, gw: &mut Gateway) -> Vec<Action> {
        self.session = Some((id, name.to_string()));
        self.session_guid = None;
        self.windows.clear();
        self.openers.clear();
        self.open_initial(gw);
        vec![Action::CloseAll]
    }

    fn open_initial(&mut self, gw: &mut Gateway) {
        let Some((s, _)) = self.session.clone() else { return };
        if !self.known {
            self.want_open = true;
            return;
        }
        let t = self.tag(Req::ShowSize);
        gw.send(&format!("show -v -q -t ${s} @iterm2_size"), Some(t), Flags::default());
    }

    fn open_windows_of_size(&mut self, w: u32, h: u32, gw: &mut Gateway) {
        let Some((s, _)) = self.session.clone() else { return };
        let h = tmux_windows::client_height(h, self.status_bar, self.version);
        let format = tmux_windows::detailed_format(self.v());
        let reqs = [Req::SessionGuid, Req::Ignore, Req::Hidden, Req::Ignore, Req::Affinities, Req::Ignore, Req::Ignore, Req::Ignore, Req::Ignore, Req::Ignore, Req::ListSessions, Req::ListWindows];
        let strict = [0usize, 10, 11];
        let list = tmux_windows::open_list(s, w, h, &format)
            .into_iter()
            .zip(reqs)
            .enumerate()
            .map(|(i, (cmd, req))| {
                let t = self.tag(req);
                (cmd, Some(t), Flags { tolerate: !strict.contains(&i), ..Default::default() })
            })
            .collect();
        gw.send_list(list, false);
    }

    /// A reply to one of the controller's commands; None when the tag is not its.
    pub fn reply(&mut self, tag: Tag, ok: bool, text: &str, data: &[u8], gw: &mut Gateway) -> Option<Vec<Action>> {
        let req = self.tags.remove(&tag)?;
        let mut out = Vec::new();
        match req {
            Req::Ignore => {}
            Req::NewWindow(affinity) => {
                if ok {
                    let w = crate::tmux_windows::window_id(text.trim());
                    self.intentional.insert(w);
                    if let Some(a) = affinity {
                        self.affinities.set_equal(&w.to_string(), &a.to_string());
                    }
                }
            }
            Req::SessionWindows(session) => {
                let rows = tmux_windows::parse_windows(text, self.v(), self.tab_bug)
                    .into_iter()
                    .map(|w| crate::tmux_dashboard::WindowRow { open: self.windows.contains_key(&w.id), id: w.id, name: w.name })
                    .collect();
                self.session_windows.insert(session, rows);
            }
            Req::PausePanes(panes) => {
                for p in panes {
                    self.paused.insert(p);
                    if let Some(m) = &mut self.monitor {
                        m.reset(p);
                    }
                    self.warnings.entry(p).or_default().reset();
                    out.push(Action::Paused { pane: p, by_tmux: false });
                }
            }
            Req::Reorder(wanted) => {
                let cmds = crate::tmux_edits::reorder(text, &wanted);
                if !cmds.is_empty() {
                    gw.send_list(cmds.into_iter().map(|c| (c, None, Flags::default())).collect(), false);
                }
            }
            Req::Relist => {
                self.outstanding = self.outstanding.saturating_sub(1);
                if self.outstanding == 0 && ok {
                    for row in crate::tmux_sizes::parse_short(text) {
                        if let Some(b) = row.border {
                            self.border.insert(row.window, b);
                        }
                        out.extend(self.layout_changed(row.window, &row.layout, row.zoomed, gw));
                    }
                }
            }
            Req::ShowSize => {
                let size = text.split_once(',').and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)));
                let (w, h) = size.filter(|_| ok).unwrap_or(self.client);
                self.open_windows_of_size(w, h, gw);
            }
            Req::SessionGuid => {
                if ok && text.is_empty() {
                    if let Some((s, _)) = &self.session {
                        let guid = new_guid();
                        gw.send(&format!("set -t ${s} @iterm2_id \"{guid}\""), None, Flags::default());
                        self.session_guid = Some(guid);
                    }
                } else if ok {
                    self.session_guid = Some(text.to_string());
                    out.push(Action::SessionGuid(text.to_string()));
                }
            }
            Req::Hidden => self.hidden = if ok { tmux_windows::hidden(text) } else { BTreeSet::new() },
            Req::Affinities => self.affinities = Affinities::parse(if ok { text } else { "" }),
            Req::ListSessions => {
                self.sessions = text.lines().filter_map(|l| l.split_once(' ')).filter_map(|(id, name)| Some((id.strip_prefix('$')?.parse().ok()?, name.to_string()))).collect();
            }
            Req::ListWindows => {
                let windows = tmux_windows::parse_windows(text, self.v(), self.tab_bug);
                let initial = tmux_windows::initial(windows, &self.hidden, &mut self.affinities, self.prefs.open_in, self.prefs.limit);
                if let Some((t, x)) = initial.notify {
                    out.push(Action::Notify(t, x));
                    out.push(Action::Attached { too_many: !initial.hide.is_empty(), hidden: initial.hide.is_empty() });
                }
                if initial.open.is_empty() {
                    gw.accept_notifications();
                    out.push(Action::InitialWindowsOpened);
                }
                self.initial_pending = initial.open.len();
                // Too many: they are saved hidden, so the next attach says so.
                if !initial.hide.is_empty() {
                    self.hidden.extend(initial.hide.iter().copied());
                    if let Some((s, _)) = &self.session {
                        gw.send(&crate::tmux_persist::hidden_command(*s, &self.hidden), None, Flags::default());
                    }
                }
                for w in initial.open {
                    out.extend(self.open_window(w, true, false, gw));
                }
            }
            Req::DisplayOne => {
                if let Some(w) = tmux_windows::parse_windows(text, self.v(), self.tab_bug).into_iter().next() {
                    if !self.windows.contains_key(&w.id) && !self.openers.values().any(|o| o.window.id == w.id) {
                        let anonymous = !self.intentional.remove(&w.id);
                        out.extend(self.open_window(w, false, anonymous, gw));
                    }
                }
            }
            Req::Capture { opener, pane, part } => {
                let Some(o) = self.openers.get_mut(&opener) else { return Some(out) };
                if part != Part::Other {
                    let c = o.captured.entry(pane).or_default();
                    if ok {
                        match part {
                            Part::Primary => c.primary = data.to_vec(),
                            Part::Alt => c.alt = data.to_vec(),
                            Part::State => c.state = text.to_string(),
                            Part::Pending => c.pending = tmux_state::pending_bytes(data),
                            Part::Other => {}
                        }
                    }
                }
                o.waiting = o.waiting.saturating_sub(1);
                if o.waiting == 0 {
                    let o = self.openers.remove(&opener).expect("opener");
                    let initial = o.initial && o.update.is_none() && !o.unpause;
                    out.push(self.finish_opener(o));
                    if initial && self.initial_pending > 0 {
                        self.initial_pending -= 1;
                        if self.initial_pending == 0 {
                            out.push(Action::InitialWindowsOpened);
                        }
                    }
                }
            }
        }
        Some(out)
    }

    /// Sends the capture list for `panes` of `window` under `opener`; its last command is
    /// `initial`.
    fn capture(&mut self, opener: u64, window: u32, panes: &[u32], initial: bool, zoomed: bool, gw: &mut Gateway) -> usize {
        let format = tmux_state::format();
        let mut list = Vec::new();
        for &p in panes {
            let cmds = tmux_windows::capture_list(p, self.prefs.max_history, self.version, self.pause_mode, &format);
            let parts = [Part::Primary, Part::Alt, Part::State, Part::Pending, Part::Other];
            for (i, cmd) in cmds.into_iter().enumerate() {
                let part = parts[i.min(4)];
                let t = self.tag(Req::Capture { opener, pane: p, part });
                list.push((cmd, Some(t), Flags { tolerate: true, wants_data: true, ..Default::default() }));
            }
        }
        if zoomed {
            let t = self.tag(Req::Capture { opener, pane: u32::MAX, part: Part::Other });
            list.push((format!("resize-pane -Z -t @{window}"), Some(t), Flags { tolerate: true, ..Default::default() }));
        }
        let n = list.len();
        gw.send_list(list, initial);
        n
    }

    fn open_window(&mut self, window: Window, initial: bool, anonymous: bool, gw: &mut Gateway) -> Vec<Action> {
        let Some(node) = tmux_layout::parse(&window.layout) else {
            return vec![Action::Abort(format!("Error parsing layout {}", window.layout))];
        };
        if let Some(b) = &window.border_status {
            self.border.insert(window.id, b.clone());
        }
        let node = self.adjusted(window.id, node);
        let panes = tmux_layout::panes(&node);
        let id = self.next_opener;
        self.next_opener += 1;
        let waiting = self.capture(id, window.id, &panes, initial, window.zoomed(), gw);
        self.openers.insert(id, Opener { window, node, panes, waiting, captured: HashMap::new(), initial, anonymous, unpause: false, update: None });
        Vec::new()
    }

    fn adjusted(&self, window: u32, node: Node) -> Node {
        let b = self.border.get(&window).map(String::as_str);
        let h = match &node {
            Node::Split { h, .. } | Node::Leaf { h, .. } => *h,
        };
        tmux_layout::border_status(node, b == Some("top"), b == Some("bottom"), h)
    }

    fn finish_opener(&mut self, mut o: Opener) -> Action {
        let sizes: HashMap<u32, (u32, u32)> = tmux_layout::sizes(&o.node).into_iter().map(|(p, w, h)| (p, (w, h))).collect();
        let tab_bug = self.tab_bug;
        let panes: Vec<PaneOpen> = o
            .panes
            .iter()
            .map(|&p| {
                let c = o.captured.remove(&p).unwrap_or_default();
                let (w, h) = sizes.get(&p).copied().unwrap_or((80, 24));
                let state = tmux_state::parse(&c.state, p, tab_bug);
                PaneOpen { pane: p, w, h, restore: tmux_state::restore_bytes(&state, &c.primary, &c.alt, h as usize), pending: c.pending }
            })
            .collect();
        let all = tmux_layout::panes(&o.node);
        if o.unpause {
            return Action::Unpaused(panes);
        }
        self.nodes.insert(o.window.id, o.node.clone());
        match o.update {
            // The window's pane list changed when the layout came.
            Some((removed, kept, zoomed)) => {
                let _ = all;
                Action::Relayout { window: o.window.id, node: o.node, added: panes, removed, kept, zoomed }
            }
            None => {
                let open: Vec<u32> = self.windows.keys().copied().collect();
                let place = tmux_windows::place(o.window.id, &self.affinities, &open, self.prefs.prevent_tab, o.initial, o.anonymous, self.prefs.open_in, self.prefs.anonymous_location);
                self.windows.insert(o.window.id, all);
                Action::Open { window: o.window, node: o.node, panes, place }
            }
        }
    }

    /// A notification the gateway passed on.
    pub fn note(&mut self, note: Note, gw: &mut Gateway) -> Vec<Action> {
        match note {
            Note::Pause(pane) => {
                if self.window_of(pane).is_none() || !self.paused.insert(pane) {
                    return Vec::new();
                }
                if let Some(m) = &mut self.monitor {
                    m.reset(pane);
                }
                self.warnings.entry(pane).or_default().reset();
                if self.prefs.unpause_automatically {
                    self.auto_unpause.push((pane, self.clock + 1.0));
                    return Vec::new();
                }
                return vec![Action::Paused { pane, by_tmux: true }];
            }
            Note::ExtendedOutput { pane, latency_ms, data } => {
                if let Some(m) = &mut self.monitor {
                    m.set_latency(pane, latency_ms as f64 / 1000.0, self.clock);
                }
                if self.window_of(pane).is_some() {
                    return vec![Action::Output(pane, data)];
                }
            }
            Note::Output { pane, data } => {
                if self.window_of(pane).is_some() {
                    return vec![Action::Output(pane, data)];
                }
            }
            Note::LayoutChange { window, layout, zoomed, .. } => return self.layout_changed(window, &layout, zoomed, gw),
            Note::WindowAdd(window) => {
                self.windows_changed();
                if !self.windows.contains_key(&window) && !self.hidden.contains(&window) {
                    let t = self.tag(Req::DisplayOne);
                    let format = tmux_windows::detailed_format(self.v());
                    gw.send(&format!("display -p -F {format} -t @{window}"), Some(t), Flags { tolerate: true, ..Default::default() });
                }
            }
            Note::WindowClose(window) => {
                self.windows_changed();
                self.openers.retain(|_, o| o.window.id != window);
                if self.windows.remove(&window).is_some() {
                    return vec![Action::Close(window)];
                }
            }
            Note::WindowRenamed { window, name } => {
                self.windows_changed();
                if self.windows.contains_key(&window) {
                    return vec![Action::Rename(window, name)];
                }
            }
            Note::SessionChanged { session, name } => return self.session_changed(session, &name, gw),
            Note::SessionsChanged => {
                if self.sessions_due.is_none() {
                    self.sessions_due = Some(self.clock + 1.5);
                }
            }
            Note::SessionRenamed { session, name } => {
                if let Some((s, n)) = &mut self.session {
                    if *s == session {
                        *n = name;
                    }
                }
            }
            Note::WindowPaneChanged { window, pane } => {
                if self.windows.get(&window).is_some_and(|ps| ps.contains(&pane)) {
                    return vec![Action::SelectPane(window, pane)];
                }
            }
            Note::SessionWindowChanged { session, window } => {
                if self.session.as_ref().is_some_and(|(s, _)| *s == session) && self.windows.contains_key(&window) {
                    return vec![Action::SelectWindow(window)];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn layout_changed(&mut self, window: u32, layout: &str, zoomed: Option<bool>, gw: &mut Gateway) -> Vec<Action> {
        let Some(old) = self.windows.get(&window).cloned() else { return Vec::new() };
        let Some(node) = tmux_layout::parse(layout) else {
            return vec![Action::Abort(format!("Error parsing layout {layout}"))];
        };
        let node = self.adjusted(window, node);
        let now = tmux_layout::panes(&node);
        // `old` holds panes an earlier change is still capturing, so they are not added twice.
        let added: Vec<u32> = now.iter().copied().filter(|p| !old.contains(p)).collect();
        let removed: Vec<u32> = old.iter().copied().filter(|p| !now.contains(p)).collect();
        let kept: Vec<u32> = now.iter().copied().filter(|p| old.contains(p)).collect();
        self.windows.insert(window, now);
        self.nodes.insert(window, node.clone());
        if added.is_empty() {
            return vec![Action::Relayout { window, node, added: Vec::new(), removed, kept, zoomed }];
        }
        let id = self.next_opener;
        self.next_opener += 1;
        let waiting = self.capture(id, window, &added, false, false, gw);
        let w = Window { id: window, layout: layout.to_string(), ..Default::default() };
        self.openers.insert(id, Opener { window: w, node, panes: added, waiting, captured: HashMap::new(), initial: false, anonymous: false, unpause: false, update: Some((removed, kept, zoomed)) });
        Vec::new()
    }
}

/// A new GUID for `@iterm2_id`.
fn new_guid() -> String {
    uuid::Uuid::new_v4().to_string().to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux_gateway::Event;

    struct Rig {
        gw: Gateway,
        ctl: Controller,
        n: u32,
    }

    impl Rig {
        /// tmux 3.1: capture-pane -N and user vars, no pause mode.
        fn new() -> Rig {
            Rig::with_version(3.1)
        }

        fn with_version(v: f64) -> Rig {
            let mut gw = Gateway::new("\r");
            for l in ["%begin 1 1 0", "%end 1 1 0"] {
                gw.line(l.as_bytes());
            }
            let mut ctl = Controller::new(100);
            ctl.version_known(Some(v), false, false, &mut gw);
            let mut r = Rig { gw, ctl, n: 10 };
            if v >= 3.2 {
                // pause mode's refresh-client -fpause-after goes first.
                r.answer(Some(""));
            }
            r
        }

        /// Feeds tmux lines; replies go to the controller, notes too.
        fn lines(&mut self, lines: &[&str]) -> Vec<Action> {
            let mut out = Vec::new();
            for l in lines {
                for e in self.gw.line(l.as_bytes()) {
                    match e {
                        Event::Reply { tag, ok, text, data } => out.extend(self.ctl.reply(tag, ok, &text, &data, &mut self.gw).unwrap_or_default()),
                        Event::Note(n) => out.extend(self.ctl.note(n, &mut self.gw)),
                        _ => {}
                    }
                }
            }
            out
        }

        /// Answers the next command with `reply` (None: an error).
        fn answer(&mut self, reply: Option<&str>) -> Vec<Action> {
            self.n += 1;
            let n = self.n;
            let mut ls = vec![format!("%begin {n} {n} 1")];
            if let Some(r) = reply {
                ls.extend(r.split('\n').map(String::from));
            }
            ls.push(format!("%{} {n} {n} 1", if reply.is_some() { "end" } else { "error" }));
            let refs: Vec<&str> = ls.iter().map(String::as_str).collect();
            self.lines(&refs)
        }

        fn sent(&mut self) -> String {
            String::from_utf8(self.gw.take_writes()).unwrap()
        }
    }

    const ONE_PANE: &str = "main\t@0\tbash\t80\t24\tb25d,80x24,0,0,0\t*\t1\tb25d,80x24,0,0,0\toff";

    /// Session changed, size and the attach list answered, one window with one pane listed.
    fn attached() -> Rig {
        let mut r = Rig::new();
        let a = r.lines(&["%session-changed $0 main"]);
        assert_eq!(a, vec![Action::CloseAll]);
        assert_eq!(r.sent(), "show -v -q -t $0 @iterm2_size\r");
        r.answer(Some(""));
        let list = r.sent();
        assert!(list.starts_with("show -v -q -t $0 @iterm2_id; refresh-client -C 80,24; "), "{list}");
        for _ in 0..11 {
            assert!(r.answer(Some("")).is_empty());
        }
        assert!(r.answer(Some(ONE_PANE)).is_empty(), "captures come first");
        r
    }

    #[test]
    fn early_output_is_not_doubled() {
        let mut r = attached();
        let caps = r.sent();
        assert!(caps.starts_with("set -t $0 @iterm2_id \""), "an empty @iterm2_id gets one: {caps}");
        assert!(caps.contains("\rcapture-pane -peqJN -t \"%0\" -S -1000; capture-pane -peqJN -a -t \"%0\""), "{caps}");
        assert!(r.answer(Some("")).is_empty(), "the set command's reply");
        assert!(r.lines(&["%output %0 early"]).is_empty(), "dropped until the capture list is answered");
        r.answer(Some("line1\nline2"));
        r.answer(Some(""));
        r.answer(Some("pane_id=%0\tcursor_x=5\tcursor_y=1"));
        r.answer(Some("early"));
        let a = r.answer(Some(""));
        let [Action::Open { window, panes, place, .. }, Action::InitialWindowsOpened] = &a[..] else { panic!("{a:?}") };
        assert_eq!((window.id, place), (0, &Place::NewWindow));
        assert_eq!(panes[0].pending, b"early", "once, from the capture");
        assert_eq!((panes[0].w, panes[0].h), (80, 24));
        assert!(panes[0].restore.starts_with(b"line1\r\nline2"));
        assert_eq!(r.lines(&["%output %0 late"]), vec![Action::Output(0, b"late".to_vec())]);
        assert!(r.lines(&["%output %9 other"]).is_empty(), "a pane Tandem does not show");
    }

    fn opened() -> Rig {
        let mut r = attached();
        r.sent();
        for _ in 0..6 {
            r.answer(Some(""));
        }
        r
    }

    #[test]
    fn layout_changes_add_and_remove_panes() {
        let mut r = opened();
        let a = r.lines(&["%layout-change @0 8205,80x24,0,0{40x24,0,0,0,39x24,41,0,1} 8205,80x24,0,0{40x24,0,0,0,39x24,41,0,1} *"]);
        assert!(a.is_empty(), "the new pane is captured first");
        assert!(r.sent().starts_with("capture-pane -peqJN -t \"%1\""));
        for _ in 0..4 {
            r.answer(Some(""));
        }
        let a = r.answer(Some(""));
        let [Action::Relayout { window: 0, added, removed, kept, .. }] = &a[..] else { panic!("{a:?}") };
        assert_eq!((added.iter().map(|p| p.pane).collect::<Vec<_>>(), removed.clone()), (vec![1], vec![]));
        assert_eq!(kept, &vec![0], "the tab is found through a pane it already has");
        assert_eq!(r.ctl.windows[&0], vec![0, 1]);
        // Pane 0 goes and pane 2 comes in one change.
        r.lines(&["%layout-change @0 aaaa,80x24,0,0{40x24,0,0,2,39x24,41,0,1} x *"]);
        for _ in 0..4 {
            r.answer(Some(""));
        }
        let a = r.answer(Some(""));
        let [Action::Relayout { added, removed, .. }] = &a[..] else { panic!("{a:?}") };
        assert_eq!((added[0].pane, removed.clone()), (2, vec![0]));
        // A change with no new pane acts at once.
        let a = r.lines(&["%layout-change @0 aaaa,80x24,0,0,1 x *"]);
        assert!(matches!(&a[..], [Action::Relayout { removed, added, .. }] if *removed == vec![2] && added.is_empty()));
        assert_eq!(r.lines(&["%window-renamed @0 vi"]), vec![Action::Rename(0, "vi".into())]);
        assert_eq!(r.lines(&["%window-close @0"]), vec![Action::Close(0)]);
        assert!(r.ctl.windows.is_empty());
    }

    #[test]
    fn two_quick_splits_each_get_their_captures() {
        let mut r = opened();
        r.sent();
        r.lines(&["%layout-change @0 8205,80x24,0,0{40x24,0,0,0,39x24,41,0,1} x *"]);
        r.lines(&["%layout-change @0 aaaa,80x24,0,0{40x24,0,0,0,19x24,41,0,1,19x24,61,0,2} x *"]);
        let mut acts = Vec::new();
        for i in 0..10 {
            acts.extend(r.answer(Some(&format!("h{i}"))));
        }
        let added: Vec<(u32, Vec<u8>)> = acts
            .iter()
            .flat_map(|a| match a {
                Action::Relayout { added, .. } => added.iter().map(|p| (p.pane, p.restore.clone())).collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(added.iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![1, 2], "{acts:?}");
        assert!(added[0].1.starts_with(b"h0"), "pane 1 gets the first change's captures");
        assert!(added[1].1.starts_with(b"h5"), "pane 2 gets the second change's captures");
        assert_eq!(r.ctl.windows[&0], vec![0, 1, 2]);
    }

    #[test]
    fn a_pane_closed_in_tandem_comes_back_with_the_next_layout() {
        let mut r = opened();
        r.sent();
        r.lines(&["%layout-change @0 8205,80x24,0,0{40x24,0,0,0,39x24,41,0,1} x *"]);
        for _ in 0..5 {
            r.answer(Some(""));
        }
        r.ctl.forget_pane(1);
        assert_eq!(r.ctl.windows[&0], vec![0]);
        r.lines(&["%layout-change @0 8205,80x24,0,0{40x24,0,0,0,39x24,41,0,1} x *"]);
        assert!(r.sent().starts_with("capture-pane -peqJN -t \"%1\""), "tmux still has it, so it opens again");
    }

    #[test]
    fn resizes_follow_the_size_mode() {
        let mut r = opened();
        r.sent();
        r.ctl.tabs_resized(&[(0, (100, 30))], &mut r.gw);
        assert_eq!(r.sent(), "resize-window -x 100 -y 30 -t @0\r", "tmux 3.1: per window with resize-window");
        r.answer(Some(""));
        r.ctl.tabs_resized(&[(0, (100, 30))], &mut r.gw);
        assert!(r.sent().is_empty(), "unchanged");
        // Fixed size (variable sizes off): one client size, then the relist relays out.
        r.ctl.prefs.variable_sizes = false;
        r.ctl.tabs_resized(&[(0, (90, 20)), (5, (100, 25))], &mut r.gw);
        let sent = r.sent();
        assert!(sent.starts_with("set -t $0 @iterm2_size 90,20; refresh-client -C 90,20; list-windows -F "), "{sent}");
        r.answer(Some(""));
        r.answer(Some(""));
        let a = r.answer(Some("@0 b25d,90x20,0,0,0 * b25d,90x20,0,0,0 off"));
        assert!(matches!(&a[..], [Action::Relayout { window: 0, kept, .. }] if *kept == vec![0]), "{a:?}");
    }

    #[test]
    fn drags_send_one_resize_on_release() {
        let mut r = opened();
        r.sent();
        r.ctl.resize_pane(1, -3, true, &mut r.gw);
        let sent = r.sent();
        assert!(sent.starts_with("resize-pane -L -t \"%1\" 3; list-windows -F "), "{sent}");
        r.answer(Some(""));
        let a = r.answer(Some("@0 b25d,80x24,0,0,0 *Z b25d,80x24,0,0,0 off"));
        assert!(matches!(&a[..], [Action::Relayout { zoomed: Some(true), .. }]), "{a:?}");
        r.ctl.zoom(0, &mut r.gw);
        assert!(r.sent().starts_with("resize-pane -Z -t \"%0\"; list-windows"));
        r.answer(Some(""));
        r.answer(Some(""));
        r.ctl.swap(0, 1, &mut r.gw);
        assert!(r.sent().starts_with("swap-pane -s \"%0\" -t \"%1\"; list-windows"));
        r.answer(Some(""));
        r.answer(Some(""));
        r.ctl.reorder(vec![2, 0], &mut r.gw);
        assert_eq!(r.sent(), "list-windows -F \"#{window_id}\"\r");
        r.answer(Some("@0\n@2"));
        assert_eq!(r.sent(), "swap-window -s @0 -t @2\r");
    }

    #[test]
    fn new_tabs_join_their_window() {
        let mut r = opened();
        r.sent();
        r.ctl.new_window(Some(0), &crate::tmux_edits::Dir::Home, Some((80, 25)), &mut r.gw);
        assert_eq!(r.sent(), "refresh-client -C 80,25; new-window -PF '#{window_id}'\r");
        r.answer(Some(""));
        r.answer(Some("@7"));
        r.lines(&["%window-add @7"]);
        r.answer(Some("main\t@7\tzsh\t80\t24\tb25d,80x24,0,0,9\t\t0\tb25d,80x24,0,0,9\toff"));
        for _ in 0..4 {
            r.answer(Some(""));
        }
        let a = r.answer(Some(""));
        assert!(matches!(&a[..], [Action::Open { place: Place::WithSibling(0), .. }]), "{a:?}");
    }

    #[test]
    fn hides_and_affinities_are_saved() {
        let mut r = opened();
        r.sent();
        assert_eq!(r.ctl.hide(0, &mut r.gw), vec![Action::Close(0)]);
        assert_eq!(r.sent(), "set -t $0 @hidden \"i_30\"\r");
        r.answer(Some(""));
        let g = [crate::tmux_persist::Group { windows: vec![1, 2], terminal: "pty-9".into(), full_screen: false }];
        r.ctl.save_affinities(&g, &mut r.gw);
        assert!(r.sent().starts_with("set -t $0 @affinities \"a_"));
        r.ctl.save_affinities(&g, &mut r.gw);
        assert!(r.sent().is_empty(), "unchanged");
        r.ctl.kill_window(2, &mut r.gw);
        assert_eq!(r.sent(), "kill-window -t @2\r");
    }

    #[test]
    fn double_attach_is_found() {
        let mut r = Rig::new();
        r.lines(&["%session-changed $0 main"]);
        r.sent();
        r.answer(Some(""));
        r.sent();
        let a = r.answer(Some("ABC-123"));
        assert_eq!(a, vec![Action::SessionGuid("ABC-123".into())]);
        assert_eq!(r.ctl.session_guid.as_deref(), Some("ABC-123"));
        let (heading, text, buttons) = crate::tmux_persist::attach_warning(true);
        assert_eq!((heading, text), ("Cannot Attach", "This instance of Tandem is already attached to this session"));
        assert_eq!(buttons, vec!["OK", "Reveal", "Force Detach Other"]);
        assert_eq!(crate::tmux_persist::attach_warning(false).2, vec!["OK"]);
    }

    /// tmux 3.4 attached with one window, one pane (%0), six captures a pane.
    fn opened_34() -> Rig {
        let mut r = Rig::with_version(3.4);
        r.lines(&["%session-changed $0 main"]);
        r.answer(Some(""));
        for _ in 0..11 {
            r.answer(Some(""));
        }
        r.answer(Some(ONE_PANE));
        r.answer(Some(""));
        let caps = r.sent();
        assert!(caps.contains("refresh-client -A '%0:continue'"), "pause mode adds continue: {caps}");
        for _ in 0..6 {
            r.answer(Some(""));
        }
        r.sent();
        r
    }

    #[test]
    fn automatic_unpause_waits_a_second() {
        let mut r = opened_34();
        r.ctl.prefs.unpause_automatically = true;
        r.ctl.set_clock(100.0);
        assert!(r.lines(&["%pause %0"]).is_empty(), "no bar when it unpauses by itself");
        assert!(r.ctl.tick(100.5, &mut r.gw).is_empty());
        r.ctl.tick(101.0, &mut r.gw);
        assert!(r.sent().contains("refresh-client -A '%0:continue'"), "a second later it recaptures");
    }

    #[test]
    fn unpause_replaces_contents() {
        let mut r = opened_34();
        assert_eq!(r.lines(&["%pause %0"]), vec![Action::Paused { pane: 0, by_tmux: true }]);
        assert!(r.ctl.is_paused(0));
        r.ctl.unpause(&[0], &mut r.gw);
        assert!(r.sent().starts_with("capture-pane -peqJN -t \"%0\""));
        r.answer(Some("fresh"));
        for _ in 0..4 {
            r.answer(Some(""));
        }
        let a = r.answer(Some(""));
        let [Action::Unpaused(panes)] = &a[..] else { panic!("{a:?}") };
        assert!(panes[0].restore.starts_with(b"fresh"));
        assert!(!r.ctl.is_paused(0));
    }

    #[test]
    fn pause_needs_pause_mode() {
        let mut r = opened();
        r.sent();
        assert!(!r.ctl.pause_mode());
        r.ctl.pause_panes(&[0], &mut r.gw);
        assert!(r.sent().is_empty(), "tmux 3.1 has no pause mode");
        let mut r = opened_34();
        assert!(r.ctl.pause_mode());
        r.ctl.pause_panes(&[0], &mut r.gw);
        assert_eq!(r.sent(), "refresh-client -A '%0:pause'\r");
        assert_eq!(r.answer(Some("")), vec![Action::Paused { pane: 0, by_tmux: false }]);
    }

    #[test]
    fn age_limit_changes_are_sent() {
        let mut r = opened_34();
        r.ctl.set_age_limit(30.0, &mut r.gw);
        assert_eq!(r.sent(), "refresh-client -fpause-after=30\r");
        r.ctl.set_age_limit(30.0, &mut r.gw);
        assert!(r.sent().is_empty(), "unchanged");
    }

    #[test]
    fn dashboard_lists_follow_the_session() {
        let mut r = opened();
        r.sent();
        r.ctl.dashboard_windows(0, 10.0);
        r.ctl.dashboard_windows(0, 10.2);
        r.ctl.poll(11.0, &mut r.gw);
        assert!(r.sent().is_empty(), "iTerm2 waits 1.5 s");
        r.ctl.poll(11.6, &mut r.gw);
        let sent = r.sent();
        assert!(sent.starts_with("list-windows -F \"#{session_name}") && sent.ends_with(" -t \"$0\"\r"), "{sent}");
        assert_eq!(sent.matches("list-windows").count(), 1, "once per request");
        r.answer(Some(&format!("{ONE_PANE}\nmain\t@4\tlogs\t80\t24\tb25d,80x24,0,0,9\t\t0\tb25d,80x24,0,0,9\toff")));
        let rows = &r.ctl.session_windows[&0];
        assert_eq!(rows.iter().map(|w| (w.id, w.name.as_str(), w.open)).collect::<Vec<_>>(), vec![(0, "bash", true), (4, "logs", false)]);
        r.lines(&["%sessions-changed"]);
        r.ctl.poll(20.0, &mut r.gw);
        r.ctl.poll(21.6, &mut r.gw);
        assert_eq!(r.sent(), "list-sessions -F \"#{session_id} #{session_name}\"\r");
        r.answer(Some("$0 main\n$1 other"));
        assert_eq!(r.ctl.sessions, vec![(0, "main".into()), (1, "other".into())]);
    }

    #[test]
    fn initial_opens_are_announced_and_lists_refresh() {
        let mut r = attached();
        r.sent();
        r.answer(Some(""));
        let mut acts = Vec::new();
        for _ in 0..5 {
            acts.extend(r.answer(Some("")));
        }
        assert!(matches!(acts.last(), Some(Action::InitialWindowsOpened)), "after the last initial window: {acts:?}");
        r.ctl.dashboard_windows(0, 10.0);
        assert_eq!(r.ctl.next_due(), Some(11.5), "the loop wakes for the Dashboard's list");
        r.ctl.poll(11.5, &mut r.gw);
        r.sent();
        r.answer(Some(ONE_PANE));
        assert_eq!(r.ctl.next_due(), None);
        r.ctl.set_clock(20.0);
        r.lines(&["%window-renamed @0 vi"]);
        assert_eq!(r.ctl.next_due(), Some(21.5), "a rename lists the Dashboard's windows again");
    }

    #[test]
    fn session_change_closes_everything() {
        let mut r = opened();
        r.sent();
        assert_eq!(r.lines(&["%session-changed $2 other"]), vec![Action::CloseAll]);
        assert!(r.ctl.windows.is_empty());
        assert_eq!(r.sent(), "show -v -q -t $2 @iterm2_size\r", "the new session's windows open");
        // A window tmux adds is listed, then opened like the others (anonymous).
        let mut r = opened();
        r.sent();
        r.lines(&["%window-add @4"]);
        assert!(r.sent().starts_with("display -p -F \"#{session_name}\t#{window_id}"));
        r.answer(Some("main\t@4\tzsh\t80\t24\tb25d,80x24,0,0,6\t\t0\tb25d,80x24,0,0,6\toff"));
        for _ in 0..4 {
            r.answer(Some(""));
        }
        let a = r.answer(Some(""));
        assert!(matches!(&a[..], [Action::Open { window, place: Place::Focused, .. }] if window.id == 4));
    }

    #[test]
    fn versions_wait_before_windows_open() {
        let mut gw = Gateway::new("\r");
        for l in ["%begin 1 1 0", "%end 1 1 0"] {
            gw.line(l.as_bytes());
        }
        let mut ctl = Controller::new(100);
        ctl.session_changed(1, "s", &mut gw);
        gw.line(b"%session-changed $1 s");
        assert!(gw.take_writes().is_empty(), "no version yet");
        ctl.version_known(Some(3.4), false, false, &mut gw);
        assert_eq!(gw.take_writes(), b"refresh-client -fpause-after=120\rshow -v -q -t $1 @iterm2_size\r", "pause mode first, as iTerm2's didGuessVersion");
    }
}
