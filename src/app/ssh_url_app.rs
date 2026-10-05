//! Opening an ssh: URL from outside or from a Win-click (m14e Task 3). The URL rule is
//! `ssh_url`; this is the app side: which profile a URL opens, the session copy it changes, the
//! Illegal Username warning, and the Win-click.

use super::*;

/// The profile and whether it came from a binding for a URL from outside (iTerm2's `getUrl`
/// falls back to the default profile). Only `ssh:` URLs (any case): None for anything else.
pub(super) fn url_plan(url: &str, bound: Option<&str>, default_guid: &str) -> Option<(String, bool)> {
    if !is_ssh(url) {
        return None;
    }
    Some(match bound {
        Some(g) => (g.to_string(), true),
        None => (default_guid.to_string(), false),
    })
}

/// A clicked link opens only when a profile is bound; an unbound one stays refused (Decision).
pub(super) fn url_plan_for_click(url: &str, bound: Option<&str>) -> Option<String> {
    url_plan(url, bound, "").filter(|(_, from_binding)| *from_binding).map(|(g, _)| g)
}

/// P4: a Win-click in a pane whose text a remote host may have written never opens an ssh: tab.
pub(super) fn click_plan(remote: bool, url: &str, bound: Option<&str>) -> Option<String> {
    if remote {
        return None;
    }
    url_plan_for_click(url, bound)
}

/// The profile keys a URL changes on `base`, as a session copy sets them (None: the profile is
/// used unchanged). The URL path is never a profile key: it is a remote folder, and the copy's
/// Working Directory feeds the local folder check (ruling r1, revised P9).
pub(super) fn session_values(base: &crate::profile::Profile, url: &str, s: &crate::ssh_url::Settings) -> Result<Option<Vec<(String, Option<toml::Value>)>>, crate::ssh_url::UrlError> {
    let Some(changed) = crate::ssh_url::profile_for_url(base, url, s)? else { return Ok(None) };
    let mut values: Vec<(String, Option<toml::Value>)> = changed.values.iter().filter(|(k, v)| base.values.get(*k) != Some(*v)).map(|(k, v)| (k.clone(), Some(v.clone()))).collect();
    // A link from a web page never types the person's Initial Text into the host it names
    // (iTerm2 blanks it for URL launches, `iTermController.m:2188`); an empty text stays unchanged.
    if !base.string("Initial Text").is_empty() {
        values.push(("Initial Text".to_string(), Some(toml::Value::String(String::new()))));
    }
    Ok(Some(values))
}

impl App {
    /// The profile `ssh` is bound to, when Tandem is the default handler (iTerm2's `profileForScheme`).
    pub(super) fn bound_ssh_profile(&self) -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        let table = self.globals.table(crate::url_schemes::HANDLERS_KEY);
        crate::url_schemes::profile_for_scheme(&table, crate::url_schemes::system().as_ref(), "ssh", &exe, &|g| self.store.get(g).is_some())
    }

    /// An ssh: URL for Tandem to open (iTerm2's `getUrl`): the bound profile, else the default.
    /// False when Tandem does not handle it (not ssh:).
    pub(super) fn open_url_in_tandem(&mut self, url: &str) -> bool {
        let default = self.store.default_profile().guid();
        let bound = self.bound_ssh_profile();
        let Some((guid, _)) = url_plan(url, bound.as_deref(), &default) else { return false };
        self.open_ssh_url(url, Some(guid))
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
        let before = (self.tabs.len(), self.pending_windows.len());
        let copy = values.map(|values| {
            let saved = self.store.shared_guid(&guid);
            let shared = self.store.profiles.iter().find(|p| p.guid() == saved).unwrap_or_else(|| self.store.default_profile()).clone();
            let fresh = uuid::Uuid::new_v4().to_string().to_uppercase();
            let divorced = crate::session_profile::divorce(&shared, fresh.clone());
            self.add_session_copy(divorced, &shared, &values, fresh)
        });
        self.new_tab(Some(copy.as_deref().unwrap_or(&guid)));
        let started = (self.tabs.len(), self.pending_windows.len()) != before;
        if let (false, Some(copy)) = (started, &copy) {
            self.forget_session_profile(copy);
        }
        started
    }

    /// An ssh: URL from outside (the pipe, or the command line once the window exists): the flood
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
        if self.open_url_in_tandem(url) {
            self.banners.set("ssh-url", text);
            self.bring_forward();
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

fn is_ssh(url: &str) -> bool {
    url.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("ssh"))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!prompted.contains("open_url_in_tandem") && !prompted.contains("ssh_url_app"), "P5: App::open_url is unchanged");
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use crate::profile::Profile;
    use crate::ssh_url::Settings;

    fn settings(integration: bool) -> Settings {
        Settings { integration, scheme_path: "ssh".into(), valid_user_chars: "_-+.".into(), support_path: true }
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
}
