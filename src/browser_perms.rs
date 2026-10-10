//! The browser permission vocabulary (milestone 13x6, Task 1), pure: iTerm2's six permission types
//! with their raw values and texts (`BP:10-83`), the two decisions, the origin string a decision is
//! keyed by (`BPM:245-265`) and the mapping from WebView2's permission kinds.
//!
//! Default deny is total. A person's saved Allow, for a kind Tandem asks about, is the only thing
//! that ever answers "allow": every other WebView2 kind (the ones Tandem does not ask about, and
//! any kind a later WebView2 adds) is Deny, and so is an origin that cannot be keyed
//! (`answer_for`, `route_kind`). `STATE_DEFAULT` (WebView2's own prompt, remembered outside
//! Tandem's database) is never an answer to a request; it only means "no saved state" when
//! mirroring a reset into the profile (`state_of`).

/// One of iTerm2's six permission types, in its `CaseIterable` order (`BP:10-16`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PermissionType {
    Notification,
    Camera,
    Microphone,
    CameraAndMicrophone,
    Geolocation,
    AudioPlayback,
}

impl PermissionType {
    pub const ALL: [PermissionType; 6] = [
        PermissionType::Notification,
        PermissionType::Camera,
        PermissionType::Microphone,
        PermissionType::CameraAndMicrophone,
        PermissionType::Geolocation,
        PermissionType::AudioPlayback,
    ];

    /// The value stored in the database (`BP:11-16`).
    pub fn raw(self) -> &'static str {
        match self {
            Self::Notification => "notification",
            Self::Camera => "camera",
            Self::Microphone => "microphone",
            Self::CameraAndMicrophone => "cameraAndMicrophone",
            Self::Geolocation => "geolocation",
            Self::AudioPlayback => "audioPlayback",
        }
    }

    /// Exact, case-sensitive: an unknown stored value is no permission.
    pub fn parse(raw: &str) -> Option<PermissionType> {
        Self::ALL.into_iter().find(|t| t.raw() == raw)
    }

    /// `BP:21-31`.
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Notification => "Notifications",
            Self::Camera => "Camera",
            Self::Microphone => "Microphone",
            Self::CameraAndMicrophone => "Camera and Microphone",
            Self::Geolocation => "Location",
            Self::AudioPlayback => "Audio Playback",
        }
    }

    /// `BP:40-50`.
    pub fn dialog_title(self) -> &'static str {
        match self {
            Self::Notification => "Allow Notifications",
            Self::Camera => "Allow Camera",
            Self::Microphone => "Allow Microphone",
            Self::CameraAndMicrophone => "Allow Camera and Microphone",
            Self::Geolocation => "Allow Location",
            Self::AudioPlayback => "Allow Audio Playback",
        }
    }

    /// `BP:59-69`. `origin` is the normalized origin exactly as `normalize_origin` returns it. It
    /// is always ASCII, so the dialog shows punycode (`xn--...`); it must never be decoded to
    /// Unicode for display (homograph spoofing).
    pub fn request_message(self, origin: &str) -> String {
        let what = match self {
            Self::Notification => "wants to send you notifications.",
            Self::Camera => "wants to use your camera.",
            Self::Microphone => "wants to use your microphone.",
            Self::CameraAndMicrophone => "wants to use your camera and microphone.",
            Self::Geolocation => "wants to know your location.",
            Self::AudioPlayback => "wants to play audio.",
        };
        format!("The website {origin} {what}")
    }
}

/// What a person decided (`BP:75-83`). There is no third value: "not decided" is the absence of a row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Granted,
    Denied,
}

impl Decision {
    pub fn raw(self) -> &'static str {
        match self {
            Self::Granted => "granted",
            Self::Denied => "denied",
        }
    }

    /// Exact: anything but the two stored values is no decision (never read as consent).
    pub fn parse(raw: &str) -> Option<Decision> {
        match raw {
            "granted" => Some(Self::Granted),
            "denied" => Some(Self::Denied),
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Granted => "Allowed",
            Self::Denied => "Blocked",
        }
    }
}

/// The dialog's two buttons (`BPM:220-221`).
pub const ALLOW: &str = "Allow";
pub const BLOCK: &str = "Block";

/// `COREWEBVIEW2_PERMISSION_STATE`.
pub const STATE_DEFAULT: i32 = 0;
pub const STATE_ALLOW: i32 = 1;
pub const STATE_DENY: i32 = 2;

/// The state a stored decision is mirrored into the WebView2 profile as; `None` (no row) resets it.
pub fn state_of(d: Option<Decision>) -> i32 {
    match d {
        None => STATE_DEFAULT,
        Some(Decision::Granted) => STATE_ALLOW,
        Some(Decision::Denied) => STATE_DENY,
    }
}

