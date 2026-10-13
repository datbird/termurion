//! Opening an ssh: URL from outside or from a Win-click (m14e Task 3). The URL rule is
//! `ssh_url`; this is the app side: which profile a URL opens, the session copy it changes, the
//! Illegal Username warning, and the Win-click.

use super::*;

/// The profile and whether it came from a binding for a URL from outside (iTerm2's `getUrl`
/// falls back to the default profile). Only `ssh:`, `telnet:` and `ftp:` URLs (any case): None for
/// anything else.
pub(super) fn url_plan(url: &str, bound: Option<&str>, default_guid: &str) -> Option<(String, bool)> {
    if crate::url_open::scheme_of(url).is_none() {
        return None;
    }
    Some(match bound {
        Some(g) => (g.to_string(), true),
        None => (default_guid.to_string(), false),
    })
}

/// Where an outside URL opens (`getUrl`, `AD:2311-2322`): a profile that has a hotkey window
/// opens it there when there is one fitting (shown, and a tab in it) or makes the window with the
/// URL as its first session; any other profile opens a tab in the current window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum UrlHome {
    Tab,
    HotkeyExisting,
    HotkeyNew,
}

pub(super) fn url_home(has_hotkey: bool, window_exists: bool) -> UrlHome {
    match (has_hotkey, window_exists) {
        (false, _) => UrlHome::Tab,
        (true, true) => UrlHome::HotkeyExisting,
        (true, false) => UrlHome::HotkeyNew,
    }
}

/// A clicked link opens only when a profile is bound; an unbound one stays refused (Decision).
/// Only ssh: links are clickable (14e); telnet: and ftp: arrive from outside.
pub(super) fn url_plan_for_click(url: &str, bound: Option<&str>) -> Option<String> {
    if !is_ssh(url) {
        return None;
    }
    url_plan(url, bound, "").filter(|(_, from_binding)| *from_binding).map(|(g, _)| g)
}

/// P4: a Win-click in a pane whose text a remote host may have written never opens an ssh: tab.
pub(super) fn click_plan(remote: bool, url: &str, bound: Option<&str>) -> Option<String> {
    if remote {
        return None;
    }
    url_plan_for_click(url, bound)
}

/// Whether hovering `url` shows a link (underline, hand): an ssh: URL only when the click would
/// open it, so hover agrees with the click (final wave ruling). Other URLs are not decided here.
pub(super) fn hover_open(remote: bool, url: &str, bound: Option<&str>) -> bool {
    !is_ssh(url) || click_plan(remote, url, bound).is_some()
}

/// The profile keys a URL changes on `base`, as a session copy sets them (None: the profile is
/// used unchanged). The URL path is never a profile key: it is a remote folder, and the copy's
/// Working Directory feeds the local folder check (ruling r1, revised P9).
pub(super) fn session_values(base: &crate::profile::Profile, url: &str, s: &crate::ssh_url::Settings) -> Result<Option<Vec<(String, Option<toml::Value>)>>, crate::ssh_url::UrlError> {
    let Some(changed) = crate::ssh_url::profile_for_url(base, url, s)? else {
        // A profile with a command of its own is used unchanged, but an ssh: launch still never
        // types its Initial Text (`iTermController.m:2188`).
        let blank = crate::url_open::scheme_of(url).is_some() && !base.string("Initial Text").is_empty();
        return Ok(blank.then(|| vec![("Initial Text".to_string(), Some(toml::Value::String(String::new())))]));
    };
    let mut values: Vec<(String, Option<toml::Value>)> = changed.values.iter().filter(|(k, v)| base.values.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), Some(v.clone()))).collect();
    // A link from a web page never types the person's Initial Text into the host it names
    // (iTerm2 blanks it for URL launches, `iTermController.m:2188`); an empty text stays unchanged.
    if !base.string("Initial Text").is_empty() {
        values.push(("Initial Text".to_string(), Some(toml::Value::String(String::new()))));
    }
    Ok(Some(values))
}

/// Whether a launch URL can open at all (a good host, a legal user name). A URL that fails here
/// must not queue a hotkey window's first session, nor raise an "opening" banner.
pub(super) fn url_accepted(base: &crate::profile::Profile, url: &str, s: &crate::ssh_url::Settings) -> bool {
    !matches!(session_values(base, url, s), Err(_))
}

