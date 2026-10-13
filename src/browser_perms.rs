//! The browser permission vocabulary (milestone 13x6, Task 1), pure: iTerm2's six permission types
//! with their raw values and texts (`BP:10-83`), the two decisions, the origin string a decision is
//! keyed by (`BPM:245-265`) and the mapping from WebView2's permission kinds.
//!
//! Default deny is total. A person's saved Allow, for a kind Termurion asks about, is the only thing
//! that ever answers "allow": every other WebView2 kind (the ones Termurion does not ask about, and
//! any kind a later WebView2 adds) is Deny, and so is an origin that cannot be keyed
//! (`answer_for`, `route_kind`). `STATE_DEFAULT` (WebView2's own prompt, remembered outside
//! Termurion's database) is never an answer to a request; it only means "no saved state" when
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

/// The permission type of a WebView2 `COREWEBVIEW2_PERMISSION_KIND` Termurion asks about:
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
/// There is no third route ("leave it to WebView2"): that would be a grant nobody asked Termurion about.
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
/// decision for it: `STATE_ALLOW` only for a kind Termurion asks about with a saved Granted; every
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
/// close (`BNH:58-62`), so a revoked origin cannot touch Termurion's balloon.
pub fn close_honored(saved: Option<Decision>) -> bool {
    saved == Some(Decision::Granted)
}

/// Longest origin shown in a notification's title (the tray title holds 63 characters).
const NOTE_ORIGIN_CHARS: usize = 60;
const NOTE_TEXT_CHARS: usize = 120;
const NOTE_BODY_CHARS: usize = 200;

/// An origin as a label at most `max` characters long that always keeps the END of the host (the
/// registrable domain and the port, the part that names the sender): a long host is cut from the
/// LEFT after the scheme, behind an ellipsis, as Chromium shows origins. Hidden characters are
/// dropped first. `https://termurion-update.microsoft.com.aaaa.evil.example` never reads as
/// microsoft.com.
pub fn origin_label(origin: &str, max: usize) -> String {
    let clean = crate::inert_files::clean_name(origin);
    let chars: Vec<char> = clean.chars().collect();
    if chars.len() <= max {
        return clean;
    }
    let head: Vec<char> = clean.find("://").map_or(String::new(), |i| clean[..i + 3].to_string()).chars().collect();
    // A scheme too long to leave room: the end alone.
    let head = if head.len() + 2 > max { Vec::new() } else { head };
    let keep = max.saturating_sub(head.len() + 1);
    head.iter().chain(['\u{2026}'].iter()).chain(chars[chars.len() - keep..].iter()).collect()
}

/// `s` with its hidden characters dropped and at most `max` characters, the last an ellipsis when
/// it was cut (a cut text never reads as complete).
fn clip(s: &str, max: usize) -> String {
    let chars: Vec<char> = crate::inert_files::clean_name(s).chars().collect();
    if chars.len() <= max {
        return chars.into_iter().collect();
    }
    chars[..max.saturating_sub(1)].iter().chain(['\u{2026}'].iter()).collect()
}

/// A web notification's balloon text (B4). The origin (punycode, as `normalize_origin` made it) is
/// ALWAYS the title, so a page can never pose as Termurion or as another site; the page's own title
/// and body, cleaned of control and bidi characters and capped, are the body. A notification with
/// no text at all still says who sent it (an empty body removes the balloon).
pub fn notification_text(title: &str, body: &str, origin: &str) -> (String, String) {
    let clean = |s: &str| clip(s, NOTE_TEXT_CHARS);
    // The END of the origin always shows (Task 6 review I2).
    let head = origin_label(origin, NOTE_ORIGIN_CHARS);
    let (t, b) = (clean(title), clean(body));
    let text = match (t.is_empty(), b.is_empty()) {
        (true, true) => "(no text)".to_string(),
        (false, true) => t,
        (true, false) => b,
        (false, false) => format!("{t}: {b}"),
    };
    (head, clip(&text, NOTE_BODY_CHARS))
}

/// A web notification taken off Termurion's record: whom to tell it closed, and `notify::shown_count()`
/// right after it was shown (the balloon is cleared only while that is still the count:
/// `notify::clear_if_shown`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteGone {
    pub pane: crate::layout::PaneId,
    pub token: u64,
    pub shown_at: u64,
}

/// The one web notification on Termurion's balloon: the pane, the page's token, the origin and the
/// show count it was shown at. The balloon holds one notification, so a newer one (or any other
/// Termurion alert) replaces it, and the page is told the old one closed.
#[derive(Debug, Default)]
pub struct NoteScreen {
    on: Option<(NoteGone, String)>,
}

impl NoteScreen {
    /// A web notification is now on the balloon (shown at `shown_at`); returns the one it replaced.
    pub fn shown(&mut self, pane: crate::layout::PaneId, token: u64, origin: &str, shown_at: u64) -> Option<NoteGone> {
        self.on.replace((NoteGone { pane, token, shown_at }, origin.to_string())).map(|(g, _)| g)
    }

