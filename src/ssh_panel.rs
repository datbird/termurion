//! The remote file panel's model (milestone 14d, `SSHFilePanel.swift`, `SSHFilePanelFileList.swift`,
//! `SSHFolderDialog.swift`): the connected hosts, the folder shown, its history, the listing and
//! the selection, hidden files, and Go to Folder with its completions. Pure: it answers every
//! gesture with the requests to send, and takes the replies back (`answered`); nothing here
//! talks to a host. Every string in a listing came from the remote, so the window shows them
//! through `inert_files::shown_name` (P14) and nothing here is ever built into a local path.

use crate::remote_file::{self, RemoteFile};
use crate::ssh_args::Identity;
use crate::ssh_files::{self, FileSub, Sorting};

pub const TITLE: &str = "Open";
pub const BACK: &str = "Back";
pub const FORWARD: &str = "Forward";
pub const SEARCH: &str = "Search";
pub const USE_SYSTEM_PANEL: &str = "Use System Panel\u{2026}";
pub const OPEN: &str = "Open";
pub const CANCEL: &str = "Cancel";
pub const GO_TO_FOLDER: &str = "Go to Folder\u{2026}";
pub const GO_TO_HOME: &str = "Go to Home";
pub const GO_UP: &str = "Go Up";
pub const REFRESH: &str = "Refresh";
pub const SHOW_HIDDEN: &str = "Show Hidden Files";
pub const CONNECTED_HOSTS: &str = "Connected Hosts";
pub const COLUMN_NAME: &str = "Name";
pub const COLUMN_DATE: &str = "Date Created";
pub const COLUMN_SIZE: &str = "Size";
pub const COLUMN_KIND: &str = "Kind";
pub const GO_TO_TITLE: &str = "Go to the folder:";
pub const GO_TO_INFO: &str = "Type a pathname or select from the pop-up menu";
pub const GO: &str = "Go";
pub const PATH_PLACEHOLDER: &str = "Enter path (e.g., /usr/local/bin)";
pub const LOADING_COMPLETIONS: &str = "Loading completions\u{2026}";

/// Go to Folder's heading for a host (`FD:163`).
pub fn folder_on(host: &str) -> String {
    format!("Folder on {host}")
}

/// What a request that cannot be sent says (`SEX:14-27`, with the host: Tandem's, "The
/// connection to <host> was lost.").
pub fn connection_lost(host: &str) -> String {
    format!("The connection to {host} was lost.")
}

/// The most entries of one listing the panel keeps (P18); the rest are dropped with a notice.
pub const LIST_MAX: usize = 100_000;
/// The most search results the panel keeps (P18; Task 8 fills them).
pub const SEARCH_MAX: usize = 10_000;
/// The notice that goes with a truncated search (Tandem's).
pub const SEARCH_TRUNCATED: &str = "Only the first 10000 results are shown.";
/// The notice that goes with a truncated listing (Tandem's).
pub const TRUNCATED: &str = "Only the first 100000 items are shown.";

/// What a request asks, and what to do with its answer.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Stat `path` before going there; a failure leaves the panel where it was.
    Verify { host: Identity, path: String },
    /// List `path`.
    List { path: String },
    /// Go to Folder's completions for `base`: `list` empty is the stat of `base`, else the
    /// listing of that folder.
    Complete { base: String, list: String },
}

/// What the panel asks the app to send.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    File { generation: u64, host: Identity, op: Op, sub: FileSub },
    SearchStart { generation: u64, host: Identity, query: String, base: String },
    SearchStop { host: Identity },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Column {
    Name,
    Date,
    Size,
    Kind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    Regular,
    Search { generation: u64, query: String },
}

#[derive(Clone, Debug, PartialEq)]
pub enum DoubleClick {
    /// A folder: the requests that go into it.
    Navigate(Vec<Request>),
    /// A file: the same as the Open button.
    Open,
}

/// A navigation waiting for its stat.
#[derive(Clone, Debug)]
struct Nav {
    host: Identity,
    path: String,
    /// List the path even when its stat fails (selecting a host, `FP:289-296`).
    force: bool,
}

pub struct Panel {
    pub hosts: Vec<Identity>,
    pub host: Option<Identity>,
    pub path: Option<String>,
    pub show_hidden: bool,
    pub mode: Mode,
    pub loading: bool,
    pub error: Option<String>,
    /// A truncated listing's notice.
    pub notice: Option<String>,
    /// Go to Folder is waiting for its completions.
    pub completing: bool,
    files: Vec<RemoteFile>,
    selected: Vec<String>,
    anchor: Option<String>,
    sort: Column,
    ascending: bool,
    /// (host, path) entries and the index of the one shown.
    history: Vec<(Identity, String)>,
    at: usize,
    /// The last path listed for each host.
    last: Vec<(Identity, String)>,
    next_gen: u64,
    nav_gen: u64,
    comp_gen: u64,
    nav: Option<Nav>,
    completions: Vec<String>,
    comp_found: bool,
    comp_pending: usize,
    /// The id the remote gave the running search.
    search_id: Option<String>,
}

impl Panel {
    pub fn new(hosts: Vec<Identity>, preferred: Option<Identity>) -> Panel {
        let host = preferred.filter(|p| hosts.contains(p)).or_else(|| hosts.first().cloned());
        Panel {
            hosts,
            host,
            path: None,
            show_hidden: false,
            mode: Mode::Regular,
            loading: false,
            error: None,
            notice: None,
            completing: false,
            files: Vec::new(),
            selected: Vec::new(),
            anchor: None,
            sort: Column::Name,
            ascending: true,
            history: Vec::new(),
            at: 0,
            last: Vec::new(),
            next_gen: 0,
            nav_gen: 0,
            comp_gen: 0,
            nav: None,
            completions: Vec::new(),
            comp_found: false,
            comp_pending: 0,
            search_id: None,
        }
    }

    fn bump(&mut self) -> u64 {
        self.next_gen += 1;
        self.next_gen
    }

