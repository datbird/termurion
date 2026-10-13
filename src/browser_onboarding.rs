//! The browser's onboarding (milestone 13x8 Task 4; `OH`, `WPH`, `PSB`): the two pages that change
//! settings (`onboarding-setup`, `onboarding-profile`), the welcome page's redirect, the closed set
//! of actions a page may ask for, the replies, and the profile rules. Pure: the App applies the
//! effects (`app::browser_pages_app`), and the page's message is accepted only through
//! `browser_pages::route` (the message's source is the check, never a secret in the JSON; ruling
//! R13x8-4).

use crate::browser;
use crate::profile::Profile;
use crate::store::Store;
use serde_json::Value;

pub const SETUP_URL: &str = "iterm2-about:onboarding-setup";
pub const PROFILE_URL: &str = "iterm2-about:onboarding-profile";
pub const INTRO_URL: &str = "iterm2-about:onboarding-intro";
/// The defaults key the last step sets (`OH:130`); it has no Settings row.
pub const COMPLETED_KEY: &str = "NoSyncBrowserOnboardingCompleted";

/// What a page can ask for (`OH:85-129`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    EnableAdBlocker,
    EnableInstantReplay,
    CreateBrowserProfile,
    CheckProfileExists,
    SwitchToProfile,
    GetSettingsStatus,
    CompleteOnboarding,
}

impl Action {
    /// The one page whose buttons send this action. A message from the other page is dropped.
    pub fn page(&self) -> &'static str {
        match self {
            Action::CreateBrowserProfile | Action::CheckProfileExists | Action::SwitchToProfile => "onboarding-profile",
            Action::EnableAdBlocker | Action::EnableInstantReplay | Action::GetSettingsStatus | Action::CompleteOnboarding => "onboarding-setup",
        }
    }

    /// Whether the action writes a profile (the three the page script could otherwise loop, P11).
    pub fn writes_profile(&self) -> bool {
        matches!(self, Action::CreateBrowserProfile | Action::EnableInstantReplay | Action::SwitchToProfile)
    }
}

/// The URL of a page of this module, exactly (no query, no fragment), or None.
pub fn page_url_of(url: &str) -> Option<&'static str> {
    if url == SETUP_URL {
        Some("onboarding-setup")
    } else if url == PROFILE_URL {
        Some("onboarding-profile")
    } else {
        None
    }
}

/// The action a message body names. iTerm2 refuses a message without its `sessionSecret` (`OH:77-82`);
/// Termurion keeps the field (the copied page sends it) but decides by the message's source, so only its
/// type is checked, here. A body with another key, or an unknown action, is None (`OH:132-133`).
pub fn parse(body: &Value) -> Option<Action> {
    let map = body.as_object()?;
    if !map.keys().all(|k| k == "action" || k == "sessionSecret") {
        return None;
    }
    match map.get("sessionSecret") {
        None | Some(Value::String(_)) => {}
        _ => return None,
    }
    Some(match map.get("action")?.as_str()? {
        "enableAdBlocker" => Action::EnableAdBlocker,
        "enableInstantReplay" => Action::EnableInstantReplay,
        "createBrowserProfile" => Action::CreateBrowserProfile,
        "checkProfileExists" => Action::CheckProfileExists,
        "switchToProfile" => Action::SwitchToProfile,
        "getSettingsStatus" => Action::GetSettingsStatus,
        "completeOnboarding" => Action::CompleteOnboarding,
        _ => return None,
    })
}

/// `updateStatus('<id>', <bool>);` (`OH:138`). The id is one of the page's own element ids; anything
/// else answers nothing, so no text but a fixed word and a bool reaches the page.
pub fn status_script(id: &str, enabled: bool) -> String {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_lowercase() || b == b'-') {
        return String::new();
    }
    format!("updateStatus('{id}', {enabled});")
}

pub fn created_script(created: bool) -> String {
    format!("onProfileCreated({created});")
}