    /// Another alert took the balloon (or the person navigated away): the one on record, taken.
    pub fn take(&mut self) -> Option<NoteGone> {
        self.on.take().map(|(g, _)| g)
    }

    /// The origin of the notification on record under `token`, when it is that one.
    pub fn origin_of(&self, pane: crate::layout::PaneId, token: u64) -> Option<&str> {
        self.on.as_ref().filter(|(g, _)| g.pane == pane && g.token == token).map(|(_, o)| o.as_str())
    }

    /// The pane's notification, taken, when it is the one on record.
    pub fn take_pane(&mut self, pane: crate::layout::PaneId) -> Option<NoteGone> {
        if self.on.as_ref().is_some_and(|(g, _)| g.pane == pane) { self.take() } else { None }
    }

    /// The notification of `origin`, taken, when it is the one on record (its row was reset).
    pub fn take_origin(&mut self, origin: &str) -> Option<NoteGone> {
        if self.on.as_ref().is_some_and(|(_, o)| o == origin) { self.take() } else { None }
    }
}

// ---- Audio (Task 7, `BM:2058-2122`, `BAH`) ----

/// The question bar's buttons (`BM:2092-2095`); `_X` marks Alt+X.
pub const ALLOW_ONCE: &str = "Allow _Once";
pub const ALLOW_ALWAYS: &str = "Allow _Always";
pub const DENY_ONCE: &str = "_Deny Once";
pub const DENY_ALWAYS: &str = "De_ny Always";
pub const AUDIO_BUTTONS: [&str; 4] = [ALLOW_ONCE, ALLOW_ALWAYS, DENY_ONCE, DENY_ALWAYS];

/// What a play event does (`BM:2073-2083`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioAct {
    Nothing,
    Unmute,
    /// Mute if needed and record the page as muted by Termurion.
    Mute,
    /// The same, and ask.
    Ask,
}

/// Whether a page starts muted: unless its origin is Granted (a Denied or unknown origin is silent
/// until the person allows it).
pub fn premute(saved: Option<Decision>) -> bool {
    saved != Some(Decision::Granted)
}

/// What a play event does for a page whose origin has `saved`, with `muted` the view's state now.
/// `disabled` is a once answer's "until the next document" (`BAH:72`).
pub fn on_playing(disabled: bool, saved: Option<Decision>, muted: bool) -> AudioAct {
    if disabled {
        return AudioAct::Nothing;
    }
    match saved {
        Some(Decision::Denied) => AudioAct::Mute,
        Some(Decision::Granted) if muted => AudioAct::Unmute,
        Some(Decision::Granted) => AudioAct::Nothing,
        None => AudioAct::Ask,
    }
}

pub fn audio_prompt(origin: &str) -> String {
    format!("Audio was muted. Allow playback by {origin}?")
}

pub fn audio_bar_id(origin: &str) -> String {
    format!("NoSyncMuteAudio_{origin}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioAnswer {
    AllowOnce,
    AllowAlways,
    DenyOnce,
    DenyAlways,
    Closed,
}

impl AudioAnswer {
    /// The bar's answer: the index into `AUDIO_BUTTONS`; None or out of range is Closed.
    pub fn parse(index: Option<usize>) -> AudioAnswer {
        match index {
            Some(0) => AudioAnswer::AllowOnce,
            Some(1) => AudioAnswer::AllowAlways,
            Some(2) => AudioAnswer::DenyOnce,
            Some(3) => AudioAnswer::DenyAlways,
            _ => AudioAnswer::Closed,
        }
    }
}

/// (decision to save, unmute now, disable the handler until the next document) (`BM:2098-2122`).
pub fn apply(answer: AudioAnswer) -> (Option<Decision>, bool, bool) {
    match answer {
        AudioAnswer::AllowOnce => (None, true, true),
        AudioAnswer::AllowAlways => (Some(Decision::Granted), true, false),
        AudioAnswer::DenyOnce => (None, false, true),
        AudioAnswer::DenyAlways => (Some(Decision::Denied), false, false),
        AudioAnswer::Closed => (None, false, false),
    }
}

/// One pane's audio state. It lives on the pane, not the view (a remade view keeps it). Every
/// change comes from the person's choices and the engine's play events, never from a page message.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioState {
    /// Termurion muted the page on a play event (`currentPageIsMuted`, `BM:590-592`).
    pub muted_by_us: bool,
    /// A once answer ended the question for this document (`BAH:72`).
    pub disabled: bool,
    /// Counts the pane's documents: a question asked of an earlier one is stale.
    pub epoch: u64,
}

/// What an answer to the bar did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Answered {
    pub save: Option<Decision>,
    pub unmute: bool,
}

