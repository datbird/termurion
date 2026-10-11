//! The browser's history, bookmarks and local pages on the app side (milestone 13x, Task 7): the
//! database opened on first use, visits and titles recorded as pages load, the `iterm2-about:`
//! pages answered, and the messages they send back judged by `browser_pages::route` (the source
//! first, then the closed set). Web content is untrusted here: nothing a page says becomes a path,
//! a command or a navigation except through that one route.

use super::*;
use crate::browser::{self, USER_ID};
use crate::browser_db::Db;
use crate::browser_pages::{self, Reply};
use crate::webhost::Answer;
use crate::webhost_win;

/// Seconds since 1970 (the database's date column).
fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// A URL the history keeps: a web page (http or https). Tandem's own pages are not history
/// (`recordVisit` skips `about:`), and a `file:` path is not recorded.
fn recordable(url: &str) -> bool {
    matches!(crate::webhost::scheme_of(url).as_str(), "http" | "https")
}

impl App {
    /// The history and bookmarks database: Dev Null Mode's in-memory one, else the persistent one,
    /// opened on first use. When it cannot be opened (a banner says so once) history lives in
    /// memory for this run.
    pub(super) fn browser_db(&mut self, dev_null: bool) -> Option<&Db> {
        if dev_null {
            return Some(self.browser_db_mem.get_or_insert_with(Db::open_in_memory));
        }
        if self.browser_db.is_none() && !self.browser_db_failed {
            let opened = webhost_win::local_appdata().ok_or_else(|| "no local application data folder".to_string()).and_then(|base| Db::open(&crate::browser_db::path(&base, USER_ID)));
            match opened {
                Ok(db) => self.browser_db = Some(db),
                Err(_) => {
                    // The reason can name a path: the log gets only that it failed.
                    crate::rlog!("browser: the history database could not be opened");
                    self.browser_db_failed = true;
                    self.banners.set("browser", "The browser's history and bookmarks could not be opened; they are kept in memory for this run.");
                }
            }
        }
        if self.browser_db_failed {
            return Some(self.browser_db_mem.get_or_insert_with(Db::open_in_memory));
        }
        self.browser_db.as_ref()
    }

