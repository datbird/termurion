//! A browser pane's navigation transitions, the arrangement keys it adds, the page color and the
//! deferred load (milestone 13x2, Task 9). Pure: the app side is `app/browser_restore_app.rs`.
//!
//! The URL itself is 13x's (`browser::URL_KEY`, `browser::arrangement_url`, `browser_app::restored_url`);
//! this module adds only the transition vocabulary beside `browser::TRANSITION_*`, the link target key
//! and the page color. Everything read from an arrangement file or a page is bounded and cleaned here.

use crate::browser::{TRANSITION_BACK_FORWARD, TRANSITION_LINK, TRANSITION_OTHER, TRANSITION_REDIRECT, TRANSITION_RELOAD, TRANSITION_TYPED};
use std::time::{Duration, Instant};

/// `PS:334`: the key of a session's link target in an arrangement.
pub const TARGET_KEY: &str = "Browser Target";
/// The longest link target Tandem keeps (an arrangement file is another party's).
pub const MAX_TARGET: usize = 256;
/// `PS:5887-5891`: the page color is applied at most once in this long.
pub const COLOR_INTERVAL: Duration = Duration::from_millis(250);
/// The longest answer of `COLOR_SCRIPT` that is read.
const MAX_COLOR_TEXT: usize = 64;

/// The page's background color (Decision: WebView2 has no under-page color, so computed CSS).
pub const COLOR_SCRIPT: &str = "(function(){ const b = document.body; const c = b ? getComputedStyle(b).backgroundColor : ''; if (c && c !== 'rgba(0, 0, 0, 0)' && c !== 'transparent') return c; return getComputedStyle(document.documentElement).backgroundColor; })()";

/// `BrowserNavigationState` (`BNS:23-82`): the URL the app asked for and how the last navigation began.
#[derive(Default)]
pub struct NavState {
    pub last: Option<i64>,
    pub requested: Option<String>,
}

impl NavState {
    /// `willLoadURL`: Tandem is about to load `url` (a typed address, a restored page).
    pub fn will_load(&mut self, url: &str) {
        // A URL over the bound is never kept, so it is never matched as typed.
        self.requested = Some(url.to_string()).filter(|u| u.len() <= crate::browser::MAX_URL);
    }

    /// `willNavigate`, with WebView2's kinds (0 reload, 1 back or forward, 2 new document).
    pub fn starting(&mut self, url: &str, kind: i32, user: bool, redirect: bool) -> i64 {
        // WebView2 reports redirects; WebKit's navigation type has none (Decision: iTerm2's value 4).
        let t = if redirect {
            TRANSITION_REDIRECT
        } else {
            match kind {
                0 => TRANSITION_RELOAD,
                1 => TRANSITION_BACK_FORWARD,
                // A new document: a load Tandem started for the requested URL is typed (`BNS:73-76`,
                // WebView2's canonical form of the text allowed); then a person's click is a link;
                // anything else is other. The requested URL comes first: WebView2 reports
                // `IsUserInitiated` for every load started through its API (final review I1), where
                // WebKit never calls `loadURL` a link activation. WebView2 does not say a navigation
                // is a form post, so FormSubmit is never produced (Decision).
                _ if self.requested.as_deref().is_some_and(|r| crate::webhost::same_navigation_uri(r, url)) => TRANSITION_TYPED,
                _ if user => TRANSITION_LINK,
                _ => TRANSITION_OTHER,
            }
        };
        self.last = Some(t);
        t
    }
}