/// The permission type of a WebView2 `COREWEBVIEW2_PERMISSION_KIND` Tandem asks about:
/// 1 Microphone, 2 Camera, 3 Geolocation, 4 Notification. Every other kind, known or not, is None.
pub fn from_kind(kind: i32) -> Option<PermissionType> {
    match kind {
        1 => Some(PermissionType::Microphone),
        2 => Some(PermissionType::Camera),
        3 => Some(PermissionType::Geolocation),
        4 => Some(PermissionType::Notification),
        _ => None,
    }
}

/// The WebView2 kinds a saved decision is mirrored into the profile as. Only notifications and
/// location, which a page reads synchronously. Camera, microphone, both and audio are `[]`: iTerm2
/// looks up the exact type only (`BPM:27`, `BP:206-208`), and a mirrored camera state would let
/// WebView2 answer a camera-only request from a saved "cameraAndMicrophone" row, or raise only the
/// microphone half of a camera and microphone request after a saved Camera row.
pub fn kinds_of(t: PermissionType) -> &'static [i32] {
    match t {
        PermissionType::Notification => &[4],
        PermissionType::Geolocation => &[3],
        PermissionType::Camera
        | PermissionType::Microphone
        | PermissionType::CameraAndMicrophone
        | PermissionType::AudioPlayback => &[],
    }
}

/// What to do with a permission request of a WebView2 kind: ask about it, or refuse it outright.
/// There is no third route ("leave it to WebView2"): that would be a grant nobody asked Tandem about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KindRoute {
    Ask(PermissionType),
    Deny,
}

/// Total over every `i32`: kinds 0 and 5 to 12 (sensors, clipboard, downloads, file access,
/// autoplay, fonts, MIDI, window management), negative values and kinds not yet invented all Deny.
pub fn route_kind(kind: i32) -> KindRoute {
    match from_kind(kind) {
        Some(t) => KindRoute::Ask(t),
        None => KindRoute::Deny,
    }
}

/// The final `COREWEBVIEW2_PERMISSION_STATE` for a request of `kind` given the person's saved
/// decision for it: `STATE_ALLOW` only for a kind Tandem asks about with a saved Granted; every
/// other case is `STATE_DENY`. Never `STATE_DEFAULT`.
pub fn answer_for(kind: i32, saved: Option<Decision>) -> i32 {
    match (route_kind(kind), saved) {
        (KindRoute::Ask(_), Some(Decision::Granted)) => STATE_ALLOW,
        _ => STATE_DENY,
    }
}

/// An RFC 3986 scheme: a letter, then letters, digits, `+`, `-`, `.`.
fn scheme_ok(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_ascii_alphabetic())
        && c.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// A registered name in LDH form (punycode labels are LDH): labels of 1 to 63 letters, digits and
/// hyphens, none starting or ending with a hyphen, at most 253 characters in all. Anything else
/// (non-ASCII, spaces, controls, bidi marks, percent escapes, underscores) is refused, so a
/// page-chosen host cannot smuggle text into a dialog or collide with another key.
fn host_ok(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 253
        && h.split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

fn ipv6_ok(h: &str) -> bool {
    h.len() >= 4
        && h.len() <= 47
        && h.starts_with('[')
        && h.ends_with(']')
        && h[1..h.len() - 1].parse::<std::net::Ipv6Addr>().is_ok()
}

/// iTerm2's origin string for a requester (`BPM:245-265`): `scheme://host`, with `:port` kept
/// unless it is 80 or 443 (for any scheme). `blob:` is unwrapped to its inner origin. None for a
/// requester that has no usable origin (opaque, host-less, malformed): iTerm2 would key them all
/// as `about://`, `data://`, `file://`, so one Allow would grant every such page. A None origin
/// never reaches the database; the request is denied (Task 4).
pub fn normalize_origin(url: &str) -> Option<String> {
    let url = match url.get(..5) {
        Some(p) if p.eq_ignore_ascii_case("blob:") => &url[5..],
        _ => url,
    };
    let (scheme, rest) = url.split_once("://")?;
    if !scheme_ok(scheme) {
        return None;
    }
    let scheme = scheme.to_ascii_lowercase();
    if matches!(scheme.as_str(), "about" | "data" | "javascript" | "file" | "blob") {
        return None;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    // The browser reads `\` as a path separator in a special URL, so `a.b\@evil.com` is host `a.b`
    // to it; this function would say `evil.com`. Refuse rather than disagree.
    if authority.contains('\\') {
        return None;
    }
    let hostport = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let (host, port) = if hostport.starts_with('[') {
        let close = hostport.find(']')?;
        let (h, after) = hostport.split_at(close + 1);
        match after {
            "" => (h, None),
            _ => (h, Some(after.strip_prefix(':')?)),
        }
    } else {
        match hostport.split_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (hostport, None),
        }
    };
    let host = host.to_ascii_lowercase();
    if !(host_ok(&host) || ipv6_ok(&host)) {
        return None;
    }
    match port {
        None => Some(format!("{scheme}://{host}")),
        Some(p) => {
            if p.is_empty() || p.len() > 5 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let p: u32 = p.parse().ok()?;
            if p > 65535 {
                return None;
            }
            if p == 80 || p == 443 {
                Some(format!("{scheme}://{host}"))
            } else {
                Some(format!("{scheme}://{host}:{p}"))
            }
        }
    }
}

/// Whether Windows lets desktop programs use a feature: read from the privacy switches, never
/// prompted (no per-app "not determined" state exists for a desktop program).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Allowed,
    Denied,
}