impl AudioState {
    /// A main-frame navigation began; returns whether the view should now be muted (both ways, so a
    /// redirect to a granted origin unmutes). A new document (`redirect` false) resets the state
    /// (`BM:1255-1256`); a redirect only re-evaluates the pre-mute. The caller never passes the
    /// isolated world's own `about:blank` navigations.
    pub fn starting(&mut self, redirect: bool, saved: Option<Decision>) -> bool {
        if !redirect {
            self.muted_by_us = false;
            self.disabled = false;
            self.epoch = self.epoch.wrapping_add(1);
        }
        premute(saved)
    }

    /// The document started playing audio. Returns (set the view muted to, ask).
    pub fn playing(&mut self, saved: Option<Decision>, muted: bool) -> (Option<bool>, bool) {
        let act = on_playing(self.disabled, saved, muted);
        match act {
            AudioAct::Nothing => (None, false),
            AudioAct::Unmute => {
                self.muted_by_us = false;
                (Some(false), false)
            }
            AudioAct::Mute | AudioAct::Ask => {
                self.muted_by_us = true;
                ((!muted).then_some(true), act == AudioAct::Ask)
            }
        }
    }

    /// The bar asked at `asked` (an epoch) was answered. `same_url` says the pane is still on the page
    /// URL it was asked about, `same_origin` on its origin. A save happens whatever the page did
    /// since; an unmute only for the page that was asked, and a once answer only ends the question
    /// of the document that asked it (`BM:2099`, review M8).
    pub fn answered(&mut self, asked: u64, same_url: bool, same_origin: bool, answer: AudioAnswer) -> Answered {
        let (save, unmute, disable) = apply(answer);
        let same_doc = asked == self.epoch;
        let unmute = unmute
            && match answer {
                AudioAnswer::AllowAlways => same_origin,
                _ => same_doc && same_url,
            };
        if disable && same_doc && (unmute || answer == AudioAnswer::DenyOnce) {
            self.disabled = true;
        }
        if unmute {
            self.muted_by_us = false;
        }
        Answered { save, unmute }
    }

    /// "Unmute Current Page".
    pub fn unmute(&mut self) {
        self.muted_by_us = false;
    }
}

// --- The toolbar menu's site items (Task 8, `BT:381-409`) ---

/// A reset item's id is this and the type's raw value.
pub const RESET_PREFIX: &str = "browser-menu:reset-permission:";
pub const UNMUTE_ID: &str = "browser-menu:unmute";
pub const UNMUTE_TITLE: &str = "Unmute Current Page";

/// `BT:388-392`.
pub fn reset_title(t: PermissionType, d: Decision) -> String {
    format!("Reset {} Permission ({})", t.display_name(), d.display_name())
}

/// The site items of the toolbar menu: (title, id), None a separator. One reset item for every
/// saved row, every type included (audio playback too, `BVC:1063-1070`), sorted by display name
/// (`BT:386`); a separator after them; then Unmute and a separator when the page is muted.
pub fn site_menu_items(saved: &[(PermissionType, Decision)], muted: bool) -> Vec<Option<(String, String)>> {
    let mut rows: Vec<(PermissionType, Decision)> = saved.to_vec();
    rows.sort_by(|a, b| a.0.display_name().cmp(b.0.display_name()));
    rows.dedup_by_key(|r| r.0);
    let mut out: Vec<Option<(String, String)>> = rows.iter().map(|(t, d)| Some((reset_title(*t, *d), format!("{RESET_PREFIX}{}", t.raw())))).collect();
    if !out.is_empty() {
        out.push(None);
    }
    if muted {
        out.push(Some((UNMUTE_TITLE.to_string(), UNMUTE_ID.to_string())));
        out.push(None);
    }
    out
}

/// What a site item's id asks for. Anything else is not ours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuPick {
    Reset(PermissionType),
    Unmute,
}

pub fn menu_pick(id: &str) -> Option<MenuPick> {
    if id == UNMUTE_ID {
        return Some(MenuPick::Unmute);
    }
    PermissionType::parse(id.strip_prefix(RESET_PREFIX)?).map(MenuPick::Reset)
}

/// The menu's line naming the site the items are for: the normalized (punycode) origin, hidden
/// characters dropped, the END of a long host kept (R13x6-O2).
pub fn menu_origin_line(origin: &str) -> String {
    origin_label(origin, 48)
}

// --- The iterm2-about:permissions page (Task 9) ---

/// The page's URL and its message handler (`BPVH:18`).
pub const PAGE_URL: &str = "iterm2-about:permissions";
pub const PAGE_HANDLER: &str = "iterm2-about:permissions";
/// Rows per page (`BPVH:66-70`).
pub const PAGE_LIMIT: usize = 50;
const PAGE_MAX_OFFSET: u64 = 100_000;
const PAGE_MAX_LIMIT: u64 = 100;

