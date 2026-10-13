//! File, ssh and error pages on the app side (milestone 13x8, Tasks 1 and 2): which page the main
//! frame is on, the answer to a request for an `iterm2-file:` or `iterm2-ssh:` URL, and the error
//! page of a failed load (its content is this pane's state, never a URL's). Every answer carries the
//! no-script policy (`browser_pages::file_headers`, R13x8-NS); a request that is not the pane's own
//! navigation or a subresource of its page's tree is a 403 with no text. Tasks 3 and 9 add the
//! arms that serve a file, a listing and a remote file; until then every allowed request is a 404.

use super::*;
use crate::browser_pages::{self, NOT_FOUND_HTML};
use crate::webhost::Answer;

/// Milliseconds on a clock that only moves forward (the error page's rate cap).
fn mono_ms() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
}

impl App {
    /// A main-frame navigation began and the one verdict let it go (`Event::Starting`): a file or
    /// ssh URL becomes the pending navigation (only its Document request is answered), any other URL
    /// clears it. The page root itself waits for the commit. Redirects into a local scheme never get
    /// this far (`webhost::navigation_allowed`).
    pub(super) fn browser_page_starting(&mut self, pane: PaneId, url: &str, nav: u64) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        bv.blocked_url = None;
        bv.nav_root = browser_pages::page_root(url);
        bv.nav_url = bv.nav_root.is_some().then(|| url.to_string());
        bv.nav_id = nav;
        // A new navigation that is not Tandem's error page ends the pending one (Task 2 review M1).
        if !url.eq_ignore_ascii_case(browser_pages::ERROR_URL) {
            bv.pending_error = None;
        }
    }

    /// A main-frame navigation committed (`Event::Committed`, the same event 13x7's NavTracker takes,
    /// after it). The root set by the Source change stays only when the navigation that committed is
    /// the file or ssh navigation that started last and the page is its URL (a reload of the same page
    /// changes no Source and keeps it); any other commit drops it. Fail closed in either event order:
    /// were the commit to come before the Source change, the Source change sets the root after it.
    pub(super) fn browser_page_nav_committed(&mut self, pane: PaneId, nav: u64) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if !page_root_holds(bv.nav_id, bv.nav_url.as_deref(), bv.page_url.as_deref(), nav) {
            bv.page_root = None;
            bv.page_url = None;
        }
        self.browser_local_root_sync(pane);
    }

    /// The tree the navigation guard lets a listing's links go to: the root the person opened the file
    /// page at. A page committed inside the old root keeps it; any other file page (one the person or
    /// Tandem opened, since a click out of the root was refused) starts a new one; leaving file pages
    /// ends it (13x8 Task 9 review I1).
    pub(super) fn browser_local_root_sync(&mut self, pane: PaneId) {
        let Some(bv) = self.browser_views.get(&pane) else { return };
        let Some(view) = bv.view.as_ref() else { return };
        let old = view.local_root();
        let next = match (&bv.page_root, &old) {
            (None, _) => None,
            (Some(new), Some(old)) if new.scheme == old.scheme && new.host == old.host && new.folder.to_lowercase().starts_with(&old.folder.to_lowercase()) => Some(old.clone()),
            (Some(new), _) => Some(new.clone()),
        };
        if next != old {
            view.set_local_root(next);
        }
    }

    /// The view's Source changed (a navigation committed): the page root is the pending navigation's
    /// when the Source is that URL, else none (a web page, or any other document, holds no root).
    pub(super) fn browser_page_committed(&mut self, pane: PaneId, source: &str) {
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        // A same-document move (`a.html#x`) changes the Source with no new navigation: the page is still
        // its pending URL, fragment aside (Task 3: file pages have anchors).
        let bare = |u: &str| u.split('#').next().unwrap_or("").to_string();
        if bv.nav_url.as_deref().is_some_and(|n| bare(n) == bare(source)) && bv.nav_root.is_some() {
            bv.page_root = bv.nav_root.clone();
            bv.page_url = bv.nav_url.clone();
        } else {
            bv.page_root = None;
            bv.page_url = None;
        }
        // Any other page committed: no later request takes a stale error page (Task 2 review M1).
        if !source.eq_ignore_ascii_case(browser_pages::ERROR_URL) {
            bv.pending_error = None;
        }
        self.browser_local_root_sync(pane);
    }

    /// A main-frame load failed (`BM:1329-1390`): the error page when this URL is not the one that
    /// failed last. A cancelled load shows nothing (`BM:1342-1345`), nor does a `file:` failure (Task 3's
    /// page), an `iterm2-file:` one (`BM:1382`; an `iterm2-ssh:` failure does get the page) or a failure of a
    /// local page. The page's text comes from `pending_error`, set here from the failure and the URL
    /// the NavigationStarting named; the load is Tandem's own (`navigate_termurion`), so it passes the one
    /// verdict as Tandem's load and its document request is the pane's.
    pub(super) fn browser_load_failed(&mut self, pane: PaneId, url: &str, status: i32, http: i32) {
        let failure = browser_pages::failure_of(status);
        let scheme = crate::webhost::scheme_of(url);
        if failure == browser_pages::Failure::Cancelled || url.is_empty() || matches!(scheme.as_str(), "file" | browser_pages::FILE_SCHEME | browser_pages::ABOUT) {
            return;
        }
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        // The main document the ad blocker refused (a synthetic 403): Tandem's page says so, not
        // Chromium's "HTTP ERROR 403" (merge review 2).
        let unfragmented = |u: &str| u.split('#').next().unwrap_or("").to_string();
        let blocked = bv.blocked_url.take().is_some_and(|b| unfragmented(&b) == unfragmented(url));
        // A server that answered (4xx, 5xx) shows its own page, and its answer ends the earlier failure
        // like a page that loaded (`BM:1290-1292`; merge review 1).
        if !blocked && browser_pages::server_answered(http) {
            bv.last_failed = None;
            bv.pending_error = None;
            return;
        }
        if !browser_pages::should_show_error(Some(url), bv.last_failed.as_deref()) || !bv.error_burst.allow(std::time::Instant::now()) {
            return;
        }
        let html = if blocked { browser_pages::described_error_page(browser_pages::BLOCKED_TEXT, Some(url)) } else { browser_pages::error_page(failure, Some(url)) };
        if html.is_empty() {
            return;
        }
        bv.last_failed = Some(url.to_string());
        bv.pending_error = Some(html);
        if let Some(v) = bv.view.as_ref() {
            v.navigate_termurion(browser_pages::ERROR_URL);
        }
    }

    /// A main-frame load finished: leaving the error page for a page that loaded ends the failure
    /// (`BM:1290-1292`); the error page's own load keeps it.
    pub(super) fn browser_load_succeeded(&mut self, pane: PaneId, url: &str) {
        if url.eq_ignore_ascii_case(browser_pages::ERROR_URL) {
            return;
        }
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.last_failed = None;
            bv.pending_error = None;
        }
    }

    /// The welcome page until onboarding is done (`WPH:45-73`): the redirect to the intro, under the
    /// same policy as every local page. None for any other URL, once onboarding is completed, and for
    /// a request that is not the document itself.
    pub(super) fn browser_welcome_answer(&mut self, url: &str, context: i32) -> Option<Answer> {
        if browser_pages::page_name(url) != Some("welcome") || context != browser_pages::CONTEXT_DOCUMENT {
            return None;
        }
        let nonce = browser_pages::new_nonce();
        let html = crate::browser_onboarding::redirect_document(self.globals.bool(crate::browser_onboarding::COMPLETED_KEY), &nonce)?;
        Some(Answer::Respond { status: 200, mime: "text/html; charset=utf-8".to_string(), body: html.into_bytes(), headers: browser_pages::response_headers(&nonce) })
    }

    /// An onboarding page's request (`OH:76-135`, `PSB:195-271`). It is already typed and belongs to
    /// the page that sent it (`browser_pages::route`); it acts only when the pane's own main-frame URL
    /// (WebView2's `Source`, kept in `BrowserIo.url`) is that page, so a web page, a frame or a page
    /// that navigated away changes nothing and is answered nothing. A setting changes through the
    /// owners the Settings window uses (`Globals::set` then `apply_edit`, `set_session_values`,
    /// `remarry`); the page's own click handler is trusted-only (`TERMURION_SCRIPT`). Nothing is logged.
    pub(super) fn browser_onboarding_message(&mut self, pane: PaneId, source: &str, action: crate::browser_onboarding::Action) {
        use crate::browser_onboarding as ob;
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).unwrap_or_default();
        let Some(page) = ob::page_url_of(&url).filter(|p| ob::page_url_of(source) == Some(*p) && action.page() == *p) else { return };
        if !self.browser_views.get_mut(&pane).is_some_and(|bv| bv.onboarding.allow(&action, mono_ms())) {
            return;
        }
        let mut calls: Vec<String> = Vec::new();
        match action {
            ob::Action::EnableAdBlocker => {
                self.globals.set("WebKitAdblockEnabled", toml::Value::Boolean(true));
                self.apply_edit(Edit::GlobalsChanged);
                calls.push(ob::status_script("adblocker-status", true));
            }
            ob::Action::EnableInstantReplay => {
                self.set_session_values(pane, &[("Instant Replay".to_string(), Some(toml::Value::Boolean(true)))]);
                let shared = self.panes.get(&pane).map(|p| self.store.shared_guid(&p.profile));
                if let Some(shared) = shared.filter(|g| ob::enable_replay(&mut self.store, g)) {
                    self.apply_edit(Edit::Changed(shared));
                }
                calls.push(ob::status_script("replay-status", true));
            }
            ob::Action::CreateBrowserProfile => match ob::create_browser_profile(&mut self.store) {
                Some(guid) => {
                    if let Some(bv) = self.browser_views.get_mut(&pane) {
                        bv.onboarding.found = Some(guid.clone());
                    }
                    self.apply_edit(Edit::Changed(guid));
                    calls.push(ob::created_script(true));
                }
                None => {
                    let found = self.onboarding_find_profile(pane);
                    if let Some(bv) = self.browser_views.get_mut(&pane) {
                        bv.onboarding.found = found;
                    }
                    calls.push(ob::created_script(false));
                }
            },
            ob::Action::CheckProfileExists => {
                if ob::has_browser_profile(&self.store) {
                    let found = self.onboarding_find_profile(pane);
                    if let Some(bv) = self.browser_views.get_mut(&pane) {
                        bv.onboarding.found = found;
                    }
                    calls.push(ob::created_script(false));
                }
            }
            ob::Action::SwitchToProfile => {
                let remembered = self.browser_views.get(&pane).and_then(|bv| bv.onboarding.found.clone());
                let guid = remembered.or_else(|| self.onboarding_find_profile(pane));
                if let Some(guid) = guid.filter(|g| self.store.get(g).is_some()) {
                    self.remarry(pane, &guid);
                }
                // The page no longer navigates on its own: Tandem loads the setup step once the switch is handled.
                self.browser_navigate(pane, ob::SETUP_URL);
            }
            ob::Action::GetSettingsStatus => {
                let replay = self.panes.get(&pane).and_then(|p| self.store.get(&p.profile)).is_some_and(|p| p.bool("Instant Replay"));
                calls.push(ob::initial_script(self.globals.bool("WebKitAdblockEnabled"), replay, ob::has_browser_profile(&self.store)));
            }
            ob::Action::CompleteOnboarding => {
                self.globals.set(ob::COMPLETED_KEY, toml::Value::Boolean(true));
                self.apply_edit(Edit::GlobalsChanged);
                // Completed first, then the welcome page loads (host-driven, so no lost race can loop the redirect).
                self.browser_navigate(pane, "iterm2-about:welcome");
            }
        }
        // Every answer runs in the page, guarded: it does nothing if the pane left that page.
        for call in calls.into_iter().filter(|c| !c.is_empty()) {
            for reply in browser_pages::script(page, &call) {
                if let (browser_pages::Reply::Script(js), Some(v)) = (reply, self.browser_views.get(&pane).and_then(|b| b.view.as_ref())) {
                    v.execute_script(&js, 0);
                }
            }
        }
    }

    /// The settings page (`SH:44-83`) as the pane's store shows it: the ad blocker's switch and list
    /// URL, whether this is a Dev Null pane. Served under the same nonce policy as every local page,
    /// only for the document itself.
    pub(super) fn browser_settings_answer(&mut self, pane: PaneId, url: &str, context: i32) -> Option<Answer> {
        if browser_pages::page_name(url) != Some(crate::browser_settings_page::PAGE) || context != browser_pages::CONTEXT_DOCUMENT {
            return None;
        }
        let state = crate::browser_settings_page::PageState { adblock_enabled: self.globals.bool("WebKitAdblockEnabled"), adblock_url: self.globals.string("AdblockListURL"), dev_null: self.pane_dev_null(pane), show_extensions: false };
        let nonce = browser_pages::new_nonce();
        let html = crate::browser_settings_page::document(&state, &nonce)?;
        Some(Answer::Respond { status: 200, mime: "text/html; charset=utf-8".to_string(), body: html.into_bytes(), headers: browser_pages::response_headers(&nonce) })
    }

    /// A settings page request (`SH:108-190`). It is already typed and bounded (`browser_pages::route`)
    /// and acts only when the pane's own main-frame URL is the settings page and so is the message's
    /// source, so a web page, a frame or a page that navigated away changes nothing and is answered
    /// nothing. Each action goes through the owner that already does it: the web view for clearing,
    /// `open_password_manager` (the page never lists or shows a password), the warning keys through
    /// `warnings::Store` for link opening. The page's JSON never names a settings key. The Get and
    /// The extension actions are the approved-folder flow of `browser_extensions_app` (Task 10).
    pub(super) fn browser_settings_message(&mut self, pane: PaneId, source: &str, msg: crate::browser_settings_page::Msg) {
        use crate::browser_settings_page as sp;
        use crate::warnings::Store as _;
        let url = self.panes.get(&pane).and_then(|p| p.browser_io()).map(|io| io.url.clone()).unwrap_or_default();
        if url != sp::SETTINGS_URL || source != sp::SETTINGS_URL {
            return;
        }
        if !self.browser_views.get_mut(&pane).is_some_and(|bv| bv.settings.allow(&msg, mono_ms())) {
            return;
        }
        let mut calls: Vec<String> = Vec::new();
        match msg {
            sp::Msg::ClearCookies => {
                if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
                    v.delete_all_cookies();
                }
            }
            // Native confirmation first: it wipes bookmarks too, so the page's own confirm() is not enough.
            sp::Msg::ClearAllData => {
                let asked = self.browser_views.get_mut(&pane).is_some_and(|bv| !std::mem::replace(&mut bv.settings.confirming, true));
                if asked {
                    let ask = super::scripts_alerts::ask(sp::CLEAR_HEADING, sp::CLEAR_TEXT, &[sp::CLEAR_BUTTON, crate::api_auth::CANCEL], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::BrowserClearData { pane });
                    self.ask_scripts(ask);
                }
            }
            sp::Msg::OpenPasswordManager => self.open_password_manager(None, Some(pane)),
            sp::Msg::SetLinkOpening(word) => match sp::link_pref_writes(&word) {
                Some((pref, writes)) => {
                    for (key, value) in writes {
                        match value {
                            Some(v) => self.globals.set(&key, v),
                            None => self.globals.remove(&key),
                        }
                    }
                    self.apply_edit(Edit::GlobalsChanged);
                    calls.push(sp::status_script(pref.text(), "success"));
                }
                None => calls.push(sp::status_script(sp::LINK_INVALID, "error")),
            },
            sp::Msg::GetLinkOpening => {
                let id = crate::browser::OPEN_LINKS_ID;
                let silenced = crate::warnings::is_silenced(&self.globals, id, super::warnings_app::unix_now());
                let selection = match self.globals.get(&crate::warnings::selection_key(id)) {
                    Some(toml::Value::Integer(n)) => Some(n),
                    _ => None,
                };
                calls.push(sp::link_ui_script(sp::link_pref_of(silenced, selection)));
            }
            sp::Msg::GetAdblockSettings => calls.push(sp::adblock_ui_script(self.globals.bool("WebKitAdblockEnabled"), &self.globals.string("AdblockListURL"))),
            // Answered from memory, or later from a worker (adblock_stats_request): never a disk read here.
            sp::Msg::GetAdblockStats => self.adblock_stats_request(),
            sp::Msg::SetAdblockEnabled(on) => {
                self.globals.set("WebKitAdblockEnabled", toml::Value::Boolean(on));
                self.apply_edit(Edit::GlobalsChanged);
                calls.push(sp::status_script(if on { sp::ADBLOCK_ON } else { sp::ADBLOCK_OFF }, "success"));
            }
            // An empty address changes nothing and answers nothing (`SH:246`).
            sp::Msg::SetAdblockUrl(url) if url.is_empty() => {}
            sp::Msg::SetAdblockUrl(url) => {
                if sp::filter_url_ok(&url) {
                    self.globals.set("AdblockListURL", toml::Value::String(url));
                    self.apply_edit(Edit::GlobalsChanged);
                    calls.push(sp::status_script(sp::FILTER_URL_UPDATED, "success"));
                } else {
                    calls.push(sp::status_script(sp::INVALID_FILTER_URL, "error"));
                }
            }
            sp::Msg::ForceAdblockUpdate => {
                if !self.adblock_refresh_now() {
                    calls.push(sp::status_script(&sp::rules_failed(sp::ADBLOCK_IS_OFF), "error"));
                }
            }
            sp::Msg::GetSearchSettings => {
                // Shown as used: a saved value that is not a usable template shows the default.
                let search = crate::browser::search_template(&self.globals.string("SearchCommand"));
                let suggest = self.globals.string("SearchSuggestURL");
                let suggest = if sp::suggest_url_ok(&suggest) { suggest } else { crate::browser::DEFAULT_SUGGEST.to_string() };
                calls.push(sp::search_ui_script(&search, &suggest));
            }
            sp::Msg::SetSearchCommand(url) => {
                if sp::search_command_ok(&url) {
                    self.globals.set("SearchCommand", toml::Value::String(url));
                    self.apply_edit(Edit::GlobalsChanged);
                    calls.push(sp::status_script(sp::SEARCH_UPDATED, "success"));
                } else {
                    calls.push(sp::status_script(sp::INVALID_SEARCH, "error"));
                }
            }
            sp::Msg::SetSearchSuggestUrl(url) => {
                if sp::suggest_url_ok(&url) {
                    let text = if url.is_empty() { sp::SUGGEST_DISABLED } else { sp::SUGGEST_UPDATED };
                    self.globals.set("SearchSuggestURL", toml::Value::String(url));
                    self.apply_edit(Edit::GlobalsChanged);
                    calls.push(sp::status_script(text, "success"));
                } else {
                    calls.push(sp::status_script(sp::INVALID_SUGGEST, "error"));
                }
            }
            sp::Msg::GetProxySettings => {
                let port = self.globals.int("BrowserProxyPort");
                calls.push(sp::proxy_ui_script(self.globals.bool("BrowserProxyEnabled"), &self.globals.string("BrowserProxyHost"), port));
            }
            sp::Msg::SetProxyEnabled(on) => {
                self.globals.set("BrowserProxyEnabled", toml::Value::Boolean(on));
                self.apply_edit(Edit::GlobalsChanged);
                calls.push(sp::status_script(if on { sp::PROXY_ON } else { sp::PROXY_OFF }, "success"));
            }
            // An empty host changes nothing and answers nothing (`SH:383`).
            sp::Msg::SetProxyHost(host) if host.is_empty() => {}
            sp::Msg::SetProxyHost(host) => {
                // The host is what `adblock::proxy_host_ok` accepts, nothing that is more than a host
                // (a space or `--switch` is refused); `proxy_args` judges it again before Chromium.
                if crate::adblock::proxy_host_ok(&host) {
                    self.globals.set("BrowserProxyHost", toml::Value::String(host));
                    self.apply_edit(Edit::GlobalsChanged);
                    calls.push(sp::status_script(sp::PROXY_HOST_UPDATED, "success"));
                } else {
                    calls.push(sp::status_script(sp::INVALID_PROXY_HOST, "error"));
                }
            }
            sp::Msg::SetProxyPort(port) => {
                if sp::port_ok(port) {
                    self.globals.set("BrowserProxyPort", toml::Value::Integer(port));
                    self.apply_edit(Edit::GlobalsChanged);
                    calls.push(sp::status_script(sp::PROXY_PORT_UPDATED, "success"));
                } else {
                    calls.push(sp::status_script(sp::INVALID_PORT, "error"));
                }
            }
            sp::Msg::GetExtensions => self.browser_extensions_request(pane, false, true),
            sp::Msg::SetExtensionEnabled { id, enabled } => calls.extend(self.browser_extension_set(pane, &id, enabled)),
            sp::Msg::RevealExtensionsDirectory => calls.extend(self.browser_extensions_reveal(pane)),
            sp::Msg::ChooseExtensionFolder => calls.extend(self.browser_extensions_choose(pane)),
        }
        self.browser_settings_run(pane, calls);
    }

    /// The rule statistics reach every pane showing the settings page.
    pub(super) fn browser_settings_stats(&mut self, rules: usize, last: f64) {
        use crate::browser_settings_page as sp;
        let text = sp::stats_script(rules, last);
        let panes: Vec<PaneId> = self.browser_views.keys().copied().filter(|p| self.panes.get(p).and_then(|x| x.browser_io()).is_some_and(|io| io.url == sp::SETTINGS_URL)).collect();
        for pane in panes {
            self.browser_settings_run(pane, vec![text.clone()]);
        }
    }

    /// Every browser pane showing the settings page is told how an update it asked for ended
    /// (`BM:1843-1848`). The text is Tandem's own or a bounded reason, never the server's.
    pub(super) fn browser_settings_adblock_result(&mut self, result: Result<(), String>) {
        use crate::browser_settings_page as sp;
        let text = match result {
            Ok(()) => sp::status_script(sp::RULES_UPDATED, "success"),
            Err(e) => sp::status_script(&sp::rules_failed(&crate::inert_files::display_text(&e, 200, false)), "error"),
        };
        let panes: Vec<PaneId> = self.browser_views.keys().copied().filter(|p| self.panes.get(p).and_then(|x| x.browser_io()).is_some_and(|io| io.url == sp::SETTINGS_URL)).collect();
        for pane in panes {
            self.browser_settings_run(pane, vec![text.clone()]);
        }
    }

    /// Runs reply calls in the settings page, guarded: nothing happens if the pane left that page.
    pub(super) fn browser_settings_run(&mut self, pane: PaneId, calls: Vec<String>) {
        for call in calls.into_iter().filter(|c| !c.is_empty()) {
            for reply in browser_pages::script(crate::browser_settings_page::PAGE, &call) {
                if let (browser_pages::Reply::Script(js), Some(v)) = (reply, self.browser_views.get(&pane).and_then(|b| b.view.as_ref())) {
                    v.execute_script(&js, 0);
                }
            }
        }
    }

    /// The answer to "Clear all website data?": only Clear (0), and only while the pane still shows
    /// the settings page, starts the clear (the data-cleared event then erases the database).
    pub(super) fn browser_clear_data_answered(&mut self, pane: PaneId, index: usize) {
        use crate::browser_settings_page as sp;
        if let Some(bv) = self.browser_views.get_mut(&pane) {
            bv.settings.confirming = false;
        }
        let on_page = self.panes.get(&pane).and_then(|p| p.browser_io()).is_some_and(|io| io.url == sp::SETTINGS_URL);
        if index != 0 || !on_page {
            return;
        }
        let started = self.browser_views.get_mut(&pane).is_some_and(|bv| match bv.view.as_ref() {
            Some(v) => {
                bv.settings.clearing = true;
                v.clear_browsing_data();
                true
            }
            None => false,
        });
        if !started {
            self.browser_settings_run(pane, vec![sp::alert_script(sp::DATA_CLEAR_FAILED)]);
        }
    }

    /// The web view finished Clear All Data (`Event::DataCleared`, `SH:204-227`). Only a clear the
    /// settings page asked for counts (the flag is set by that request and taken here). The database
    /// of the pane's own store is erased, then the page alerts the result.
    pub(super) fn browser_data_cleared(&mut self, pane: PaneId, ok: bool) {
        use crate::browser_settings_page as sp;
        if !self.browser_views.get_mut(&pane).is_some_and(|bv| std::mem::take(&mut bv.settings.clearing)) {
            return;
        }
        let message = if !ok {
            sp::DATA_CLEAR_FAILED.to_string()
        } else {
            let dev_null = self.pane_dev_null(pane);
            match self.browser_db_erase(dev_null) {
                Ok(()) => sp::DATA_CLEARED.to_string(),
                Err(path) => sp::db_delete_failed(path.as_deref()),
            }
        };
        self.browser_settings_run(pane, vec![sp::alert_script(&message)]);
    }

    /// The browser profile onboarding switches to (`PSB:251-271`).
    fn onboarding_find_profile(&self, pane: PaneId) -> Option<String> {
        let is_browser = |g: &str| self.store.get(g).is_some_and(|p| crate::browser::is_browser(&p.string("Custom Command")));
        let current = self.panes.get(&pane).map(|p| p.profile.clone());
        let original = current.as_deref().map(|g| self.store.shared_guid(g)).filter(|o| Some(o.as_str()) != current.as_deref());
        crate::browser_onboarding::find_browser_profile(current.as_deref().map(|g| (g, is_browser(g))), original.as_deref().map(|g| (g, is_browser(g))), &crate::browser_onboarding::profiles_of(&self.store))
    }

    /// The answer to a request for `iterm2-about:error`: None for any other URL. Only the document
    /// request gets a page, from the pane's own `pending_error` (taken once) or the generic page; a
    /// subresource or any other context is a 404. The URL carries nothing the page shows.
    pub(super) fn browser_error_answer(&mut self, pane: PaneId, url: &str, context: i32) -> Option<Answer> {
        if !url.eq_ignore_ascii_case(browser_pages::ERROR_URL) {
            return None;
        }
        let nonce = browser_pages::new_nonce();
        let html = if context == browser_pages::CONTEXT_DOCUMENT {
            let pending = self.browser_views.get_mut(&pane).and_then(|b| b.pending_error.take());
            Some(pending.filter(|h| !h.is_empty()).unwrap_or_else(browser_pages::default_error_page)).filter(|h| !h.is_empty())
        } else {
            None
        };
        let (status, mime, body) = match html {
            Some(h) => (200, "text/html; charset=utf-8", browser_pages::with_nonce(&h, &nonce)),
            None => (404, "text/plain; charset=utf-8", "Not found".to_string()),
        };
        Some(Answer::Respond { status, mime: mime.to_string(), body: body.into_bytes(), headers: browser_pages::response_headers(&nonce) })
    }

    /// The answer to a request for an `iterm2-file:` or `iterm2-ssh:` URL. A request that is not the
    /// pane's own navigation or a subresource of its page's tree is a 403 with no text; an allowed
    /// `iterm2-file:` request is read on a worker (`file_serve::Pool`) and answered from `web_tick`
    /// (`files_tick`), so the UI thread never touches a file; an allowed `iterm2-ssh:` request is a remote
    /// file over the person's own session (`ssh_page_request`). The refusal and every file answer carry the sandbox policy.
    pub(super) fn browser_page_request(&mut self, pane: PaneId, token: u64, url: &str, context: i32, top_url: &str) {
        let Some(bv) = self.browser_views.get(&pane) else { return };
        let pending = bv.nav_root.as_ref().zip(bv.nav_url.as_deref());
        let allowed = browser_pages::page_request_allowed(pending, bv.page_root.as_ref(), top_url, url, context);
        let file = crate::webhost::scheme_of(url) == browser_pages::FILE_SCHEME;
        let ssh = crate::webhost::scheme_of(url) == browser_pages::SSH_SCHEME;
        // A subresource is judged against the committed page's tree; the page itself against nothing.
        let root = if context == browser_pages::CONTEXT_DOCUMENT { None } else { bv.page_root.clone() };
        let status = if allowed { 404 } else { 403 };
        if allowed && file {
            let job = crate::file_serve::Job { pane, token, url: url.to_string(), context, root };
            let notify = self.notify.clone();
            if self.file_pool.get_or_insert_with(|| crate::file_serve::Pool::new(notify)).submit(job) {
                return;
            }
        }
        // A remote file over the person's own ssh session; answered when its transfer ends.
        if allowed && ssh {
            return self.ssh_page_request(pane, token, url, context == browser_pages::CONTEXT_DOCUMENT);
        }
        let answer = Answer::Respond { status, mime: "text/html; charset=utf-8".to_string(), body: NOT_FOUND_HTML.as_bytes().to_vec(), headers: browser_pages::file_headers() };
        if let Some(v) = self.browser_views.get(&pane).and_then(|b| b.view.as_ref()) {
            v.answer_request(token, answer);
        }
    }

    /// The answers the workers finished (`web_tick`): each goes to the view that asked, if it is
    /// still there and still waiting (a view that was made again ignores a token it never issued).
    pub(super) fn files_tick(&mut self) {
        let Some(pool) = self.file_pool.as_ref() else { return };
        for done in pool.finished() {
            if let Some(v) = self.browser_views.get(&done.pane).and_then(|b| b.view.as_ref()) {
                v.answer_request(done.token, done.answer);
            }
        }
    }

    /// A `file:` navigation the host cancelled (`Event::FileRedirect`, after the one verdict): the same
    /// path as an `iterm2-file:` page, loaded as Tandem's own navigation so the one verdict lets it
    /// through; a path the one decoder refuses shows "Invalid file path" (`LPM:190-195`).
    pub(super) fn browser_file_redirect(&mut self, pane: PaneId, uri: &str) {
        let target = crate::webhost::iterm2_file_url(uri).filter(|u| browser_pages::file_path_of(u).is_ok());
        let Some(bv) = self.browser_views.get_mut(&pane) else { return };
        if let Some(url) = target {
            bv.last_failed = None;
            if let Some(v) = bv.view.as_ref() {
                v.navigate_termurion(&url);
            }
            return;
        }
        if !bv.error_burst.allow(std::time::Instant::now()) {
            return;
        }
        let shown = crate::inert_files::display_text(&crate::semantic::file_url_path(uri).unwrap_or_else(|| uri.to_string()), 512, false);
        let html = browser_pages::described_error_page(&browser_pages::invalid_path_text(&shown), Some(uri));
        if html.is_empty() {
            return;
        }
        bv.last_failed = Some(uri.to_string());
        bv.pending_error = Some(html);
        if let Some(v) = bv.view.as_ref() {
            v.navigate_termurion(browser_pages::ERROR_URL);
        }
    }
}