    pub(super) fn pane_dev_null(&self, pane: PaneId) -> bool {
        self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| io.dev_null)
    }

    /// A finished load: one history row for the page that is really showing (`didCommit`). The
    /// title goes in only when this page has sent one already; later titles update the rows.
    pub(super) fn browser_record_visit(&mut self, pane: PaneId) {
        let dev_null = self.pane_dev_null(pane);
        let Some(url) = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).filter(|u| recordable(u)) else { return };
        let (title, transition) = match self.browser_views.get_mut(&pane) {
            Some(bv) => (bv.last_title.clone(), bv.bar.transition.take().unwrap_or(browser::TRANSITION_OTHER)),
            None => return,
        };
        let session = pane.to_string();
        if let Some(db) = self.browser_db(dev_null) {
            db.record_visit(&url, title.as_deref(), Some(&session), None, transition, now());
        }
    }

    /// A page's title changed (`titleDidChange`): both tables learn it for the page's URL.
    pub(super) fn browser_record_title(&mut self, pane: PaneId, title: &str) {
        let dev_null = self.pane_dev_null(pane);
        let Some(url) = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).filter(|u| recordable(u)) else { return };
        if let Some(db) = self.browser_db(dev_null) {
            db.update_title(title, &url);
        }
    }

    /// Whether the pane's page is bookmarked, and whether it can be (a web page).
    pub(super) fn browser_bookmark_state(&mut self, pane: PaneId) -> (bool, bool) {
        let dev_null = self.pane_dev_null(pane);
        let Some(url) = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()) else { return (false, false) };
        if !recordable(&url) {
            return (false, false);
        }
        let marked = self.browser_db(dev_null).is_some_and(|db| db.is_bookmarked(&url));
        (marked, true)
    }

    /// Add Bookmark / Remove Bookmark for the pane's page (`BVC:985-1010`).
    pub(super) fn browser_toggle_bookmark(&mut self, pane: PaneId) {
        let dev_null = self.pane_dev_null(pane);
        let Some((url, title)) = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| (io.url.clone(), io.title.clone())) else { return };
        if !recordable(&url) || dev_null {
            return;
        }
        let title = self.browser_views.get(&pane).and_then(|b| b.last_title.clone()).unwrap_or(title);
        if let Some(db) = self.browser_db(false) {
            if db.is_bookmarked(&url) {
                db.remove_bookmark(&url);
            } else {
                db.add_bookmark(&url, Some(&title), now());
            }
        }
    }

    /// A request for an `iterm2-about:` URL: the page under the policy, or a 404 (P8).
    pub(super) fn browser_local_request(&mut self, pane: PaneId, token: u64, url: &str, context: i32, top_url: &str) {
        // Tandem's own scheme keeps its answer; every other request is the ad blocker's to judge.
        let scheme = crate::webhost::scheme_of(url);
        // A file or ssh page is answered by its own owner, under the sandbox policy (13x8).
        if matches!(scheme.as_str(), browser_pages::FILE_SCHEME | browser_pages::SSH_SCHEME) {
            self.browser_page_request(pane, token, url, context, top_url);
            return;
        }
        let source = if scheme == "iterm2-about" { self.browser_error_answer(pane, url, context).or_else(|| self.browser_source_answer(pane, url, context)).or_else(|| self.browser_welcome_answer(url, context)) } else { None };
        let answer: Answer = if let Some(source) = source {
            source
        } else if scheme == "iterm2-about" { browser_pages::answer(url, context, top_url) } else { self.adblock_answer(url, context, top_url) };
        // The main document the blocker refused: remembered, so its failed load shows Tandem's page.
        if matches!(answer, Answer::Block) && context == browser_pages::CONTEXT_DOCUMENT {
            if let Some(bv) = self.browser_views.get_mut(&pane) {
                bv.blocked_url = Some(url.to_string());
            }
        }
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.answer_request(token, answer);
        }
    }

    /// A message a page posted. It counts only when WebView2 reports an `iterm2-about:` source for
    /// it (`browser_pages::route` checks, never the JSON), only from the page whose handler it
    /// names, from a closed set with typed arguments; its replies run in the page that asked or
    /// load a URL the database already holds.
    pub(super) fn local_page_message(&mut self, pane: PaneId, source: &str, json: &str) {
        let dev_null = self.pane_dev_null(pane);
        let replies = match self.browser_db(dev_null) {
            Some(db) => browser_pages::route(db, json, source),
            None => return,
        };
        // The page's "revoked" confirmation follows its operation and runs only if that acted.
        let mut confirmed = true;
        for reply in replies {
            match reply {
                Reply::Script(_) if !confirmed => {}
                Reply::Script(js) => {
                    if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                        v.execute_script(&js, 0);
                    }
                }
                Reply::Navigate(url) => self.browser_navigate(pane, &url),
                Reply::Onboarding(action) => self.browser_onboarding_message(pane, source, action),
                Reply::Permission(op) => {
                    confirmed = self.permission_page_op(pane, dev_null, op);
                    if !confirmed {
                        self.banners.set("action", "Could not remove the saved permission.");
                    }
                }
            }
        }
    }

    /// What Open Quickly searches in the browser: bookmarks and visited sites (seen twice or more
    /// in 90 days) of the persistent database, only when the browser is allowed
    /// (`browserAllowedCheckingIfNot:NO`); Dev Null's memory is never searched.
    pub(super) fn quickly_browser_sources(&mut self, query: &str) -> Option<crate::open_quickly::BrowserSources> {
        if self.web_runtime() == crate::webhost::Runtime::Missing || !self.globals.bool("BrowserProfiles") {
            return None;
        }
        let text = crate::open_quickly::parse(query).1;
        // A person who never browsed has no database: searching must not make one.
        let exists = self.browser_db.is_some() || webhost_win::local_appdata().is_some_and(|base| crate::localfs::local_exists(&crate::browser_db::path(&base, USER_ID)));
        if !exists {
            return Some(crate::open_quickly::BrowserSources::default());
        }
        let db = self.browser_db(false)?;
        let bookmarks = db.search_bookmarks(&text, 0, 50).into_iter().map(|(url, title, _)| (title.unwrap_or_default(), url)).collect();
        let visited = db.visits_matching_since(&text, now() - 90 * 24 * 60 * 60, 2, 50).into_iter().map(|(url, title)| (title.unwrap_or_default(), url)).collect();
        Some(crate::open_quickly::BrowserSources { bookmarks, visited })
    }

    /// Opens a bookmark, a visited site or a URL from Open Quickly (`OQW:503-520`): in the current
    /// session when it is a browser, else in a new tab with a Browser profile. The URL is judged
    /// again here (a stored row never loads a scheme the browser would not).
    pub(super) fn quickly_open_url(&mut self, raw: &str) {
        let Some(url) = browser::normalize_url(raw).filter(|u| matches!(crate::webhost::scheme_of(u).as_str(), "http" | "https" | "iterm2-about")) else {
            return;
        };
        if let Some(pane) = self.focused_id().filter(|p| self.is_browser_pane(*p)) {
            self.browser_navigate(pane, &url);
            self.bring_forward();
            return;
        }
        let default = self.store.default_profile();
        let guid = if self.is_browser_profile(default) { Some(default.guid()) } else { self.store.profiles.iter().find(|p| self.is_browser_profile(p)).map(|p| p.guid()) };
        let Some(guid) = guid else {
            self.banners.set("browser", "There is no Browser profile: make one in Settings > Profiles (Profile type).");
            return;
        };
        self.browser_url_override = Some(url);
        self.new_tab(Some(&guid));
        self.bring_forward();
    }
}