/// `HKCU Software\Microsoft\Windows\CurrentVersion\PushNotifications` `ToastEnabled`: 0 means app notifications are off, a missing value
/// (or any other) means on.
pub fn notifications_system(toast_enabled: Option<u32>) -> System {
    if toast_enabled == Some(0) {
        System::Denied
    } else {
        System::Allowed
    }
}

/// The three `ConsentStore\location` values (machine, user, desktop apps): "Deny" in any of them
/// means denied.
pub fn location_system(machine: Option<&str>, user: Option<&str>, non_packaged: Option<&str>) -> System {
    if [machine, user, non_packaged].into_iter().flatten().any(|v| v.eq_ignore_ascii_case("Deny")) {
        System::Denied
    } else {
        System::Allowed
    }
}

/// What a request becomes (`BPM:25-59`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// A saved decision answers, with no dialog.
    Answer(Decision),
    /// Refused with no dialog and nothing saved (a system switch is off).
    Deny,
    /// The dialog asks, and its answer is saved.
    Ask(PermissionType),
    /// A type iTerm2 never asks about: the caller refuses it (default deny is total).
    Default,
}

/// `requestPermission` (`BPM:25-59`): the saved decision of the exact type first, then the system
/// switch for notifications and location (`BPM:164-214`), then the dialog (`BPM:150-162`).
pub fn plan(t: Option<PermissionType>, saved: Option<Decision>, system: System) -> Plan {
    let Some(t) = t else { return Plan::Default };
    if t == PermissionType::AudioPlayback {
        // Audio playback is never asked through the manager (`BPM:51-52`).
        return Plan::Default;
    }
    if let Some(d) = saved {
        return Plan::Answer(d);
    }
    match t {
        PermissionType::Notification | PermissionType::Geolocation if system == System::Denied => Plan::Deny,
        t => Plan::Ask(t),
    }
}

/// The one question that covers requests drained together (`BPM:96-119`): camera with microphone
/// is `CameraAndMicrophone`, else the first type. An empty list (never passed) is
/// `AudioPlayback`, which `plan` refuses.
pub fn group(kinds: &[PermissionType]) -> PermissionType {
    use PermissionType::{Camera, CameraAndMicrophone, Microphone};
    if kinds.contains(&CameraAndMicrophone) || (kinds.contains(&Camera) && kinds.contains(&Microphone)) {
        return CameraAndMicrophone;
    }
    kinds.first().copied().unwrap_or(PermissionType::AudioPlayback)
}

/// What a dialog's answer is (`BPM:224-225`) and whether it is saved: Allow (0) and Block (1) are
/// saved; a closed window (None) or any other index blocks this request and saves nothing, so a
/// close never grants.
pub fn answer_of(index: Option<usize>) -> (Decision, bool) {
    match index {
        Some(0) => (Decision::Granted, true),
        Some(1) => (Decision::Denied, true),
        _ => (Decision::Denied, false),
    }
}

/// The host answer for a decision of type `t`; `saved` says the decision is in the database.
/// `saves` (the host mirrors the grant into the profile before completing) only for a saved Allow
/// of the kinds a page reads synchronously; a Deny is never mirrored.
/// (Task 1's `answer_for` is the by-kind rule; this one is the dialog's.)
pub fn perm_answer(t: PermissionType, d: Decision, saved: bool) -> crate::webhost::PermAnswer {
    crate::webhost::PermAnswer::State { state: state_of(Some(d)), saves: saved && d == Decision::Granted && !kinds_of(t).is_empty() }
}

/// The profile entries `rows` mirror, oldest first (the LAST row of a (kind, origin) wins): each
/// row's `kinds_of` with its state. Media and audio rows are never mirrored (Task 1).
pub fn mirror_entries(rows: &[(String, PermissionType, Decision)]) -> Vec<(i32, String, i32)> {
    let mut out: Vec<(i32, String, i32)> = Vec::new();
    for (origin, t, d) in rows {
        for &kind in kinds_of(*t) {
            let state = state_of(Some(*d));
            match out.iter_mut().find(|(k, o, _)| *k == kind && o == origin) {
                Some(entry) => entry.2 = state,
                None => out.push((kind, origin.clone(), state)),
            }
        }
    }
    out
}

