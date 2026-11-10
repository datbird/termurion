//! Receiving ssh: URLs from outside Termurion (m14e Task 3): the command line a protocol handler
//! passes, and the per-user pipe that hands a URL to a running Termurion. A URL here comes from a
//! browser or any program of the same user, so it is hostile input: one `url_ok` rule serves the
//! command line and the pipe, and `ssh_url::parse` is the second gate before anything opens.
//! The same pipe is the one channel for every request a second `termurion.exe` has (m13v1 Task 1:
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

/// The schemes Termurion takes from outside (m13v1 P7): the one list for the command line, the pipe
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

/// What handing a URL to a running Termurion came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handed {
    /// A running Termurion of this user took it.
    Taken,
    /// Nothing listens on the pipe: no Termurion of this user runs.
    NoServer,
    /// Another user (or another elevation) serves or holds the pipe name: this Termurion must run.
    OtherOwner(String),
    /// A Termurion may run but did not take it (the pipe stayed busy, a write failed).
    Failed(String),
}

/// Whether the process serving the launch pipe is Termurion: its token passes the peer rule (same
/// user, same elevation and integrity, no AppContainer or restricted token: `api_peer::judge`)
/// and its image is this Termurion's own program (case ignored). Anything else, an unreadable image
/// included, is not Termurion and is sent nothing.
pub fn server_verdict(theirs: &crate::api_peer::Token, mine: &crate::api_peer::Token, their_image: Option<&str>, their_session: Option<u32>, my_session: Option<u32>, my_image: &str) -> Result<(), String> {
    // The one peer rule, with the same Windows session (a same-user Termurion of another session never takes a request).
    let peer = |token: &crate::api_peer::Token, session| crate::api_peer::Peer { token: token.clone(), session };
    crate::api_peer::judge_peer(&peer(theirs, their_session), &peer(mine, my_session)).map_err(|e| if e == crate::api_peer::OTHER_SESSION { "the launch pipe is held by a program of another session".to_string() } else { e })?;
    match their_image {
        Some(i) if i.eq_ignore_ascii_case(my_image) => Ok(()),
        Some(_) => Err("the launch pipe is held by a program that is not this Termurion".into()),
        None => Err("the program holding the launch pipe could not be identified".into()),
    }
}

/// What a URL launch does after the hand-over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterHand {
    /// The running Termurion has the URL: this process ends.
    Exit,
    /// No Termurion of this user runs: this one starts and opens the URL.
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
    AfterHand::Refuse(format!("a running Termurion did not take the link ({why}); nothing opened"))
}

/// What a Termurion that cannot take its launch pipe does (13u S4): one rule for `run()` and for an
/// idle `-Embedding` process (round 5 race B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LostPipe {
    /// Every request went to the running Termurion: this process ends.
    Handed,
    /// A hand-over was refused (a holder that may be Termurion did not answer), or nothing was a
    /// Start: this process ends, so two Termurions never run.
    Refused,
    /// The holder is not Termurion (every kept request was a Start), or a handed-off console waits to be
    /// shown: this one runs, without the pipe.
    RunWithout,
}

/// The rule over each request's `hand_over` result; `console_waiting` is a handoff that would be lost.
/// The feature the Unix launch pipe reports as Unsupported.
pub const LISTEN_FEATURE: &str = "Single-instance link handoff";

/// Why `listen` did not serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenError {
    /// This OS has no launch pipe at all (not because another program holds it).
    Unsupported(crate::platform::Unsupported),
    /// The pipe could not be made or is taken; the text says why.
    Failed(String),
}

impl std::fmt::Display for ListenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ListenError::Unsupported(u) => u.fmt(f),
            ListenError::Failed(s) => f.write_str(s),
        }
    }
}

pub fn lost_pipe(after: &[AfterHand], console_waiting: bool) -> LostPipe {
    if console_waiting {
        return LostPipe::RunWithout;
    }
    if after.iter().all(|a| *a == AfterHand::Exit) {
        return LostPipe::Handed;
    }
    if after.iter().any(|a| matches!(a, AfterHand::Refuse(_))) || !after.contains(&AfterHand::Start) {
        return LostPipe::Refused;
    }
    LostPipe::RunWithout
}

/// The byte a listener answers with once it has a request (round 5 item 6): a sender counts its
/// request as taken only when it reads this.
pub const ACK: u8 = 0x06;

#[cfg(target_os = "macos")]
pub use no_pipe::{Serving, hand_to_running, listen, probe_running};
#[cfg(target_os = "linux")]
pub use sock::{Serving, hand_request_on, hand_to_running, listen, probe_running, serve_on, verify_image};
#[cfg(windows)]
pub use pipe::{Serving, hand_request_checked, hand_request_on, hand_to_running, hand_to_running_on, listen, listen_on, probe_on, probe_running, serve_on};

type TokenSink = Box<dyn Fn(String) + Send>;
static ACTIVATION: std::sync::Mutex<Option<TokenSink>> = std::sync::Mutex::new(None);

/// What the app does with the activation token a second start sent with its request (Linux):
/// called just before that request is handed on, on the same thread. Elsewhere nothing calls it.
pub fn on_activation(f: Box<dyn Fn(String) + Send>) {
    *ACTIVATION.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

#[cfg(target_os = "linux")]
fn tell_activation(token: String) {
    if let Some(f) = ACTIVATION.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        f(token);
    }
}

/// macOS has no single-instance link pipe: the system's reopen event wakes the running app, so nothing runs to hand to, and nothing can be served.
#[cfg(target_os = "macos")]
mod no_pipe {
    use super::*;

    /// No running Termurion can be found, so this one starts and opens the request itself.
    pub fn hand_to_running(_request: &crate::launch::Request) -> AfterHand {
        AfterHand::Start
    }

    /// Nothing answers: there is no pipe to ask.
    pub fn probe_running() -> Handed {
        Handed::NoServer
    }

