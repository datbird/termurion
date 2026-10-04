//! Receiving ssh: URLs from outside Tandem (m14e Task 3): the command line a protocol handler
//! passes, and the per-user pipe that hands a URL to a running Tandem. A URL here comes from a
//! browser or any program of the same user, so it is hostile input: one `url_ok` rule serves the
//! command line and the pipe, and `ssh_url::parse` is the second gate before anything opens.

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

/// The one rule for a URL from outside (P10): scheme `ssh` (ignoring case), at most 8 KiB, no
/// quote, no control, whitespace or invisible formatting character.
pub fn url_ok(url: &str) -> bool {
    url.len() <= MAX_URL
        && url.split_once(':').is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("ssh"))
        && !url.chars().any(|c| c == '"' || crate::ssh_url::is_hostile_char(c))
}

/// `argv` is the whole `args_os`, `argv[0]` included. `Url` only for exactly `[exe, "--url", u]`
/// with `url_ok(u)`; `None` when no argument after `argv[0]` is `--url` (ignoring case) or starts
/// with `ssh:` (ignoring case); anything else is `Refused`.
pub fn url_argument(argv: &[OsString]) -> UrlArg {
    let names_url = |a: &OsString| {
        let s = a.to_string_lossy();
        s.eq_ignore_ascii_case("--url") || s.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("ssh:"))
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
    if !url_ok(url) {
        return None;
    }
    Some(format!("open-url\n{url}\n").into_bytes())
}

/// The URL of a message exactly as `pipe_message` makes it; None for anything else.
pub fn parse_message(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let url = text.strip_prefix("open-url\n")?.strip_suffix('\n')?;
    url_ok(url).then(|| url.to_string())
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
    format!("{n} ssh link{} ignored", if n == 1 { "" } else { "s" })
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
    let parsed = crate::ssh_url::parse(url)?;
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
    Some(format!("Opened an ssh link to {who}."))
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
pub use pipe::{hand_to_running, hand_to_running_on, listen, listen_on};

#[cfg(windows)]
mod pipe {
    use super::*;
    use crate::winpipe;

    /// Hands `url` to a running Tandem of this user, retrying a busy pipe briefly.
    pub fn hand_to_running(url: &str) -> AfterHand {
        let name = match winpipe::url_pipe_name() {
            Ok(n) => n,
            Err(e) => return AfterHand::Refuse(format!("no link pipe name: {e}")),
        };
        let mut attempt = || {
            let handed = hand_to_running_on(&name, url, &allow_foreground);
            if let Handed::OtherOwner(e) = &handed {
                crate::host::log(&format!("url: the link pipe is held by another owner, so this Tandem runs: {e}"));
            }
            handed
        };
        hand_over(&mut attempt, 3, &|| std::thread::sleep(Duration::from_millis(300)))
    }

    /// Lets the Tandem that serves the pipe (and only it) come forward (P16).
    fn allow_foreground(pid: u32) {
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid) };
    }

    pub fn hand_to_running_on(name: &str, url: &str, grant: &dyn Fn(u32)) -> Handed {
        let Some(message) = pipe_message(url) else { return Handed::Failed("the URL is not one Tandem accepts".into()) };
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
            Ok(pid) => grant(pid),
            Err(e) => return Handed::Failed(e),
        }
        if let Err(e) = pipe.write_all(&message) {
            return Handed::Failed(e);
        }
        // The listener closes the pipe when it has read; waiting for that keeps the message from
        // being dropped with a closed handle.
        let _ = pipe.read_timeout(&mut [0u8; 1], Duration::from_secs(2));
        Handed::Taken
    }

    pub fn listen(notify: Box<dyn Fn(String) + Send>) -> Result<std::thread::JoinHandle<()>, String> {
        listen_on(&winpipe::url_pipe_name()?, Duration::from_secs(2), notify)
    }

    pub fn listen_on(name: &str, wait: Duration, notify: Box<dyn Fn(String) + Send>) -> Result<std::thread::JoinHandle<()>, String> {
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
            if let Some(url) = read_message(&pipe, wait) {
                notify(url);
            }
            // A silent or finished client is closed either way, so it never blocks the next one.
            pipe.shutdown();
        }))
    }

    /// One message, read within `wait` of the accept and at most 8 KiB in all.
    fn read_message(pipe: &winpipe::Pipe, wait: Duration) -> Option<String> {
        let end = Instant::now() + wait;
        let mut bytes: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];
        while bytes.iter().filter(|b| **b == b'\n').count() < 2 {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() || bytes.len() > MAX_URL + 16 {
                return None;
            }
            match pipe.read_timeout(&mut chunk, left) {
                Ok(Some(n)) if n > 0 => bytes.extend_from_slice(&chunk[..n]),
                _ => break,
            }
        }
        parse_message(&bytes)
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
        assert_eq!(ignored_banner(1), "1 ssh link ignored");
        assert_eq!(ignored_banner(6), "6 ssh links ignored");
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

    #[cfg(windows)]
    mod windows {
        use super::super::*;
        use crate::winpipe::tests::test_name;
        use std::sync::mpsc;

        fn listening(name: &str, wait: Duration) -> mpsc::Receiver<String> {
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            listen_on(name, wait, Box::new(move |u| drop(tx.lock().unwrap().send(u)))).unwrap();
            rx
        }

        #[test]
        fn url_pipe_carries_one_url() {
            let name = test_name("url");
            assert_eq!(hand_to_running_on(&name, "ssh://h", &|_| {}), Handed::NoServer, "nothing listens");
            let rx = listening(&name, Duration::from_secs(2));
            assert_eq!(hand_to_running_on(&name, "ssh://h", &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "ssh://h");
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