pub fn initial_script(adblock: bool, replay: bool, profile_exists: bool) -> String {
    format!("updateInitialStatus({{\n    adBlockerEnabled: {adblock},\n    instantReplayEnabled: {replay},\n    profileExists: {profile_exists}\n}});")
}

/// The profile onboarding switches to (`PSB:251-271`): the session's own when it is a browser
/// profile, else its original's, else any browser profile. Each is (guid, is a browser profile).
pub fn find_browser_profile(current: Option<(&str, bool)>, original: Option<(&str, bool)>, all: &[(String, bool)]) -> Option<String> {
    if let Some((guid, true)) = current {
        return Some(guid.to_string());
    }
    if let Some((guid, true)) = original {
        return Some(guid.to_string());
    }
    all.iter().find(|(_, browser)| *browser).map(|(guid, _)| guid.clone())
}

/// What `iterm2-about:welcome` answers until onboarding is done (`WPH:47-60`).
pub const REDIRECT_HTML: &str = "<!DOCTYPE html>\n<html>\n<head>\n    <meta http-equiv=\"refresh\" content=\"0; url=iterm2-about:onboarding-intro\">\n</head>\n<body>\n    <script>window.location.href = \"iterm2-about:onboarding-intro\";</script>\n</body>\n</html>";

/// The redirect page while onboarding is not completed, None once it is (`WPH:45-73`).
pub fn welcome_redirect(completed: bool) -> Option<&'static str> {
    (!completed).then_some(REDIRECT_HTML)
}

/// The redirect as served: its one script carries the response's nonce (the policy forbids a bare
/// one); a text that does not hold exactly one is not served.
pub fn redirect_document(completed: bool, nonce: &str) -> Option<String> {
    let html = welcome_redirect(completed)?;
    (html.matches("<script>").count() == 1).then(|| html.replacen("<script>", &format!("<script nonce=\"{nonce}\">"), 1))
}

// --- The two copied pages, adapted at load time (R13x3-1/-2 pattern) ---

/// The setup page: Termurion's name in the title, the nonce on its script, each inline `onclick`
/// replaced by an attribute Termurion's delegated listener reads. (name, iTerm2's text, Termurion's text)
pub const SETUP_PATCHES: [(&str, &str, &str); 7] = [
    ("TITLE", "<title>iTerm2 Browser - Setup</title>", "<title>Termurion Browser - Setup</title>"),
    ("SCRIPT", "    <script>\n        const sessionSecret = '{{SECRET}}';", "    <script nonce=\"{{NONCE}}\">\n        const sessionSecret = '{{SECRET}}';"),
    ("TERMURION", "    </script>\n</head>", "    </script>\n    <script nonce=\"{{NONCE}}\">\n        {{TERMURION_SCRIPT}}\n    </script>\n</head>"),
    ("ADBLOCK", "onclick=\"enableAdBlocker()\"", "data-termurion-action=\"enableAdBlocker\""),
    ("REPLAY", "onclick=\"enableInstantReplay()\"", "data-termurion-action=\"enableInstantReplay\""),
    ("FINISH", "onclick=\"finishSetup()\"", "data-termurion-action=\"finishSetup\""),
    // Completion is host-driven: Termurion completes, then loads the welcome page, so a lost race between
    // the message and the page's own navigation cannot send the welcome redirect round again.
    ("FINISH_NAV", "            // Navigate to a blank welcome page\n            window.location.href = 'iterm2-about:welcome';\n", ""),
];

pub const PROFILE_PATCHES: [(&str, &str, &str); 6] = [
    ("TITLE", "<title>iTerm2 Browser - Create Profile</title>", "<title>Termurion Browser - Create Profile</title>"),
    ("SCRIPT", "    <script>\n        const sessionSecret = '{{SECRET}}';", "    <script nonce=\"{{NONCE}}\">\n        const sessionSecret = '{{SECRET}}';"),
    ("TERMURION", "    </script>\n</head>", "    </script>\n    <script nonce=\"{{NONCE}}\">\n        {{TERMURION_SCRIPT}}\n    </script>\n</head>"),
    ("CREATE", "onclick=\"createBrowserProfile()\"", "data-termurion-action=\"createBrowserProfile\""),
    ("CONTINUE", "onclick=\"continueToSetup()\"", "data-termurion-action=\"continueToSetup\""),
    // Host-driven too: the switch is handled, then Termurion loads the setup page.
    ("CONTINUE_NAV", "            // Navigate to setup page\n            window.location.href = 'iterm2-about:onboarding-setup';\n", ""),
];

