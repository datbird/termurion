//! The in-app routes that replace iTerm2's `iterm2:` links for chats (`AppKit/
//! iTermApplicationDelegate.m:2016-2135`, `PTYSession.m:237-247`). Tandem's routes are reached
//! only from its own indicator menu and from a terminal command cell's reveal-mark link, never
//! from text a model wrote, so they carry no token. Pure.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    UnlinkSessionChat(String),
    RevealChatForSession(String),
    DisableStreaming(String),
    RevealMark { session: String, mark: String },
}

const SCHEME: &str = "iterm2:";
const UNLINK: &str = "unlink-session-chat";
const REVEAL_CHAT: &str = "reveal-chat-for-session";
const DISABLE: &str = "disable-streaming-session-chat";
const REVEAL_MARK: &str = "reveal-mark";

/// Percent-encodes everything but unreserved characters, as a query value.
fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Decodes `%XX`; None for a malformed or non-UTF-8 value.
fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The route as iTerm2's URL, without the token.
pub fn format(r: &Route) -> String {
    match r {
        Route::UnlinkSessionChat(s) => format!("{SCHEME}{UNLINK}?s={}", encode(s)),
        Route::RevealChatForSession(s) => format!("{SCHEME}{REVEAL_CHAT}?s={}", encode(s)),
        Route::DisableStreaming(s) => format!("{SCHEME}{DISABLE}?s={}", encode(s)),
        Route::RevealMark { session, mark } => format!("{SCHEME}{REVEAL_MARK}?s={}&m={}", encode(session), encode(mark)),
    }
}

/// A terminal command cell's reveal-mark URL for a session and a mark id.
pub fn mark_url(session: &str, mark: u32) -> String {
    format(&Route::RevealMark { session: session.to_string(), mark: mark.to_string() })
}

/// The route a URL names; imported URLs carry a `t=` token, which is ignored. Unknown paths,
/// a missing session, or a mark that is not a number give None.
pub fn parse(url: &str) -> Option<Route> {
    let rest = url.strip_prefix(SCHEME)?;
    let (path, query) = rest.split_once('?')?;
    let mut session = None;
    let mut mark = None;
    for pair in query.split('&') {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        match k {
            "s" => session = Some(decode(v)?),
            "m" => mark = Some(decode(v)?),
            _ => {}
        }
    }
    let session = session.filter(|s| !s.is_empty())?;
    match path {
        UNLINK => Some(Route::UnlinkSessionChat(session)),
        REVEAL_CHAT => Some(Route::RevealChatForSession(session)),
        DISABLE => Some(Route::DisableStreaming(session)),
        REVEAL_MARK => {
            let mark = mark.filter(|m| !m.is_empty() && m.bytes().all(|b| b.is_ascii_digit()))?;
            Some(Route::RevealMark { session, mark })
        }
        _ => None,
    }
}

/// The indicator menu's items: the popover's link labels with what each does (`IH:380-400`).
pub fn indicator_routes(linked: bool, streaming: bool, session: &str) -> Vec<(String, Route)> {
    let mut out = Vec::new();
    if linked {
        out.push(("Unlink from AI Chat".to_string(), Route::UnlinkSessionChat(session.to_string())));
        out.push(("Reveal AI Chat".to_string(), Route::RevealChatForSession(session.to_string())));
    }
    if streaming {
        out.push(("Stop sending".to_string(), Route::DisableStreaming(session.to_string())));
    }
    out
}

/// What a terminal command cell's link may run: only a reveal-mark route (minor m10); any other
/// route there is ignored.
pub fn from_command_cell(url: &str) -> Option<Route> {
    parse(url).filter(|r| matches!(r, Route::RevealMark { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_round_trip() {
        let unlink = Route::UnlinkSessionChat("s1".into());
        let reveal = Route::RevealChatForSession("s1".into());
        let stop = Route::DisableStreaming("s1".into());
        assert_eq!(format(&unlink), "iterm2:unlink-session-chat?s=s1");
        assert_eq!(format(&reveal), "iterm2:reveal-chat-for-session?s=s1");
        assert_eq!(format(&stop), "iterm2:disable-streaming-session-chat?s=s1");
        assert_eq!(mark_url("s1", 7), "iterm2:reveal-mark?s=s1&m=7");
        for r in [unlink, reveal, stop, Route::RevealMark { session: "s1".into(), mark: "7".into() }] {
            assert_eq!(parse(&format(&r)), Some(r));
        }
        // An imported URL carries a token.
        assert_eq!(parse("iterm2:unlink-session-chat?s=s1&t=abc"), Some(Route::UnlinkSessionChat("s1".into())));
        assert_eq!(parse("iterm2:reveal-mark?t=abc&s=s1&m=7"), Some(Route::RevealMark { session: "s1".into(), mark: "7".into() }));
        // Percent encoding, both ways.
        assert_eq!(parse("iterm2:unlink-session-chat?s=a%20b%26c"), Some(Route::UnlinkSessionChat("a b&c".into())));
        assert_eq!(format(&Route::UnlinkSessionChat("a b&c".into())), "iterm2:unlink-session-chat?s=a%20b%26c");
        // Not routes.
        assert_eq!(parse("https://x"), None);
        assert_eq!(parse("iterm2:unlink-session-chat"), None, "no session");
        assert_eq!(parse("iterm2:unlink-session-chat?s="), None, "blank session");
        assert_eq!(parse("iterm2:unlink-session-chat?s=%zz"), None, "bad escape");
        assert_eq!(parse("iterm2:format-c?s=1"), None, "unknown path");
        assert_eq!(parse("iterm2:reveal-mark?s=1&m=x"), None, "mark is a number");
        assert_eq!(parse("iterm2:reveal-mark?s=1"), None, "no mark");
        // The indicator menu.
        assert_eq!(
            indicator_routes(true, true, "s1"),
            vec![("Unlink from AI Chat".to_string(), Route::UnlinkSessionChat("s1".into())), ("Reveal AI Chat".to_string(), Route::RevealChatForSession("s1".into())), ("Stop sending".to_string(), Route::DisableStreaming("s1".into()))]
        );
        assert_eq!(indicator_routes(true, false, "s1").len(), 2);
        assert!(indicator_routes(false, false, "s1").is_empty());
    }

    #[test]
    fn a_command_cell_reaches_only_reveal_mark() {
        assert!(from_command_cell("iterm2:reveal-mark?s=s1&m=7").is_some());
        assert_eq!(from_command_cell("iterm2:unlink-session-chat?s=s1"), None);
        assert_eq!(from_command_cell("iterm2:disable-streaming-session-chat?s=s1"), None);
        assert_eq!(from_command_cell("iterm2:reveal-chat-for-session?s=s1"), None);
        assert_eq!(from_command_cell("https://example.com"), None);
    }
}