/// What to send `set_permission_states` so the profile agrees with the database for kinds 1 to 4:
/// a reset for every existing entry of those kinds that `wanted` does not hold (compared after
/// `normalize_origin`, since the exact form WebView2 reads back is unverified; the reset uses the
/// entry's own key so it hits the stored one), then every wanted entry. Entries of kinds 0 and 5
/// to 12 are never touched. With nothing wanted (the table unreadable) no grant of kinds 1 to 4
/// is left standing.
pub fn sync_plan(existing: &[(i32, String, i32)], wanted: &[(i32, String, i32)]) -> Vec<(i32, String, i32)> {
    let key = |o: &str| normalize_origin(o).unwrap_or_else(|| o.to_string());
    let held: Vec<(i32, String)> = wanted.iter().map(|(k, o, _)| (*k, key(o))).collect();
    let mut out: Vec<(i32, String, i32)> = Vec::new();
    for (kind, origin, _) in existing {
        if (1..=4).contains(kind) && !held.contains(&(*kind, key(origin))) {
            let reset = (*kind, origin.clone(), STATE_DEFAULT);
            if !out.contains(&reset) {
                out.push(reset);
            }
        }
    }
    out.extend(wanted.iter().cloned());
    out
}

/// Most entries and longest entry the sweep's answer may have.
const MAX_FRAME_ORIGINS: usize = 256;
const MAX_FRAME_ORIGIN_LEN: usize = 2048;

/// The sweep script's answer as the page-chosen strings it is: an array of at most 256 strings of
/// at most 2048 bytes, else None (unacceptable: `sweep_reloads` then reloads the page).
pub fn frame_origins(v: &serde_json::Value) -> Option<Vec<String>> {
    let items = v.as_array()?;
    if items.len() > MAX_FRAME_ORIGINS {
        return None;
    }
    items.iter().map(|i| i.as_str().filter(|s| s.len() <= MAX_FRAME_ORIGIN_LEN).map(str::to_string)).collect()
}

/// iTerm2's revocation sweep (`BM:545-586`): the page's main origin is `origin`, or the script's
/// list holds it. A failed script (None) falls back to the main origin only (`BM:576-583`). Every
/// origin is compared after `normalize_origin` ("null" and the like match nothing).
pub fn page_uses_origin(main_url: &str, frame_origins: Option<&[String]>, origin: &str) -> bool {
    let Some(want) = normalize_origin(origin) else { return false };
    if normalize_origin(main_url).as_deref() == Some(want.as_str()) {
        return true;
    }
    frame_origins.is_some_and(|list| list.iter().any(|o| normalize_origin(o).as_deref() == Some(want.as_str())))
}

/// Whether the revocation sweep reloads a page whose main URL is `main_url`. `answer` is the sweep
/// script's result: Err when the script failed (iTerm2 then compares the main origin only,
/// `BM:576-583`). An answer the page made unacceptable (over a cap, not an array, a non-string)
/// reloads the page (fail closed): the page controls it, so it must never mean "not used".
pub fn sweep_reloads(main_url: &str, answer: Result<&serde_json::Value, ()>, origin: &str) -> bool {
    match answer {
        Err(()) => page_uses_origin(main_url, None, origin),
        Ok(v) => match frame_origins(v) {
            Some(list) => page_uses_origin(main_url, Some(&list), origin),
            None => true,
        },
    }
}

/// The script behind the revocation sweep (13x2's file, read through its table).
pub fn check_iframe_origins_js() -> &'static str {
    crate::browser_js::resource("check-iframe-origins.js").unwrap_or("")
}

/// Whether a page's notification may be shown: only for an origin with a saved Granted
/// notifications decision (`BNH:136-140`); the database decides, never the page.
pub fn show_notification(saved: Option<Decision>) -> bool {
    saved == Some(Decision::Granted)
}

/// Whether a page's close of its notification is honored: iTerm2 checks the origin again before a
/// close (`BNH:58-62`), so a revoked origin cannot touch Tandem's balloon.
pub fn close_honored(saved: Option<Decision>) -> bool {
    saved == Some(Decision::Granted)
}

/// Longest origin shown in a notification's title (the tray title holds 63 characters).
const NOTE_ORIGIN_CHARS: usize = 60;
const NOTE_TEXT_CHARS: usize = 120;
const NOTE_BODY_CHARS: usize = 200;

/// A web notification's balloon text (B4). The origin (punycode, as `normalize_origin` made it) is
/// ALWAYS the title, so a page can never pose as Tandem or as another site; the page's own title
/// and body, cleaned of control and bidi characters and capped, are the body. A notification with
/// no text at all still says who sent it (an empty body removes the balloon).
pub fn notification_text(title: &str, body: &str, origin: &str) -> (String, String) {
    let clean = |s: &str| crate::inert_files::display_text(s, NOTE_TEXT_CHARS, false);
    let head = crate::inert_files::display_text(origin, NOTE_ORIGIN_CHARS, false);
    let (t, b) = (clean(title), clean(body));
    let text = match (t.is_empty(), b.is_empty()) {
        (true, true) => "(no text)".to_string(),
        (false, true) => t,
        (true, false) => b,
        (false, false) => format!("{t}: {b}"),
    };
    (head, crate::inert_files::display_text(&text, NOTE_BODY_CHARS, false))
}

