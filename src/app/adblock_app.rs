//! The App's side of ad blocking and the browser proxy (milestone 13x, Task 8). While
//! `WebKitAdblockEnabled` is on and a browser view exists: the saved list loads once, a download
//! runs when `adblock::needs_update` says so (checked hourly, as iTerm2's timer does, `ABM:73-85`),
//! and each request a browser view offers is judged by the in-memory matcher without waiting for
//! anything. Off: no filter on the views, no CSS, no download. The proxy is the three globals
//! turned into the one environment argument; a change rebuilds the browser views.
//! App-wide state only (never in `swap_live`).

use std::io::Write;
use std::sync::mpsc::{Receiver, Sender, channel};

use super::*;
use crate::adblock::{self, Rules};
use crate::ai_transport::{Call, HttpEvent, HttpRequest, Options, Proxy};
use crate::localfs::{self, LocalPath};
use crate::webhost::{Answer, Store};
use crate::webhost_win;

/// iTerm2's name for the saved list (`ABM:39`).
const FILE: &str = "adblock-compiled.json";
/// How often the schedule is looked at (iTerm2's timer, `ABM:73`).
const CHECK_EVERY: Duration = Duration::from_secs(3600);
/// How long a browser environment that failed after a proxy change is tried again: the old
/// browser process may still hold the folder for a moment.
const REBUILD_WINDOW: Duration = Duration::from_secs(20);
const REBUILD_RETRY: Duration = Duration::from_millis(400);

/// What a worker thread hands back.
enum Done {
    /// The saved list (None: there is none, or it is not usable).
    Disk(Option<Rules>),
    /// A downloaded list, compiled and saved, or why not.
    Fetched(Result<Rules, String>),
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Disk {
    #[default]
    Untried,
    Loading,
    Settled,
}

#[derive(Default)]
pub(super) struct Adblock {
    rules: Option<Arc<Rules>>,
    call: Option<Box<dyn Call>>,
    chan: Option<(Sender<Done>, Receiver<Done>)>,
    disk: Disk,
    /// A downloaded list is being compiled and saved.
    working: bool,
    next_check: Option<Instant>,
    /// The list address last looked at (None until the first look: not a change).
    url_seen: Option<String>,
    force: bool,
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// The folder the saved list lives in: `<LOCALAPPDATA>\Tandem`, judged (a cache, so machine-local
/// like the browser database, P10).
fn data_dir() -> std::io::Result<LocalPath> {
    let base = webhost_win::local_appdata().ok_or_else(localfs::refused)?;
    let dir = localfs::judge_io(base.join("Tandem"))?;
    localfs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Saves the list owner-only through a temporary file renamed over the old one (P14).
fn save_list(dir: &LocalPath, json: &str) -> std::io::Result<()> {
    let (tmp, file) = (dir.unfollowed(format!("{FILE}.tmp")), dir.unfollowed(FILE));
    let (Some(tmp), Some(file)) = (tmp, file) else { return Err(localfs::refused()) };
    let _ = localfs::remove_unfollowed(&tmp);
    let mut f = localfs::create_owner_only(&tmp)?;
    f.write_all(json.as_bytes())?;
    f.sync_all()?;
    drop(f);
    localfs::rename_unfollowed(&tmp, &file)
}

/// The saved list's text: a plain file no larger than the cap (it is bounded again when read).
fn load_list(dir: &LocalPath) -> Option<String> {
    let p = dir.child(FILE)?;
    let st = localfs::lstat(&p).ok()?;
    (st.is_file && !st.is_reparse && st.len <= adblock::MAX_LIST_BYTES as u64).then(|| localfs::read_to_string(&p).ok()).flatten()
}

impl App {
    pub(super) fn adblock_on(&self) -> bool {
        self.globals.bool("WebKitAdblockEnabled")
    }

    /// The one place the proxy argument is made: only from the three globals, only through
    /// `adblock::proxy_args` (P15).
    pub(super) fn browser_proxy_args(&self) -> String {
        let g = &self.globals;
        crate::adblock::proxy_args(g.bool("BrowserProxyEnabled"), &g.string("BrowserProxyHost"), g.int("BrowserProxyPort"))
    }

    /// `host:port` while the proxy is on (the toolbar indicator and its notice).
    pub(super) fn browser_proxy_label(&self) -> Option<String> {
        let args = self.browser_proxy_args();
        args.strip_prefix("--proxy-server=http://").map(str::to_string)
    }

    /// The toolbar's parts, with the proxy indicator while the proxy is on.
    pub(super) fn browser_geom(&self, bar: crate::layout::Rect, dev_null: bool) -> crate::browser::Geometry {
        crate::browser::geometry_with(bar, self.scale(), dev_null, self.browser_proxy_label().is_some())
    }

    /// The answer for one request of a browser page: blocked when the list says so, else on its way.
    /// In memory and bounded (`adblock::blocks` stops after 2 ms), so the UI thread never waits.
    pub(super) fn adblock_answer(&self, url: &str, context: i32, top_url: &str) -> Answer {
        match &self.adblock.rules {
            Some(r) if self.adblock_on() && adblock::blocks(r, url, adblock::resource_of(context), top_url) => Answer::Block,
            _ => Answer::Continue,
        }
    }

    /// A finished page: the list's element hiding CSS goes in (iTerm2's `adblock-css-injection.js`).
    pub(super) fn adblock_inject(&mut self, pane: PaneId) {
        let Some(rules) = self.adblock.rules.clone().filter(|_| self.adblock_on()) else { return };
        let Some(top) = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()) else { return };
        let css = adblock::hiding_css(&rules, &top);
        if css.is_empty() {
            return;
        }
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.execute_script(&adblock::injection_script(&css), 0);
        }
    }