/// Termurion's own script for both pages: one delegated listener. A click a script made up
/// (`isTrusted` false) does nothing, so a setting changes only by a click of the person on the page's
/// own button; the page's own functions then post the message.
pub const TERMURION_SCRIPT: &str = "document.addEventListener('click', function (e) {
            if (!e.isTrusted || !e.target || !e.target.closest) { return; }
            var el = e.target.closest('[data-termurion-action]');
            if (!el || el.disabled) { return; }
            switch (el.getAttribute('data-termurion-action')) {
            case 'enableAdBlocker': enableAdBlocker(); break;
            case 'enableInstantReplay': enableInstantReplay(); break;
            case 'finishSetup': finishSetup(); break;
            case 'createBrowserProfile': createBrowserProfile(); break;
            case 'continueToSetup': continueToSetup(); break;
            }
        });";

/// The page as served for `name` (`onboarding-setup` or `onboarding-profile`), or None: a patch
/// missed (fail closed), or another name.
pub fn document(name: &str, nonce: &str) -> Option<String> {
    use crate::browser_js::resource;
    let (file, patches): (&str, &[(&str, &str, &str)]) = match name {
        "onboarding-setup" => ("onboarding-setup.html", &SETUP_PATCHES),
        "onboarding-profile" => ("onboarding-profile.html", &PROFILE_PATCHES),
        _ => return None,
    };
    match crate::view_source::apply_patches(resource(file)?, patches) {
        // `SECRET` stays iTerm2's slot but holds nothing: the host decides by the source.
        Ok(html) => Some(crate::browser_pages::page(&html, &[("NONCE", nonce), ("SECRET", ""), ("TERMURION_SCRIPT", TERMURION_SCRIPT)])),
        Err(e) => {
            crate::rlog!("{}", crate::dlog::name(&e));
            None
        }
    }
}

// --- Profile rules (PSB:195-271), over the shared store ---

/// Every shared profile as (guid, is a browser profile).
pub fn profiles_of(store: &Store) -> Vec<(String, bool)> {
    store.profiles.iter().map(|p| (p.guid(), browser::is_browser(&p.string("Custom Command")))).collect()
}

pub fn has_browser_profile(store: &Store) -> bool {
    store.profiles.iter().any(|p| browser::is_browser(&p.string("Custom Command")))
}

/// Create Browser Profile (`PSB:216-233`): a profile named "Web Browser" whose type is Browser, only
/// when no browser profile exists. Its Guid when made.
pub fn create_browser_profile(store: &mut Store) -> Option<String> {
    if has_browser_profile(store) {
        return None;
    }
    let mut p = Profile::new(browser::DEFAULT_NAME);
    // The Settings window's Profile type pick (the one owner of what Browser means).
    let (value, automatic_icon) = browser::set_profile_type(1);
    p.set("Custom Command", toml::Value::String(value.to_string()));
    if automatic_icon {
        p.set("Icon", toml::Value::Integer(1));
    }
    let guid = p.guid();
    store.add(p);
    Some(guid)
}

/// Instant Replay on in the saved profile (`PSB:209-213`): the shared profile `guid` names.
pub fn enable_replay(store: &mut Store, shared: &str) -> bool {
    match store.get_mut(shared) {
        Some(p) => {
            p.set("Instant Replay", toml::Value::Boolean(true));
            true
        }
        None => false,
    }
}

