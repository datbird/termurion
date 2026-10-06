//! Receiving ssh: URLs from outside Tandem (m14e Task 3): the command line a protocol handler
//! passes, and the per-user pipe that hands a URL to a running Tandem. A URL here comes from a
//! browser or any program of the same user, so it is hostile input: one `url_ok` rule serves the
//! command line and the pipe, and `ssh_url::parse` is the second gate before anything opens.
//! The same pipe is the one channel for every request a second `tandem.exe` has (m13v1 Task 1:
//! `launch::Request`); `launch::decode` is its only parser.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::time::{Duration, Instant};

/// The longest URL taken from outside.
pub const MAX_URL: usize = 8 * 1024;
/// External URL tabs opened per `FLOOD_WINDOW` at most (P13).
pub const FLOOD_MAX: usize = 4;
pub const FLOOD_WINDOW: Duration = Duration::from_secs(10);
/// URLs that may wait for the first window (P13).
pub const QUEUE_MAX: usize = 8;

/// What the command line says about a URL (R14de-9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlArg {
    /// No URL involved: a plain start.
    None,
    Url(String),
    /// Names a URL but not in the one accepted form: nothing opens.
    Refused(String),
}

/// The schemes Tandem takes from outside (m13v1 P7): the one list for the command line, the pipe
/// and every check after them. iTerm2's `x-man-page` is macOS only and is not offered.
pub const EXTERNAL_SCHEMES: [&str; 3] = ["ssh", "telnet", "ftp"];

/// The one rule for a URL from outside (P10): a scheme of `EXTERNAL_SCHEMES` (ignoring case), at
/// most 8 KiB, no quote, no control, whitespace or invisible formatting character.
pub fn url_ok(url: &str) -> bool {
    url.len() <= MAX_URL
        && crate::url_open::scheme_of(url).is_some()
        && !url.chars().any(|c| c == '"' || crate::ssh_url::is_hostile_char(c))
}

/// `argv` is the whole `args_os`, `argv[0]` included. `Url` only for exactly `[exe, "--url", u]`
/// with `url_ok(u)`; `None` when no argument after `argv[0]` is `--url` (ignoring case) or starts
/// with an external scheme and a colon (ignoring case); anything else is `Refused`.
pub fn url_argument(argv: &[OsString]) -> UrlArg {
    let names_url = |a: &OsString| {
        let s = a.to_string_lossy();
        s.eq_ignore_ascii_case("--url") || crate::url_open::scheme_of(&s).is_some()
    };
    if !argv.iter().skip(1).any(names_url) {
        return UrlArg::None;
    }
    let refused = || UrlArg::Refused(format!("command line with {} arguments", argv.len().saturating_sub(1)));
    match argv {
        [_, flag, url] if flag.to_str() == Some("--url") => match url.to_str() {
            Some(u) if url_ok(u) => UrlArg::Url(u.to_string()),
            _ => refused(),
        },
        _ => refused(),
    }
}

/// "open-url\n" + url + "\n". None for a URL that fails `url_ok`.
pub fn pipe_message(url: &str) -> Option<Vec<u8>> {
    crate::launch::encode(&crate::launch::Request::Url { url: url.to_string() })
}

/// The URL of a message exactly as `pipe_message` makes it; None for anything else (a message of
/// another kind included: `launch::decode` reads those).
pub fn parse_message(bytes: &[u8]) -> Option<String> {
    match crate::launch::decode(bytes)? {
        crate::launch::Request::Url { url } => Some(url),
        _ => None,
    }
}

/// P13's flood cap: at most `FLOOD_MAX` external URLs admitted per `FLOOD_WINDOW`; the rest are
/// counted, to show one banner.
#[derive(Default)]
pub struct Flood {
    stamps: VecDeque<Instant>,
    ignored: usize,
}