    /// The schedule, the worker results and the rebuild retry. Returns the next time to look.
    pub(super) fn adblock_tick(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut due = None;
        if let Some(t) = self.web_envs.retry_at {
            if now >= t {
                self.web_envs.retry_at = None;
                self.redraw();
            } else {
                due = Some(t);
            }
        }
        if !self.adblock_on() {
            if self.adblock.rules.is_some() || self.adblock.call.is_some() || self.adblock.chan.is_some() || self.adblock.disk != Disk::Untried {
                self.adblock = Adblock::default();
                self.banners.clear("adblock");
            }
            return due;
        }
        self.adblock_poll();
        // Nothing is downloaded without a browser view (the list serves views).
        if self.browser_views.is_empty() {
            return due;
        }
        let url = self.globals.string("AdblockListURL");
        match &self.adblock.url_seen {
            None => self.adblock.url_seen = Some(url.clone()),
            Some(old) if *old != url => {
                self.adblock.url_seen = Some(url.clone());
                self.adblock.force = true;
                self.adblock.next_check = Some(now);
            }
            _ => {}
        }
        if self.adblock.disk == Disk::Untried {
            self.adblock.disk = Disk::Loading;
            let tx = self.adblock_sender();
            let notify = self.notify.clone();
            std::thread::spawn(move || {
                let rules = data_dir().ok().and_then(|d| load_list(&d)).and_then(|json| adblock::compile(&json).ok());
                let _ = tx.send(Done::Disk(rules));
                notify();
            });
            return due;
        }
        if self.adblock.disk != Disk::Settled || self.adblock.call.is_some() || self.adblock.working {
            return due;
        }
        let next = *self.adblock.next_check.get_or_insert(now);
        if now >= next {
            self.adblock.next_check = Some(now + CHECK_EVERY);
            let last = self.globals.raw(adblock::LAST_UPDATE_KEY).and_then(|v| v.as_integer());
            if std::mem::take(&mut self.adblock.force) || adblock::needs_update(last, unix_now(), self.adblock.rules.is_some()) {
                self.adblock_download(&url);
            }
        }
        let next = self.adblock.next_check.unwrap_or(now + CHECK_EVERY);
        Some(due.map_or(next, |d| d.min(next)))
    }

    fn adblock_sender(&mut self) -> Sender<Done> {
        self.adblock.chan.get_or_insert_with(channel).0.clone()
    }