/// The one web notification on Tandem's balloon: the pane, the page's token and the origin. The
/// balloon holds one notification, so a newer one (or any other Tandem alert) replaces it, and the
/// page is told the old one closed.
#[derive(Debug, Default)]
pub struct NoteScreen {
    on: Option<(crate::layout::PaneId, u64, String)>,
}

impl NoteScreen {
    /// A web notification is now on the balloon; returns the one it replaced.
    pub fn shown(&mut self, pane: crate::layout::PaneId, token: u64, origin: &str) -> Option<(crate::layout::PaneId, u64)> {
        self.on.replace((pane, token, origin.to_string())).map(|(p, t, _)| (p, t))
    }

    /// Another alert took the balloon (or the person navigated away): the one on screen, taken.
    pub fn take(&mut self) -> Option<(crate::layout::PaneId, u64)> {
        self.on.take().map(|(p, t, _)| (p, t))
    }

    /// The origin of the notification on screen under `token`, when it is that one.
    pub fn origin_of(&self, pane: crate::layout::PaneId, token: u64) -> Option<&str> {
        self.on.as_ref().filter(|(p, t, _)| *p == pane && *t == token).map(|(_, _, o)| o.as_str())
    }

    /// The pane's notification, taken, when it is the one on screen.
    pub fn take_pane(&mut self, pane: crate::layout::PaneId) -> Option<(crate::layout::PaneId, u64)> {
        if self.on.as_ref().is_some_and(|(p, _, _)| *p == pane) { self.take() } else { None }
    }