#[cfg(test)]
mod tests {
    fn prod() -> &'static str {
        let src = include_str!("browser_db_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// A page's message reaches the database only through the one route, which checks the source
    /// first; this file never parses the page's JSON or reads a handler name itself.
    #[test]
    fn a_page_cannot_drive_a_local_pages_handler() {
        let p = prod();
        let f = &p[p.find("pub(super) fn local_page_message").unwrap()..p.find("pub(super) fn quickly_browser_sources").unwrap()];
        assert!(f.contains("browser_pages::route(db, json, source)"));
        assert!(!f.contains("serde_json") && !f.contains("\"handler\"") && !f.contains("clear_history") && !f.contains("delete_history"), "the route alone touches rows");
        // The browser pane's event handler passes the view's reported source, never anything from the JSON.
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        // 13x2: the router rebuilds `{handler, body}` from the routed parts and `local_page_ok` guards it.
        let scripts = include_str!("browser_scripts_app.rs");
        let scripts = &scripts[..scripts.find("#[cfg(test)]").unwrap()];
        assert!(!app.contains("self.local_page_message("), "browser_app hands messages to the router, never to a page handler");
        let router = &scripts[scripts.find("pub(super) fn browser_message").unwrap()..scripts.find("pub(super) fn browser_page_open").unwrap()];
        assert!(router.contains("local_page_ok(&r)") && router.contains("self.local_page_message(pane, &r.source, &json)"));
        assert!(app.contains("Event::Request { token, url, context, top_url, .. } => self.browser_local_request(pane, *token, url, *context, top_url)"));
    }

    /// Open Quickly activation re-judges the stored URL.
    #[test]
    fn open_quickly_urls_are_judged_again() {
        let p = prod();
        let f = &p[p.find("pub(super) fn quickly_open_url").unwrap()..p.find("#[cfg(test)]").unwrap_or(p.len())];
        assert!(f.contains("browser::normalize_url(raw)") && f.contains("\"http\" | \"https\" | \"iterm2-about\""));
        assert!(!f.contains("shell_open") && !f.contains("run_external"));
        let q = include_str!("open_quickly_app.rs");
        assert!(q.contains("ItemKind::Bookmark(u) | ItemKind::Visited(u) | ItemKind::Url(u) => self.quickly_open_url(u)"));
    }

    /// The history is not recorded for Tandem's own pages or file paths, and the database opens only
    /// from the judged local application data folder.
    #[test]
    fn history_records_web_pages_only() {
        let p = prod();
        assert!(p.contains("\"http\" | \"https\""));
        assert!(p.contains("webhost_win::local_appdata()") && p.contains("crate::browser_db::path(&base, USER_ID)"));
        assert!(!p.contains("std::env::var"), "the folder is the known folder, never a variable a parent can redirect");
    }
}

#[cfg(test)]
mod permissions_page_tests {
    fn prod() -> &'static str {
        let src = include_str!("browser_db_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// The page's actions go through the one write queue (`revoke_permission` and its siblings), and
    /// only from a pane whose main frame (WebView2's `Source`, never the message) is the page.
    #[test]
    fn permissions_page_actions_follow_iterm2() {
        let p = prod();
        let f = &p[p.find("pub(super) fn local_page_message").unwrap()..p.find("pub(super) fn quickly_browser_sources").unwrap()];
        assert!(f.contains("Reply::Permission(op)") && f.contains("confirmed = self.permission_page_op(pane, dev_null, op)") && f.contains("Reply::Script(_) if !confirmed"), "revoked is shown only when the operation acted");
        let perms = include_str!("browser_perms_app.rs");
        let perms = &perms[..perms.find("#[cfg(test)]").unwrap()];
        let g = &perms[perms.find("pub(super) fn permission_page_op").unwrap()..];
        let g = &g[..g.find("\n    }\n").unwrap()];
        assert!(g.contains("bp::page_message_allowed(&url, None)"), "the pane's own main-frame URL decides");
        assert!(g.contains("self.revoke_permission(dev_null, &origin, kind)") && g.contains("self.revoke_all_permissions(dev_null, &origin)"));
        assert!(g.contains("bp::clear_origins("), "clear all revokes each origin through the same queue");
        assert!(!g.contains("db.revoke") && !g.contains("set_permission_state"), "no write beside the queue");
        assert!(!g.contains("serde_json") && !g.contains("body"), "no value from the page's JSON");
    }
}