    fn adblock_download(&mut self, url: &str) {
        if !adblock::list_url_ok(url) {
            crate::rlog!("adblock: the list address was refused");
            self.banners.set("adblock", adblock::invalid_url_text(&crate::inert_files::display_text(url, 120, false)));
            return;
        }
        crate::rlog!("adblock: downloading the list");
        let req = HttpRequest { method: "GET", url: url.to_string(), headers: Vec::new(), body: Vec::new(), stream: false };
        // The system proxy, not the browser's (Decision): the list is Tandem's own request.
        let options = Options { idle: Duration::from_secs(30), proxy: Proxy::System };
        self.adblock.call = Some(self.ai_transport.start(req, options, self.notify.clone()));
    }

    fn adblock_poll(&mut self) {
        if let Some(call) = self.adblock.call.as_mut() {
            match call.try_event() {
                Some(HttpEvent::Done { body, error }) => {
                    self.adblock.call = None;
                    if error.is_some() || body.len() > adblock::MAX_LIST_BYTES {
                        self.adblock_failed();
                    } else {
                        self.adblock.working = true;
                        let tx = self.adblock_sender();
                        let notify = self.notify.clone();
                        std::thread::spawn(move || {
                            let result = adblock::compile(&body).and_then(|rules| match data_dir().and_then(|d| save_list(&d, &body)) {
                                Ok(()) => Ok(rules),
                                Err(_) => Err("The adblock list could not be saved".to_string()),
                            });
                            let _ = tx.send(Done::Fetched(result));
                            notify();
                        });
                    }
                }
                Some(HttpEvent::Chunk(_)) | None => {}
            }
        }
        let results: Vec<Done> = self.adblock.chan.as_ref().map(|(_, rx)| rx.try_iter().collect()).unwrap_or_default();
        for done in results {
            match done {
                Done::Disk(rules) => {
                    self.adblock.disk = Disk::Settled;
                    if let Some(r) = rules {
                        crate::rlog!("adblock: saved rules loaded {}", r.len());
                        self.adblock.rules = Some(Arc::new(r));
                    }
                }
                Done::Fetched(Ok(r)) => {
                    self.adblock.working = false;
                    crate::rlog!("adblock: rules updated {}", r.len());
                    self.adblock.rules = Some(Arc::new(r));
                    self.globals.set(adblock::LAST_UPDATE_KEY, toml::Value::Integer(unix_now()));
                    let _ = self.globals.save();
                    self.banners.clear("adblock");
                }
                Done::Fetched(Err(_)) => {
                    self.adblock.working = false;
                    self.adblock_failed();
                }
            }
        }
    }

    /// An update failed: logged (no address, no reason text), and the stale banner after 7 days.
    fn adblock_failed(&mut self) {
        crate::rlog!("adblock: the update failed");
        let Some(last) = self.globals.raw(adblock::LAST_UPDATE_KEY).and_then(|v| v.as_integer()) else { return };
        if let Some(text) = adblock::stale_text((unix_now() - last).max(0) / 86_400) {
            self.banners.set("adblock", text);
        }
    }

    /// The proxy or ad blocking changed (`web_settings_changed`). WebView2 cannot change a proxy on
    /// a live environment nor add a request filter to a live view, so every browser view closes and
    /// comes back at its URL (the proxy also drops the browser environments).
    pub(super) fn browser_settings_changed(&mut self) {
        let proxy = self.browser_proxy_args();
        let ad = self.adblock_on();
        let proxy_changed = self.web_envs.proxy_seen.as_ref().is_some_and(|old| *old != proxy);
        let ad_changed = self.web_envs.adblock_seen.is_some_and(|old| old != ad);
        self.web_envs.proxy_seen = Some(proxy);
        self.web_envs.adblock_seen = Some(ad);
        if ad && ad_changed {
            self.adblock.next_check = Some(Instant::now());
        }
        if proxy_changed {
            // One notice per change, naming the address (P15): an import or a script can set it too.
            match self.browser_proxy_label() {
                Some(label) => self.banners.set("browser-proxy", format!("Browser proxy is on: {label}")),
                None => self.banners.clear("browser-proxy"),
            }
        }
        if proxy_changed || ad_changed {
            self.browser_views_reset(proxy_changed);
        }
    }