/// Drops the launch URL kept for a hotkey window that was not made, so a later press of the
/// hotkey does not open it long after the launch.
pub(super) fn forget_hotkey_url(urls: &mut HashMap<String, String>, window: &super::windows_ui::NewWindow) {
    if let super::windows_ui::NewWindow::Hotkey(guid, _) = window {
        urls.remove(guid);
    }
}

impl App {
    /// The profile `ssh` is bound to, when Termurion is the default handler (iTerm2's `profileForScheme`).
    pub(super) fn bound_ssh_profile(&self) -> Option<String> {
        self.bound_profile("ssh")
    }

    /// The profile `scheme` (ssh, telnet or ftp) is bound to, when Termurion is its default handler.
    pub(super) fn bound_profile(&self, scheme: &str) -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        let table = self.globals.table(crate::url_schemes::HANDLERS_KEY);
        crate::url_schemes::profile_for_scheme(&table, crate::url_schemes::system().as_ref(), scheme, &exe, &|g| self.store.get(g).is_some())
    }

    /// The profile an ssh:, telnet: or ftp: URL opens in and where (iTerm2's `getUrl`): the scheme's
    /// bound profile, else the default; a profile with a hotkey window opens it there. None when
    /// Termurion does not handle the scheme.
    fn url_route(&self, url: &str) -> Option<(String, UrlHome)> {
        let default = self.store.default_profile().guid();
        let bound = crate::url_open::scheme_of(url).and_then(|scheme| self.bound_profile(scheme));
        let (guid, _) = url_plan(url, bound.as_deref(), &default)?;
        let has_hotkey = self.store.get(&guid).is_some_and(|p| crate::hotkey::Prefs::from_profile(p).has_hotkey);
        let home = url_home(has_hotkey, self.hotkey_window_of(&guid).is_some());
        Some((guid, home))
    }

    /// An ssh:, telnet: or ftp: URL for Termurion to open (iTerm2's `getUrl`, `AD:2311-2322`): in the
    /// profile's hotkey window when it has one (shown, with a tab in it; made, with the URL as
    /// its first session), else a tab. False when Termurion does not handle it (another scheme).
    pub(super) fn open_url_in_termurion(&mut self, url: &str) -> bool {
        let Some((guid, home)) = self.url_route(url) else { return false };
        match home {
            UrlHome::Tab => self.open_ssh_url(url, Some(guid)),
            UrlHome::HotkeyExisting => {
                self.show_hotkey_window(&guid, super::hotkey_ui::foreground());
                self.open_ssh_url(url, Some(guid))
            }
            UrlHome::HotkeyNew => {
                // A URL that would be refused (bad host, illegal user name) queues nothing: the dialog or
                // silence of `open_ssh_url`, no window, no banner.
                let base = self.profile(Some(&guid));
                if !url_accepted(&base, url, &crate::ssh_url::Settings::from_globals(&self.globals)) {
                    return self.open_ssh_url(url, Some(guid));
                }
                // The window is made on the next turn of the event loop and takes the URL as its first session.
                self.hotkey_urls.insert(guid.clone(), url.to_string());
                self.show_hotkey_window(&guid, super::hotkey_ui::foreground());
                true
            }
        }
    }

    /// Opens a tab for `url` in profile `guid` (None: the default). A profile the URL changes is
    /// divorced into a session copy, one per URL, forgotten again when no tab took it (P8); the
    /// shared profile is never changed. True when a tab started.
    pub(super) fn open_ssh_url(&mut self, url: &str, guid: Option<String>) -> bool {
        if self.gpu.is_none() {
            return false;
        }
        let guid = guid.unwrap_or_else(|| self.store.default_profile().guid());
        let base = self.profile(Some(&guid));
        let settings = crate::ssh_url::Settings::from_globals(&self.globals);
        let values = match session_values(&base, url, &settings) {
            Ok(v) => v,
            Err(crate::ssh_url::UrlError::IllegalUsername(user)) => {
                let text = crate::ssh_url::illegal_username(&super::files_ui::shown_name(&user));
                let ask = super::scripts_alerts::ask(crate::ssh_url::ILLEGAL_USERNAME_HEADING, &text, &[crate::api_auth::OK], None, crate::warnings::Kind::Persistent, super::scripts_alerts::Then::Nothing);
                self.ask_scripts(ask);
                return false;
            }
            // A bad host or no host opens nothing and shows nothing (iTerm2's; Decision).
            Err(_) => return false,
        };
        let before = (self.tabs.len(), self.pending_tabs());
        let copy = values.map(|values| {
            let saved = self.store.shared_guid(&guid);
            let shared = self.store.profiles.iter().find(|p| p.guid() == saved).unwrap_or_else(|| self.store.default_profile()).clone();
            let fresh = uuid::Uuid::new_v4().to_string().to_uppercase();
            let divorced = crate::session_profile::divorce(&shared, fresh.clone());
            self.add_session_copy(divorced, &shared, &values, fresh)
        });
        self.new_tab(Some(copy.as_deref().unwrap_or(&guid)));
        let started = (self.tabs.len(), self.pending_tabs()) != before;
        if let (false, Some(copy)) = (started, &copy) {
            self.forget_session_profile(copy);
        }
        started
    }

    /// An ssh:, telnet: or ftp: URL from outside (the pipe, or the command line once the window exists): the flood
    /// cap, then a tab, a banner naming the target (P13), and the window comes forward.
    pub(super) fn receive_url(&mut self, url: String) {
        if !crate::url_launch::url_ok(&url) {
            return;
        }
        let now = Instant::now();
        if !self.url_flood.admit(now) {
            return self.ignored_urls_banner();
        }
        if self.gpu.is_none() {
            if !crate::url_launch::queue_for_start(&mut self.pending_urls_at_start, &mut self.url_flood, url) {
                self.ignored_urls_banner();
            }
            return;
        }
        if let Some(w) = self.last_window {
            self.enter(w);
        }
        self.open_external_url(&url);
    }

    /// The URLs that waited for the first window (their cap was applied when they arrived).
    pub(super) fn open_pending_urls(&mut self) {
        for url in std::mem::take(&mut self.pending_urls_at_start) {
            self.open_external_url(&url);
        }
        self.ignored_urls_banner();
    }

    fn open_external_url(&mut self, url: &str) {
        let Some(text) = crate::url_launch::open_banner(url) else { return };
        // A hotkey window brings itself forward; the normal window must not cover it.
        let hotkey = self.url_route(url).is_some_and(|(_, home)| home != UrlHome::Tab);
        if self.open_url_in_termurion(url) {
            self.banners.set("ssh-url", text);
            if !hotkey {
                self.bring_forward();
            }
            self.redraw();
        }
    }

    fn ignored_urls_banner(&mut self) {
        let n = self.url_flood.ignored();
        if n > 0 {
            self.banners.set("ssh-url-ignored", crate::url_launch::ignored_banner(n));
            self.redraw();
        }
    }
}