    /// Always Err: the launch pipe is not available on Unix, and the caller logs the text.
    pub fn listen(_notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<Serving, ListenError> {
        Err(ListenError::Unsupported(crate::platform::Unsupported { feature: super::LISTEN_FEATURE, plural: false }))
    }

    /// A served pipe. Nothing makes one on Unix.
    pub struct Serving {
        never: std::convert::Infallible,
    }

    impl Serving {
        pub fn stop(&mut self) {
            match self.never {}
        }

        pub fn stopped(&self) -> bool {
            match self.never {}
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn unix_launch_pipe_is_unsupported() {
            assert!(matches!(listen(Box::new(|_| {})), Err(ListenError::Unsupported(_))));
            assert_eq!(probe_running(), Handed::NoServer);
        }
    }
}

/// The Linux launch socket: `launch.sock` in the private 0700 runtime folder
/// (`$XDG_RUNTIME_DIR/termurion`), 0600, a peer of another user dropped unread, and a socket left
/// by a dead Termurion replaced (`platform::unix::ipc`). The messages are the Windows pipe's, with
/// the desktop's activation token in front when the second start was given one.
#[cfg(target_os = "linux")]
mod sock {
    use super::*;
    use crate::platform::ipc;

    /// The token the desktop gave this start (GNOME, KDE: XDG activation; X11: startup notification).
    fn own_token() -> Option<String> {
        ["XDG_ACTIVATION_TOKEN", "DESKTOP_STARTUP_ID"].iter().find_map(|v| std::env::var(v).ok()).filter(|t| crate::launch::token_ok(t))
    }

    /// Hands `request` to a running Termurion of this user, retrying a busy socket briefly. With no
    /// socket name (no private runtime folder) this one starts, and says why.
    pub fn hand_to_running(request: &crate::launch::Request) -> AfterHand {
        let name = match ipc::url_pipe_name() {
            Ok(n) => n,
            Err(e) => {
                crate::host_base::log(&format!("launch: no launch socket ({e}), so this Termurion runs"));
                return AfterHand::Start;
            }
        };
        let token = own_token();
        // One id for this start: every retry carries it, so a request taken once is run once.
        let id = request_id();
        let mut attempt = || {
            let handed = hand_envelope_on(&name, request, Some(&id), token.as_deref(), &verify_server);
            if let Handed::OtherOwner(e) = &handed {
                crate::host_base::log(&format!("launch: the launch socket is held by another program, so this Termurion runs: {e}"));
            }
            handed
        };
        let tries = std::cell::Cell::new(0u32);
        hand_over(&mut attempt, HAND_TRIES, &|| std::thread::sleep(hand_pause(tries.replace(tries.get() + 1))))
    }

    /// A fresh request id: 16 random bytes as hex.
    pub fn request_id() -> String {
        let mut b = [0u8; 16];
        if getrandom::fill(&mut b).is_err() {
            // No random source: the pid and the time still tell two starts apart.
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
            return format!("{:x}{:x}", std::process::id(), t);
        }
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// A busy running Termurion is asked this many times (a burst of starts, review R1).
    pub const HAND_TRIES: usize = 6;

    /// The wait before try `n + 2`: 100 ms doubling to at most 1 s (about 2.5 s in all).
    pub fn hand_pause(n: u32) -> Duration {
        Duration::from_millis((100u64 << n.min(4)).min(1000))
    }

    /// Whether a Termurion of this user answers on the socket, with nothing sent.
    pub fn probe_running() -> Handed {
        let name = match ipc::url_pipe_name() {
            Ok(n) => n,
            Err(e) => return Handed::Failed(format!("no launch socket: {e}")),
        };
        match ipc::connect_owned(&name, Duration::from_secs(1)) {
            Ok(Some(p)) => match p.server_pid().and_then(verify_server) {
                Ok(()) => Handed::Taken,
                Err(e) => Handed::OtherOwner(e),
            },
            Ok(None) => Handed::NoServer,
            Err(ipc::ConnectError::Denied(e)) => Handed::OtherOwner(e),
            Err(ipc::ConnectError::Other(e)) => Handed::Failed(e),
        }
    }

    /// Whether the program `theirs` serving the socket is this Termurion: the same program file,
    /// as Windows compares image paths (review N11). Two runs of one AppImage each mount it
    /// somewhere new (`/tmp/.mount_XXXX`), so there the path inside the mount is compared; an
    /// updated binary under a running copy reads "(deleted)".
    pub fn verify_image(theirs: &std::path::Path, mine: &std::path::Path) -> Result<(), String> {
        let theirs = theirs.to_string_lossy();
        let theirs = theirs.trim_end_matches(" (deleted)");
        let mine = mine.to_string_lossy();
        if !theirs.starts_with('/') {
            return Err("the program holding the launch socket could not be identified".into());
        }
        // The path after an AppImage's mount folder (`.mount_` and its random tail).
        let inside = |p: &str| p.split('/').position(|c| c.starts_with(".mount_")).map(|i| p.split('/').skip(i + 1).collect::<Vec<_>>().join("/"));
        let same = theirs == mine || matches!((inside(theirs), inside(&mine)), (Some(a), Some(b)) if a == b);
        if same { Ok(()) } else { Err("the launch socket is held by a program that is not this Termurion".into()) }
    }

    fn verify_server(pid: u32) -> Result<(), String> {
        let theirs = std::fs::read_link(format!("/proc/{pid}/exe")).map_err(|_| "the program holding the launch socket could not be identified".to_string())?;
        let mine = std::env::current_exe().map_err(|e| e.to_string())?;
        verify_image(&theirs, &mine)
    }

    /// Why the socket at `path` can never serve: a file that is not a socket, or a path longer
    /// than a socket address holds. Then Termurion runs without it instead of refusing to
    /// start (review N6).
    pub fn unusable(path: &std::path::Path) -> Option<String> {
        use std::os::unix::fs::FileTypeExt;
        // sockaddr_un's 108 bytes, with room for the NUL.
        if path.as_os_str().len() >= 108 {
            return Some(format!("the launch socket path {} is too long", path.display()));
        }
        match std::fs::symlink_metadata(path) {
            Ok(m) if !m.file_type().is_socket() => Some(format!("{} is not a socket", path.display())),
            _ => None,
        }
    }

    /// Whether another Termurion holds the socket's lock now (it is between taking the lock and
    /// listening). Read from /proc/locks, never by taking the lock, which would make that start
    /// lose its own bind (review N7).
    pub fn lock_held(path: &std::path::Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        let mut lock = path.as_os_str().to_owned();
        lock.push(".lock");
        let Ok(m) = std::fs::symlink_metadata(std::path::Path::new(&lock)) else { return false };
        let Ok(locks) = std::fs::read_to_string("/proc/locks") else { return false };
        locks_name(&locks, m.ino())
    }

    /// Whether /proc/locks lists a FLOCK on inode `ino` (its device is not compared: btrfs's stat
    /// names another device than /proc/locks does). Only a short wait rests on it.
    pub fn locks_name(locks: &str, ino: u64) -> bool {
        let tail = format!(":{ino}");
        locks.lines().any(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            f.get(1) == Some(&"FLOCK") && f.get(5).is_some_and(|d| d.ends_with(&tail))
        })
    }

    /// Connects at once: a refused connect is a dead socket, answered at once (review N8), unless
    /// another start holds the lock, which is about to listen; then it is waited for up to 1 s (N7).
    fn connect_now(name: &str) -> Result<Option<ipc::Pipe>, ipc::ConnectError> {
        let end = Instant::now() + Duration::from_secs(1);
        loop {
            match ipc::connect_owned(name, Duration::ZERO)? {
                Some(p) => return Ok(Some(p)),
                None if Instant::now() < end && lock_held(std::path::Path::new(name)) => std::thread::sleep(Duration::from_millis(20)),
                None => return Ok(None),
            }
        }
    }

    /// `hand_envelope_on` with no request id.
    pub fn hand_request_on(name: &str, request: &crate::launch::Request, token: Option<&str>, verify: &dyn Fn(u32) -> Result<(), String>) -> Handed {
        hand_envelope_on(name, request, None, token, verify)
    }

    /// The only writer to the socket: one message, one connection, counted taken only on the ack.
    pub fn hand_envelope_on(name: &str, request: &crate::launch::Request, id: Option<&str>, token: Option<&str>, verify: &dyn Fn(u32) -> Result<(), String>) -> Handed {
        let Some(message) = crate::launch::encode_envelope(request, id, token) else { return Handed::Failed("the request is not one Termurion accepts".into()) };
        // A socket that can never serve is no Termurion: this one runs without it (and says so).
        if let Some(why) = unusable(std::path::Path::new(name)) {
            return Handed::OtherOwner(why);
        }
        let pipe = match connect_now(name) {
            Ok(Some(p)) => p,
            Ok(None) => return Handed::NoServer,
            Err(ipc::ConnectError::Denied(e)) => return Handed::OtherOwner(e),
            Err(ipc::ConnectError::Other(e)) => return Handed::Failed(e),
        };
        if let Err(e) = pipe.server_is_this_user() {
            return Handed::OtherOwner(e);
        }
        match pipe.server_pid().and_then(verify) {
            Ok(()) => {}
            Err(e) => return Handed::OtherOwner(e),
        }
        if let Err(e) = pipe.write_all(&message) {
            return Handed::Failed(e);
        }
        let mut answer = [0u8; 1];
        // A close without the ack after the message went out is a busy or stopping Termurion,
        // not none: tried again, and a gone socket is NoServer on that try (review R1).
        match pipe.read_timeout(&mut answer, Duration::from_secs(2)) {
            Ok(Some(1)) if answer[0] == ACK => Handed::Taken,
            Ok(None) => Handed::Failed("the running Termurion did not answer".into()),
            _ => Handed::Failed("the running Termurion closed without an answer".into()),
        }
    }

    /// The launch socket served on its own thread until `stop`.
    pub fn listen(notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<Serving, ListenError> {
        serve_on(&ipc::url_pipe_name().map_err(ListenError::Failed)?, Duration::from_secs(2), notify).map_err(ListenError::Failed)
    }

    /// A served socket. `stop` ends the serving: every request read before it returns has been
    /// handed on, and the socket file is gone afterwards.
    pub struct Serving {
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Serving {
        pub fn stop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }

        pub fn stopped(&self) -> bool {
            self.thread.is_none()
        }
    }

    /// Clients read at once at most. A silent client holds only its own reader, never the next
    /// start (review N9). Past the cap the next client waits in the socket's backlog until a
    /// reader is free, never closed unanswered (review R1).
    pub const READERS: usize = 4;

    /// The app's sink and the ids of the requests it was handed lately, under one lock, so two
    /// readers of the same retried request cannot both hand it on.
    struct Served {
        notify: Box<dyn Fn(crate::launch::Request) + Send>,
        seen: std::collections::VecDeque<String>,
    }

    /// Request ids remembered; a retry comes within seconds, long before 64 other starts.
    pub const SEEN_IDS: usize = 64;

    type Notify = std::sync::Arc<std::sync::Mutex<Served>>;

    pub fn serve_on(name: &str, wait: Duration, notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<Serving, String> {
        let mut listener = ipc::Listener::bind(name).map_err(|e| e.replace("session host", "Termurion"))?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let notify: Notify = std::sync::Arc::new(std::sync::Mutex::new(Served { notify, seen: Default::default() }));
        let thread = std::thread::Builder::new()
            .name("launch-socket".into())
            .spawn(move || {
                let mut readers: Vec<std::thread::JoinHandle<()>> = Vec::new();
                let serve = |p: ipc::Pipe, readers: &mut Vec<std::thread::JoinHandle<()>>| {
                    let n = notify.clone();
                    match std::thread::Builder::new().name("launch-read".into()).spawn(move || take_one(&p, wait, &n)) {
                        Ok(r) => readers.push(r),
                        Err(e) => crate::host_base::log(&format!("launch: no reader thread: {e}")),
                    }
                };
                while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
                    // All readers busy: the next client stays queued in the backlog meanwhile.
                    readers.retain(|r| !r.is_finished());
                    if readers.len() >= READERS {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    match listener.accept(Duration::from_secs(1)) {
                        Ok(Some(p)) => serve(p, &mut readers),
                        Ok(None) => {}
                        Err(e) => {
                            crate::host_base::log(&format!("launch: accept failed: {e}"));
                            std::thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
                if let Ok(Some(p)) = listener.accept(Duration::ZERO) {
                    serve(p, &mut readers);
                }
                // Every request read before `stop` returns is handed on.
                for r in readers {
                    let _ = r.join();
                }
            })
            .map_err(|e| format!("the launch socket thread did not start: {e}"))?;
        Ok(Serving { stop, thread: Some(thread) })
    }

    /// One client: read, acknowledged once held, closed, then handed on (its token first).
    fn take_one(pipe: &ipc::Pipe, wait: Duration, notify: &Notify) {
        let got = read_message(pipe, wait);
        if got.is_some() {
            let _ = pipe.write_all(&[ACK]);
        }
        pipe.shutdown();
        if let Some(e) = got {
            // One request at a time, its token just before it.
            let mut served = notify.lock().unwrap_or_else(|e| e.into_inner());
            // A retry of a request already handed on (its ack was late) is acked, not run again.
            if let Some(id) = &e.id {
                if served.seen.contains(id) {
                    crate::host_base::log("launch: a retried request was already taken");
                    return;
                }
                if served.seen.len() >= SEEN_IDS {
                    served.seen.pop_front();
                }
                served.seen.push_back(id.clone());
            }
            if let Some(t) = e.token {
                tell_activation(t);
            }
            (served.notify)(e.request);
        }
    }

    fn read_message(pipe: &ipc::Pipe, wait: Duration) -> Option<crate::launch::Envelope> {
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
            if let Some(got) = crate::launch::decode_envelope(&bytes) {
                return Some(got);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        use std::sync::mpsc;

        /// A private scratch folder for the sockets (never the real runtime folder), removed after.
        struct Dir(std::path::PathBuf);
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        fn dir() -> Dir {
            let d = std::env::temp_dir().join(crate::platform::ipc::test_name("launch-sock"));
            std::fs::DirBuilder::new().mode(0o700).create(&d).unwrap();
            Dir(d)
        }
        fn ok(_: u32) -> Result<(), String> {
            Ok(())
        }

        #[test]
        fn the_socket_name_is_in_the_private_runtime_folder() {
            let name = ipc::url_pipe_name().unwrap();
            assert!(name.ends_with("/launch.sock"), "{name}");
        }

        /// A second start hands every kind over with its token; the token reaches the app first.
        #[test]
        fn a_second_start_hands_its_request_and_token_over() {
            let d = dir();
            let name = d.0.join("launch.sock").to_string_lossy().to_string();
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, None, &ok), Handed::NoServer, "nothing listens yet");
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let tokens = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let t2 = tokens.clone();
            on_activation(Box::new(move |t| t2.lock().unwrap().push(t)));
            let mut serving = serve_on(&name, Duration::from_secs(2), Box::new(move |r| drop(tx.lock().unwrap().send(r)))).unwrap();
            let mode = std::fs::symlink_metadata(&name).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "only this user may connect");
            use crate::launch::Request::*;
            for r in [Reopen, NewWindow { guid: Some("ABC-1".into()) }, NewTab { guid: None }, Arrangement { name: "Dev".into() }, Open { paths: vec!["/home/u/a b.sh".into()] }, Url { url: "ssh://h".into() }] {
                assert_eq!(hand_request_on(&name, &r, None, &ok), Handed::Taken, "{r:?}");
                assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), r);
            }
            assert_eq!(hand_request_on(&name, &Reopen, Some("tok-1"), &ok), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), Reopen);
            assert_eq!(*tokens.lock().unwrap(), vec!["tok-1".to_string()]);
            // A server the check refuses is sent nothing.
            assert!(matches!(hand_request_on(&name, &Reopen, None, &|_| Err("not Termurion".into())), Handed::OtherOwner(_)));
            assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
            // A second listener on the same name loses: one Termurion per user.
            assert!(serve_on(&name, Duration::from_secs(2), Box::new(|_| {})).is_err());
            serving.stop();
            assert!(std::fs::symlink_metadata(&name).is_err(), "the socket goes with the listener");
            assert_eq!(hand_request_on(&name, &Reopen, None, &ok), Handed::NoServer);
        }

        /// A socket a crashed Termurion left behind is no server: the next start replaces it.
        #[test]
        fn a_stale_socket_is_replaced() {
            let d = dir();
            let path = d.0.join("launch.sock");
            let name = path.to_string_lossy().to_string();
            // Bound and dropped without unlinking: what a killed process leaves.
            let raw = std::os::unix::net::UnixListener::bind(&path).unwrap();
            drop(raw);
            assert!(std::fs::symlink_metadata(&path).is_ok());
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, None, &ok), Handed::NoServer, "a dead socket answers nobody");
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let mut serving = serve_on(&name, Duration::from_secs(2), Box::new(move |r| drop(tx.lock().unwrap().send(r)))).unwrap();
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, None, &ok), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), crate::launch::Request::Reopen);
            serving.stop();
        }

        /// A folder other users can enter is refused: the folder is the boundary.
        #[test]
        fn an_open_folder_is_refused() {
            let d = dir();
            std::fs::set_permissions(&d.0, std::fs::Permissions::from_mode(0o755)).unwrap();
            let name = d.0.join("launch.sock").to_string_lossy().to_string();
            assert!(serve_on(&name, Duration::from_secs(2), Box::new(|_| {})).is_err());
        }

        /// Review N11: the same program file, as on Windows; an AppImage compares inside its mount.
        #[test]
        fn the_server_must_be_termurion() {
            use std::path::Path;
            assert!(verify_image(Path::new("/usr/bin/termurion"), Path::new("/usr/bin/termurion")).is_ok());
            assert!(verify_image(Path::new("/opt/t/termurion"), Path::new("/usr/bin/termurion")).is_err(), "another build named termurion is not this one");
            assert!(verify_image(Path::new("/home/u/bin/termurion"), Path::new("/tmp/.mount_cd34/usr/bin/termurion")).is_err());
            assert!(verify_image(Path::new("/tmp/.mount_ab12/usr/bin/termurion"), Path::new("/tmp/.mount_cd34/usr/bin/termurion")).is_ok(), "two AppImage runs");
            assert!(verify_image(Path::new("/tmp/.mount_ab12/usr/bin/other"), Path::new("/tmp/.mount_cd34/usr/bin/termurion")).is_err());
            assert!(verify_image(Path::new("/usr/bin/termurion (deleted)"), Path::new("/usr/bin/termurion")).is_ok(), "updated under a running copy");
            assert!(verify_image(Path::new("/usr/bin/python3"), Path::new("/usr/bin/termurion")).is_err());
            assert!(verify_image(Path::new(""), Path::new("/usr/bin/termurion")).is_err());
        }

        /// Review N6: a file that is not a socket, or a path too long for one, never stops a start:
        /// the hand-over is OtherOwner, so this Termurion runs without the socket and says so.
        #[test]
        fn an_unusable_socket_path_lets_termurion_run() {
            let d = dir();
            let path = d.0.join("launch.sock");
            std::fs::write(&path, b"x").unwrap();
            let name = path.to_string_lossy().to_string();
            assert!(serve_on(&name, Duration::from_secs(2), Box::new(|_| {})).is_err());
            assert!(matches!(hand_request_on(&name, &crate::launch::Request::Reopen, None, &ok), Handed::OtherOwner(_)));
            assert_eq!(hand_over(&mut || hand_request_on(&name, &crate::launch::Request::Reopen, None, &ok), 3, &|| {}), AfterHand::Start);
            assert_eq!(lost_pipe(&[AfterHand::Start], false), LostPipe::RunWithout, "run() then runs without it");
            let long = d.0.join("a".repeat(120)).to_string_lossy().to_string();
            assert!(matches!(hand_request_on(&long, &crate::launch::Request::Reopen, None, &ok), Handed::OtherOwner(e) if e.contains("too long")));
        }

        /// Review N8: a socket a crashed Termurion left answers "nobody" at once, not after a second.
        #[test]
        fn a_dead_socket_answers_at_once() {
            let d = dir();
            let path = d.0.join("launch.sock");
            drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
            let t = Instant::now();
            assert_eq!(hand_request_on(&path.to_string_lossy(), &crate::launch::Request::Reopen, None, &ok), Handed::NoServer);
            assert!(t.elapsed() < Duration::from_millis(300), "{:?}", t.elapsed());
        }

        /// Review N7: a start that lost the lock to another waits for that one's socket. The lock
        /// stays held while the other binds and answers, as a real Listener holds it from before its
        /// bind for its whole life, so there is no gap to race (review R2).
        #[test]
        fn a_start_waits_for_the_one_holding_the_lock() {
            use std::io::{Read, Write};
            use std::os::fd::AsRawFd;
            let d = dir();
            let path = d.0.join("launch.sock");
            let name = path.to_string_lossy().to_string();
            let mut lock = path.as_os_str().to_owned();
            lock.push(".lock");
            let f = std::fs::File::create(std::path::Path::new(&lock)).unwrap();
            assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) }, 0);
            assert!(lock_held(&path), "/proc/locks shows the lock");
            let p2 = path.clone();
            // The other start: it binds 300 ms later under the held lock and acks one request.
            let other = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(300));
                let l = std::os::unix::net::UnixListener::bind(&p2).unwrap();
                let (mut c, _) = l.accept().unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 256];
                while crate::launch::decode_with_token(&got).is_none() {
                    let n = c.read(&mut buf).unwrap();
                    assert!(n > 0, "the client closed early");
                    got.extend_from_slice(&buf[..n]);
                }
                c.write_all(&[ACK]).unwrap();
                crate::launch::decode_with_token(&got).unwrap().0
            });
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, None, &ok), Handed::Taken, "it waited for the socket");
            assert_eq!(other.join().unwrap(), crate::launch::Request::Reopen);
            drop(f);
            assert!(locks_name("1: FLOCK  ADVISORY  WRITE 12 00:1a:345 0 EOF\n", 345));
            assert!(!locks_name("1: POSIX  ADVISORY  WRITE 12 00:1a:345 0 EOF\n", 345));
            assert!(!locks_name("1: FLOCK  ADVISORY  WRITE 12 00:1a:1345 0 EOF\n", 345));
        }

        /// Review R1: a burst of starts, more than there are readers, all reach the one Termurion:
        /// none is closed unanswered (each would be a second Termurion).
        #[test]
        fn a_burst_of_starts_all_reach_the_running_one() {
            let d = dir();
            let name = d.0.join("launch.sock").to_string_lossy().to_string();
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let mut serving = serve_on(&name, Duration::from_millis(500), Box::new(move |r| drop(tx.lock().unwrap().send(r)))).unwrap();
            // Every reader held by a silent client first, so the burst must queue.
            let silent: Vec<_> = (0..READERS).map(|_| std::os::unix::net::UnixStream::connect(&name).unwrap()).collect();
            std::thread::sleep(Duration::from_millis(100));
            let burst: Vec<_> = (0..10)
                .map(|i| {
                    let n = name.clone();
                    std::thread::spawn(move || {
                        let r = crate::launch::Request::Arrangement { name: format!("a{i}") };
                        let tries = std::cell::Cell::new(0u32);
                        hand_over(&mut || hand_request_on(&n, &r, None, &ok), HAND_TRIES, &|| std::thread::sleep(hand_pause(tries.replace(tries.get() + 1))))
                    })
                })
                .collect();
            let after: Vec<AfterHand> = burst.into_iter().map(|t| t.join().unwrap()).collect();
            assert!(after.iter().all(|a| *a == AfterHand::Exit), "no second Termurion: {after:?}");
            let mut got: Vec<String> = (0..10)
                .map(|_| match rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                    crate::launch::Request::Arrangement { name } => name,
                    other => panic!("{other:?}"),
                })
                .collect();
            got.sort();
            let mut want: Vec<String> = (0..10).map(|i| format!("a{i}")).collect();
            want.sort();
            assert_eq!(got, want);
            drop(silent);
            serving.stop();
            assert_eq!(hand_pause(0), Duration::from_millis(100));
            assert_eq!(hand_pause(9), Duration::from_secs(1));
            assert!((0..HAND_TRIES as u32 - 1).map(hand_pause).sum::<Duration>() <= Duration::from_secs(3), "a few seconds in all");
        }

        /// A start whose ack came late tries again with the same id: the request runs once, and
        /// both tries count as taken.
        #[test]
        fn a_retried_request_runs_once() {
            let d = dir();
            let name = d.0.join("launch.sock").to_string_lossy().to_string();
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let mut serving = serve_on(&name, Duration::from_secs(2), Box::new(move |r| drop(tx.lock().unwrap().send(r)))).unwrap();
            let id = request_id();
            assert!(crate::launch::id_ok(&id) && id != request_id(), "{id}");
            let r = crate::launch::Request::NewTab { guid: None };
            assert_eq!(hand_envelope_on(&name, &r, Some(&id), None, &ok), Handed::Taken);
            assert_eq!(hand_envelope_on(&name, &r, Some(&id), None, &ok), Handed::Taken, "the retry is acked");
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), r);
            assert!(rx.recv_timeout(Duration::from_millis(500)).is_err(), "not run twice");
            // Another start's request is its own.
            assert_eq!(hand_envelope_on(&name, &r, Some(&request_id()), None, &ok), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), r);
            serving.stop();
        }

        /// Review N9: a silent client of this user does not hold up a real second start.
        #[test]
        fn a_silent_client_does_not_hold_up_the_next() {
            let d = dir();
            let name = d.0.join("launch.sock").to_string_lossy().to_string();
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let mut serving = serve_on(&name, Duration::from_secs(2), Box::new(move |r| drop(tx.lock().unwrap().send(r)))).unwrap();
            let _silent = std::os::unix::net::UnixStream::connect(&name).unwrap();
            std::thread::sleep(Duration::from_millis(100));
            let t = Instant::now();
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, None, &ok), Handed::Taken);
            assert!(t.elapsed() < Duration::from_millis(1000), "{:?}", t.elapsed());
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), crate::launch::Request::Reopen);
            serving.stop();
        }
    }
}