    /// The path a host opens at: the last one shown for it, the saved one, its home, "/"
    /// (`FP:225-331`).
    fn default_path(&self, host: &Identity, saved: Option<String>, home: Option<String>) -> String {
        let last = self.last.iter().find(|(h, _)| h == host).map(|(_, p)| p.clone());
        last.or(saved.filter(|s| !s.is_empty())).or(home.filter(|h| !h.is_empty())).unwrap_or_else(|| "/".to_string())
    }

    fn verify(&self, generation: u64, host: &Identity, path: &str) -> Request {
        Request::File { generation, host: host.clone(), op: Op::Verify { host: host.clone(), path: path.to_string() }, sub: FileSub::Stat { path: path.to_string() } }
    }

    fn list(&self, generation: u64, host: &Identity, path: &str) -> Request {
        Request::File { generation, host: host.clone(), op: Op::List { path: path.to_string() }, sub: FileSub::Ls { path: path.to_string(), sort: Sorting::ByName } }
    }

    /// Starts at the default host (the preferred one, else the first) and its default path.
    pub fn start(&mut self, saved: &dyn Fn(&Identity) -> Option<String>, home: &dyn Fn(&Identity) -> Option<String>) -> Vec<Request> {
        let Some(host) = self.host.clone().filter(|h| self.hosts.contains(h)).or_else(|| self.hosts.first().cloned()) else { return Vec::new() };
        self.host = Some(host.clone());
        let path = self.default_path(&host, saved(&host), home(&host));
        self.navigate(&host, &path, true)
    }

    /// `FP:346-372`: history is appended before going, then the path is stat'ed first and a
    /// failure leaves the panel where it was.
    pub fn navigate(&mut self, host: &Identity, path: &str, history: bool) -> Vec<Request> {
        let mut out = self.leave_search();
        if history {
            if !self.history.is_empty() {
                self.history.truncate(self.at + 1);
            }
            self.history.push((host.clone(), path.to_string()));
            self.at = self.history.len() - 1;
        }
        out.extend(self.begin(host, path, false));
        out
    }

    /// Back to the regular listing; a search that ran is stopped (its results go too).
    fn leave_search(&mut self) -> Vec<Request> {
        if self.mode == Mode::Regular {
            return Vec::new();
        }
        self.mode = Mode::Regular;
        self.search_id = None;
        self.files.clear();
        self.selected.clear();
        self.notice = None;
        self.host.clone().map(|host| Request::SearchStop { host }).into_iter().collect()
    }

    fn begin(&mut self, host: &Identity, path: &str, force: bool) -> Vec<Request> {
        let generation = self.bump();
        self.nav_gen = generation;
        self.nav = Some(Nav { host: host.clone(), path: path.to_string(), force });
        self.loading = true;
        vec![self.verify(generation, host, path)]
    }

    /// The panel shows `host` at its default path, listed even when its stat fails
    /// (`FP:289-296`: only `navigate` stays put).
    pub fn select_host(&mut self, host: &Identity, saved: Option<String>, home: Option<String>) -> Vec<Request> {
        let mut out = self.leave_search();
        let path = self.default_path(host, saved, home);
        self.host = Some(host.clone());
        if !self.history.is_empty() {
            self.history.truncate(self.at + 1);
        }
        self.history.push((host.clone(), path.clone()));
        self.at = self.history.len() - 1;
        out.extend(self.begin(host, &path, true));
        out
    }

    /// An answer. One for an old generation, or for nothing the panel waits for, changes nothing.
    pub fn answered(&mut self, generation: u64, op: &Op, content: &str, status: i32) -> Vec<Request> {
        match op {
            Op::Verify { host, path } => {
                if generation != self.nav_gen {
                    return Vec::new();
                }
                let Some(nav) = self.nav.take() else { return Vec::new() };
                if nav.host != *host || nav.path != *path {
                    self.nav = Some(nav);
                    return Vec::new();
                }
                let is_folder = status == 0 && RemoteFile::parse(content).is_ok_and(|f| f.is_folder() || matches!(f.kind, remote_file::Kind::Symlink(_)));
                if (!is_folder && !nav.force) || !self.hosts.contains(host) {
                    self.loading = false;
                    return Vec::new();
                }
                self.host = Some(host.clone());
                self.path = Some(path.clone());
                self.error = None;
                self.notice = None;
                self.files.clear();
                self.selected.clear();
                self.anchor = None;
                self.loading = true;
                vec![self.list(generation, host, path)]
            }
            Op::List { path } => {
                if generation != self.nav_gen || self.path.as_deref() != Some(path.as_str()) {
                    return Vec::new();
                }
                self.loading = false;
                self.error = None;
                self.notice = None;
                self.files.clear();
                match ssh_files::check(status) {
                    Err(e) => self.error = Some(e.text().to_string()),
                    Ok(()) => match remote_file::parse_list(content) {
                        Ok(mut files) => {
                            if files.len() > LIST_MAX {
                                files.truncate(LIST_MAX);
                                self.notice = Some(TRUNCATED.to_string());
                            }
                            self.files = files;
                            if let Some(h) = self.host.clone() {
                                self.last.retain(|(x, _)| *x != h);
                                self.last.push((h, path.clone()));
                            }
                        }
                        Err(e) => self.error = Some(e),
                    },
                }
                Vec::new()
            }
            Op::Complete { base, list } => self.completion_answered(generation, base, list, content, status),
        }
    }

    pub fn back(&mut self) -> Vec<Request> {
        let Some(i) = (0..self.at.min(self.history.len())).rev().find(|j| self.hosts.contains(&self.history[*j].0)) else { return Vec::new() };
        self.go_to_entry(i)
    }

    pub fn forward(&mut self) -> Vec<Request> {
        let Some(i) = (self.at + 1..self.history.len()).find(|j| self.hosts.contains(&self.history[*j].0)) else { return Vec::new() };
        self.go_to_entry(i)
    }