impl Flood {
    /// True when a URL arriving at `now` may open; false counts it as ignored.
    pub fn admit(&mut self, now: Instant) -> bool {
        while self.stamps.front().is_some_and(|t| now.saturating_duration_since(*t) >= FLOOD_WINDOW) {
            self.stamps.pop_front();
        }
        // A new burst counts its own ignored links (fix r1 Minor).
        if self.stamps.is_empty() {
            self.ignored = 0;
        }
        if self.stamps.len() >= FLOOD_MAX {
            self.ignored += 1;
            return false;
        }
        self.stamps.push_back(now);
        true
    }

    /// Counts a URL dropped for another reason (the start queue was full).
    pub fn ignore(&mut self) {
        self.ignored += 1;
    }

    pub fn ignored(&self) -> usize {
        self.ignored
    }
}

/// The one banner for dropped URLs.
pub fn ignored_banner(n: usize) -> String {
    format!("{n} link{} ignored", if n == 1 { "" } else { "s" })
}

/// Adds an admitted URL to the ones waiting for the first window; false (counted as ignored)
/// when `QUEUE_MAX` already wait.
pub fn queue_for_start(pending: &mut Vec<String>, flood: &mut Flood, url: String) -> bool {
    if pending.len() >= QUEUE_MAX {
        flood.ignore();
        return false;
    }
    pending.push(url);
    true
}

/// The banner an external open shows, naming what the link connects to as `user@host:port`
/// through `shown_name` (URL text is the sender's). None for a URL with no host: it opens nothing.
pub fn open_banner(url: &str) -> Option<String> {
    let scheme = crate::url_open::scheme_of(url)?;
    let parsed = crate::ssh_url::parse_as(url, scheme)?;
    let host = parsed.host?;
    let mut who = String::new();
    if let Some(user) = &parsed.user {
        who.push_str(&crate::inert_files::shown_name(user));
        who.push('@');
    }
    who.push_str(&crate::inert_files::shown_name(&host));
    if let Some(port) = parsed.port {
        who.push_str(&format!(":{port}"));
    }
    let article = if scheme == "telnet" { "a" } else { "an" };
    Some(format!("Opened {article} {scheme} link to {who}."))
}

/// What handing a URL to a running Tandem came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handed {
    /// A running Tandem of this user took it.
    Taken,
    /// Nothing listens on the pipe: no Tandem of this user runs.
    NoServer,
    /// Another user (or another elevation) serves or holds the pipe name: this Tandem must run.
    OtherOwner(String),
    /// A Tandem may run but did not take it (the pipe stayed busy, a write failed).
    Failed(String),
}

/// Whether the process serving the launch pipe is Tandem: its token passes the peer rule (same
/// user, same elevation and integrity, no AppContainer or restricted token: `api_peer::judge`)
/// and its image is this Tandem's own program (case ignored). Anything else, an unreadable image
/// included, is not Tandem and is sent nothing.
pub fn server_verdict(theirs: &crate::api_peer::Token, mine: &crate::api_peer::Token, their_image: Option<&str>, their_session: Option<u32>, my_session: Option<u32>, my_image: &str) -> Result<(), String> {
    crate::api_peer::judge(theirs, mine)?;
    // The same Windows session (a same-user Tandem of another session never takes a request).
    match (their_session, my_session) {
        (Some(a), Some(b)) if a == b => {}
        _ => return Err("the launch pipe is held by a program of another session".into()),
    }
    match their_image {
        Some(i) if i.eq_ignore_ascii_case(my_image) => Ok(()),
        Some(_) => Err("the launch pipe is held by a program that is not this Tandem".into()),
        None => Err("the program holding the launch pipe could not be identified".into()),
    }
}

/// What a URL launch does after the hand-over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterHand {
    /// The running Tandem has the URL: this process ends.
    Exit,
    /// No Tandem of this user runs: this one starts and opens the URL.
    Start,
    /// This process ends without opening anything, with the reason to log.
    Refuse(String),
}