#[cfg(windows)]
mod pipe {
    use super::*;
    use crate::platform::ipc;
    use crate::platform::windows::ipc as wipc;

    /// Hands `request` to a running Termurion of this user, retrying a busy pipe briefly.
    pub fn hand_to_running(request: &crate::launch::Request) -> AfterHand {
        let name = match ipc::url_pipe_name() {
            Ok(n) => n,
            Err(e) => return AfterHand::Refuse(format!("no link pipe name: {e}")),
        };
        let mut attempt = || {
            let handed = hand_request_on(&name, request, &allow_foreground);
            if let Handed::OtherOwner(e) = &handed {
                crate::host_base::log(&format!("url: the link pipe is held by another owner, so this Termurion runs: {e}"));
            }
            handed
        };
        hand_over(&mut attempt, 3, &|| std::thread::sleep(Duration::from_millis(300)))
    }

    /// Whether a Termurion of this user answers on the launch pipe, with nothing sent (an
    /// `-Embedding` start asks before it makes anything: B9).
    pub fn probe_running() -> Handed {
        match ipc::url_pipe_name() {
            Ok(n) => probe_on(&n, &verify_server),
            Err(e) => Handed::Failed(format!("no link pipe name: {e}")),
        }
    }

    /// `probe_running` on pipe `name`; a holder `verify` refuses is `OtherOwner`.
    pub fn probe_on(name: &str, verify: &dyn Fn(u32) -> Result<(), String>) -> Handed {
        let pipe = match ipc::connect_owned(name, Duration::from_secs(1)) {
            Ok(Some(p)) => p,
            Ok(None) => return Handed::NoServer,
            Err(ipc::ConnectError::Denied(e)) => return Handed::OtherOwner(e),
            Err(ipc::ConnectError::Other(e)) => return Handed::Failed(e),
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

    /// Lets the Termurion that serves the pipe (and only it) come forward (P16).
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
        server_verdict(&wipc::process_token_facts(pid)?, &wipc::own_token_facts()?, wipc::process_image(pid).as_deref(), wipc::session_of(pid), wipc::session_of(std::process::id()), &mine.to_string_lossy())
    }

    /// `hand_request_on` with the server check given: a holder `verify` refuses gets nothing, and
    /// is `OtherOwner` (this Termurion runs without a listener, as when another user holds the name).
    pub fn hand_request_checked(name: &str, request: &crate::launch::Request, grant: &dyn Fn(u32), verify: &dyn Fn(u32) -> Result<(), String>) -> Handed {
        let Some(message) = crate::launch::encode(request) else { return Handed::Failed("the request is not one Termurion accepts".into()) };
        let pipe = match ipc::connect_owned(name, Duration::from_secs(1)) {
            Ok(Some(p)) => p,
            Ok(None) => return Handed::NoServer,
            Err(ipc::ConnectError::Denied(e)) => return Handed::OtherOwner(e),
            Err(ipc::ConnectError::Other(e)) => return Handed::Failed(e),
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
        // The listener answers `ACK` when it has the request, then closes; a Termurion that does not
        // answer within 2 s is hung, not a taker. A close or a broken pipe without the ack is no
        // taker either (a listener closing in that instant, round 5 item 6): this Termurion starts.
        let mut answer = [0u8; 1];
        match pipe.read_timeout(&mut answer, Duration::from_secs(2)) {
            Ok(Some(1)) if answer[0] == ACK => Handed::Taken,
            Ok(None) => Handed::Failed("the running Termurion did not answer".into()),
            _ => Handed::NoServer,
        }
    }

    /// The launch pipe served on its own thread until `stop`.
    pub fn listen(notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<Serving, ListenError> {
        serve_on(&ipc::url_pipe_name().map_err(ListenError::Failed)?, Duration::from_secs(2), notify).map_err(ListenError::Failed)
    }

    /// The listening thread of a pipe served for the life of the process.
    pub fn listen_on(name: &str, wait: Duration, notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<std::thread::JoinHandle<()>, String> {
        let mut s = serve_on(name, wait, notify)?;
        s.thread.take().ok_or_else(|| "no listening thread".to_string())
    }

    /// A served pipe. `stop` ends the serving: every request read before it returns has been
    /// handed to `notify`, and the name is free afterwards (a later start finds no server).
    pub struct Serving {
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Serving {
        /// Stops accepting and waits for the thread: at most one accept wait and one read.
        pub fn stop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }

        pub fn stopped(&self) -> bool {
            self.thread.is_none()
        }
    }

    pub fn serve_on(name: &str, wait: Duration, notify: Box<dyn Fn(crate::launch::Request) + Send>) -> Result<Serving, String> {
        let mut listener = ipc::Listener::bind(name)?;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(std::sync::atomic::Ordering::SeqCst) {
                let pipe = match listener.accept(Duration::from_secs(1)) {
                    Ok(Some(p)) => p,
                    Ok(None) => continue,
                    Err(e) => {
                        crate::host_base::log(&format!("url: accept failed: {e}"));
                        if listener.renew().is_err() {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(200));
                        continue;
                    }
                };
                // The DACL admits this user at this elevation only; nothing more is checked of the client.
                take_one(&pipe, wait, &notify);
            }
            // A client that connected as the last wait ended is served too, before the name goes.
            if let Ok(Some(pipe)) = listener.accept(Duration::ZERO) {
                take_one(&pipe, wait, &notify);
            }
        });
        Ok(Serving { stop, thread: Some(thread) })
    }

    /// One client: its message is read, acknowledged with `ACK` once it is held here (the sender
    /// counts it taken only then), the pipe is closed, and the request handed on. A silent or finished
    /// client is closed either way, so it never blocks the next one, and the ack never waits for the
    /// callback.
    fn take_one(pipe: &ipc::Pipe, wait: Duration, notify: &dyn Fn(crate::launch::Request)) {
        let request = read_message(pipe, wait);
        if request.is_some() {
            let _ = pipe.write_all(&[ACK]);
        }
        pipe.shutdown();
        if let Some(request) = request {
            notify(request);
        }
    }

    /// One message, read within `wait` of the accept and at most `launch::MAX_MESSAGE` in all:
    /// it ends where `launch::decode` accepts it. A longer, malformed or silent client is dropped.
    fn read_message(pipe: &ipc::Pipe, wait: Duration) -> Option<crate::launch::Request> {
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

    #[test]
    fn listen_errors_keep_their_text() {
        assert_eq!(ListenError::Failed("x".into()).to_string(), "x");
        let want = format!("Single-instance link handoff is not available on {} yet", crate::platform::OS_NAME);
        assert_eq!(ListenError::Unsupported(crate::platform::Unsupported { feature: LISTEN_FEATURE, plural: false }).to_string(), want);
    }

    fn argv(parts: &[&str]) -> Vec<OsString> {
        std::iter::once(r"C:\T\termurion.exe").chain(parts.iter().copied()).map(OsString::from).collect()
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

    /// Fix r1 Important: a second full Termurion starts only when none of this user runs (or
    /// another user holds the pipe); a busy or failed hand-over retries, then ends refused.
    #[test]
    fn a_busy_hand_over_never_starts_a_second_termurion() {
        let busy = || Handed::Failed("the pipe stayed busy".into());
        let (mut f, calls) = sequence(vec![busy(), busy(), busy(), busy()]);
        assert!(matches!(hand_over(&mut f, 3, &|| {}), AfterHand::Refuse(_)));
        assert_eq!(calls.get(), 3, "retried, then refused");
        let (mut f, calls) = sequence(vec![busy(), Handed::Taken]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Exit);
        assert_eq!(calls.get(), 2);
        let (mut f, _) = sequence(vec![busy(), Handed::NoServer]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Start, "the running Termurion ended meanwhile");
        let (mut f, calls) = sequence(vec![Handed::NoServer]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Start);
        assert_eq!(calls.get(), 1);
        let (mut f, _) = sequence(vec![Handed::OtherOwner("squatter".into())]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Start);
        let (mut f, _) = sequence(vec![Handed::Taken]);
        assert_eq!(hand_over(&mut f, 3, &|| {}), AfterHand::Exit);
    }

    /// 13u S4, the one rule for a launch pipe another holder took (run() and an idle `-Embedding`
    /// process, round 5 race B): everything handed over ends this process; a refused hand-over, or
    /// no Start at all, ends it too unless a handed-off console waits; otherwise it runs without.
    #[test]
    fn a_lost_pipe_has_one_rule() {
        use AfterHand::*;
        let refuse = || Refuse("busy".into());
        assert_eq!(lost_pipe(&[Exit, Exit], false), LostPipe::Handed);
        assert_eq!(lost_pipe(&[], false), LostPipe::Handed);
        assert_eq!(lost_pipe(&[Exit, refuse()], false), LostPipe::Refused);
        assert_eq!(lost_pipe(&[Start, refuse()], false), LostPipe::Refused, "one refused is enough");
        assert_eq!(lost_pipe(&[Start, Exit], false), LostPipe::RunWithout, "a holder that is not Termurion");
        assert_eq!(lost_pipe(&[refuse()], true), LostPipe::RunWithout, "a console waits: it is shown");
        assert_eq!(lost_pipe(&[Exit], true), LostPipe::RunWithout);
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
        // A lone surrogate is a Windows-only OsString.
        #[cfg(windows)] // test: a lone surrogate (OsStringExt::from_wide)
        {
            use std::os::windows::ffi::OsStringExt;
            let mut bad = argv(&["--url"]);
            bad.push(OsString::from_wide(&[0x73, 0x73, 0x68, 0x3a, 0xd800]));
            assert!(matches!(url_argument(&bad), UrlArg::Refused(_)));
        }
    }

    #[test]
    fn pipe_message_follows_the_rule() {
        assert_eq!(pipe_message("ssh://h").as_deref(), Some(&b"open-url\nssh://h\n"[..]));
        for bad in ["ssh://h\n", "ssh://h\r", "ssh://h\0"] {
            assert_eq!(pipe_message(bad), None, "{bad:?}");
        }
        assert!(pipe_message(&format!("ssh://{}", "a".repeat(MAX_URL - 6))).is_some());
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

    /// Review r1 Important 1: a pipe server is Termurion only when its token passes the peer rule
    /// and its image is this Termurion's own program.
    #[test]
    fn the_server_must_be_termurion() {
        const S: Option<u32> = Some(1);
        use crate::api_peer::{Token, HIGH_RID, LOW_RID, MEDIUM_RID};
        let mine = Token { sid: "S-1-5-21-1".into(), elevated: false, integrity: MEDIUM_RID, app_container: false, restricted: false };
        let exe = r"C:\Termurion\termurion.exe";
        assert_eq!(server_verdict(&mine, &mine, Some(r"c:\termurion\TERMURION.EXE"), S, S, exe), Ok(()), "same token, same image, case ignored");
        let low = Token { integrity: LOW_RID, ..mine.clone() };
        assert!(server_verdict(&low, &mine, Some(exe), S, S, exe).is_err(), "a low-integrity squatter");
        let high = Token { integrity: HIGH_RID, elevated: true, ..mine.clone() };
        assert!(server_verdict(&high, &mine, Some(exe), S, S, exe).is_err());
        assert!(server_verdict(&Token { app_container: true, ..mine.clone() }, &mine, Some(exe), S, S, exe).is_err());
        assert!(server_verdict(&Token { sid: "S-1-5-21-2".into(), ..mine.clone() }, &mine, Some(exe), S, S, exe).is_err());
        assert!(server_verdict(&mine, &mine, Some(r"C:\Temp\evil.exe"), S, S, exe).is_err(), "another image");
        assert!(server_verdict(&mine, &mine, None, S, S, exe).is_err(), "an unreadable image is refused");
        // Review r1 residual: the server must run in this process's session.
        assert!(server_verdict(&mine, &mine, Some(exe), Some(2), Some(1), exe).is_err(), "a same-user Termurion of another session");
        assert!(server_verdict(&mine, &mine, Some(exe), None, Some(1), exe).is_err(), "an unreadable session is refused");
        assert!(server_verdict(&mine, &mine, Some(exe), Some(1), None, exe).is_err());
        assert_eq!(server_verdict(&mine, &mine, Some(exe), Some(3), Some(3), exe), Ok(()));
    }

    #[cfg(windows)] // test: the Windows pipe tests
    mod windows {
        use super::super::*;
        use crate::platform::windows::ipc::tests::test_name;
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
        /// answer is `OtherOwner` (this Termurion then runs), not a hang or a hand-over.
        #[test]
        fn a_server_that_is_not_termurion_gets_no_request() {
            let name = test_name("squatter");
            let rx = listening_all(&name, Duration::from_secs(2));
            let granted = std::cell::Cell::new(false);
            let open = crate::launch::Request::Open { paths: vec![r"C:\w\a.ps1".into()] };
            let handed = hand_request_checked(&name, &open, &|_| granted.set(true), &|_| Err("not Termurion".to_string()));
            assert_eq!(handed, Handed::OtherOwner("not Termurion".into()));
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

        /// Task 5 review, Important 3: an idle `-Embedding` start stops serving before it ends. The
        /// listener hands on every request it read before `stop` returns, then frees the name, so a
        /// later start finds no server and runs itself instead of being answered and dropped.
        #[test]
        fn a_stopped_listener_hands_on_what_it_took_then_frees_the_name() {
            let name = test_name("launch-stop");
            let (tx, rx) = mpsc::channel();
            let tx = std::sync::Mutex::new(tx);
            let mut serving = serve_on(&name, Duration::from_secs(2), Box::new(move |r| drop(tx.lock().unwrap().send(r)))).unwrap();
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::Taken);
            let t = Instant::now();
            serving.stop();
            assert!(t.elapsed() < Duration::from_secs(4), "stop waits at most one accept and one read");
            assert_eq!(rx.try_recv(), Ok(crate::launch::Request::Reopen), "what it took was handed on before stop returned");
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::NoServer, "the name is free");
            assert!(serving.stopped());
        }

        /// B1: a client that connects and says nothing never blocks the next request of any kind.
        #[test]
        fn a_silent_client_does_not_block_the_next_request() {
            for (i, request) in every_kind().into_iter().enumerate() {
                let name = test_name(&format!("launch-silent-{i}"));
                let rx = listening_all(&name, Duration::from_millis(200));
                let silent = crate::platform::ipc::connect(&name, Duration::from_secs(1)).unwrap().unwrap();
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
                let pipe = crate::platform::ipc::connect(&name, Duration::from_secs(1)).unwrap().unwrap();
                let _ = pipe.write_all(bad);
                assert_eq!(pipe.read_timeout(&mut [0u8; 1], Duration::from_secs(3)), Ok(Some(0)), "dropped and closed");
            }
            assert!(rx.try_recv().is_err(), "nothing was delivered");
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), crate::launch::Request::Reopen);
        }

        /// P16: the running Termurion's pid, never everyone, may take the foreground.
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

        /// Round 5 item 6 (Task 1 minor 7): a request counts as taken only when the server says so with
        /// its ack byte. A server that reads it and closes without the ack (the microsecond window
        /// before a listener's close, or one that drops the request) is no taker: NoServer, so the
        /// request is retried or this Termurion starts.
        #[test]
        fn a_close_without_the_ack_is_not_taken() {
            let name = test_name("launch-noack");
            let mut listener = crate::platform::ipc::Listener::bind(&name).unwrap();
            let server = std::thread::spawn(move || {
                let pipe = listener.accept(Duration::from_secs(5)).unwrap().unwrap();
                let _ = pipe.read_timeout(&mut [0u8; 64], Duration::from_secs(2));
                pipe.shutdown();
            });
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::NoServer);
            server.join().unwrap();
            // The real listener acks after it has queued the request.
            let name = test_name("launch-ack");
            let rx = listening_all(&name, Duration::from_secs(2));
            assert_eq!(hand_request_on(&name, &crate::launch::Request::Reopen, &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), crate::launch::Request::Reopen);
        }

        /// R14de-10: a client that connects and sends nothing is dropped and never blocks the next.
        #[test]
        fn silent_url_client_is_dropped() {
            let name = test_name("url-silent");
            let rx = listening(&name, Duration::from_millis(200));
            let silent = crate::platform::ipc::connect(&name, Duration::from_secs(1)).unwrap().unwrap();
            let t = Instant::now();
            assert_eq!(hand_to_running_on(&name, "ssh://h2", &|_| {}), Handed::Taken);
            assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), "ssh://h2");
            assert!(t.elapsed() < Duration::from_secs(3));
            assert_eq!(silent.read_timeout(&mut [0u8; 1], Duration::from_secs(1)), Ok(Some(0)), "the listener closed the silent pipe");
        }
    }
}
