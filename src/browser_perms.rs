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

    /// `BP:59-69`. `origin` is a normalized origin's host part as the caller chooses to show it.
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
}