    pub fn on_screen(&self) -> bool {
        self.on.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(u: &str) -> Option<String> {
        normalize_origin(u)
    }
    fn s(x: &str) -> Option<String> {
        Some(x.to_string())
    }

    #[test]
    fn origins_follow_iterm2() {
        assert_eq!(n("https://Example.com/a?b"), s("https://example.com"));
        assert_eq!(n("https://a.b:443/x"), s("https://a.b"));
        assert_eq!(n("http://a.b:80/"), s("http://a.b"));
        assert_eq!(n("https://a.b:80/"), s("https://a.b"));
        assert_eq!(n("http://a.b:443/"), s("http://a.b"));
        assert_eq!(n("https://a.b:8443/"), s("https://a.b:8443"));
        assert_eq!(n("http://[::1]:8080/"), s("http://[::1]:8080"));
        assert_eq!(n("https://u:p@a.b/"), s("https://a.b"));
        assert_eq!(n("blob:https://a.b/0f2c-uuid"), s("https://a.b"));
        assert_eq!(n("BLOB:https://a.b/0f2c-uuid"), s("https://a.b"));
        assert_eq!(n("blob:null/0f2c"), None);
        assert_eq!(n("a.b"), None);
        assert_eq!(n("localhost:8080"), None);
        for u in ["about:blank", "data:text/html,<p>x", "javascript:alert(1)", "file:///C:/x.html"] {
            assert_eq!(n(u), None, "{u}");
        }
        assert_eq!(n("a+b.c-d://h/"), s("a+b.c-d://h"));
        for u in ["-x://a.b/", "1x://a.b/", "ht tp://a.b/", "://a.b/", "https:///x"] {
            assert_eq!(n(u), None, "{u}");
        }
    }

    #[test]
    fn hosts_are_strict_and_ports_bounded() {
        // Only LDH labels, punycode and bracketed IPv6; page-chosen text never reaches a dialog.
        assert_eq!(n("https://xn--bcher-kva.example/"), s("https://xn--bcher-kva.example"));
        for u in [
            "https://b\u{fc}cher.example/",
            "https://a b.c/",
            "https://a.b\u{202e}/",
            "https://a_b.c/",
            "https://a%2eb/",
            "https://a.b:99999/",
            "https://a.b:65536/",
            "https://a.b:/x",
            "https://a.b:12x/",
            "https://[::1/",
            "https://[zz::1]:80/",
            "https://-a.b/",
            "https://a..b/",
        ] {
            assert_eq!(n(u), None, "{u}");
        }
        assert_eq!(n("https://a.b:65535/"), s("https://a.b:65535"));
        let long = format!("https://{}.example/", "a".repeat(250));
        assert_eq!(n(&long), None);
        let label64 = format!("https://{}.example/", "a".repeat(64));
        assert_eq!(n(&label64), None);
    }

    #[test]
    fn backslash_and_trailing_dot_fail_closed() {
        // A backslash in the authority: the browser and this function could disagree on the host.
        for u in ["https://a.b\\@evil.com/", "https://evil.com\\@a.b/", "https://a.b\\x/"] {
            assert_eq!(n(u), None, "{u}");
        }
        // A trailing dot is REFUSED, not canonicalized: `example.com.` and `example.com` are
        // different keys to the browser, and refusing means a deny, never a shared grant.
        for u in ["https://example.com./", "https://example.com.:8443/", "https://.example.com/"] {
            assert_eq!(n(u), None, "{u}");
        }
    }

    #[test]
    fn permission_texts_follow_iterm2() {
        let raws: Vec<_> = PermissionType::ALL.iter().map(|t| t.raw()).collect();
        assert_eq!(
            raws,
            ["notification", "camera", "microphone", "cameraAndMicrophone", "geolocation", "audioPlayback"]
        );
        assert_eq!(PermissionType::parse("camera"), Some(PermissionType::Camera));
        assert_eq!(PermissionType::parse("Camera"), None);
        for t in PermissionType::ALL {
            assert_eq!(PermissionType::parse(t.raw()), Some(t));
        }
        let names: Vec<_> = PermissionType::ALL.iter().map(|t| t.display_name()).collect();
        assert_eq!(
            names,
            ["Notifications", "Camera", "Microphone", "Camera and Microphone", "Location", "Audio Playback"]
        );
        let titles: Vec<_> = PermissionType::ALL.iter().map(|t| t.dialog_title()).collect();
        assert_eq!(
            titles,
            [
                "Allow Notifications",
                "Allow Camera",
                "Allow Microphone",
                "Allow Camera and Microphone",
                "Allow Location",
                "Allow Audio Playback"
            ]
        );
        let msgs: Vec<_> = PermissionType::ALL.iter().map(|t| t.request_message("a.b")).collect();
        assert_eq!(
            msgs,
            [
                "The website a.b wants to send you notifications.",
                "The website a.b wants to use your camera.",
                "The website a.b wants to use your microphone.",
                "The website a.b wants to use your camera and microphone.",
                "The website a.b wants to know your location.",
                "The website a.b wants to play audio."
            ]
        );
        assert_eq!(Decision::Granted.raw(), "granted");
        assert_eq!(Decision::Denied.raw(), "denied");
        assert_eq!(Decision::parse("granted"), Some(Decision::Granted));
        assert_eq!(Decision::parse("denied"), Some(Decision::Denied));
        assert_eq!(Decision::parse("Granted"), None);
        assert_eq!(Decision::parse(""), None);
        assert_eq!(Decision::Granted.display_name(), "Allowed");
        assert_eq!(Decision::Denied.display_name(), "Blocked");
        assert_eq!((ALLOW, BLOCK), ("Allow", "Block"));
    }

    #[test]
    fn kinds_follow_webview2() {
        assert_eq!(from_kind(1), Some(PermissionType::Microphone));
        assert_eq!(from_kind(2), Some(PermissionType::Camera));
        assert_eq!(from_kind(3), Some(PermissionType::Geolocation));
        assert_eq!(from_kind(4), Some(PermissionType::Notification));
        for k in [0, 5, 6, 7, 8, 9, 10, 11, 12, -1, 13, 99, i32::MAX, i32::MIN] {
            assert_eq!(from_kind(k), None, "{k}");
        }
        assert_eq!(kinds_of(PermissionType::Notification), &[4]);
        assert_eq!(kinds_of(PermissionType::Geolocation), &[3]);
        for t in [
            PermissionType::Camera,
            PermissionType::Microphone,
            PermissionType::CameraAndMicrophone,
            PermissionType::AudioPlayback,
        ] {
            assert!(kinds_of(t).is_empty());
        }
        assert_eq!(state_of(None), 0);
        assert_eq!(state_of(Some(Decision::Granted)), 1);
        assert_eq!(state_of(Some(Decision::Denied)), 2);
    }

    #[test]
    fn default_deny_is_total() {
        // Every kind WebView2 has, and every number it might gain, is answered. Only a person's
        // saved Allow for a kind Tandem asks about can ever come out as Allow.
        for k in -5..=64 {
            for saved in [None, Some(Decision::Denied), Some(Decision::Granted)] {
                let a = answer_for(k, saved);
                let expect_allow = from_kind(k).is_some() && saved == Some(Decision::Granted);
                assert_eq!(a == STATE_ALLOW, expect_allow, "kind {k} saved {saved:?}");
                assert_ne!(a, STATE_DEFAULT, "kind {k} must never fall to WebView2's own prompt");
            }
        }
        for k in [0, 5, 6, 7, 8, 9, 10, 11, 12, 13, 1000, i32::MAX, i32::MIN] {
            assert_eq!(answer_for(k, Some(Decision::Granted)), STATE_DENY, "kind {k}");
            assert_eq!(route_kind(k), KindRoute::Deny, "kind {k}");
        }
        assert_eq!(route_kind(4), KindRoute::Ask(PermissionType::Notification));
        // Nothing saved for a kind Tandem asks about: Deny until the person answers.
        assert_eq!(answer_for(4, None), STATE_DENY);
        assert_eq!(answer_for(4, Some(Decision::Granted)), STATE_ALLOW);
        // A missing or unparsable stored row is no consent.
        assert_eq!(Decision::parse("allow"), None);
        assert_eq!(Decision::parse("1"), None);
    }

    #[test]
    fn request_plan_follows_iterm2() {
        use Decision::*;
        use PermissionType::*;
        assert_eq!(plan(Some(Camera), Some(Granted), System::Allowed), Plan::Answer(Granted));
        assert_eq!(plan(Some(Camera), Some(Denied), System::Denied), Plan::Answer(Denied));
        assert_eq!(plan(Some(Notification), None, System::Denied), Plan::Deny);
        assert_eq!(plan(Some(Geolocation), None, System::Denied), Plan::Deny);
        assert_eq!(plan(Some(Geolocation), None, System::Allowed), Plan::Ask(Geolocation));
        assert_eq!(plan(Some(Camera), None, System::Denied), Plan::Ask(Camera));
        assert_eq!(plan(None, None, System::Allowed), Plan::Default);
        assert_eq!(plan(Some(AudioPlayback), Some(Granted), System::Allowed), Plan::Default, "never asked through the manager");
    }

    #[test]
    fn system_state_follows_windows() {
        assert_eq!(notifications_system(Some(0)), System::Denied);
        assert_eq!(notifications_system(Some(1)), System::Allowed);
        assert_eq!(notifications_system(None), System::Allowed);
        assert_eq!(location_system(Some("Allow"), Some("Allow"), None), System::Allowed);
        assert_eq!(location_system(Some("Deny"), None, None), System::Denied);
        assert_eq!(location_system(None, Some("Deny"), None), System::Denied);
        assert_eq!(location_system(None, None, Some("Deny")), System::Denied);
        assert_eq!(location_system(None, None, None), System::Allowed);
    }

    #[test]
    fn requests_group_as_iterm2() {
        use PermissionType::*;
        assert_eq!(group(&[Camera]), Camera);
        assert_eq!(group(&[Microphone, Camera]), CameraAndMicrophone);
        assert_eq!(group(&[Camera, Camera]), Camera);
        assert_eq!(group(&[Notification]), Notification);
        assert_eq!(group(&[]), AudioPlayback, "an empty list is refused by plan");
    }

    #[test]
    fn dialog_answer_follows_iterm2() {
        use crate::webhost::PermAnswer::State;
        use PermissionType::*;
        assert_eq!(answer_of(Some(0)), (Decision::Granted, true));
        assert_eq!(answer_of(Some(1)), (Decision::Denied, true));
        assert_eq!(answer_of(None), (Decision::Denied, false));
        assert_eq!(answer_of(Some(2)), (Decision::Denied, false));
        assert_eq!(perm_answer(Notification, Decision::Granted, true), State { state: 1, saves: true });
        assert_eq!(perm_answer(Geolocation, Decision::Denied, true), State { state: 2, saves: false }, "a Deny is never mirrored");
        assert_eq!(perm_answer(Camera, Decision::Granted, true), State { state: 1, saves: false });
        assert_eq!(perm_answer(Notification, Decision::Denied, false), State { state: 2, saves: false });
    }

    fn row(o: &str, t: PermissionType, d: Decision) -> (String, PermissionType, Decision) {
        (o.to_string(), t, d)
    }

    fn e(k: i32, o: &str, st: i32) -> (i32, String, i32) {
        (k, o.to_string(), st)
    }

    #[test]
    fn profile_mirror_follows_the_database() {
        use Decision::*;
        use PermissionType::*;
        let rows = [row("https://a.b", Notification, Granted), row("https://c.d", CameraAndMicrophone, Denied), row("https://e.f", AudioPlayback, Granted), row("https://g.h", Geolocation, Denied)];
        assert_eq!(mirror_entries(&rows), vec![e(4, "https://a.b", 1), e(3, "https://g.h", 2)], "media and audio are never mirrored");
        let rows = [row("https://a.b", Notification, Granted), row("https://a.b", Notification, Denied)];
        assert_eq!(mirror_entries(&rows), vec![e(4, "https://a.b", 2)], "the later row wins");
    }

    #[test]
    fn sync_touches_only_asked_kinds() {
        let existing = [e(6, "https://a.b", 1), e(9, "https://a.b", 1), e(4, "https://a.b", 1), e(2, "https://c.d", 1), e(4, "https://e.f:443", 2)];
        let wanted = [e(4, "https://e.f", 2)];
        assert_eq!(sync_plan(&existing, &wanted), vec![e(4, "https://a.b", 0), e(2, "https://c.d", 0), e(4, "https://e.f", 2)]);
        assert!(sync_plan(&[], &[]).is_empty());
        // A stale grant the table does not hold is reset; a table that cannot be read (empty
        // wanted) therefore leaves no grant of kinds 1 to 4 standing.
        assert_eq!(sync_plan(&[e(3, "https://x.y", 1), e(4, "https://x.y", 1)], &[]), vec![e(3, "https://x.y", 0), e(4, "https://x.y", 0)]);
        // A profile Allow where the table says Deny is overwritten, not kept.
        assert_eq!(sync_plan(&[e(4, "https://a.b", 1)], &[e(4, "https://a.b", 2)]), vec![e(4, "https://a.b", 2)]);
        // An origin that does not normalize is never in the table: reset under its own key.
        assert_eq!(sync_plan(&[e(4, "null", 1)], &[]), vec![e(4, "null", 0)]);
    }

    #[test]
    fn revocation_reloads_as_iterm2() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(page_uses_origin("https://a.b/x", None, "https://a.b"));
        assert!(page_uses_origin("https://c.d/", Some(&s(&["https://a.b"])), "https://a.b"));
        assert!(!page_uses_origin("https://c.d/", Some(&[]), "https://a.b"));
        assert!(!page_uses_origin("https://c.d/", None, "https://a.b"), "a failed script compares the main origin only");
        assert!(page_uses_origin("https://c.d/", Some(&s(&["https://a.b:443"])), "https://a.b"), "compared after normalizing");
        assert!(!page_uses_origin("https://c.d/", Some(&s(&["null", "https://a.b.evil"])), "https://a.b"));
        assert!(!check_iframe_origins_js().is_empty());
        assert_eq!(check_iframe_origins_js(), crate::browser_js::resource("check-iframe-origins.js").unwrap());
    }