    fn go_to_entry(&mut self, i: usize) -> Vec<Request> {
        let mut out = self.leave_search();
        self.at = i;
        let (h, p) = self.history[i].clone();
        out.extend(self.begin(&h, &p, false));
        out
    }

    pub fn can_back(&self) -> bool {
        (0..self.at.min(self.history.len())).any(|j| self.hosts.contains(&self.history[j].0))
    }

    pub fn can_forward(&self) -> bool {
        (self.at + 1..self.history.len()).any(|j| self.hosts.contains(&self.history[j].0))
    }

    /// `FP:1293-1328`: nothing at "/".
    pub fn go_up(&mut self) -> Vec<Request> {
        let (Some(host), Some(path)) = (self.host.clone(), self.path.clone()) else { return Vec::new() };
        if path == "/" {
            return Vec::new();
        }
        self.navigate(&host, &remote_file::posix_parent(&path), true)
    }

    pub fn go_home(&mut self, home: Option<&str>) -> Vec<Request> {
        let Some(host) = self.host.clone() else { return Vec::new() };
        let target = home.filter(|h| !h.is_empty()).unwrap_or("/").to_string();
        self.navigate(&host, &target, true)
    }

    pub fn refresh(&mut self) -> Vec<Request> {
        let (Some(host), Some(path)) = (self.host.clone(), self.path.clone()) else { return Vec::new() };
        self.navigate(&host, &path, false)
    }

    /// `FP:1356-1459` `expandPath`: only `~` and `~/` are home; the rest is relative to the
    /// folder shown, and standardized.
    pub fn expand_path(&self, input: &str, home: Option<&str>) -> String {
        let home = home.filter(|h| !h.is_empty()).unwrap_or("/");
        let full = if input == "~" {
            home.to_string()
        } else if let Some(rest) = input.strip_prefix("~/") {
            remote_file::posix_join(home, rest)
        } else if input.starts_with('/') {
            input.to_string()
        } else {
            remote_file::posix_join(self.path.as_deref().unwrap_or("/"), input)
        };
        remote_file::posix_standardize(&full)
    }

    /// Go to Folder's completions for what is typed: the stat of the path first.
    pub fn completions(&mut self, input: &str, home: Option<&str>) -> Vec<Request> {
        let Some(host) = self.host.clone() else { return Vec::new() };
        let base = self.expand_path(input, home);
        let generation = self.bump();
        self.comp_gen = generation;
        self.completions.clear();
        self.completing = true;
        self.comp_found = false;
        self.comp_pending = 0;
        vec![Request::File { generation, host, op: Op::Complete { base: base.clone(), list: String::new() }, sub: FileSub::Stat { path: base } }]
    }

    pub fn completion_list(&self) -> &[String] {
        &self.completions
    }

    fn complete_list_request(&self, generation: u64, base: &str, dir: &str) -> Option<Request> {
        let host = self.host.clone()?;
        Some(Request::File { generation, host, op: Op::Complete { base: base.to_string(), list: dir.to_string() }, sub: FileSub::Ls { path: dir.to_string(), sort: Sorting::ByName } })
    }

    fn completion_answered(&mut self, generation: u64, base: &str, list: &str, content: &str, status: i32) -> Vec<Request> {
        if generation != self.comp_gen {
            return Vec::new();
        }
        let lower_base = base.to_lowercase();
        if list.is_empty() {
            // The stat: a folder lists its siblings and its children, anything else its siblings.
            self.comp_found = status == 0 && RemoteFile::parse(content).is_ok_and(|f| f.is_folder());
            let parent = remote_file::posix_parent(base);
            let mut dirs = vec![parent.clone()];
            if self.comp_found && base != parent {
                dirs.push(base.to_string());
            }
            let out: Vec<Request> = dirs.iter().filter_map(|d| self.complete_list_request(generation, base, d)).collect();
            self.comp_pending = out.len();
            if out.is_empty() {
                self.completing = false;
            }
            return out;
        }
        if status == 0 {
            if let Ok(files) = remote_file::parse_list(content) {
                let children = list == base;
                let child_prefix = if lower_base == "/" { "/".to_string() } else { format!("{lower_base}/") };
                for f in files.iter().filter(|f| f.is_folder()) {
                    let lower = f.absolute_path.to_lowercase();
                    let keep = if !self.comp_found {
                        lower.starts_with(&lower_base)
                    } else if children {
                        lower.starts_with(&child_prefix)
                    } else {
                        // Siblings: the remote path as sent against the lower-cased base (`FP:1403-1407`).
                        f.absolute_path.starts_with(&lower_base)
                    };
                    if keep && self.completions.len() < LIST_MAX {
                        self.completions.push(lower);
                    }
                }
            }
        }
        self.comp_pending = self.comp_pending.saturating_sub(1);
        if self.comp_pending == 0 {
            self.completing = false;
            self.completions.sort_unstable();
            self.completions.dedup();
        }
        Vec::new()
    }

    /// The connected hosts changed: a host that left while shown switches the panel to the
    /// first remaining one at its default path, and none leaves it empty.
    pub fn hosts_changed(&mut self, hosts: Vec<Identity>, home: &dyn Fn(&Identity) -> Option<String>) -> Vec<Request> {
        self.hosts = hosts;
        if self.hosts.is_empty() {
            self.mode = Mode::Regular;
            self.search_id = None;
            self.host = None;
            self.path = None;
            self.files.clear();
            self.selected.clear();
            self.loading = false;
            self.error = None;
            self.notice = None;
            self.completing = false;
            self.nav = None;
            // Anything still in flight is for a host that is gone.
            self.nav_gen = self.bump();
            self.comp_gen = self.nav_gen;
            return Vec::new();
        }
        if self.host.as_ref().is_some_and(|h| self.hosts.contains(h)) {
            return Vec::new();
        }
        let first = self.hosts[0].clone();
        let home = home(&first);
        self.select_host(&first, None, home)
    }