/// Per-pane onboarding state (in the pane's `BrowserView`, dropped with its view).
pub struct State {
    /// The profile Create or Check found (`createdProfileGuid`, `OH:21`): Switch uses it.
    pub found: Option<String>,
    /// At most one profile write a second.
    limit: crate::cooldown::Cooldown<u8>,
}

impl Default for State {
    fn default() -> Self {
        State { found: None, limit: crate::cooldown::Cooldown::new(1000, 4) }
    }
}

impl State {
    /// Whether this action may act now: a profile write twice within a second does not (P11).
    pub fn allow(&mut self, action: &Action, now_ms: u64) -> bool {
        if !action.writes_profile() {
            return true;
        }
        // One window per action: a Switch right after a Create is not the loop the limit is for.
        let key = match action {
            Action::CreateBrowserProfile => 1,
            Action::SwitchToProfile => 2,
            _ => 3,
        };
        if self.limit.active(&key, now_ms) {
            return false;
        }
        self.limit.start(key, now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn act(name: &str) -> Option<Action> {
        parse(&json!({"action": name, "sessionSecret": "s"}))
    }

    #[test]
    fn onboarding_actions_follow_iterm2() {
        assert_eq!(act("enableAdBlocker"), Some(Action::EnableAdBlocker));
        for (name, want) in [
            ("enableInstantReplay", Action::EnableInstantReplay),
            ("createBrowserProfile", Action::CreateBrowserProfile),
            ("checkProfileExists", Action::CheckProfileExists),
            ("switchToProfile", Action::SwitchToProfile),
            ("getSettingsStatus", Action::GetSettingsStatus),
            ("completeOnboarding", Action::CompleteOnboarding),
        ] {
            assert_eq!(act(name), Some(want), "{name}");
        }
        assert_eq!(act("x"), None);
        assert_eq!(act("EnableAdBlocker"), None, "names are exact");
        assert_eq!(parse(&json!({"action": "enableAdBlocker", "sessionSecret": 5})), None, "the secret field is typed");
        assert_eq!(parse(&json!({"action": "enableAdBlocker", "sessionSecret": "s", "x": 1})), None, "a closed set of keys");
        assert_eq!(parse(&json!({"sessionSecret": "s"})), None);
        assert_eq!(parse(&json!(["enableAdBlocker"])), None);
        assert_eq!(status_script("adblocker-status", true), "updateStatus('adblocker-status', true);");
        assert_eq!(status_script("adblocker-status', 1); evil('", true), "", "an id is a fixed word");
        assert_eq!(created_script(false), "onProfileCreated(false);");
        let s = initial_script(true, false, true);
        assert!(s.contains("adBlockerEnabled: true") && s.contains("instantReplayEnabled: false") && s.contains("profileExists: true"));
        assert_eq!(find_browser_profile(Some(("A", true)), Some(("B", true)), &[]), Some("A".into()));
        assert_eq!(find_browser_profile(Some(("A", false)), Some(("B", true)), &[]), Some("B".into()));
        let all = [("C".to_string(), false), ("D".to_string(), true)];
        assert_eq!(find_browser_profile(Some(("A", false)), None, &all), Some("D".into()));
        assert_eq!(find_browser_profile(None, None, &[("C".to_string(), false)]), None);
        // Each action belongs to the page whose buttons send it.
        assert_eq!(Action::CreateBrowserProfile.page(), "onboarding-profile");
        assert_eq!(Action::EnableAdBlocker.page(), "onboarding-setup");
        assert_eq!(page_url_of(SETUP_URL), Some("onboarding-setup"));
        assert_eq!(page_url_of("iterm2-about:onboarding-setup#x"), None);
    }

    #[test]
    fn welcome_redirect_follows_iterm2() {
        let r = welcome_redirect(false).expect("redirects until completed");
        assert!(r.contains("<meta http-equiv=\"refresh\" content=\"0; url=iterm2-about:onboarding-intro\">"));
        assert!(r.contains("window.location.href = \"iterm2-about:onboarding-intro\";"));
        assert_eq!(welcome_redirect(true), None);
        let served = redirect_document(false, "n0nce").unwrap();
        assert!(served.contains("<script nonce=\"n0nce\">window.location.href") && !served.contains("<script>"), "the one script carries the nonce");
        assert_eq!(redirect_document(true, "n0nce"), None);
    }

    #[test]
    fn the_pages_are_patched_copies_under_the_nonce_policy() {
        for (name, file, patches) in [("onboarding-setup", "onboarding-setup.html", &SETUP_PATCHES[..]), ("onboarding-profile", "onboarding-profile.html", &PROFILE_PATCHES[..])] {
            let raw = crate::browser_js::resource(file).unwrap();
            for (n, target, _) in patches {
                assert_eq!(raw.matches(target).count(), 1, "{name} {n}");
                assert!(crate::view_source::apply_patches(&format!("{raw}{target}"), patches).is_err(), "a second copy of {n} is a miss");
                assert!(crate::view_source::apply_patches(&raw.replace(target, "x"), patches).is_err(), "a missing {n} serves no page");
            }
            let html = document(name, "n0nce").expect("served");
            assert!(!html.contains("onclick=") && !html.contains("{{"), "{name}: no inline handler, no unfilled slot");
            assert!(html.matches("<script").count() == 2 && html.matches("<script nonce=\"n0nce\">").count() == 2, "{name}: every script has the nonce");
            assert!(html.contains("const sessionSecret = '';") && html.contains("e.isTrusted"));
            assert!(html.contains("<title>Termurion Browser"));
            assert!(!html.contains("iTerm2"), "{name}: visible names are Termurion's");
        }
        assert!(!document("onboarding-setup", "n").unwrap().contains("window.location.href = 'iterm2-about:welcome'"), "Finish is host-driven");
        assert!(!document("onboarding-profile", "n").unwrap().contains("window.location.href = 'iterm2-about:onboarding-setup'"), "Continue is host-driven");
        assert_eq!(document("welcome", "n"), None);
        // The copies stay byte-identical to iTerm2's (the two tests above would fail on a drifted target).
        assert!(crate::browser_js::resource("onboarding-setup.html").unwrap().contains("onclick=\"enableAdBlocker()\""));
    }

    fn store() -> Store {
        let dir = std::env::temp_dir().join(format!("termurion-onb-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Store::load(&dir.join("profiles.toml")).unwrap()
    }

    #[test]
    fn profile_effects_follow_iterm2() {
        let mut st = store();
        assert!(!has_browser_profile(&st));
        let guid = create_browser_profile(&mut st).expect("made");
        let p = st.get(&guid).unwrap();
        assert_eq!((p.name().as_str(), p.string("Custom Command").as_str()), (browser::DEFAULT_NAME, browser::BROWSER_VALUE));
        assert!(has_browser_profile(&st));
        let n = st.profiles.len();
        assert_eq!(create_browser_profile(&mut st), None, "one browser profile is enough");
        assert_eq!(st.profiles.len(), n);
        assert!(profiles_of(&st).contains(&(guid.clone(), true)));
        assert!(enable_replay(&mut st, &guid) && st.get(&guid).unwrap().bool("Instant Replay"));
        assert!(!enable_replay(&mut st, "nope"));
    }

    #[test]
    fn profile_writes_are_rate_limited() {
        let mut s = State::default();
        assert!(s.allow(&Action::CreateBrowserProfile, 10_000));
        assert!(s.allow(&Action::SwitchToProfile, 10_500), "Switch right after Create is not dropped");
        assert!(!s.allow(&Action::CreateBrowserProfile, 10_600), "a second Create within a second");
        assert!(!s.allow(&Action::SwitchToProfile, 10_700), "a second Switch within a second");
        assert!(s.allow(&Action::EnableAdBlocker, 10_500), "a global is not a profile write");
        assert!(s.allow(&Action::EnableInstantReplay, 11_100));
        assert!(s.allow(&Action::CreateBrowserProfile, 11_100));
    }
}