/// Whether a page message counts: the pane's main frame (WebView2's own `Source`, never a value
/// from the message) is exactly the page and the sender is that main frame (review M6).
pub fn page_message_allowed(main_url: &str, frame: Option<u64>) -> bool {
    main_url == PAGE_URL && frame.is_none()
}

/// What `loadPermissions` asks for (`BPVH:66-70`).
#[derive(Clone, Debug, PartialEq)]
pub struct PageQuery {
    pub offset: usize,
    pub limit: usize,
    pub search: String,
    pub type_filter: String,
    pub status_filter: String,
}

impl PageQuery {
    /// iTerm2's defaults for anything missing or of the wrong type; Termurion clamps the numbers so a
    /// page cannot ask for an absurd slice.
    pub fn from_message(body: &serde_json::Value) -> PageQuery {
        let num = |key: &str, default: u64, hi: u64| body.get(key).and_then(|v| v.as_i64()).map_or(default, |n| (n.max(0) as u64).min(hi));
        let text = |key: &str| body.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        PageQuery {
            offset: num("offset", 0, PAGE_MAX_OFFSET) as usize,
            limit: num("limit", PAGE_LIMIT as u64, PAGE_MAX_LIMIT).max(1) as usize,
            search: text("searchQuery"),
            type_filter: text("permissionTypeFilter"),
            status_filter: text("statusFilter"),
        }
    }
}

/// The page's rows and whether more follow (`BPVH:100-158`): filters (an unknown type or status
/// filters nothing), a lower-cased search over the origin and the type's display name, newest
/// first, then the slice (an offset past the end is empty, where iTerm2 would trap).
pub fn page_rows(rows: &[crate::browser_db_perms::PermissionRow], q: &PageQuery) -> (Vec<serde_json::Value>, bool) {
    let type_filter = PermissionType::parse(&q.type_filter);
    let status_filter = Decision::parse(&q.status_filter);
    let needle = q.search.to_lowercase();
    let mut kept: Vec<&crate::browser_db_perms::PermissionRow> = rows
        .iter()
        .filter(|r| type_filter.is_none_or(|t| r.kind == t))
        .filter(|r| status_filter.is_none_or(|d| r.decision == d))
        .filter(|r| needle.is_empty() || r.origin.to_lowercase().contains(&needle) || r.kind.display_name().to_lowercase().contains(&needle))
        .collect();
    kept.sort_by(|a, b| b.created_at.total_cmp(&a.created_at));
    let start = q.offset.min(kept.len());
    let end = start.saturating_add(q.limit).min(kept.len());
    let page = kept[start..end]
        .iter()
        .map(|r| serde_json::json!({"origin": r.origin, "permissionType": r.kind.raw(), "decision": r.decision.raw(), "createdAt": r.created_at, "updatedAt": r.updated_at}))
        .collect();
    (page, end < kept.len())
}