    /// Show Hidden Files asks for the listing again (`FP:1765-1768`).
    pub fn toggle_hidden(&mut self) -> Vec<Request> {
        self.show_hidden = !self.show_hidden;
        // Search results are filtered where they are shown: nothing to ask again.
        if self.mode != Mode::Regular {
            return Vec::new();
        }
        let (Some(host), Some(path)) = (self.host.clone(), self.path.clone()) else { return Vec::new() };
        let generation = self.bump();
        self.nav_gen = generation;
        self.nav = None;
        self.loading = true;
        vec![self.list(generation, &host, &path)]
    }

    /// Search (`FP:961-1017`): typing a query stops the running search and starts one in the
    /// folder shown; an empty query returns to the regular listing, listed again.
    pub fn search(&mut self, query: &str) -> Vec<Request> {
        let (Some(host), Some(path)) = (self.host.clone(), self.path.clone()) else { return Vec::new() };
        if query.is_empty() {
            if self.mode == Mode::Regular {
                return Vec::new();
            }
            let mut out = self.leave_search();
            let generation = self.bump();
            self.nav_gen = generation;
            self.nav = None;
            self.loading = true;
            out.push(self.list(generation, &host, &path));
            return out;
        }
        let generation = self.bump();
        self.nav = None;
        // Results of an earlier listing or search must not answer this one.
        self.nav_gen = generation;
        self.mode = Mode::Search { generation, query: query.to_string() };
        self.search_id = None;
        self.files.clear();
        self.selected.clear();
        self.anchor = None;
        self.error = None;
        self.notice = None;
        self.loading = true;
        vec![Request::SearchStop { host: host.clone() }, Request::SearchStart { generation, host, query: query.to_string(), base: path }]
    }

    /// The remote named search `id` for generation `generation`.
    pub fn search_started(&mut self, generation: u64, id: &str) {
        if self.search_generation() == Some(generation) {
            self.search_id = Some(id.to_string());
            self.loading = false;
        }
    }

    /// One result of the search of `generation`; an older generation's is dropped, and no more
    /// than `SEARCH_MAX` are kept (P18).
    pub fn search_result(&mut self, generation: u64, file: RemoteFile) {
        if self.search_generation() != Some(generation) {
            return;
        }
        if self.files.len() >= SEARCH_MAX {
            self.notice = Some(SEARCH_TRUNCATED.to_string());
            return;
        }
        self.files.push(file);
    }

    /// The search of `generation` could not start.
    pub fn search_failed(&mut self, generation: u64, text: &str) {
        if self.search_generation() == Some(generation) {
            self.loading = false;
            self.error = Some(text.to_string());
        }
    }

    /// The id of the running search, once the remote named it.
    pub fn search_id(&self) -> Option<&str> {
        self.search_id.as_deref()
    }

    /// The generation of the current search, when the panel is searching.
    pub fn search_generation(&self) -> Option<u64> {
        match &self.mode {
            Mode::Search { generation, .. } => Some(*generation),
            Mode::Regular => None,
        }
    }

    /// A second click on the same column reverses it (`FL:845`).
    pub fn sort_by(&mut self, c: Column) {
        if c == self.sort {
            self.ascending = !self.ascending;
        } else {
            self.sort = c;
            self.ascending = true;
        }
    }

    /// The rows shown: hidden files dropped unless asked for, in the sort order. Name ignores
    /// case; Size counts a folder as 0; Date sorts by ctime although the column shows mtime
    /// (`FL:474-478`).
    pub fn visible(&self) -> Vec<&RemoteFile> {
        let mut rows: Vec<&RemoteFile> = self.files.iter().filter(|f| self.show_hidden || !f.is_hidden()).collect();
        let name = |f: &RemoteFile| f.name().to_lowercase();
        match self.sort {
            Column::Name => rows.sort_by_key(|f| (name(f), f.name())),
            Column::Size => rows.sort_by(|a, b| a.size().unwrap_or(0).cmp(&b.size().unwrap_or(0)).then_with(|| name(a).cmp(&name(b)))),
            Column::Date => rows.sort_by(|a, b| a.unix_ctime().unwrap_or(f64::MIN).total_cmp(&b.unix_ctime().unwrap_or(f64::MIN)).then_with(|| name(a).cmp(&name(b)))),
            Column::Kind => rows.sort_by(|a, b| remote_file::kind_text(a).cmp(&remote_file::kind_text(b)).then_with(|| name(a).cmp(&name(b)))),
        }
        if !self.ascending {
            rows.reverse();
        }
        rows
    }

    pub fn is_selected(&self, path: &str) -> bool {
        self.selected.iter().any(|p| p == path)
    }

    /// A click on a row: alone it selects only that row, with Ctrl it toggles it, with Shift
    /// it selects the range from the last plain click.
    pub fn click(&mut self, path: &str, ctrl: bool, shift: bool) {
        let order: Vec<String> = self.visible().iter().map(|f| f.absolute_path.clone()).collect();
        if !order.iter().any(|p| p == path) {
            return;
        }
        if shift {
            let anchor = self.anchor.clone().filter(|a| order.contains(a)).unwrap_or_else(|| path.to_string());
            let (a, b) = (order.iter().position(|p| *p == anchor), order.iter().position(|p| p == path));
            if let (Some(a), Some(b)) = (a, b) {
                self.selected = order[a.min(b)..=a.max(b)].to_vec();
            }
            self.anchor = Some(anchor);
        } else if ctrl {
            if let Some(i) = self.selected.iter().position(|p| p == path) {
                self.selected.remove(i);
            } else {
                self.selected.push(path.to_string());
            }
            self.anchor = Some(path.to_string());
        } else {
            self.selected = vec![path.to_string()];
            self.anchor = Some(path.to_string());
        }
    }

    /// The selected rows that are shown, in the order shown.
    pub fn chosen(&self) -> Vec<RemoteFile> {
        self.visible().into_iter().filter(|f| self.is_selected(&f.absolute_path)).cloned().collect()
    }