/// Tries `attempt` up to `tries` times, `pause` between tries, and says what the launch does.
pub fn hand_over(attempt: &mut dyn FnMut() -> Handed, tries: usize, pause: &dyn Fn()) -> AfterHand {
    let mut why = String::from("no attempt");
    for n in 0..tries {
        if n > 0 {
            pause();
        }
        match attempt() {
            Handed::Taken => return AfterHand::Exit,
            Handed::NoServer | Handed::OtherOwner(_) => return AfterHand::Start,
            Handed::Failed(e) => why = e,
        }
    }
    AfterHand::Refuse(format!("a running Tandem did not take the link ({why}); nothing opened"))
}

#[cfg(windows)]
pub use pipe::{hand_request_checked, hand_request_on, hand_to_running, hand_to_running_on, listen, listen_on, probe_on, probe_running};

#[cfg(windows)]
mod pipe {
    use super::*;
    use crate::winpipe;

    /// Hands `request` to a running Tandem of this user, retrying a busy pipe briefly.
    pub fn hand_to_running(request: &crate::launch::Request) -> AfterHand {
        let name = match winpipe::url_pipe_name() {
            Ok(n) => n,
            Err(e) => return AfterHand::Refuse(format!("no link pipe name: {e}")),
        };
        let mut attempt = || {
            let handed = hand_request_on(&name, request, &allow_foreground);
            if let Handed::OtherOwner(e) = &handed {
                crate::host::log(&format!("url: the link pipe is held by another owner, so this Tandem runs: {e}"));
            }
            handed
        };
        hand_over(&mut attempt, 3, &|| std::thread::sleep(Duration::from_millis(300)))
    }

    /// Whether a Tandem of this user answers on the launch pipe, with nothing sent (an
    /// `-Embedding` start asks before it makes anything: B9).
    pub fn probe_running() -> Handed {
        match winpipe::url_pipe_name() {
            Ok(n) => probe_on(&n, &verify_server),
            Err(e) => Handed::Failed(format!("no link pipe name: {e}")),
        }
    }

    /// `probe_running` on pipe `name`; a holder `verify` refuses is `OtherOwner`.
    pub fn probe_on(name: &str, verify: &dyn Fn(u32) -> Result<(), String>) -> Handed {
        let pipe = match winpipe::connect_owned(name, Duration::from_secs(1)) {
            Ok(Some(p)) => p,
            Ok(None) => return Handed::NoServer,
            Err(winpipe::ConnectError::Denied(e)) => return Handed::OtherOwner(e),
            Err(winpipe::ConnectError::Other(e)) => return Handed::Failed(e),
        };
        if let Err(e) = pipe.server_is_this_user() {
            return Handed::OtherOwner(e);
        }
        match pipe.server_pid() {
            Ok(pid) => match verify(pid) {
                Ok(()) => Handed::Taken,
                Err(e) => Handed::OtherOwner(e),
            },
            Err(e) => Handed::Failed(e),
        }
    }

    /// Lets the Tandem that serves the pipe (and only it) come forward (P16).
    fn allow_foreground(pid: u32) {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
    }

    pub fn hand_to_running_on(name: &str, url: &str, grant: &dyn Fn(u32)) -> Handed {
        hand_request_on(name, &crate::launch::Request::Url { url: url.to_string() }, grant)
    }

    /// The only writer to the pipe: one message, one connection.
    pub fn hand_request_on(name: &str, request: &crate::launch::Request, grant: &dyn Fn(u32)) -> Handed {
        hand_request_checked(name, request, grant, &verify_server)
    }

    /// `server_verdict` for the process `pid` serving the pipe.
    fn verify_server(pid: u32) -> Result<(), String> {
        let mine = std::env::current_exe().map_err(|e| e.to_string())?;
        server_verdict(&winpipe::process_token_facts(pid)?, &winpipe::own_token_facts()?, winpipe::process_image(pid).as_deref(), winpipe::session_of(pid), winpipe::session_of(std::process::id()), &mine.to_string_lossy())
    }