    /// Closes every browser view; each is made again at its page's URL. With `drop_envs` the browser
    /// environments go too and are made again with the new arguments.
    fn browser_views_reset(&mut self, drop_envs: bool) {
        let panes: Vec<PaneId> = self.browser_views.keys().copied().collect();
        for pane in panes {
            let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone());
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                bv.view = None;
                bv.pending_url = url;
                bv.retried = false;
                bv.settled = false;
            }
        }
        if drop_envs {
            self.web_envs.drop_browser_stores();
            self.web_envs.rebuild_until = Some(Instant::now() + REBUILD_WINDOW);
        }
        self.redraw();
    }

    /// A store's environment failed to start: right after a proxy change that is the old browser
    /// process still holding the folder, so it is tried again shortly.
    pub(super) fn web_env_failed(&mut self, store: Store) {
        if matches!(store, Store::Browser | Store::BrowserDevNull) && self.web_envs.rebuild_until.is_some_and(|t| Instant::now() < t) {
            self.web_envs.retry_at = Some(Instant::now() + REBUILD_RETRY);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> LocalPath {
        let p = std::env::temp_dir().join(format!("tandem-adblock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        localfs::judge(&p).expect("the temp folder is local")
    }

    #[test]
    fn the_saved_list_is_atomic_owner_only_and_bounded() {
        let d = dir("save");
        assert!(load_list(&d).is_none());
        save_list(&d, "[1]").unwrap();
        assert_eq!(load_list(&d).as_deref(), Some("[1]"));
        save_list(&d, "[2]").unwrap();
        assert_eq!(load_list(&d).as_deref(), Some("[2]"), "replaced");
        assert!(d.unfollowed(format!("{FILE}.tmp")).map(|t| !t.path().exists()).unwrap(), "no temporary file is left");
        let sddl = localfs::dacl_sddl(&d.unfollowed(FILE).unwrap()).unwrap();
        assert_eq!(sddl.matches("(A;").count(), 1, "{sddl}");
        // Over the cap is not read.
        std::fs::write(d.path().join(FILE), vec![b' '; adblock::MAX_LIST_BYTES + 1]).unwrap();
        assert!(load_list(&d).is_none());
        let _ = std::fs::remove_dir_all(d.path());
    }

    /// Off means off: the tick clears everything and starts nothing; the download needs a browser
    /// view; the proxy argument is made in one place from the three globals.
    #[test]
    fn ad_blocking_off_starts_nothing() {
        let src = include_str!("adblock_app.rs");
        let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let tick = &live[live.find("pub(super) fn adblock_tick").unwrap()..live.find("fn adblock_sender").unwrap()];
        assert!(tick.find("if !self.adblock_on()").unwrap() < tick.find("self.adblock_poll()").unwrap());
        assert!(tick.find("self.browser_views.is_empty()").unwrap() < tick.find("self.adblock_download(").unwrap());
        assert!(tick.find("self.browser_views.is_empty()").unwrap() < tick.find("Disk::Loading").unwrap(), "no read without a view");
        assert_eq!(live.matches("crate::adblock::proxy_args(").count(), 1);
        // The answer to a request needs no wait: it reads memory only.
        let answer = &live[live.find("pub(super) fn adblock_answer").unwrap()..live.find("pub(super) fn adblock_inject").unwrap()];
        assert!(!answer.contains("thread") && !answer.contains("recv") && !answer.contains("localfs"));
        // The list address is judged (https only) before any request.
        let dl = &live[live.find("fn adblock_download").unwrap()..live.find("fn adblock_poll").unwrap()];
        assert!(dl.find("list_url_ok").unwrap() < dl.find("self.ai_transport.start").unwrap());
    }

    /// A request for Tandem's own scheme keeps its answer first; everything else is the blocker's.
    #[test]
    fn local_pages_keep_their_answer() {
        let src = include_str!("browser_db_app.rs");
        let live = src.split("#[cfg(test)]\nmod tests").next().unwrap();
        let f = &live[live.find("fn browser_local_request").unwrap()..];
        assert!(f.contains("scheme_of(url) == \"iterm2-about\" { browser_pages::answer(url, context, top_url) } else { self.adblock_answer(url, context, top_url) }"));
    }
}