    /// `FP:1969-1976`: one item of any kind opens; several only when none is a folder.
    pub fn open_enabled(&self) -> bool {
        let c = self.chosen();
        match c.len() {
            0 => false,
            1 => true,
            _ => c.iter().all(|f| !f.is_folder()),
        }
    }

    /// `FP:1981-1991`: a folder is entered, a file opens.
    pub fn double_click(&mut self, path: &str) -> DoubleClick {
        let folder = self.files.iter().find(|f| f.absolute_path == path).is_some_and(RemoteFile::is_folder);
        match (folder, self.host.clone()) {
            (true, Some(host)) => DoubleClick::Navigate(self.navigate(&host, path, true)),
            (true, None) => DoubleClick::Navigate(Vec::new()),
            _ => DoubleClick::Open,
        }
    }
}

/// The saved path's key (`FP:1608-1670`): "SSHFilePanel.currentPath." and the identity's compact text.
pub fn saved_path_key(id: &Identity, local_user: &str) -> String {
    format!("SSHFilePanel.currentPath.{}", id.compact(local_user))
}

/// The longest key kept (P15), and the most keys.
pub const SAVED_KEY_MAX: usize = 200;
pub const SAVED_KEYS_MAX: usize = 32;
/// The setting that remembers the order the keys were first saved in, so the oldest can go.
pub const SAVED_KEYS_LIST: &str = "SSHFilePanel.currentPathKeys";

/// P15: the key to save a host's path under, only for an identity safe to name a setting (the
/// identity text of an unvouched nested hop is remote-chosen) and short enough.
pub fn saved_path_key_checked(id: &Identity, local_user: &str) -> Option<String> {
    let key = saved_path_key(id, local_user);
    (crate::ssh_jumps::identity_is_safe(id) && key.chars().count() <= SAVED_KEY_MAX && !key.chars().any(char::is_control)).then_some(key)
}