    /// `hand_request_on` with the server check given: a holder `verify` refuses gets nothing, and
    /// is `OtherOwner` (this Tandem runs without a listener, as when another user holds the name).
    pub fn hand_request_checked(name: &str, request: &crate::launch::Request, grant: &dyn Fn(u32), verify: &dyn Fn(u32) -> Result<(), String>) -> Handed {
        let Some(message) = crate::launch::encode(request) else { return Handed::Failed("the request is not one Tandem accepts".into()) };
        let pipe = match winpipe::connect_owned(name, Duration::from_secs(1)) {
            Ok(Some(p)) => p,
            Ok(None) => return Handed::NoServer,
            Err(winpipe::ConnectError::Denied(e)) => return Handed::OtherOwner(e),
            Err(winpipe::ConnectError::Other(e)) => return Handed::Failed(e),
        };
        if let Err(e) = pipe.server_is_this_user() {
            return Handed::OtherOwner(e);
        }
        match pipe.server_pid() {
            Ok(pid) => {
                if let Err(e) = verify(pid) {
                    return Handed::OtherOwner(e);
                }
                grant(pid)
            }
            Err(e) => return Handed::Failed(e),
        }
        if let Err(e) = pipe.write_all(&message) {
            return Handed::Failed(e);
        }
        // The listener closes the pipe when it has read (before it acts on the request), so a
        // close is the answer; a Tandem that does not close within 2 s is hung, not a taker.
        match pipe.read_timeout(&mut [0u8; 1], Duration::from_secs(2)) {
            Ok(None) => Handed::Failed("the running Tandem did not answer".into()),
            _ => Handed::Taken,
        }
    }