    #[test]
    fn the_sweeps_answer_is_bounded_and_typed() {
        use serde_json::json;
        // An answer over a cap, or of the wrong shape, reloads (fail closed); a failed script
        // compares the main origin only (iTerm2); a good answer reloads only on a match.
        let o = "https://a.b";
        let main = "https://c.d/";
        assert!(sweep_reloads(main, Ok(&json!(vec!["https://x.y"; 257])), o), "more than 256 entries: reload the whole pane");
        assert!(sweep_reloads(main, Ok(&json!([format!("https://{}", "a".repeat(2050))])), o), "an entry over 2048: reload");
        assert!(sweep_reloads(main, Ok(&json!("https://x.y")), o), "not an array: reload");
        assert!(sweep_reloads(main, Ok(&json!(["https://x.y", 4])), o), "a non-string: reload");
        assert!(!sweep_reloads(main, Ok(&json!(vec!["https://x.y"; 256])), o), "256 others: no match, no reload");
        assert!(sweep_reloads(main, Ok(&json!(["https://a.b"])), o));
        assert!(!sweep_reloads(main, Err(()), o), "a failed script: main origin only");
        assert!(sweep_reloads("https://a.b/x", Err(()), o));
        assert_eq!(frame_origins(&json!(["https://a.b", "null"])), Some(vec!["https://a.b".to_string(), "null".to_string()]));
        assert_eq!(frame_origins(&json!([])), Some(vec![]));
        assert_eq!(frame_origins(&json!("https://a.b")), None, "not an array");
        assert_eq!(frame_origins(&json!(["https://a.b", 4])), None, "an entry that is not a string");
        assert_eq!(frame_origins(&json!(vec!["https://a.b"; 257])), None, "more than 256 entries");
        assert!(frame_origins(&json!(vec!["https://a.b"; 256])).is_some());
        assert_eq!(frame_origins(&json!([format!("https://{}", "a".repeat(2050))])), None, "an entry over 2048");
    }