/// Every origin that has a saved row, once each: what Clear All Permissions revokes one by one.
pub fn clear_origins(rows: &[crate::browser_db_perms::PermissionRow]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    rows.iter().filter(|r| seen.insert(r.origin.as_str())).map(|r| r.origin.clone()).collect()
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
    fn site_menu_follows_iterm2() {
        use Decision::*;
        use PermissionType::*;
        let r = |t: PermissionType| format!("{RESET_PREFIX}{}", t.raw());
        let some = |a: &str, b: String| Some((a.to_string(), b));
        assert_eq!(
            site_menu_items(&[(Notification, Granted), (Camera, Denied), (Geolocation, Granted)], false),
            vec![some("Reset Camera Permission (Blocked)", r(Camera)), some("Reset Location Permission (Allowed)", r(Geolocation)), some("Reset Notifications Permission (Allowed)", r(Notification)), None]
        );
        let muted = site_menu_items(&[(Camera, Denied)], true);
        assert_eq!(muted[1..], [None, some("Unmute Current Page", UNMUTE_ID.to_string()), None]);
        assert_eq!(site_menu_items(&[], true), vec![some("Unmute Current Page", UNMUTE_ID.to_string()), None]);
        assert!(site_menu_items(&[], false).is_empty());
        assert_eq!(
            site_menu_items(&[(Notification, Granted), (AudioPlayback, Granted)], false),
            vec![some("Reset Audio Playback Permission (Allowed)", r(AudioPlayback)), some("Reset Notifications Permission (Allowed)", r(Notification)), None]
        );
    }

    #[test]
    fn menu_ids_are_closed_and_the_origin_line_keeps_the_host_end() {
        use PermissionType::*;
        assert_eq!(menu_pick(&format!("{RESET_PREFIX}camera")), Some(MenuPick::Reset(Camera)));
        assert_eq!(menu_pick(&format!("{RESET_PREFIX}audioPlayback")), Some(MenuPick::Reset(AudioPlayback)));
        assert_eq!(menu_pick(UNMUTE_ID), Some(MenuPick::Unmute));
        for bad in ["", "browser-menu:reset-permission:", "browser-menu:reset-permission:Camera", "browser-menu:reset-permission:all", "Settings"] {
            assert_eq!(menu_pick(bad), None, "{bad:?}");
        }
        assert_eq!(menu_origin_line("https://xn--e1afmkfd.xn--p1ai"), "https://xn--e1afmkfd.xn--p1ai", "punycode as it is");
        let long = format!("https://{}.example.com", "a".repeat(80));
        let line = menu_origin_line(&long);
        assert!(line.chars().count() <= 48 && line.ends_with("example.com") && line.contains('\u{2026}'));
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
        // saved Allow for a kind Termurion asks about can ever come out as Allow.
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
        // Nothing saved for a kind Termurion asks about: Deny until the person answers.
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
    fn a_page_cannot_pose_as_termurion() {
        let (t, b) = notification_text("Session Ended", "Termurion: your session \"x\" just terminated\u{202e}\n", "https://evil.example");
        assert_eq!(t, "https://evil.example", "the origin is always the title");
        assert!(!b.contains('\u{202e}') && !b.contains('\n'));
        assert!(b.starts_with("Session Ended: "));
        // Task 6 review I2: a long lookalike host keeps its END (the real domain and the port),
        // cut from the left behind an ellipsis; never the lookalike head alone.
        let fake = format!("https://termurion-update.microsoft.com.{}.evil.example:8443", "a".repeat(60));
        let (t, b) = notification_text(&"x".repeat(500), &"y".repeat(500), &fake);
        assert!(t.chars().count() <= 60, "{t}");
        assert!(t.starts_with("https://\u{2026}") && t.ends_with(".evil.example:8443"), "the sender's domain is visible: {t}");
        assert!(!t.contains("microsoft.com"), "{t}");
        let long = format!("https://{}.example", "a".repeat(200));
        assert!(notification_text("t", "b", &long).0.ends_with("aaa.example"));
        assert_eq!(origin_label("https://a.b:8443", 60), "https://a.b:8443", "a short origin is whole");
        // Minor 1: a cut text says so with an ellipsis.
        assert!(b.chars().count() <= 200 && b.ends_with('\u{2026}'), "{b}");
        assert!(!notification_text("Hi", "there", "https://a.b").1.contains('\u{2026}'));
        // Minor 5: zero-width characters are dropped from what a page wrote.
        let (_, b) = notification_text("Ta\u{200b}ndem", "x\u{200d}y\u{feff}", "https://a.b");
        assert_eq!(b, "Tandem: xy");
    }

    #[test]
    fn one_web_note_is_on_screen() {
        let mut n = NoteScreen::default();
        let g = |pane, token, shown_at| NoteGone { pane, token, shown_at };
        assert_eq!(n.shown(1, 1, "https://a.b", 4), None);
        assert_eq!(n.origin_of(1, 1), Some("https://a.b"));
        assert_eq!(n.origin_of(1, 2), None, "an older or other token is not on screen");
        assert_eq!(n.shown(1, 2, "https://c.d", 5), Some(g(1, 1, 4)), "the replaced one is returned to be reported closed");
        assert_eq!(n.take_pane(9), None);
        assert_eq!(n.take_origin("https://a.b"), None);
        assert_eq!(n.take_origin("https://c.d"), Some(g(1, 2, 5)), "a reset origin's note is taken");
        assert_eq!(n.take(), None);
        n.shown(1, 3, "https://c.d", 6);
        assert_eq!(n.take_pane(1), Some(g(1, 3, 6)));
        assert_eq!(n.take(), None);
    }

    /// Task 6 review I1: a page can close only its OWN notification. A Termurion alert shown after it
    /// (any caller of `notify::show_with`, not only `post_alert`) moves the show count, so the
    /// page's close clears nothing; the page is still told its note closed.
    #[test]
    fn a_web_note_closes_only_itself() {
        let mut n = NoteScreen::default();
        n.shown(1, 7, "https://a.b", 10);
        // A Termurion alert ("debug logging was turned on") is shown: the count is 11 now.
        let gone = n.take().unwrap();
        assert!(!crate::notify::still_shown(gone.shown_at, 11), "the Termurion alert is not the page's to clear");
        assert!(crate::notify::still_shown(gone.shown_at, 10), "its own still is");
    }

    #[test]
    fn audio_follows_iterm2() {
        assert!(premute(None) && premute(Some(Decision::Denied)) && !premute(Some(Decision::Granted)));
        let (g, d) = (Some(Decision::Granted), Some(Decision::Denied));
        assert_eq!(on_playing(false, d, true), AudioAct::Mute);
        assert_eq!(on_playing(false, d, false), AudioAct::Mute);
        assert_eq!(on_playing(false, g, false), AudioAct::Nothing);
        assert_eq!(on_playing(false, g, true), AudioAct::Unmute, "a page pre-muted before its origin was granted never stays silent");
        assert_eq!(on_playing(false, None, true), AudioAct::Ask);
        assert_eq!(on_playing(false, None, false), AudioAct::Ask);
        for (saved, muted) in [(g, true), (None, false), (d, true)] {
            assert_eq!(on_playing(true, saved, muted), AudioAct::Nothing, "disabled after a once answer");
        }
        assert_eq!(apply(AudioAnswer::AllowOnce), (None, true, true));
        assert_eq!(apply(AudioAnswer::AllowAlways), (g, true, false));
        assert_eq!(apply(AudioAnswer::DenyOnce), (None, false, true));
        assert_eq!(apply(AudioAnswer::DenyAlways), (d, false, false));
        assert_eq!(apply(AudioAnswer::Closed), (None, false, false));
    }

    #[test]
    fn audio_bar_follows_iterm2() {
        assert_eq!(audio_prompt("https://a.b"), "Audio was muted. Allow playback by https://a.b?");
        assert_eq!(audio_bar_id("https://a.b"), "NoSyncMuteAudio_https://a.b");
        assert_eq!(AUDIO_BUTTONS, ["Allow _Once", "Allow _Always", "_Deny Once", "De_ny Always"]);
        let bar = crate::infobar::Bar { id: audio_bar_id("https://a.b"), text: audio_prompt("https://a.b"), buttons: AUDIO_BUTTONS.iter().map(|b| b.to_string()).collect(), important: true };
        let labels: Vec<String> = (0..4).map(|i| bar.label(i)).collect();
        assert_eq!(labels, ["Allow Once", "Allow Always", "Deny Once", "Deny Always"]);
        assert_eq!([bar.shortcut('o'), bar.shortcut('a'), bar.shortcut('d'), bar.shortcut('n')], [Some(0), Some(1), Some(2), Some(3)]);
        assert_eq!(AudioAnswer::parse(Some(0)), AudioAnswer::AllowOnce);
        assert_eq!(AudioAnswer::parse(Some(1)), AudioAnswer::AllowAlways);
        assert_eq!(AudioAnswer::parse(Some(2)), AudioAnswer::DenyOnce);
        assert_eq!(AudioAnswer::parse(Some(3)), AudioAnswer::DenyAlways);
        assert_eq!(AudioAnswer::parse(Some(4)), AudioAnswer::Closed);
        assert_eq!(AudioAnswer::parse(None), AudioAnswer::Closed);
    }

    /// A pane the way `App` drives it: the saved rows, the view's mute, the pane's URL and the bars.
    struct Pane {
        saved: std::collections::HashMap<String, Decision>,
        st: AudioState,
        muted: bool,
        url: String,
        calls: Vec<String>,
        /// (id, text, epoch, url, origin) of the bar showing.
        bar: Option<(String, String, u64, String, String)>,
        announces: usize,
    }

    impl Pane {
        fn new() -> Pane {
            Pane { saved: Default::default(), st: AudioState::default(), muted: false, url: String::new(), calls: vec![], bar: None, announces: 0 }
        }
        fn saved(&self, url: &str) -> Option<Decision> {
            normalize_origin(url).and_then(|o| self.saved.get(&o).copied())
        }
        fn set_muted(&mut self, on: bool) {
            self.muted = on;
            self.calls.push(format!("set_muted({on})"));
        }
        fn starting(&mut self, url: &str, redirect: bool) {
            self.url = url.to_string();
            let on = self.st.starting(redirect, self.saved(url));
            self.set_muted(on);
        }
        fn playing(&mut self, on: bool) {
            if !on {
                return;
            }
            let origin = normalize_origin(&self.url).unwrap();
            let (set, ask) = self.st.playing(self.saved(&self.url), self.muted);
            if let Some(m) = set {
                self.set_muted(m);
            }
            if ask {
                self.announces += 1;
                if self.bar.as_ref().is_some_and(|b| b.0 == audio_bar_id(&origin)) {
                    // The duplicate id answers None at once.
                    return self.answer_for(None, self.st.epoch, self.url.clone(), origin);
                }
                self.bar = Some((audio_bar_id(&origin), audio_prompt(&origin), self.st.epoch, self.url.clone(), origin));
            }
        }
        fn press(&mut self, index: usize) {
            let (_, _, epoch, url, origin) = self.bar.take().unwrap();
            self.answer_for(Some(index), epoch, url, origin);
        }
        fn answer_for(&mut self, index: Option<usize>, epoch: u64, url: String, origin: String) {
            let same_origin = normalize_origin(&self.url).as_deref() == Some(origin.as_str());
            let out = self.st.answered(epoch, self.url == url, same_origin, AudioAnswer::parse(index));
            if let Some(d) = out.save {
                self.saved.insert(origin, d);
            }
            if out.unmute {
                self.set_muted(false);
            }
        }
    }

    #[test]
    fn browser_audio_flow_follows_iterm2() {
        // Nothing saved: muted at start, asked on play, Allow Always saves and unmutes.
        let mut p = Pane::new();
        p.st.muted_by_us = true;
        p.st.disabled = true;
        p.starting("https://a.b/", false);
        assert_eq!(p.calls, ["set_muted(true)"]);
        assert!(!p.st.muted_by_us && !p.st.disabled, "a new document resets both");
        p.playing(true);
        assert!(p.st.muted_by_us);
        assert_eq!(p.bar.as_ref().map(|b| (b.0.as_str(), b.1.as_str())), Some(("NoSyncMuteAudio_https://a.b", "Audio was muted. Allow playback by https://a.b?")));
        p.press(1);
        assert_eq!(p.saved.get("https://a.b"), Some(&Decision::Granted));
        assert!(!p.muted && !p.st.muted_by_us);
        p.starting("https://a.b/other", false);
        assert_eq!(p.calls.last().unwrap(), "set_muted(false)", "a later start to the same origin is unmuted");

        // Deny Once leaves it muted and silences the question for this document.
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(true);
        p.press(2);
        assert!(p.muted && p.st.muted_by_us && p.st.disabled);
        let asked = p.announces;
        p.playing(true);
        assert_eq!(p.announces, asked, "disabled: no second question");
        assert!(p.saved.is_empty());
        // The next document asks again.
        p.starting("https://a.b/", false);
        p.playing(true);
        assert_eq!(p.announces, asked + 1);

        // A redirect only re-evaluates the pre-mute and keeps what the first start set.
        let mut p = Pane::new();
        p.saved.insert("https://g.h".into(), Decision::Granted);
        p.starting("https://c.d/", false);
        p.st.disabled = true;
        p.st.muted_by_us = true;
        p.calls.clear();
        p.starting("https://g.h/", true);
        assert_eq!(p.calls, ["set_muted(false)"]);
        assert!(p.st.disabled && p.st.muted_by_us, "a redirect does not reset");
        let epoch = p.st.epoch;
        p.starting("https://g.h/", false);
        assert_eq!(p.st.epoch, epoch + 1, "reload or back/forward (any kind) is a new document");

        // Granted from another pane: unmuted on the next play.
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.saved.insert("https://a.b".into(), Decision::Granted);
        p.calls.clear();
        p.playing(true);
        assert_eq!(p.calls, ["set_muted(false)"]);
        assert!(p.bar.is_none() && !p.st.muted_by_us);

        // Denied: muted quietly, "Unmute Current Page" shows (muted_by_us).
        let mut p = Pane::new();
        p.saved.insert("https://a.b".into(), Decision::Denied);
        p.starting("https://a.b/", false);
        p.playing(true);
        assert!(p.bar.is_none() && p.muted && p.st.muted_by_us);
        p.st.unmute();
        p.set_muted(false);
        assert!(!p.st.muted_by_us && !p.muted);

        // Pause does nothing.
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(false);
        assert!(p.bar.is_none() && !p.st.muted_by_us);

        // Allow Once after the pane moved on does not unmute; nor does it disable the new page.
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(true);
        p.starting("https://a.b/next", false);
        p.press(0);
        assert!(p.muted && !p.st.disabled);
        // Allow Always after the pane moved to another origin saves the row but leaves the page muted.
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(true);
        p.starting("https://c.d/", false);
        p.press(1);
        assert_eq!(p.saved.get("https://a.b"), Some(&Decision::Granted));
        assert!(p.muted, "the new origin's page stays muted");
        // Same origin, new document: Allow Always unmutes (the origin is granted).
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(true);
        p.starting("https://a.b/x", false);
        p.press(1);
        assert!(!p.muted);
        // Deny Always saves whatever happened; Deny Once from a stale document disables nothing.
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(true);
        p.starting("https://c.d/", false);
        p.press(3);
        assert_eq!(p.saved.get("https://a.b"), Some(&Decision::Denied));
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(true);
        p.starting("https://a.b/", false);
        p.press(2);
        assert!(!p.st.disabled, "a stale Deny Once leaves the new document's question on");

        // A second play event while the bar shows asks again; the duplicate answers None and changes nothing.
        let mut p = Pane::new();
        p.starting("https://a.b/", false);
        p.playing(true);
        p.playing(true);
        assert_eq!(p.announces, 2);
        assert!(p.bar.is_some() && p.muted && !p.st.disabled && p.saved.is_empty());
    }
}

#[cfg(test)]
mod page_tests {
    use super::*;
    use crate::browser_db_perms::PermissionRow;
    use serde_json::json;

    fn row(origin: &str, kind: PermissionType, decision: Decision, created: f64) -> PermissionRow {
        PermissionRow { origin: origin.to_string(), kind, decision, created_at: created, updated_at: created + 0.5 }
    }

    fn five() -> Vec<PermissionRow> {
        use Decision::*;
        use PermissionType::*;
        vec![
            row("https://a.b", Notification, Granted, 1.0),
            row("https://c.d", Camera, Denied, 2.0),
            row("https://e.f", Geolocation, Granted, 3.0),
            row("https://a.b", Camera, Granted, 4.0),
            row("https://g.h", Microphone, Denied, 5.0),
        ]
    }

    fn q(offset: usize, limit: usize, search: &str, ty: &str, status: &str) -> PageQuery {
        PageQuery { offset, limit, search: search.into(), type_filter: ty.into(), status_filter: status.into() }
    }

    fn created(v: &[serde_json::Value]) -> Vec<i64> {
        v.iter().map(|r| r["createdAt"].as_f64().unwrap() as i64).collect()
    }

    #[test]
    fn permissions_page_follows_iterm2() {
        assert_eq!(PAGE_URL, "iterm2-about:permissions");
        assert_eq!(PAGE_HANDLER, "iterm2-about:permissions");
        assert_eq!(PageQuery::from_message(&json!({"action": "loadPermissions"})), q(0, 50, "", "", ""));
        let full = json!({"action": "loadPermissions", "offset": 4, "limit": 2, "searchQuery": "x", "permissionTypeFilter": "camera", "statusFilter": "denied"});
        assert_eq!(PageQuery::from_message(&full), q(4, 2, "x", "camera", "denied"));
        let rows = five();
        let (page, more) = page_rows(&rows, &q(0, 2, "", "", ""));
        assert_eq!((created(&page), more), (vec![5, 4], true));
        let (page, more) = page_rows(&rows, &q(4, 2, "", "", ""));
        assert_eq!((created(&page), more), (vec![1], false));
        let (page, _) = page_rows(&rows, &q(0, 50, "", "camera", ""));
        assert_eq!(created(&page), vec![4, 2], "only camera rows");
        let (page, _) = page_rows(&rows, &q(0, 50, "", "x", ""));
        assert_eq!(page.len(), 5, "an unknown type filter filters nothing (BPVH:114-118)");
        let (page, _) = page_rows(&rows, &q(0, 50, "", "", "denied"));
        assert_eq!(created(&page), vec![5, 2]);
        let (page, _) = page_rows(&rows, &q(0, 50, "", "", "nope"));
        assert_eq!(page.len(), 5, "an unknown status filter filters nothing");
        let (page, _) = page_rows(&rows, &q(0, 50, "LOCA", "", ""));
        assert_eq!(created(&page), vec![3], "the display name Location matches a geolocation row");
        let (page, _) = page_rows(&rows, &q(0, 50, "A.b", "", ""));
        assert_eq!(created(&page), vec![4, 1], "lower-cased contains on the origin");
        let (page, more) = page_rows(&rows, &q(9, 2, "", "", ""));
        assert!(page.is_empty() && !more, "an offset past the end is empty, never a trap");
        let (page, _) = page_rows(&rows, &q(0, 1, "", "", ""));
        assert_eq!(page[0], json!({"origin": "https://g.h", "permissionType": "microphone", "decision": "denied", "createdAt": 5.0, "updatedAt": 5.5}));
        assert_eq!(page[0].as_object().unwrap().len(), 5);
    }

    #[test]
    fn a_hostile_query_is_bounded() {
        let bad = json!({"offset": -3, "limit": 0, "searchQuery": 5, "permissionTypeFilter": ["camera"], "statusFilter": null});
        assert_eq!(PageQuery::from_message(&bad), q(0, 1, "", "", ""), "wrong types are defaults, the limit is at least 1");
        let big = json!({"offset": 9_000_000_000_i64, "limit": 9_000_000_000_i64});
        let p = PageQuery::from_message(&big);
        assert!(p.offset <= 100_000 && p.limit <= 100);
        let (page, _) = page_rows(&five(), &q(usize::MAX, usize::MAX, "", "", ""));
        assert!(page.is_empty(), "no overflow on a huge offset or limit");
    }

    #[test]
    fn page_messages_come_only_from_the_page() {
        assert!(page_message_allowed("iterm2-about:permissions", None));
        assert!(!page_message_allowed("iterm2-about:history", None));
        assert!(!page_message_allowed("iterm2-about:permissions", Some(3)), "an iframe");
        assert!(!page_message_allowed("iterm2-about:permissions?x", None));
        assert!(!page_message_allowed("ITERM2-ABOUT:permissions", None));
        assert!(!page_message_allowed("https://a.b/iterm2-about:permissions", None));
        assert!(!page_message_allowed("", None));
    }

    #[test]
    fn clear_all_visits_each_origin_once() {
        let got = clear_origins(&five());
        assert_eq!(got.len(), 4, "a.b once");
        assert_eq!(got.iter().filter(|o| *o == "https://a.b").count(), 1);
    }
}