    pub fn listen(notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<std::thread::JoinHandle<()>, String> {
        listen_on(&winpipe::url_pipe_name()?, Duration::from_secs(2), notify)
    }

    pub fn listen_on(name: &str, wait: Duration, notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<std::thread::JoinHandle<()>, String> {
        let mut listener = winpipe::Listener::bind(name)?;
        Ok(std::thread::spawn(move || loop {
            let pipe = match listener.accept(Duration::from_secs(1)) {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(e) => {
                    crate::host::log(&format!("url: accept failed: {e}"));
                    if listener.renew().is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };
            // The DACL admits this user at this elevation only; nothing more is checked of the client.
            let request = read_message(&pipe, wait);
            // A silent or finished client is closed either way, so it never blocks the next one,
            // and before the request runs, so the sender's answer never waits for the callback.
            pipe.shutdown();
            if let Some(request) = request {
                notify(request);
            }
        }))
    }

    /// One message, read within `wait` of the accept and at most `launch::MAX_MESSAGE` in all:
    /// it ends where `launch::decode` accepts it. A longer, malformed or silent client is dropped.
    fn read_message(pipe: &winpipe::Pipe, wait: Duration) -> Option<crate::launch::Request> {
        let end = Instant::now() + wait;
        let mut bytes: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() || bytes.len() > crate::launch::MAX_MESSAGE {
                return None;
            }
            match pipe.read_timeout(&mut chunk, left) {
                Ok(Some(n)) if n > 0 => bytes.extend_from_slice(&chunk[..n]),
                _ => return None,
            }
            if let Some(request) = crate::launch::decode(&bytes) {
                return Some(request);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<OsString> {
        std::iter::once(r"C:\T\tandem.exe").chain(parts.iter().copied()).map(OsString::from).collect()
    }

    fn sequence(results: Vec<Handed>) -> (impl FnMut() -> Handed, std::rc::Rc<std::cell::Cell<usize>>) {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let seen = calls.clone();
        let mut results = results.into_iter();
        (move || {
            seen.set(seen.get() + 1);
            results.next().unwrap_or(Handed::Failed("no more".into()))
        }, calls)
    }

    /// Fix r1 Important: a second full Tandem starts only when none of this user runs (or
    /// another user holds the pipe); a busy or failed hand-over retries, then ends refused.
    #[test]
    fn a_busy_hand_over_never_starts_a_second_tandem() {
        let busy = || Handed::Failed("the pipe stayed busy".into());
        let (mut f, calls) = sequence(vec![busy(), busy(), busy(), busy()]);
        assert!(matches!(hand_over(&mut f, 3, &|| {}), AfterHand::Refuse(_)));
        assert_eq!(calls.get(), 3, "retried, then refused");
        let (mut f, calls) = sequence(vec![busy(), Handed::Taken]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Exit);
        assert_eq!(calls.get(), 2);
        let (mut f, _) = sequence(vec![busy(), Handed::NoServer]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Start, "the running Tandem ended meanwhile");
        let (mut f, calls) = sequence(vec![Handed::NoServer]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Start);
        assert_eq!(calls.get(), 1);
        let (mut f, _) = sequence(vec![Handed::OtherOwner("squatter".into())]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Start);
        let (mut f, _) = sequence(vec![Handed::Taken]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Exit);
    }

    #[test]
    fn url_arguments_follow_windows() {
        assert_eq!(url_argument(&argv(&["--url", "ssh://h"])), UrlArg::Url("ssh://h".into()));
        assert_eq!(url_argument(&argv(&["--url", "SSH://u@h:22/x"])), UrlArg::Url("SSH://u@h:22/x".into()));
        assert_eq!(url_argument(&argv(&[])), UrlArg::None);
        assert_eq!(url_argument(&argv(&["--other"])), UrlArg::None);
        for bad in [
            &["--url", "ssh://h", "--url", "ssh://evil"][..],
            &["--url", "ssh://h", "x"],
            &["--url", "ssh://h\" --x"],
            &["x", "--url", "ssh://h"],
            &["ssh://h"],
            &["--url"],
            &["--url", "http://h"],
            &["--URL", "ssh://h", "-oProxyCommand=calc"],
            &["--URL", "ssh://h"],
            // What `"%1"` becomes for `ssh://h\`: the backslash escapes the closing quote.
            &["--url", "ssh://h\""],
        ] {
            assert!(matches!(url_argument(&argv(bad)), UrlArg::Refused(_)), "{bad:?}");
        }
        // The same URL rule as the pipe (P10): a long, bidi, control or non-UTF-8 URL is refused.
        let long = format!("ssh://h/{}", "a".repeat(MAX_URL));
        assert!(matches!(url_argument(&argv(&["--url", &long])), UrlArg::Refused(_)));
        for bad in ["ssh://h/\u{202e}x", "ssh://h/\u{1b}[31m", "ssh://h/\u{85}", "ssh://h/\u{2028}", "ssh://h x", "ssh://h/\u{200b}"] {
            assert!(matches!(url_argument(&argv(&["--url", bad])), UrlArg::Refused(_)), "{bad:?}");
        }
        use std::os::windows::ffi::OsStringExt;
        let mut bad = argv(&["--url"]);
        bad.push(OsString::from_wide(&[0x73, 0x73, 0x68, 0x3a, 0xd800]));
        assert!(matches!(url_argument(&bad), UrlArg::Refused(_)));
    }

    #[test]
    fn pipe_message_follows_the_rule() {
        assert_eq!(pipe_message("ssh://h").as_deref(), Some(&b"open-url\nssh://h\n"[..]));
        for bad in ["ssh://h\n", "ssh://h\r", "ssh://h\0"] {
            assert_eq!(pipe_message(bad), None, "{bad:?}");
        }
        assert_eq!(pipe_message(&format!("ssh://{}", "a".repeat(MAX_URL - 6))).is_some(), true);
        assert_eq!(pipe_message(&format!("ssh://{}", "a".repeat(MAX_URL))), None);
        assert_eq!(parse_message(b"open-url\nssh://h\n").as_deref(), Some("ssh://h"));
        assert_eq!(parse_message(b"open-url\nssh://h"), None);
        assert_eq!(parse_message(b"other\nx\n"), None);
        assert_eq!(parse_message(b"open-url\nssh://h\xff\n"), None);
    }

    /// P10: one `url_ok` for the argv and the pipe: a URL the pipe or the argv would carry that
    /// is not a plain ssh: URL is refused at both.
    #[test]
    fn a_bad_url_is_refused_at_argv_and_pipe() {
        for bad in ["http://h", "ssh://h\"x", "ssh://h/\u{202e}", "ssh://h/\u{1b}", "ssh://h/\u{9b}", "ssh://h/\u{2029}", "file:///c:/x", "ssh://h/\u{feff}", "ssh://h y", ""] {
            assert!(!url_ok(bad), "{bad:?}");
            assert_eq!(pipe_message(bad), None, "{bad:?}");
            assert_eq!(parse_message(format!("open-url\n{bad}\n").as_bytes()), None, "{bad:?}");
            assert!(matches!(url_argument(&argv(&["--url", bad])), UrlArg::Refused(_)), "{bad:?}");
        }
        for good in ["ssh://h", "SSH://u@h:22/a%20b", "ssh:h"] {
            assert!(url_ok(good), "{good:?}");
        }
    }

    /// P13: at most 4 external URLs per 10 s, 8 queued for the first window, one banner for the rest.
    #[test]
    fn a_flood_of_urls_is_capped() {
        let t0 = Instant::now();
        let mut flood = Flood::default();
        let admitted = (0..10).filter(|i| flood.admit(t0 + Duration::from_millis(*i))).count();
        assert_eq!(admitted, FLOOD_MAX);
        assert_eq!(flood.ignored(), 6);
        assert!(!flood.admit(t0 + Duration::from_secs(9)), "still inside the window");
        assert!(flood.admit(t0 + Duration::from_secs(11)), "the window moved on");
        let later = t0 + Duration::from_secs(3600);
        let admitted = (0..5).filter(|i| flood.admit(later + Duration::from_millis(*i))).count();
        assert_eq!((admitted, flood.ignored()), (FLOOD_MAX, 1), "a new burst counts only its own ignored links");
        let mut pending = Vec::new();
        let mut flood = Flood::default();
        let queued = (0..12).filter(|i| queue_for_start(&mut pending, &mut flood, format!("ssh://h{i}"))).count();
        assert_eq!(queued, QUEUE_MAX);
        assert_eq!(pending.len(), QUEUE_MAX);
        assert_eq!(flood.ignored(), 4);
        assert_eq!(ignored_banner(1), "1 link ignored");
        assert_eq!(ignored_banner(6), "6 links ignored");
    }

    /// P13(b), (c): the banner names user@host:port through `shown_name`; a host-less URL has none.
    #[test]
    fn the_open_banner_names_the_target() {
        assert_eq!(open_banner("ssh://bob@example.com:2222/x").as_deref(), Some("Opened an ssh link to bob@example.com:2222."));
        assert_eq!(open_banner("ssh://example.com").as_deref(), Some("Opened an ssh link to example.com."));
        assert_eq!(open_banner("ssh:h"), None);
        assert_eq!(open_banner("ssh://u@/a"), None);
        let long = format!("ssh://{}@h", "u".repeat(2000));
        assert!(open_banner(&long).unwrap().chars().count() < 400, "a long name is cut by shown_name");
    }

    /// Review r1 Important 1: a pipe server is Tandem only when its token passes the peer rule
    /// and its image is this Tandem's own program.
    #[test]
    fn the_server_must_be_tandem() {
        const S: Option<u32> = Some(1);
        use crate::api_peer::{Token, HIGH_RID, LOW_RID, MEDIUM_RID};
        let mine = Token { sid: "S-1-5-21-1".into(), elevated: false, integrity: MEDIUM_RID, app_container: false, restricted: false };
        let exe = r"C:\Tandem\tandem.exe";
        assert_eq!(server_verdict(&mine, &mine, Some(r"c:\tandem\TANDEM.EXE"), S, S, exe), Ok(()), "same token, same image, case ignored");
        let low = Token { integrity: LOW_RID, ..mine.clone() };
        assert!(server_verdict(&low, &mine, Some(exe), S, S, exe).is_err(), "a low-integrity squatter");
        let high = Token { integrity: HIGH_RID, elevated: true, ..mine.clone() };
        assert!(server_verdict(&high, &mine, Some(exe), S, S, exe).is_err());
        assert!(server_verdict(&Token { app_container: true, ..mine.clone() }, &mine, Some(exe), S, S, exe).is_err());
        assert!(server_verdict(&Token { sid: "S-1-5-21-2".into(), ..mine.clone() }, &mine, Some(exe), S, S, exe).is_err());
        assert!(server_verdict(&mine, &mine, Some(r"C:\Temp\evil.exe"), S, S, exe).is_err(), "another image");
        assert!(server_verdict(&mine, &mine, None, S, S, exe).is_err(), "an unreadable image is refused");
        // Review r1 residual: the server must run in this process's session.
        assert!(server_verdict(&mine, &mine, Some(exe), Some(2), Some(1), exe).is_err(), "a same-user Tandem of another session");
        assert!(server_verdict(&mine, &mine, Some(exe), None, Some(1), exe).is_err(), "an unreadable session is refused");
        assert!(server_verdict(&mine, &mine, Some(exe), Some(1), None, exe).is_err());
        assert_eq!(server_verdict(&mine, &mine, Some(exe), Some(3), Some(3), exe), Ok(()));
    }

    #[cfg(windows)]
    mod windows {
        use super::super::*;
        use crate::winpipe::tests::test_name;
        use std::sync::mpsc;

        fn listening(name: &str, wait: Duration) -> mpsc::Receiver<String> {
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            listen_on(name, wait, Box::new(move |r| {
                if let crate::launch::Request::Url { url } = r {
                    drop(tx.lock().unwrap().send(url));
                }
            }))
            .unwrap();
            rx
        }

        /// Review r1 Important 1: a holder the verifier refuses is never sent a request, and the
        /// answer is `OtherOwner` (this Tandem then runs), not a hang or a hand-over.
        #[test]
        fn a_server_that_is_not_tandem_gets_no_request() {
            let name = test_name("squatter");
            let rx = listening_all(&name, Duration::from_secs(2));
            let granted = std::cell::Cell::new(false);
            let open = crate::launch::Request::Open { paths: vec![r"C:\w\a.ps1".into()] };
            let handed = hand_request_checked(&name, &open, &|_| granted.set(true), &|_| Err("not Tandem".to_string()));
            assert_eq!(handed, Handed::OtherOwner("not Tandem".into()));
            assert!(!granted.get(), "no foreground right goes to a stranger");
            assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "nothing was sent");
            assert_eq!(hand_request_on(&name, &open, &|_| {}), Handed::Taken, "the real check passes for this very program");
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), open);
        }

        #[test]
        fn url_pipe_carries_one_url() {
            let name = test_name("url");
            assert_eq!(hand_to_running_on(&name, "ssh://h", &|_| {}), Handed::NoServer, "nothing listens");
            let rx = listening(&name, Duration::from_secs(2));
            assert_eq!(hand_to_running_on(&name, "ssh://h", &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "ssh://h");
        }

        fn listening_all(name: &str, wait: Duration) -> mpsc::Receiver<crate::launch::Request> {
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            listen_on(name, wait, Box::new(move |r| drop(tx.lock().unwrap().send(r)))).unwrap();
            rx
        }

        fn every_kind() -> Vec<crate::launch::Request> {
            use crate::launch::Request::*;
            vec![
                Reopen,
                Open { paths: vec![r"C:\w\a.ps1".into(), r"D:\x y\b.sh".into()] },
                Url { url: "ssh://h".into() },
                NewWindow { guid: Some("ABC-1".into()) },
                NewTab { guid: None },
                Arrangement { name: "Dev".into() },
            ]
        }

        /// The plan's `forward_and_listen` on the built pipe: an `Open` arrives whole, nothing
        /// listening is answered fast, and a listener stuck in its callback is "no answer".
        #[test]
        fn forward_and_listen() {
            let name = test_name("launch-open");
            let t = Instant::now();
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::NoServer);
            assert!(t.elapsed() < Duration::from_millis(300), "no listener is known at once");
            let rx = listening_all(&name, Duration::from_secs(2));
            let open = crate::launch::Request::Open { paths: vec![r"C:\w\a.ps1".into()] };
            assert_eq!(hand_request_on(&name, &open, &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), open);

            let name = test_name("launch-hung");
            let (release, held) = mpsc::channel::<()>();
            let held = std::sync::Mutex::new(held);
            listen_on(&name, Duration::from_secs(2), Box::new(move |_| {
                let _ = held.lock().unwrap().recv();
            })).unwrap();
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::Taken, "the first is read and closed");
            let t = Instant::now();
            let second = hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {});
            assert!(matches!(second, Handed::Failed(_)), "the thread is stuck in the callback: no answer: {second:?}");
            assert!(t.elapsed() >= Duration::from_millis(1900) && t.elapsed() < Duration::from_secs(6), "{:?} {second:?}", t.elapsed());
            let _ = release.send(());
        }

        /// B1: a client that connects and says nothing never blocks the next request of any kind.
        #[test]
        fn a_silent_client_does_not_block_the_next_request() {
            for (i, request) in every_kind().into_iter().enumerate() {
                let name = test_name(&format!("launch-silent-{i}"));
                let rx = listening_all(&name, Duration::from_millis(200));
                let silent = crate::winpipe::connect(&name, Duration::from_secs(1)).unwrap().unwrap();
                let t = Instant::now();
                assert_eq!(hand_request_on(&name, &request, &|_| {}), Handed::Taken, "{request:?}");
                assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), request);
                assert!(t.elapsed() < Duration::from_secs(3));
                assert_eq!(silent.read_timeout(&mut [0u8; 1], Duration::from_secs(1)), Ok(Some(0)));
            }
        }

        /// P2: a hostile or oversized message on the wire reaches no callback, and the next
        /// well-formed one still does.
        #[test]
        fn hostile_wire_messages_reach_no_callback() {
            let name = test_name("launch-hostile");
            let rx = listening_all(&name, Duration::from_millis(300));
            let long = format!("arrangement\n{}\n", "a".repeat(crate::launch::MAX_MESSAGE));
            for bad in [&b"reopen\nreopen\n"[..], b"junk\n", b"arrangement\na\0\n", long.as_bytes(), b"\xff\xfe"] {
                let pipe = crate::winpipe::connect(&name, Duration::from_secs(1)).unwrap().unwrap();
                let _ = pipe.write_all(bad);
                assert_eq!(pipe.read_timeout(&mut [0u8; 1], Duration::from_secs(3)), Ok(Some(0)), "dropped and closed");
            }
            assert!(rx.try_recv().is_err(), "nothing was delivered");
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), crate::launch::Request::Reopen);
        }

        /// P16: the running Tandem's pid, never everyone, may take the foreground.
        #[test]
        fn the_server_pid_is_granted_the_foreground() {
            let name = test_name("url-pid");
            let _rx = listening(&name, Duration::from_secs(2));
            let granted = std::sync::Mutex::new(Vec::new());
            assert_eq!(hand_to_running_on(&name, "ssh://h", &|pid| granted.lock().unwrap().push(pid)), Handed::Taken);
            assert_eq!(*granted.lock().unwrap(), vec![std::process::id()]);
            let src = std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("url_launch.rs")).unwrap();
            let code = &src[..src.find("#[cfg(test)]").unwrap()];
            assert!(!code.contains(concat!("ASFW", "_ANY")), "AllowSetForegroundWindow takes a pid");
        }

        /// R14de-10: a client that connects and sends nothing is dropped and never blocks the next.
        #[test]
        fn silent_url_client_is_dropped() {
            let name = test_name("url-silent");
            let rx = listening(&name, Duration::from_millis(200));
            let silent = crate::winpipe::connect(&name, Duration::from_secs(1)).unwrap().unwrap();
            let t = Instant::now();
            assert_eq!(hand_to_running_on(&name, "ssh://h2", &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "ssh://h2");
            assert!(t.elapsed() < Duration::from_secs(3));
            assert_eq!(silent.read_timeout(&mut [0u8; 1], Duration::from_secs(1)), Ok(Some(0)), "the listener closed the silent pipe");
        }
    }
}