    #[test]
    fn web_notification_follows_iterm2() {
        assert!(show_notification(Some(Decision::Granted)));
        assert!(!show_notification(Some(Decision::Denied)));
        assert!(!show_notification(None));
        assert!(close_honored(Some(Decision::Granted)));
        assert!(!close_honored(Some(Decision::Denied)));
        assert!(!close_honored(None));
        assert_eq!(notification_text("T", "B", "https://a.b"), ("https://a.b".into(), "T: B".into()));
        assert_eq!(notification_text("T", "", "https://a.b").1, "T");
        assert_eq!(notification_text("", "B", "https://a.b").1, "B");
        assert_eq!(notification_text("", "", "https://a.b").1, "(no text)", "a body-less notification still shows");
    }

    #[test]
    fn a_page_cannot_pose_as_tandem() {
        let (t, b) = notification_text("Session Ended", "Tandem: your session \"x\" just terminated\u{202e}\n", "https://evil.example");
        assert_eq!(t, "https://evil.example", "the origin is always the title");
        assert!(!b.contains('\u{202e}') && !b.contains('\n'));
        assert!(b.starts_with("Session Ended: "));
        let (t, b) = notification_text(&"x".repeat(500), &"y".repeat(500), &format!("https://{}.example", "a".repeat(200)));
        assert!(t.chars().count() <= 60 && t.starts_with("https://aaa"));
        assert!(b.chars().count() <= 200);
    }

    #[test]
    fn one_web_note_is_on_screen() {
        let mut n = NoteScreen::default();
        assert_eq!(n.shown(1, 1, "https://a.b"), None);
        assert_eq!(n.origin_of(1, 1), Some("https://a.b"));
        assert_eq!(n.origin_of(1, 2), None, "an older or other token is not on screen");
        assert_eq!(n.shown(1, 2, "https://c.d"), Some((1, 1)), "the replaced one is returned to be reported closed");
        assert_eq!(n.take_pane(9), None);
        assert_eq!(n.take_pane(1), Some((1, 2)));
        assert!(!n.on_screen());
        assert_eq!(n.take(), None);
    }
}