/// The CSS color of `COLOR_SCRIPT` as RGB: `rgb(...)` or `rgba(...)`, None when fully transparent or
/// anything else.
pub fn parse_color(css: &str) -> Option<(u8, u8, u8)> {
    let t = css.trim();
    if t.len() > MAX_COLOR_TEXT {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    let inner = lower.strip_prefix("rgba(").or_else(|| lower.strip_prefix("rgb("))?.strip_suffix(')')?;
    let (colors, alpha) = match inner.split_once('/') {
        Some((c, a)) => (c, Some(a)),
        None => (inner, None),
    };
    let mut parts: Vec<&str> = colors.split([',', ' ']).filter(|p| !p.is_empty()).collect();
    // The comma form carries its alpha as a fourth part.
    let alpha = match (alpha, parts.len()) {
        (Some(a), 3) => Some(a.trim()),
        (None, 4) => parts.pop(),
        (None, 3) => None,
        _ => return None,
    };
    if let Some(a) = alpha {
        let v: f64 = match a.strip_suffix('%') {
            Some(p) => p.trim().parse::<f64>().ok()? / 100.0,
            None => a.parse().ok()?,
        };
        if !v.is_finite() || v <= 0.0 {
            return None;
        }
    }
    let mut rgb = [0u8; 3];
    for (slot, p) in rgb.iter_mut().zip(&parts) {
        let v: f64 = p.parse().ok()?;
        if !v.is_finite() || !(0.0..=255.0).contains(&v) {
            return None;
        }
        *slot = v.round() as u8;
    }
    Some((rgb[0], rgb[1], rgb[2]))
}

/// What the page color does at most once per `COLOR_INTERVAL`: the first value applies at once, later
/// ones wait and the latest wins (`iTermRateLimitedUpdate`).
#[derive(Default)]
pub struct ColorLimit {
    last: Option<Instant>,
    pending: Option<Option<(u8, u8, u8)>>,
}

impl ColorLimit {
    /// A new reading. Some(value) is to be applied now.
    pub fn offer(&mut self, c: Option<(u8, u8, u8)>, now: Instant) -> Option<Option<(u8, u8, u8)>> {
        if self.pending.is_none() && self.last.is_none_or(|t| now.saturating_duration_since(t) >= COLOR_INTERVAL) {
            self.last = Some(now);
            return Some(c);
        }
        self.pending = Some(c);
        None
    }

    /// The waiting reading, once the interval is over.
    pub fn due(&mut self, now: Instant) -> Option<Option<(u8, u8, u8)>> {
        if self.pending.is_some() && self.last.is_none_or(|t| now.saturating_duration_since(t) >= COLOR_INTERVAL) {
            self.last = Some(now);
            return self.pending.take();
        }
        None
    }

    pub fn waiting(&self) -> bool {
        self.pending.is_some()
    }
}

/// A link target of an arrangement file: a string of at most `MAX_TARGET` characters that holds
/// nothing `display_text` would remove.
pub fn target_text(raw: &str) -> Option<String> {
    let kept = crate::inert_files::display_text(raw, MAX_TARGET + 1, false);
    (kept == raw && raw.chars().count() <= MAX_TARGET && !raw.chars().any(char::is_control)).then(|| raw.to_string())
}

/// Whether a URL may be loaded by Tandem at restore time: only what the one navigation gate answers
/// `Navigate` for (never a `file:`, `javascript:` or network path).
pub fn load_gate(url: &str) -> Option<String> {
    match crate::browser_js::nav_gate(url) {
        crate::browser_js::NavGate::Navigate(u) => Some(u),
        _ => None,
    }
}

/// Whether a pane's own load (the profile's Initial URL, a restored page already judged by
/// `load_gate` at start, or the person's own) goes: the verdict the guarded NavigationStarting gives
/// a load Tandem started (`webhost::nav_verdict` with `by_tandem`), so a deferred load after a
/// relaunch loads exactly what the same pane loads outside one (final review I2). `file_ok` is the
/// one path judge for a `file:` URL. None: refused (the caller says so, never a silent blank).
pub fn own_load(url: &str, file_ok: &dyn Fn(&str) -> bool) -> Option<String> {
    let info = crate::webhost::NavInfo { target: url, source: None, user: false, redirect: false, by_tandem: true };
    let policy = crate::webhost::NavPolicy { guard_local_schemes: true, allowed_schemes: None };
    crate::webhost::nav_verdict(&info, &policy, file_ok).then(|| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{TRANSITION_BACK_FORWARD, TRANSITION_LINK, TRANSITION_OTHER, TRANSITION_RELOAD, TRANSITION_REDIRECT, TRANSITION_TYPED};

    #[test]
    fn transitions_follow_iterm2() {
        // BH:11-17
        assert_eq!([TRANSITION_TYPED, TRANSITION_LINK, TRANSITION_RELOAD, TRANSITION_BACK_FORWARD, TRANSITION_REDIRECT, 5, TRANSITION_OTHER], [0, 1, 2, 3, 4, 5, 6]);
        let mut n = NavState::default();
        n.will_load("https://a/");
        assert_eq!(n.starting("https://a/", 2, false, false), TRANSITION_TYPED, "BNS:73-76");
        // Final review I1: WebView2 reports IsUserInitiated TRUE for a load Tandem started through
        // its API, so the requested URL is matched first (iTerm2 never calls loadURL a link), with
        // WebView2's canonical rewrite of the typed text allowed.
        n.will_load("https://a");
        assert_eq!(n.starting("https://a/", 2, true, false), TRANSITION_TYPED, "Tandem's own load is typed though WebView2 says user");
        n.will_load("https://a/");
        assert_eq!(n.starting("https://a/", 2, true, false), TRANSITION_TYPED);
        assert_eq!(n.last, Some(TRANSITION_TYPED));
        assert_eq!(n.starting("https://a/x", 2, true, false), TRANSITION_LINK, "a person's click elsewhere is still a link");
        assert_eq!(n.starting("https://b/", 2, true, false), TRANSITION_LINK);
        assert_eq!(n.starting("https://b/", 0, true, false), TRANSITION_RELOAD);
        assert_eq!(n.starting("https://b/", 1, false, false), TRANSITION_BACK_FORWARD);
        assert_eq!(n.starting("https://c/", 2, false, true), TRANSITION_REDIRECT);
        assert_eq!(n.starting("https://c/", 0, true, true), TRANSITION_REDIRECT);
        // A non-user new document that is not the requested URL is Other.
        assert_eq!(n.starting("https://z/", 2, false, false), TRANSITION_OTHER);
        // A different requested URL changes what Typed means; FormSubmit is never produced.
        n.will_load("https://z/");
        assert_eq!(n.starting("https://z/", 2, false, false), TRANSITION_TYPED);
        for kind in -1..4 {
            for user in [false, true] {
                for redirect in [false, true] {
                    assert_ne!(n.starting("https://q/", kind, user, redirect), 5);
                }
            }
        }
    }

    /// Final review I2: a parked load of the pane's own (the profile's Initial URL) loads as it does
    /// outside a relaunch: Tandem's own load through the one 13x verdict (a local `file:` and any
    /// `about:` page go; a network `file:` path never does).
    #[test]
    fn a_deferred_profile_url_loads_as_outside_a_relaunch() {
        let local = |u: &str| u.starts_with("file:///C:/");
        for url in ["file:///C:/notes/start.html", "about:version", "about:blank", "https://a.test/", "iterm2-about:welcome"] {
            assert_eq!(own_load(url, &local).as_deref(), Some(url), "{url}");
        }
        for bad in ["file://evil/s/x", "file:////evil/s/x", "file:///D:/remote.html"] {
            assert_eq!(own_load(bad, &local), None, "{bad}");
        }
    }

    #[test]
    fn a_hostile_requested_url_is_bounded() {
        let mut n = NavState::default();
        let long = format!("https://a/{}", "x".repeat(10_000));
        n.will_load(&long);
        assert!(n.requested.as_deref().is_none_or(|r| r.len() <= crate::browser::MAX_URL));
        assert_eq!(n.starting(&long, 2, false, false), TRANSITION_OTHER, "an over-long URL is never matched as typed");
    }

    #[test]
    fn arrangement_keys_follow_iterm2() {
        assert_eq!(TARGET_KEY, "Browser Target");
        assert_eq!(parse_color("rgb(255, 255, 255)"), Some((255, 255, 255)));
        assert_eq!(parse_color("rgba(10, 20, 30, 1)"), Some((10, 20, 30)));
        assert_eq!(parse_color("rgba(10, 20, 30, 0.5)"), Some((10, 20, 30)));
        assert_eq!(parse_color("rgb(1 2 3 / 50%)"), Some((1, 2, 3)));
        assert_eq!(parse_color("rgba(0, 0, 0, 0)"), None);
        assert_eq!(parse_color("rgb(1 2 3 / 0)"), None);
        assert_eq!(parse_color("transparent"), None);
        for bad in ["", "rgb()", "rgb(1,2)", "rgb(256, 0, 0)", "rgb(-1, 0, 0)", "rgb(a, b, c)", "red", "rgb(1,2,3,4,5)", "rgb(1, 2, 3", "url(x)", &"rgb(1, 2, 3)".repeat(20)] {
            assert_eq!(parse_color(bad), None, "{bad}");
        }
        assert_eq!(COLOR_INTERVAL, Duration::from_millis(250));
        assert!(COLOR_SCRIPT.contains("getComputedStyle(b).backgroundColor") && COLOR_SCRIPT.contains("documentElement"));
    }

    #[test]
    fn the_color_applies_once_per_interval() {
        let mut l = ColorLimit::default();
        let t0 = Instant::now();
        assert_eq!(l.offer(Some((1, 1, 1)), t0), Some(Some((1, 1, 1))), "the first applies at once");
        assert_eq!(l.offer(Some((2, 2, 2)), t0 + Duration::from_millis(100)), None);
        assert_eq!(l.offer(None, t0 + Duration::from_millis(120)), None, "the latest wins");
        assert!(l.waiting());
        assert_eq!(l.due(t0 + Duration::from_millis(200)), None);
        assert_eq!(l.due(t0 + Duration::from_millis(250)), Some(None));
        assert!(!l.waiting());
        assert_eq!(l.due(t0 + Duration::from_secs(5)), None);
        assert_eq!(l.offer(Some((3, 3, 3)), t0 + Duration::from_secs(5)), Some(Some((3, 3, 3))));
    }

    #[test]
    fn a_target_is_bounded_and_clean() {
        assert_eq!(target_text("docs").as_deref(), Some("docs"));
        assert_eq!(target_text(&"a".repeat(MAX_TARGET)).map(|t| t.len()), Some(MAX_TARGET));
        assert_eq!(target_text(&"a".repeat(MAX_TARGET + 1)), None);
        assert_eq!(target_text("a\u{202e}b"), None, "a bidi override spoofs");
        assert_eq!(target_text("a\u{0}b"), None);
        assert_eq!(target_text("a\nb"), None);
    }

    #[test]
    fn a_restored_url_passes_the_one_gate() {
        assert_eq!(load_gate("https://x/").as_deref(), Some("https://x/"));
        assert_eq!(load_gate("iterm2-about:welcome").as_deref(), Some("iterm2-about:welcome"));
        for bad in ["file://evil/s/x", "file:////evil/s/x", "\\\\evil\\s\\x", "javascript:x", "data:text/html,x", "file:///C:/a/b.txt", "ftp://x/"] {
            assert_eq!(load_gate(bad), None, "{bad}");
        }
    }
}