/// Notes that `key` was saved: it joins the end of `order`, and the oldest keys beyond
/// `SAVED_KEYS_MAX` are taken off and returned, for the caller to remove from the settings.
pub fn remember_saved_key(order: &mut Vec<String>, key: &str) -> Vec<String> {
    if !order.iter().any(|k| k == key) {
        order.push(key.to_string());
    }
    let extra = order.len().saturating_sub(SAVED_KEYS_MAX);
    order.drain(..extra).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ident(host: &str) -> Identity {
        Identity { host: host.into(), hostname: host.into(), username: Some("u".into()), port: 22 }
    }
    fn a() -> Identity {
        ident("a")
    }
    fn b() -> Identity {
        ident("b")
    }
    fn entry(path: &str, kind: &str, mtime: f64) -> String {
        format!(r#"{{"absolutePath":"{path}","kind":{kind},"ctime":{mtime},"mtime":{mtime}}}"#)
    }
    fn file(path: &str, size: u64, t: f64) -> String {
        entry(path, &format!(r#"{{"file":{{"_0":{{"size":{size}}}}}}}"#), t)
    }
    fn folder(path: &str, t: f64) -> String {
        entry(path, r#"{"folder":{}}"#, t)
    }
    fn list(items: &[String]) -> String {
        format!("[{}]", items.join(","))
    }
    fn none(_: &Identity) -> Option<String> {
        None
    }
    fn home_a(h: &Identity) -> Option<String> {
        (*h == a()).then(|| "/home/a".to_string())
    }
    fn first(reqs: &[Request]) -> (u64, Op, FileSub) {
        match reqs.first() {
            Some(Request::File { generation, op, sub, .. }) => (*generation, op.clone(), sub.clone()),
            other => panic!("no file request: {other:?}"),
        }
    }
    /// A panel on A at "/home/a" with the given listing answered.
    fn shown(items: &[String]) -> Panel {
        let mut p = Panel::new(vec![a(), b()], None);
        let (g, op, _) = first(&p.start(&none, &home_a));
        let (g2, op2, _) = first(&p.answered(g, &op, &folder("/home/a", 1.0), 0));
        p.answered(g2, &op2, &list(items), 0);
        p
    }

    #[test]
    fn panel_navigation_follows_iterm2() {
        let mut p = Panel::new(vec![a(), b()], None);
        let reqs = p.start(&none, &home_a);
        let g = first(&reqs).0;
        assert_eq!(reqs, vec![Request::File { generation: g, host: a(), op: Op::Verify { host: a(), path: "/home/a".into() }, sub: FileSub::Stat { path: "/home/a".into() } }]);
        let reqs = p.answered(g, &Op::Verify { host: a(), path: "/home/a".into() }, &folder("/home/a", 1.0), 0);
        assert_eq!(reqs, vec![Request::File { generation: g, host: a(), op: Op::List { path: "/home/a".into() }, sub: FileSub::Ls { path: "/home/a".into(), sort: Sorting::ByName } }]);
        assert_eq!(p.path.as_deref(), Some("/home/a"));
        assert!(p.loading);
        p.answered(g, &Op::List { path: "/home/a".into() }, &list(&[file("/home/a/x", 1, 1.0)]), 0);
        assert!(!p.loading);

        // A failed stat leaves the panel where it was, with both entries in the history.
        let reqs = p.navigate(&a(), "/tmp", true);
        let (g, op, _) = first(&reqs);
        assert!(p.answered(g, &op, "", 2).is_empty());
        assert_eq!(p.path.as_deref(), Some("/home/a"));
        assert!(!p.loading);
        assert_eq!(p.history.len(), 2);

        // Back asks for the path before, without history.
        let reqs = p.back();
        let (_, op, sub) = first(&reqs);
        assert_eq!((op, sub), (Op::Verify { host: a(), path: "/home/a".into() }, FileSub::Stat { path: "/home/a".into() }));
        assert_eq!(p.history.len(), 2);
        assert!(p.can_forward());
        // A navigation with history drops the forward entries.
        p.navigate(&a(), "/var", true);
        assert_eq!(p.history.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(), ["/home/a", "/var"]);
        assert!(!p.can_forward());

        // Back skips an entry whose host is gone.
        let mut q = Panel::new(vec![a(), b()], None);
        q.history = vec![(a(), "/x".into()), (b(), "/y".into()), (a(), "/z".into())];
        q.at = 2;
        q.hosts = vec![a()];
        let (_, op, _) = first(&q.back());
        assert_eq!(op, Op::Verify { host: a(), path: "/x".into() });
        assert!(!q.can_back());

        // Go up at "/tmp" asks for "/", at "/" nothing.
        let mut p = shown(&[]);
        let (g, op, _) = first(&p.navigate(&a(), "/tmp", true));
        let (g2, op2, _) = first(&p.answered(g, &op, &folder("/tmp", 1.0), 0));
        p.answered(g2, &op2, "[]", 0);
        let (_, op, _) = first(&p.go_up());
        assert_eq!(op, Op::Verify { host: a(), path: "/".into() });
        let mut r = shown(&[]);
        r.path = Some("/".into());
        assert!(r.go_up().is_empty());
        // Go home with no home is "/".
        let (_, op, _) = first(&r.go_home(None));
        assert_eq!(op, Op::Verify { host: a(), path: "/".into() });
        // Refresh asks for the current path without history.
        let n = r.history.len();
        let (_, op, _) = first(&r.refresh());
        assert_eq!(op, Op::Verify { host: a(), path: "/".into() });
        assert_eq!(r.history.len(), n);

        // Selecting a host whose default path's stat fails still lists it.
        let mut p = shown(&[]);
        let reqs = p.select_host(&b(), None, Some("/home/b".into()));
        let (g, op, _) = first(&reqs);
        assert_eq!(op, Op::Verify { host: b(), path: "/home/b".into() });
        let reqs = p.answered(g, &op, "", 2);
        assert_eq!(first(&reqs).1, Op::List { path: "/home/b".into() });
        assert_eq!(p.host, Some(b()));

        // An answer with an old generation changes nothing.
        let mut p = shown(&[]);
        let (g, op, _) = first(&p.navigate(&a(), "/old", true));
        p.navigate(&a(), "/new", true);
        assert!(p.answered(g, &op, &folder("/old", 1.0), 0).is_empty());
        assert_eq!(p.path.as_deref(), Some("/home/a"));

        // Searching returns to the regular listing on navigation.
        let mut p = shown(&[]);
        p.mode = Mode::Search { generation: 9, query: "q".into() };
        p.navigate(&a(), "/tmp", true);
        assert_eq!(p.mode, Mode::Regular);

        // The default path: the last one shown, then the saved one, then home, then "/".
        let p = shown(&[]);
        assert_eq!(p.default_path(&a(), Some("/saved".into()), Some("/home/a".into())), "/home/a");
        assert_eq!(p.default_path(&b(), Some("/saved".into()), Some("/home/b".into())), "/saved");
        assert_eq!(p.default_path(&b(), None, Some("/home/b".into())), "/home/b");
        assert_eq!(p.default_path(&b(), None, None), "/");
        // The saved path beats home; the last shown beats the saved path.
        let mut p = shown(&[]);
        let (g, op, _) = first(&p.navigate(&a(), "/tmp", true));
        let (g2, op2, _) = first(&p.answered(g, &op, &folder("/tmp", 1.0), 0));
        p.answered(g2, &op2, "[]", 0);
        assert_eq!(p.default_path(&a(), Some("/saved".into()), Some("/home/a".into())), "/tmp");
    }

    #[test]
    fn panel_listing_follows_iterm2() {
        let p = shown(&[file("/home/a/.b", 5, 1.0), file("/home/a/a", 30, 3.0), folder("/home/a/C", 2.0), file("/home/a/d", 10, 1.0)]);
        let names = |p: &Panel| p.visible().iter().map(|f| f.name()).collect::<Vec<_>>();
        assert_eq!(names(&p), ["a", "C", "d"]);
        let mut p = p;
        let reqs = p.toggle_hidden();
        assert_eq!(first(&reqs).1, Op::List { path: "/home/a".into() });
        assert_eq!(names(&p), [".b", "a", "C", "d"]);
        p.toggle_hidden();
        p.sort_by(Column::Size);
        assert_eq!(names(&p), ["C", "d", "a"]);
        p.sort_by(Column::Size);
        assert_eq!(names(&p), ["a", "d", "C"]);
        // Date sorts by ctime.
        p.sort_by(Column::Date);
        assert_eq!(names(&p), ["d", "C", "a"]);

        p.sort_by(Column::Name);
        p.click("/home/a/a", false, false);
        assert_eq!(p.chosen().iter().map(|f| f.name()).collect::<Vec<_>>(), ["a"]);
        p.click("/home/a/d", true, false);
        assert_eq!(p.chosen().len(), 2);
        p.click("/home/a/C", false, true);
        assert_eq!(p.chosen().iter().map(|f| f.name()).collect::<Vec<_>>(), ["C", "d"]);

        // The Open button (`FP:1969-1976`).
        let mut p = shown(&[file("/home/a/a", 1, 1.0), folder("/home/a/C", 1.0), file("/home/a/d", 1, 1.0)]);
        assert!(!p.open_enabled());
        p.click("/home/a/C", false, false);
        assert!(p.open_enabled(), "one folder");
        p.click("/home/a/a", false, false);
        p.click("/home/a/d", true, false);
        assert!(p.open_enabled(), "two files");
        p.click("/home/a/C", true, false);
        assert!(!p.open_enabled(), "a file and a folder");

        // Double-click: a folder navigates, a file opens.
        match p.double_click("/home/a/C") {
            DoubleClick::Navigate(r) => assert_eq!(first(&r).1, Op::Verify { host: a(), path: "/home/a/C".into() }),
            other => panic!("{other:?}"),
        }
        assert_eq!(p.double_click("/home/a/a"), DoubleClick::Open);

        // A listing error shows its text; an empty listing shows nothing.
        let mut p = Panel::new(vec![a()], None);
        let (g, op, _) = first(&p.start(&none, &home_a));
        let (g2, op2, _) = first(&p.answered(g, &op, &folder("/home/a", 1.0), 0));
        p.answered(g2, &op2, "", 1);
        assert_eq!(p.error.as_deref(), Some("File not found"));
        assert!(!p.loading && p.visible().is_empty());
        let mut p = shown(&[]);
        assert!(p.error.is_none() && p.visible().is_empty());
        // A reply nobody can decode is garbage.
        let (g, op, _) = first(&p.refresh());
        let (g2, op2, _) = first(&p.answered(g, &op, &folder("/home/a", 1.0), 0));
        p.answered(g2, &op2, "[", 0);
        assert_eq!(p.error.as_deref(), Some(ssh_files::GARBAGE));
    }

    #[test]
    fn panel_listing_is_capped() {
        let mut items = Vec::new();
        for i in 0..LIST_MAX + 5 {
            items.push(file(&format!("/home/a/f{i}"), 1, 1.0));
        }
        let p = shown(&items);
        assert_eq!(p.files.len(), LIST_MAX);
        assert_eq!(p.notice.as_deref(), Some(TRUNCATED));
    }

    #[test]
    fn expand_path_follows_iterm2() {
        let mut p = Panel::new(vec![a()], None);
        p.path = Some("/home/a".into());
        let h = Some("/home/a");
        assert_eq!(p.expand_path("~", h), "/home/a");
        assert_eq!(p.expand_path("~/x", h), "/home/a/x");
        assert_eq!(p.expand_path("x/../y", h), "/home/a/y");
        assert_eq!(p.expand_path("/etc/./ssh/", h), "/etc/ssh");
        assert_eq!(p.expand_path("~", None), "/");
        assert_eq!(p.expand_path("~bob", h), "/home/a/~bob");
    }

    #[test]
    fn go_to_folder_completions_follow_iterm2() {
        let home = Some("/home/a");
        let mut p = shown(&[]);
        // Not found: lists "/" and keeps folders whose lower-cased path starts with the input.
        let (g, op, sub) = first(&p.completions("/et", home));
        assert_eq!((op.clone(), sub), (Op::Complete { base: "/et".into(), list: String::new() }, FileSub::Stat { path: "/et".into() }));
        assert!(p.completing);
        let reqs = p.answered(g, &op, "", 2);
        let (_, lop, lsub) = first(&reqs);
        assert_eq!((lop.clone(), lsub), (Op::Complete { base: "/et".into(), list: "/".into() }, FileSub::Ls { path: "/".into(), sort: Sorting::ByName }));
        let root = list(&[folder("/etc", 1.0), folder("/Etc2", 1.0), folder("/usr", 1.0), file("/etcfile", 1, 1.0)]);
        assert!(p.answered(g, &lop, &root, 0).is_empty());
        assert_eq!(p.completion_list(), ["/etc", "/etc2"]);
        assert!(!p.completing);

        // Found as a folder: siblings case-sensitively, children on their lower-cased path.
        let mut p = shown(&[]);
        let (g, op, _) = first(&p.completions("/etc", home));
        let reqs = p.answered(g, &op, &folder("/etc", 1.0), 0);
        assert_eq!(reqs.len(), 2);
        let lists: Vec<Op> = reqs.iter().map(|r| first(std::slice::from_ref(r)).1).collect();
        assert_eq!(lists, [Op::Complete { base: "/etc".into(), list: "/".into() }, Op::Complete { base: "/etc".into(), list: "/etc".into() }]);
        assert!(p.completing);
        p.answered(g, &lists[0], &list(&[folder("/etc", 1.0), folder("/etc2", 1.0), folder("/Etc2", 1.0), folder("/var", 1.0)]), 0);
        assert!(p.completing, "one list is still out");
        p.answered(g, &lists[1], &list(&[folder("/etc/ssh", 1.0), folder("/etc/PAM", 1.0), file("/etc/passwd", 1, 1.0)]), 0);
        assert_eq!(p.completion_list(), ["/etc", "/etc/pam", "/etc/ssh", "/etc2"]);
        assert!(!p.completing);
        // A stale generation changes nothing.
        let old = first(&p.completions("/v", home));
        p.completions("/w", home);
        assert!(p.answered(old.0, &old.1, &folder("/v", 1.0), 0).is_empty());
        assert_eq!(folder_on("box"), "Folder on box");
    }

    #[test]
    fn panel_hosts_follow_iterm2() {
        let mut p = shown(&[]);
        assert_eq!(p.host, Some(a()));
        let reqs = p.hosts_changed(vec![b()], &|h| (*h == b()).then(|| "/home/b".to_string()));
        assert_eq!(first(&reqs).1, Op::Verify { host: b(), path: "/home/b".into() });
        assert_eq!(p.host, Some(b()));
        let reqs = p.hosts_changed(vec![], &|_| None);
        assert!(reqs.is_empty());
        assert_eq!((p.host.clone(), p.path.clone()), (None, None));
        // A host that stays changes nothing.
        let mut p = shown(&[]);
        assert!(p.hosts_changed(vec![a(), b()], &|_| None).is_empty());

        assert_eq!(saved_path_key(&a(), "me"), format!("SSHFilePanel.currentPath.{}", a().compact("me")));
        assert_eq!(connection_lost("box"), "The connection to box was lost.");
        assert_eq!((TITLE, BACK, FORWARD, SEARCH), ("Open", "Back", "Forward", "Search"));
        assert_eq!((USE_SYSTEM_PANEL, GO_TO_FOLDER), ("Use System Panel\u{2026}", "Go to Folder\u{2026}"));
        assert_eq!((GO_TO_HOME, GO_UP, REFRESH, SHOW_HIDDEN, CONNECTED_HOSTS), ("Go to Home", "Go Up", "Refresh", "Show Hidden Files", "Connected Hosts"));
        assert_eq!((COLUMN_NAME, COLUMN_DATE, COLUMN_SIZE, COLUMN_KIND), ("Name", "Date Created", "Size", "Kind"));
        assert_eq!((GO_TO_TITLE, GO_TO_INFO, GO), ("Go to the folder:", "Type a pathname or select from the pop-up menu", "Go"));
        assert_eq!((PATH_PLACEHOLDER, LOADING_COMPLETIONS), ("Enter path (e.g., /usr/local/bin)", "Loading completions\u{2026}"));
    }

    #[test]
    fn saved_path_key_is_guarded() {
        // P15: only an identity safe to name a setting, and short, gets a key.
        assert_eq!(saved_path_key_checked(&a(), "me"), Some(saved_path_key(&a(), "me")));
        let hop = |hostname: &str, user: Option<&str>| Identity { host: hostname.into(), hostname: hostname.into(), username: user.map(str::to_string), port: 22 };
        assert_eq!(saved_path_key_checked(&hop("-oProxyCommand=x", None), "me"), None);
        assert_eq!(saved_path_key_checked(&hop("h\nost", None), "me"), None);
        assert_eq!(saved_path_key_checked(&hop("h", Some("u\u{7}")), "me"), None);
        assert_eq!(saved_path_key_checked(&hop("h", Some("bad user")), "me"), None);
        assert_eq!(saved_path_key_checked(&hop("", None), "me"), None);
        let long = "h".repeat(SAVED_KEY_MAX);
        assert_eq!(saved_path_key_checked(&hop(&long, None), "me"), None, "the key is over 200 characters");
        let fits = "h".repeat(SAVED_KEY_MAX - "SSHFilePanel.currentPath.".len());
        assert!(saved_path_key_checked(&hop(&fits, None), "me").is_some());
        // At most 32 keys are kept: the oldest go.
        let mut order = Vec::new();
        for i in 0..SAVED_KEYS_MAX {
            assert!(remember_saved_key(&mut order, &format!("k{i}")).is_empty());
        }
        assert!(remember_saved_key(&mut order, "k3").is_empty(), "a key already kept moves nothing");
        assert_eq!(remember_saved_key(&mut order, "new"), ["k0"]);
        assert_eq!(order.len(), SAVED_KEYS_MAX);
        assert_eq!(order.last().map(String::as_str), Some("new"));
        assert_eq!(order.first().map(String::as_str), Some("k1"));
    }

    #[test]
    fn panel_search_follows_iterm2() {
        let mut p = shown(&[file("/home/a/x", 1, 1.0)]);
        let reqs = p.search("abc");
        let g = p.search_generation().expect("searching");
        assert_eq!(reqs, vec![Request::SearchStop { host: a() }, Request::SearchStart { generation: g, host: a(), query: "abc".into(), base: "/home/a".into() }]);
        assert_eq!(p.mode, Mode::Search { generation: g, query: "abc".into() });
        // The regular listing is gone while searching.
        assert!(p.visible().is_empty());
        assert_eq!(p.search_id(), None);
        p.search_started(g, "S1");
        assert_eq!(p.search_id(), Some("S1"));
        let r = |path: &str| RemoteFile::parse(&file(path, 1, 1.0)).unwrap();
        p.search_result(g, r("/home/a/abc1"));
        p.search_result(g, r("/home/a/sub/abc2"));
        assert_eq!(p.visible().len(), 2);
        // A result of an older search is dropped.
        p.search_result(g - 1, r("/home/a/old"));
        assert_eq!(p.visible().len(), 2);
        // A newer search drops the earlier one's results and its id.
        let reqs = p.search("abcd");
        let g2 = p.search_generation().unwrap();
        assert!(g2 > g);
        assert_eq!(reqs[0], Request::SearchStop { host: a() });
        assert!(p.visible().is_empty());
        assert_eq!(p.search_id(), None);
        p.search_started(g, "S-old");
        assert_eq!(p.search_id(), None, "an id for an old search is not the current one");
        p.search_result(g, r("/home/a/old2"));
        assert!(p.visible().is_empty());
        // An empty query returns to Regular and asks for the current listing again.
        let reqs = p.search("");
        assert_eq!(p.mode, Mode::Regular);
        assert_eq!(reqs[0], Request::SearchStop { host: a() });
        let (_, op, sub) = first(&reqs[1..]);
        assert_eq!((op, sub), (Op::List { path: "/home/a".into() }, FileSub::Ls { path: "/home/a".into(), sort: Sorting::ByName }));
        // Not searching and an empty query: nothing to do.
        assert!(p.search("").is_empty());
        // Navigating while searching stops the search.
        let mut p = shown(&[]);
        p.search("q");
        let reqs = p.navigate(&a(), "/tmp", true);
        assert_eq!(reqs[0], Request::SearchStop { host: a() });
        assert_eq!(p.mode, Mode::Regular);
        // A failed start ends the loading and says why.
        let mut p = shown(&[]);
        p.search("q");
        let g = p.search_generation().unwrap();
        p.search_failed(g, "File not found");
        assert!(!p.loading);
        assert_eq!(p.error.as_deref(), Some("File not found"));
    }

    #[test]
    fn panel_search_results_are_capped() {
        let mut p = shown(&[]);
        p.search("q");
        let g = p.search_generation().unwrap();
        let f = RemoteFile::parse(&file("/home/a/q", 1, 1.0)).unwrap();
        for i in 0..SEARCH_MAX + 50 {
            let mut f = f.clone();
            f.absolute_path = format!("/home/a/q{i}");
            p.search_result(g, f);
        }
        assert_eq!(p.visible().len(), SEARCH_MAX);
        assert_eq!(p.notice.as_deref(), Some(SEARCH_TRUNCATED));
    }
}