/// Whether a commit of navigation `nav` keeps the page root: it is the pending file or ssh navigation
/// (`nav_id`, whose URL is `nav_url`) and the committed page is that URL.
fn page_root_holds(nav_id: u64, nav_url: Option<&str>, page_url: Option<&str>, nav: u64) -> bool {
    nav_id != 0 && nav_id == nav && nav_url.is_some() && nav_url == page_url
}

#[cfg(test)]
mod tests {
    /// The root survives only its own navigation's commit.
    #[test]
    fn a_commit_keeps_only_its_own_root() {
        let x = Some("iterm2-file:///C:/x/a.html");
        assert!(super::page_root_holds(7, x, x, 7), "the file page's own commit (a reload changes no Source)");
        assert!(!super::page_root_holds(7, x, x, 8), "another navigation's commit");
        assert!(!super::page_root_holds(7, x, None, 7), "the Source has not reached it: set later by the Source change");
        assert!(!super::page_root_holds(7, x, Some("iterm2-file:///C:/x/b.html"), 7));
        assert!(!super::page_root_holds(7, None, None, 7), "a web navigation holds no root");
        assert!(!super::page_root_holds(0, x, x, 0), "no navigation started");
    }

    /// 13x7 meets 13x8 on the commit: one Committed arm, after HTTP authentication's tracker (which sees
    /// every navigation fact first), keeps or drops the root; the start records the navigation's id
    /// after authentication saw the start; a reset clears the id.
    #[test]
    fn the_commit_order_is_authentication_then_the_page_root() {
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let ev = &app[app.find("pub(super) fn browser_event(").unwrap()..];
        let ev = &ev[..ev.find("\n    }\n").unwrap()];
        let auth = ev.find("self.basic_auth_event(pane, e);").unwrap();
        assert!(auth < ev.find("match e {").unwrap(), "authentication first, for every event");
        let arm = &ev[ev.find("Event::Committed { nav, .. } => {").unwrap()..];
        let arm = &arm[..arm.find("\n            }").unwrap()];
        assert!(arm.find("self.browser_page_nav_committed(pane, *nav);").unwrap() < arm.find("self.browser_script_event(pane, e);").unwrap());
        assert_eq!(ev.matches("Event::Committed").count(), 1, "one arm");
        assert!(ev.contains("self.browser_page_starting(pane, url, *nav);"));
        let auth_fn = include_str!("browser_basic_auth_app.rs");
        assert!(auth_fn.contains("Event::Committed { nav, .. } => self.basic_auth.get_mut(&pane).map(|a| a.committed(*nav)),"));
        let scripts = include_str!("browser_scripts_app.rs");
        let reset = &scripts[scripts.find("pub(super) fn browser_reset_world").unwrap()..scripts.find("// --- Setup ---").unwrap()];
        assert!(reset.contains("bv.nav_id = 0;") && reset.contains("bv.autofill.reset();") && reset.contains("bv.password.reset();"), "both milestones' per-view state goes with the world");
    }