pub(super) fn is_ssh(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("ssh"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_ssh_link_shows_as_a_link_only_when_the_click_would_open_it() {
        assert!(!hover_open(false, "ssh://h", None), "unbound: no underline, no hand");
        assert!(hover_open(false, "ssh://h", Some("G")), "bound: it opens a tab");
        assert!(!hover_open(true, "ssh://h", Some("G")), "a remote pane never opens an ssh tab (P4)");
        assert!(hover_open(true, "https://h", None), "other links are not decided here");
        assert!(hover_open(false, "SSH://h", Some("G")));
    }

    use super::*;

    /// TWEAKS 5 review: a refused URL queues nothing; a window that failed to be made forgets its URL.
    #[test]
    fn a_refused_url_and_an_unmade_window_leave_no_stale_url() {
        let base = crate::profile::Profile::default();
        let s = crate::ssh_url::Settings { integration: false, scheme_path: String::new(), valid_user_chars: String::new(), support_path: false, ftp_path: String::new(), telnet_path: String::new() };
        assert!(url_accepted(&base, "ssh://host.example", &s));
        assert!(!url_accepted(&base, "ssh://", &s), "no host");
        let mut urls = HashMap::from([("G".to_string(), "ssh://h".to_string()), ("H".to_string(), "ssh://i".to_string())]);
        forget_hotkey_url(&mut urls, &super::super::windows_ui::NewWindow::Hotkey("G".into(), None));
        assert!(!urls.contains_key("G") && urls.contains_key("H"));
        let src = include_str!("windows_ui.rs");
        let at = src.find("// A tab on its way to the new window stays").unwrap();
        assert!(src[at - 200..at].contains("forget_hotkey_url("));
        let open = &include_str!("ssh_url_app.rs")[..];
        assert!(open[open.find("UrlHome::HotkeyNew => {").unwrap()..].contains("url_accepted("));
    }

    /// TWEAKS 5: a launch URL for a profile with a hotkey window goes to that window.
    #[test]
    fn a_launch_url_prefers_the_hotkey_window() {
        assert_eq!(url_home(false, false), UrlHome::Tab);
        assert_eq!(url_home(false, true), UrlHome::Tab, "a stale window of a profile that lost its hotkey");
        assert_eq!(url_home(true, true), UrlHome::HotkeyExisting);
        assert_eq!(url_home(true, false), UrlHome::HotkeyNew);
    }

    /// The wiring: the route is decided in `open_url_in_termurion`, the hotkey window is shown, and
    /// the window made for it opens the URL as its first session.
    #[test]
    fn the_hotkey_route_is_wired() {
        let src = include_str!("ssh_url_app.rs");
        let open = &src[src.find("fn url_route").unwrap()..src.find("fn open_ssh_url").unwrap()];
        assert!(open.contains("url_home(") && open.contains("show_hotkey_window(") && open.contains("hotkey_urls"));
        let win = include_str!("windows_ui.rs");
        let made = &win[win.find("NewWindow::Hotkey(guid, previous) =>").unwrap()..win.find("NewWindow::HotkeyRestored(guid, w) =>").unwrap()];
        assert!(made.contains("hotkey_urls.remove(") && made.contains("open_ssh_url("));
    }

    #[test]
    fn url_open_follows_iterm2() {
        assert_eq!(url_plan("ssh://h", Some("G1"), "D"), Some(("G1".to_string(), true)));
        assert_eq!(url_plan("ssh://h", None, "D"), Some(("D".to_string(), false)));
        assert_eq!(url_plan("SSH://h", None, "D"), Some(("D".to_string(), false)));
        assert_eq!(url_plan("http://h", None, "D"), None);
        assert_eq!(url_plan_for_click("ssh://h", None), None);
        assert_eq!(url_plan_for_click("ssh://h", Some("G1")), Some("G1".to_string()));
        assert_eq!(url_plan_for_click("http://h", Some("G1")), None);
    }

    /// P4: a remote pane's link never opens locally, bound or not; and the hook sits in
    /// `open_target` after `click_route` and before `semantic::plan`.
    #[test]
    fn a_remote_pane_never_opens_an_ssh_link() {
        assert_eq!(click_plan(true, "ssh://h", Some("G1")), None);
        assert_eq!(click_plan(false, "ssh://h", Some("G1")), Some("G1".to_string()));
        assert_eq!(click_plan(false, "ssh://h", None), None);
        let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app").join("semantic_ui.rs")).unwrap();
        let body = &src[src.find("fn open_target").unwrap()..];
        let route = body.find("click_route(").unwrap();
        let hook = body.find("click_plan(self.remote_text(id)").expect("open_target asks click_plan with remote_text");
        let plan = body.find("semantic::plan(").unwrap();
        assert!(route < hook && hook < plan, "click_route, then the ssh hook, then semantic::plan");
        let prompted = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app").join("prompted_ui.rs")).unwrap();
        assert!(!prompted.contains("open_url_in_termurion") && !prompted.contains("ssh_url_app"), "P5: App::open_url is unchanged");
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::profile::Profile;
    use crate::ssh_url::Settings;

    fn settings(integration: bool) -> Settings {
        Settings { integration, scheme_path: "ssh".into(), valid_user_chars: "_-+.".into(), support_path: true, ftp_path: "ftp".into(), telnet_path: "telnet".into() }
    }

    fn get<'a>(v: &'a [(String, Option<toml::Value>)], k: &str) -> Option<&'a toml::Value> {
        v.iter().find(|(key, _)| key == k).and_then(|(_, v)| v.as_ref())
    }

    /// Final-review ruling: with integration on the URL path travels as iTerm2 sends it, the cd
    /// word in the command line (the conductor runs it with the other command words and a failed
    /// cd still ends in `exec $SHELL -l`); it is never a conductor `cd` command, which aborts
    /// the integration when it fails. It is also never a profile key.
    #[test]
    fn integration_sends_the_cd_path_only_as_the_cd_word() {
        let base = Profile::default();
        let on = session_values(&base, "ssh://u@h/srv/app", &settings(true)).unwrap().unwrap();
        assert_eq!(get(&on, "Custom Command").and_then(|v| v.as_str()), Some("SSH"));
        assert!(get(&on, "Custom Directory").is_none() && get(&on, "Working Directory").is_none());
        // The URL path is the cd word only (iTerm2's way, SL:617-624): no remote-folder override
        // exists, so no conductor `cd` command can come from a URL.
        let name = format!("start_remote_dir{}", "_override");
        assert!(!include_str!("mod.rs").contains(&name));
        let off = session_values(&base, "ssh://u@h/srv/app", &settings(false)).unwrap().unwrap();
        assert_eq!(get(&off, "Custom Command").and_then(|v| v.as_str()), Some("Yes"));
        assert!(get(&off, "Working Directory").is_none());
        assert!(session_values(&base, "ssh:h", &settings(true)).is_err(), "no host opens nothing");
    }

    /// The URLs a hostile page might send: ordinary, home-relative, Windows-looking, UNC-looking
    /// (raw and percent-encoded), with and without SSH integration.
    const PROBE_URLS: &[&str] = &[
        "ssh://u@h/srv/app",
        "ssh://h/~/w",
        "ssh://h/C:/Windows",
        "ssh://h//attacker.example/share",
        "ssh://h/%2F%2Fattacker.example/share",
        "ssh://h/%2F/attacker.example/share",
        "ssh://h/%5C%5Cattacker.example%5Cshare",
        "ssh://h//",
        "ssh://h",
    ];

    /// Ruling r1 (revised P9): a URL session's copy never reaches the local folder check, for
    /// every URL, with integration on or off.
    #[test]
    fn a_url_session_never_probes_a_local_folder() {
        for integration in [true, false] {
            for url in PROBE_URLS {
                let base = Profile::default();
                let Ok(Some(values)) = session_values(&base, url, &settings(integration)) else { continue };
                let mut copy = base.clone();
                for (k, v) in values {
                    if let Some(v) = v {
                        copy.set(&k, v);
                    }
                }
                let calls = std::cell::RefCell::new(Vec::new());
                let (_, warning) = crate::profile::start_dir_probing(&copy, None, &|d| {
                    calls.borrow_mut().push(d.to_path_buf());
                    false
                });
                assert!(calls.borrow().is_empty(), "{url} (integration {integration}) probed {:?}", calls.borrow());
                assert_eq!(warning, None, "{url}: no local start-directory banner");
            }
        }
    }

    /// B2: the copy a URL opens types no Initial Text, whether or not it changes anything else,
    /// and a profile with none gains no key.
    #[test]
    fn url_session_sends_no_initial_text() {
        for integration in [true, false] {
            for url in PROBE_URLS {
                let mut base = Profile::default();
                base.set("Initial Text", toml::Value::String("echo hi".to_string()));
                let Ok(Some(values)) = session_values(&base, url, &settings(integration)) else { continue };
                let mut copy = base.clone();
                for (k, v) in values {
                    if let Some(v) = v {
                        copy.set(&k, v);
                    }
                }
                assert_eq!(copy.string("Initial Text"), "", "{url} (integration {integration})");
                let plain = Profile::default();
                let Ok(Some(values)) = session_values(&plain, url, &settings(integration)) else { continue };
                assert!(values.iter().all(|(k, _)| k != "Initial Text"), "{url}: an empty text adds no key");
            }
        }
    }

    /// A path that starts `//` or `\\` after decoding drops the cd part in both modes.
    #[test]
    fn a_unc_looking_path_drops_the_cd_part() {
        for integration in [true, false] {
            for url in ["ssh://h//attacker.example/share", "ssh://h/%2F%2Fattacker.example/share", "ssh://h/%5C%5Cattacker.example%5Cshare"] {
                let parsed = crate::ssh_url::parse(url).unwrap();
                assert_eq!(crate::ssh_url::cd_path(&parsed, &settings(integration)), None, "{url}");
                let change = crate::ssh_url::ssh_change(&parsed, &settings(integration)).unwrap();
                assert!(!change.command.contains("attacker"), "{url}: {}", change.command);
            }
        }
    }

    /// P8: a copy no tab took is forgotten, and the copy helper is pane-free.
    #[test]
    fn a_copy_nothing_took_is_forgotten() {
        let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("app").join("ssh_url_app.rs")).unwrap();
        let body = &src[src.find("fn open_ssh_url").unwrap()..src.find("fn receive_url").unwrap()];
        assert!(body.contains("self.forget_session_profile(copy)") && body.contains("add_session_copy"));
        assert!(!body.contains("store.get_mut"), "the shared profile never changes");
    }

    /// iTerm2 blanks Initial Text for every ssh: launch, also when the bound profile has a
    /// command of its own and the link changes nothing else (`profile_for_url` leaves it unchanged).
    #[test]
    fn a_custom_command_profile_gets_no_initial_text_from_a_url() {
        let mut base = Profile::default();
        base.set("Custom Command", toml::Value::String("Yes".into()));
        base.set("Command", toml::Value::String("mytool".into()));
        base.set("Initial Text", toml::Value::String("echo hi".into()));
        let values = session_values(&base, "ssh://h/", &settings(true)).unwrap().expect("a copy with the text blanked");
        assert_eq!(values, vec![("Initial Text".to_string(), Some(toml::Value::String(String::new())))]);
        let mut plain = base.clone();
        plain.set("Initial Text", toml::Value::String(String::new()));
        assert_eq!(session_values(&plain, "ssh://h/", &settings(true)).unwrap(), None, "nothing to blank, the profile is used as it is");
        assert_eq!(session_values(&base, "https://example.com/", &settings(true)).unwrap(), None, "only ssh: links");
    }

    /// B4/P18: no URL text reaches Command (beyond the generated words), Name or Working
    /// Directory, and no URL session types the person's Initial Text, for every external scheme.
    #[test]
    fn url_sessions_never_substitute_or_type() {
        let hostile = [
            "ssh://h//attacker.example/share",
            "telnet://h:23/$$PATH$$",
            "ftp://h//attacker.example/share",
            "ftp://u:p@h/$$URL$$",
            "telnet://u@h",
            "ssh://u@h/~/w",
        ];
        for custom in [true, false] {
            for url in hostile {
                let mut base = Profile::default();
                base.set("Initial Text", toml::Value::String("echo hi".into()));
                base.set("Working Directory", toml::Value::String("$$PATH$$".into()));
                base.set("Name", toml::Value::String("$$HOST$$".into()));
                if custom {
                    base.set("Custom Command", toml::Value::String("Yes".into()));
                    base.set("Command", toml::Value::String("x $$URL$$".into()));
                }
                let values = session_values(&base, url, &settings(false)).unwrap().expect("a copy blanking the text");
                let mut copy = base.clone();
                for (k, v) in values {
                    if let Some(v) = v {
                        copy.set(&k, v);
                    }
                }
                assert_eq!(copy.string("Initial Text"), "", "{url} custom {custom}");
                assert_eq!(copy.string("Working Directory"), "$$PATH$$", "{url}: never substituted");
                assert_eq!(copy.string("Name"), "$$HOST$$", "{url}: never substituted");
                if custom {
                    assert_eq!(copy.string("Command"), "x $$URL$$", "{url}: a custom command is used unchanged");
                } else {
                    assert!(!copy.string("Command").contains("echo hi"), "{url}: {}", copy.string("Command"));
                }
                let calls = std::cell::RefCell::new(Vec::new());
                let _ = crate::profile::start_dir_probing(&copy, None, &|d| {
                    calls.borrow_mut().push(d.to_path_buf());
                    false
                });
                assert!(calls.borrow().iter().all(|d| !d.to_string_lossy().contains("attacker")), "{url}: {:?}", calls.borrow());
            }
        }
    }
}