    /// 13x7's scripts never act on a local page: the autofill detector and the password button return
    /// first on anything but a web page, and 13x8's wrap returns before either on a file or ssh page.
    #[test]
    fn no_autofill_or_password_script_on_a_local_page() {
        let d = crate::browser_autofill::detector_script("s").unwrap();
        assert!(d.contains("if (location.protocol !== 'https:' && location.protocol !== 'http:') return;"));
        let b = crate::browser_password::button_script("s").unwrap();
        assert!(b.contains("if (location.protocol !== 'https:') return;"));
        for (id, code, main) in [("autofillHandler", d, true), ("passwordManager", b, false)] {
            let w = crate::browser_js::wrap(&crate::browser_js::UserScript { id, code, inject: crate::browser_js::Inject::DocumentEnd, main_frame_only: main, world: crate::browser_js::World::Client });
            let file = w.find("if (location.protocol === 'iterm2-file:' || location.protocol === 'iterm2-ssh:') return;").unwrap();
            assert!(file < w.find("__termurionWebkit").unwrap() && file < w.find("location.protocol !== 'https:'").unwrap(), "{id}");
        }
        // A fill or a password write is refused on any page that is not https.
        for url in ["iterm2-file:///C:/x/a.html", "iterm2-ssh://box/f.html", "iterm2-about:error", "iterm2-about:welcome", "file:///C:/a.html"] {
            assert!(crate::browser_autofill::refusal(url).is_some(), "{url}");
            assert!(crate::browser_password::PwPage::new(url, 1).is_none(), "{url}");
        }
    }
    fn prod() -> &'static str {
        let src = include_str!("browser_pages_app.rs");
        &src[..src.find("#[cfg(test)]").unwrap()]
    }

    /// Every answer of a file or ssh request is built in one place with the sandbox headers, and the
    /// verdict is the pure rule's.
    #[test]
    fn file_and_ssh_answers_are_sandboxed() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_page_request").unwrap()..p.find("pub(super) fn files_tick").unwrap()];
        assert!(f.contains("browser_pages::page_request_allowed(pending, bv.page_root.as_ref(), top_url, url, context)"));
        assert_eq!(f.matches("Answer::Respond").count(), 1, "one place builds the answer");
        assert!(f.contains("headers: browser_pages::file_headers()"), "the refusal carries the no-script policy too (R13x8-NS)");
        // An allowed file request is read on a worker, never here, and only for the file scheme.
        assert!(f.contains("allowed && file") && f.contains("Pool::new(notify)") && f.contains(".submit(job)"));
        assert!(f.contains("let root = if context == browser_pages::CONTEXT_DOCUMENT { None } else { bv.page_root.clone() };"), "a subresource is judged against the committed tree");
        let db = include_str!("browser_db_app.rs");
        let db = &db[..db.find("#[cfg(test)]").unwrap()];
        let r = &db[db.find("fn browser_local_request").unwrap()..db.find("pub(super) fn local_page_message").unwrap()];
        assert!(r.contains("browser_pages::FILE_SCHEME | browser_pages::SSH_SCHEME") && r.contains("self.browser_page_request(pane, token, url, context, top_url);"));
        assert!(r.find("browser_page_request").unwrap() < r.find("adblock_answer").unwrap(), "the ad blocker never sees a file or ssh URL");
    }

    /// The page root is set from the one Starting arm, ahead of the other consumers, and a view's
    /// reset clears it; a file page's messages are dropped before any handler.
    #[test]
    fn the_page_root_follows_the_main_frame() {
        let app = include_str!("browser_app.rs");
        let app = &app[..app.find("#[cfg(test)]").unwrap()];
        let start = &app[app.find("Event::Starting { url, kind, user, redirect, nav, .. } => {").unwrap()..];
        assert!(start.find("self.browser_page_starting(pane, url, *nav)").unwrap() < start.find("self.marks_method_event(pane, e)").unwrap());
        let scripts = include_str!("browser_scripts_app.rs");
        let scripts = &scripts[..scripts.find("#[cfg(test)]").unwrap()];
        let reset = &scripts[scripts.find("pub(super) fn browser_reset_world").unwrap()..scripts.find("// --- Setup ---").unwrap()];
        assert!(reset.contains("bv.page_root = None") && reset.contains("bv.page_url = None"));
        let router = &scripts[scripts.find("pub(super) fn browser_message").unwrap()..];
        let router = &router[..router.find("match r.handler.as_str()").unwrap()];
        assert!(router.contains("b.page_root.is_some()") && router.contains("r.source") && router.contains("\"iterm2-file\" | \"iterm2-ssh\"") && router.contains("return;"), "dropped by pane state and by the message's own source");
        // The root is set on the commit (the Source event), never at the start (Task 1 review, I1).
        let start_fn = &prod()[prod().find("pub(super) fn browser_page_starting").unwrap()..prod().find("pub(super) fn browser_page_nav_committed").unwrap()];
        assert!(!start_fn.contains("page_root ="), "a start sets the pending navigation only");
        assert!(app.contains("self.browser_page_committed(pane, url);"));
        let p = prod();
        // A start or a commit of anything but the error page drops a pending error page (Task 2 review M1).
        for f in ["pub(super) fn browser_page_starting", "pub(super) fn browser_page_committed"] {
            let body = &p[p.find(f).unwrap()..];
            let body = &body[..body.find("\n    }\n").unwrap()];
            assert!(body.contains("eq_ignore_ascii_case(browser_pages::ERROR_URL)") && body.contains("bv.pending_error = None;"), "{f}");
        }
        assert!(!p.contains("std::fs") && !p.contains("localfs"), "this file reads no file; the serving arms live on a worker (file_serve)");
        // The file arm is answered from the tick, and the redirect loads Tandem's own page.
        let web = include_str!("webhost_app.rs");
        let tick = &web[web.find("pub(super) fn web_tick").unwrap()..web.find("fn web_focus_recheck").unwrap()];
        assert!(tick.contains("self.files_tick();"));
        assert!(app.contains("Event::FileRedirect { uri, .. } => self.browser_file_redirect(pane, uri),"));
        let r = &p[p.find("pub(super) fn browser_file_redirect").unwrap()..];
        let r = &r[..r.find("\n    }\n").unwrap()];
        assert!(r.contains("iterm2_file_url(uri)") && r.contains("file_path_of(u).is_ok()") && r.contains("navigate_termurion(&url)") && r.contains("invalid_path_text(&shown)") && r.contains("error_burst.allow("));
    }

    /// The error page's content is this pane's state: the answer is built in one place, takes
    /// `pending_error` once for the document request only, and reads nothing from the URL; a failure
    /// is shown from the one Navigated arm, a person's load and a good page clear it, a reset too.
    #[test]
    fn failures_reach_every_handler() {
        let p = prod();
        let a = &p[p.find("pub(super) fn browser_error_answer").unwrap()..p.find("/// The answer to a request for an `iterm2-file:`").unwrap()];
        assert!(a.contains("eq_ignore_ascii_case(browser_pages::ERROR_URL)") && a.contains("pending_error.take()") && a.contains("context == browser_pages::CONTEXT_DOCUMENT"));
        assert!(a.contains("browser_pages::with_nonce(&h, &nonce)") && a.contains("browser_pages::response_headers(&nonce)") && a.matches("Answer::Respond").count() == 1);
        assert!(!a.contains("url.split") && !a.contains("query"), "nothing is read from the URL");
        let f = &p[p.find("pub(super) fn browser_load_failed").unwrap()..p.find("pub(super) fn browser_load_succeeded").unwrap()];
        assert!(f.contains("browser_pages::server_answered(http)") && f.contains("failure == browser_pages::Failure::Cancelled") && f.contains("\"file\" | browser_pages::FILE_SCHEME | browser_pages::ABOUT"), "iterm2-ssh is not excluded");
        assert!(f.contains("!blocked && browser_pages::server_answered(http)") && f.contains("bv.last_failed = None;") && f.contains("bv.blocked_url.take()"), "a server answer ends the earlier failure; a blocked document shows Tandem's page");
        assert!(!f.contains("SSH_SCHEME"));
        assert!(f.contains("should_show_error(Some(url), bv.last_failed.as_deref())") && f.contains("error_burst.allow(") && f.contains("navigate_termurion(browser_pages::ERROR_URL)"));
        let app = include_str!("browser_app.rs");
        let prod_app = &app[..app.find("#[cfg(test)]").unwrap()];
        let arm = &prod_app[prod_app.find("Event::Navigated { ok, nav, .. } => {").unwrap()..prod_app.find("Event::History { back, forward, .. } =>").unwrap()];
        assert_eq!(arm.matches("self.browser_load_failed(pane, url, *status, *http)").count(), 1, "one call, in the one arm, after the other consumers");
        assert_eq!(arm.matches("self.browser_load_succeeded(pane, url)").count(), 1);
        assert!(arm.find("self.browser_marks_navigated(pane, *ok, *nav);").unwrap() < arm.find("self.browser_load_failed").unwrap());
        assert!(prod_app.contains("fail_cert_errors: true,"));
        let nav = &prod_app[prod_app.find("pub(super) fn browser_navigate(").unwrap()..prod_app.find("pub(super) fn browser_suggest").unwrap()];
        assert!(nav.contains("bv.last_failed = None;"), "a URL the person loads starts over");
        let scripts = include_str!("browser_scripts_app.rs");
        let reset = &scripts[scripts.find("pub(super) fn browser_reset_world").unwrap()..scripts.find("// --- Setup ---").unwrap()];
        assert!(reset.contains("bv.last_failed = None") && reset.contains("bv.pending_error = None") && reset.contains("bv.error_burst = Default::default()"));
        let db = include_str!("browser_db_app.rs");
        assert!(db.contains("self.browser_error_answer(pane, url, context).or_else(|| self.browser_source_answer(pane, url, context))"));
        let win = include_str!("../webhost_win.rs");
        let win = &win[..win.find("#[cfg(test)]\nmod tests").unwrap_or(win.len())];
        assert!(win.contains("COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL") && !win.contains("ALWAYS_ALLOW"), "a certificate error never proceeds silently");
        // M4: a runtime without the event returns the cast error (the view is not made) and the pane says why.
        let reg = &win[win.find("fn register_cert_cancel").unwrap()..];
        assert!(reg.contains("view.cast::<ICoreWebView2_14>()?;") && win.contains("e(webhost::OLD_RUNTIME_WHAT, x)"));
        assert!(prod_app.contains("why.starts_with(webhost::OLD_RUNTIME_WHAT)") && prod_app.contains("self.banners.set(\"webview2-old\", webhost::OLD_RUNTIME_TEXT);"));
        // The blocker's refusal of a main document is remembered for the error page.
        let dbs = include_str!("browser_db_app.rs");
        assert!(dbs.contains("bv.blocked_url = Some(url.to_string());"));
    }

    /// Onboarding changes a setting only through the existing owners, only from the pane's own main
    /// frame on that page, and every reply is a guarded script of typed text.
    #[test]
    fn onboarding_effects_follow_iterm2() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_onboarding_message").unwrap()..p.find("fn onboarding_find_profile").unwrap()];
        assert!(f.contains("ob::page_url_of(&url).filter(|p| ob::page_url_of(source) == Some(*p) && action.page() == *p)"), "the pane's own main-frame URL, the source and the action's page agree");
        assert!(f.contains("bv.onboarding.allow(&action, mono_ms())"));
        assert!(f.contains("self.globals.set(\"WebKitAdblockEnabled\"") && f.contains("self.apply_edit(Edit::GlobalsChanged)"));
        assert!(f.contains("self.set_session_values(pane,") && f.contains("ob::enable_replay(&mut self.store, g)") && f.contains("self.remarry(pane, &guid)"));
        assert!(f.contains("ob::create_browser_profile(&mut self.store)") && f.contains("self.apply_edit(Edit::Changed(guid))"));
        assert!(f.contains("ob::COMPLETED_KEY") && f.contains("browser_pages::script(page, &call)"));
        assert!(!f.contains("serde_json") && !f.contains("rlog!") && !f.contains("dlog!"), "no page value read, nothing logged");
        let w = &p[p.find("pub(super) fn browser_welcome_answer").unwrap()..p.find("/// An onboarding page's request").unwrap()];
        assert!(w.contains("context != browser_pages::CONTEXT_DOCUMENT") && w.contains("redirect_document(self.globals.bool("));
        let db = include_str!("browser_db_app.rs");
        assert!(db.contains(".or_else(|| self.browser_welcome_answer(url, context))") && db.contains("Reply::Onboarding(action) => self.browser_onboarding_message(pane, source, action)"));
        let scripts = include_str!("browser_scripts_app.rs");
        assert!(scripts.contains("bv.onboarding = Default::default();"));
    }

    /// The settings page acts only from its own main frame, through the existing owners, and nothing
    /// the page sends names a settings key (review M8, Task 5).
    #[test]
    fn settings_messages_need_the_page() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_settings_message").unwrap()..p.find("fn browser_settings_run").unwrap()];
        assert!(f.contains("if url != sp::SETTINGS_URL || source != sp::SETTINGS_URL {\n            return;"), "the pane's own main-frame URL and the source are both the settings page");
        assert!(f.find("source != sp::SETTINGS_URL").unwrap() < f.find("bv.settings.allow(&msg, mono_ms())").unwrap());
        assert!(f.find("bv.settings.allow(&msg, mono_ms())").unwrap() < f.find("v.delete_all_cookies()").unwrap(), "limited before any effect");
        assert!(f.contains("self.open_password_manager(None, Some(pane))"));
        assert!(!f.contains("clear_browsing_data"), "Clear All Data starts only from the native answer");
        assert!(f.contains("Then::BrowserClearData { pane }") && f.contains("settings.confirming, true"), "asked once at a time");
        let ca = &p[p.find("pub(super) fn browser_clear_data_answered").unwrap()..p.find("pub(super) fn browser_data_cleared").unwrap()];
        assert!(ca.contains("index != 0 || !on_page") && ca.contains("v.clear_browsing_data()") && ca.find("index != 0").unwrap() < ca.find("clear_browsing_data").unwrap());
        let alerts = include_str!("scripts_alerts.rs");
        assert!(alerts.contains("Then::BrowserPermission(_) | Then::BrowserClearData { .. }") && alerts.contains("Then::BrowserClearData { .. } => Answer::Button(1)"), "input guard, closed window is Cancel");
        assert!(f.contains("sp::link_pref_writes(&word)") && f.contains("self.globals.set(&key, v)") && f.contains("self.globals.remove(&key)"), "link opening writes only the keys link_pref_writes returns");
        // Task 6: the writes are a closed list of keys, each after its own check.
        for key in ["WebKitAdblockEnabled", "AdblockListURL", "SearchCommand", "SearchSuggestURL", "BrowserProxyEnabled", "BrowserProxyHost", "BrowserProxyPort"] {
            assert_eq!(f.matches(&format!("self.globals.set(\"{key}\"")).count(), 1, "{key}");
        }
        assert_eq!(f.matches("self.globals.set(").count(), 8, "seven named keys and the closed link keys, nothing else");
        assert!(f.contains("if sp::search_command_ok(&url)") && f.contains("if sp::suggest_url_ok(&url)") && f.contains("if sp::filter_url_ok(&url)") && f.contains("if sp::port_ok(port)") && f.contains("if crate::adblock::proxy_host_ok(&host)"), "each value is judged before it is written");
        assert!(f.contains("sp::Msg::SetAdblockUrl(url) if url.is_empty() => {}") && f.contains("sp::Msg::SetProxyHost(host) if host.is_empty() => {}"));
        assert!(f.contains("self.adblock_refresh_now()") && !f.contains("adblock_download") && !f.contains("compile("), "the update runs through the existing owner");
        assert!(!f.contains("BrowserProxyUser") && !f.contains("--proxy-server"), "no credential, no switch text here");
        assert!(!f.contains("serde_json") && !f.contains("rlog!") && !f.contains("dlog!") && !f.contains("password(") && !f.contains("secret"), "no page JSON read, nothing logged, no password handled here");
        assert!(!f.contains("permission"), "permissions have their one writer on their own page");
        let c = &p[p.find("pub(super) fn browser_data_cleared").unwrap()..p.find("/// The browser profile onboarding switches to").unwrap()];
        assert!(c.contains("std::mem::take(&mut bv.settings.clearing)"), "only a clear the settings page asked for");
        assert!(c.contains("self.browser_db_erase(dev_null)") && c.contains("self.pane_dev_null(pane)"), "the pane's own store");
        let a = &p[p.find("pub(super) fn browser_settings_answer").unwrap()..p.find("pub(super) fn browser_settings_message").unwrap()];
        assert!(a.contains("context != browser_pages::CONTEXT_DOCUMENT") && a.contains("browser_pages::response_headers(&nonce)"));
        let db = include_str!("browser_db_app.rs");
        assert!(db.contains(".or_else(|| self.browser_settings_answer(pane, url, context))") && db.contains("Reply::Settings(msg) => self.browser_settings_message(pane, source, msg)"));
        assert!(include_str!("browser_scripts_app.rs").contains("bv.settings = Default::default();"));
        assert!(include_str!("browser_app.rs").contains("Event::DataCleared { ok, .. } => self.browser_data_cleared(pane, *ok)"));
        // The completion and the switch are host-driven (Task 4 review).
        let o = &p[p.find("pub(super) fn browser_onboarding_message").unwrap()..p.find("pub(super) fn browser_settings_answer").unwrap()];
        assert!(o.contains("self.browser_navigate(pane, ob::SETUP_URL)") && o.contains("self.browser_navigate(pane, \"iterm2-about:welcome\")"));
    }

    /// Search, ad blocking and proxy: an invalid value writes nothing and answers an error; an empty
    /// list address or proxy host answers nothing; the proxy host never reaches a switch unjudged.
    #[test]
    fn settings_writes_follow_iterm2() {
        let p = prod();
        let f = &p[p.find("pub(super) fn browser_settings_message").unwrap()..p.find("fn browser_settings_run").unwrap()];
        let arm = |name: &str| {
            let a = &f[f.find(name).unwrap()..];
            a[..a.find("\n            sp::Msg::").or_else(|| a.find("\n            _ =>")).unwrap()].to_string()
        };
        let search = arm("sp::Msg::SetSearchCommand(url) =>");
        assert!(search.contains("sp::INVALID_SEARCH, \"error\"") && search.find("search_command_ok").unwrap() < search.find("self.globals.set(").unwrap());
        let host = arm("sp::Msg::SetProxyHost(host) =>");
        assert!(host.find("proxy_host_ok(&host)").unwrap() < host.find("self.globals.set(").unwrap() && host.contains("sp::INVALID_PROXY_HOST, \"error\""));
        let port = arm("sp::Msg::SetProxyPort(port) =>");
        assert!(port.contains("toml::Value::Integer(port)") && port.contains("sp::PROXY_PORT_UPDATED") && port.contains("sp::INVALID_PORT"));
        let on = arm("sp::Msg::SetAdblockEnabled(on) =>");
        assert!(on.contains("\"WebKitAdblockEnabled\"") && on.contains("sp::ADBLOCK_ON") && on.contains("\"success\""));
        // The update result reaches only a pane that shows the settings page, and only a requested one.
        let r = &p[p.find("pub(super) fn browser_settings_adblock_result").unwrap()..];
        assert!(r.contains("io.url == sp::SETTINGS_URL"));
        let ad = include_str!("adblock_app.rs");
        let ad = &ad[..ad.find("#[cfg(test)]").unwrap()];
        assert!(ad.contains("std::mem::take(&mut self.adblock.report)") && ad.matches("self.adblock_report(").count() >= 4, "every end of an update is reported once");
        assert!(ad.contains("self.adblock.next_check = Some(Instant::now());"), "Update Now does not wait for the hourly look");
    }
}
