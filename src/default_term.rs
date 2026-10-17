//! Termurion as the Windows default terminal (m13v1 Task 5), the pure half: the delegation pair, the
//! COM registration entries, the texts, and the rules that judge a console handoff. The handoff
//! is a COM call any local program can make, so every rule here is a refusal first.

use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::reg::Registry;

pub const STARTUP_KEY: &str = r"Console\%%Startup";
pub const CONSOLE_VALUE: &str = "DelegationConsole";
pub const TERMINAL_VALUE: &str = "DelegationTerminal";
pub const CLSID_DEFAULT: &str = "{00000000-0000-0000-0000-000000000000}";
pub const CLSID_CONHOST: &str = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}";
pub const CLSID_WT_CONSOLE: &str = "{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}";
pub const CLSID_WT_TERMINAL: &str = "{E12CFF52-A866-4C77-9A90-F570A7AA2C6B}";
/// Termurion's own terminal class, fixed.
pub const CLSID_TERMURION_TERMINAL: &str = "{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}";
pub const SAVED_KEY: &str = r"Software\Termurion\PreviousDefaultTerminal";

/// The two delegation values: the console host's class and the terminal's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pair {
    pub console: String,
    pub terminal: String,
}

pub const NEEDS_WT_HEADING: &str = "Windows Terminal Required";
pub const NEEDS_WT_TEXT: &str = "Windows hands console programs to a default terminal through Windows Terminal\u{2019}s console host. Install Windows Terminal, then choose this again.";
pub const PACK_MISSING_HEADING: &str = "Default Terminal Unavailable";

pub fn pack_missing_text() -> &'static str {
    "Termurion can be the default terminal only with its own conpty.dll next to termurion.exe."
}

/// The registry key of Termurion's COM class (the one tree `unmake` deletes).
pub fn class_key() -> String {
    format!(r"Software\Classes\CLSID\{CLSID_TERMURION_TERMINAL}")
}

fn value_or_default(r: &dyn Registry, key: &str, name: &str) -> String {
    r.get(key, name).filter(|v| !v.is_empty()).unwrap_or_else(|| CLSID_DEFAULT.to_string())
}

/// The delegation pair now (a missing value is "Let Windows decide").
pub fn current(r: &dyn Registry) -> Pair {
    Pair { console: value_or_default(r, STARTUP_KEY, CONSOLE_VALUE), terminal: value_or_default(r, STARTUP_KEY, TERMINAL_VALUE) }
}

pub fn is_termurion(p: &Pair) -> bool {
    p.terminal.eq_ignore_ascii_case(CLSID_TERMURION_TERMINAL)
}

/// `AD:481-482`: the menu item is enabled while Termurion is not already the default.
pub fn make_enabled(r: &dyn Registry) -> bool {
    !is_termurion(&current(r))
}

fn entry(k: &str, n: &str, v: &str) -> (String, String, String) {
    (k.to_string(), n.to_string(), v.to_string())
}

/// The COM class and the pair that hands consoles to it: Windows Terminal's console host with
/// Termurion's terminal class. Per user (`HKCU`) only.
pub fn make_termurion_entries(exe: &Path) -> Vec<(String, String, String)> {
    let key = class_key();
    vec![
        entry(&key, "", "Termurion Terminal Handoff"),
        entry(&format!(r"{key}\LocalServer32"), "", &server_value(exe)),
        entry(STARTUP_KEY, CONSOLE_VALUE, CLSID_WT_CONSOLE),
        entry(STARTUP_KEY, TERMINAL_VALUE, CLSID_TERMURION_TERMINAL),
    ]
}

/// The `LocalServer32` value: the quoted exe only. COM appends ` -Embedding` itself when it
/// starts the server (learn.microsoft.com/windows/win32/com/localserver32).
pub fn server_value(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

/// The exe a `LocalServer32` value Termurion wrote names: exactly `"<absolute path>"`, or an older
/// Termurion's `"<absolute path>" -Embedding`. Any other shape is not Termurion's and is never touched.
fn server_exe(value: &str) -> Option<std::path::PathBuf> {
    let inner = value.strip_prefix('"')?;
    let (path, rest) = inner.split_once('"')?;
    let shape = rest.is_empty() || rest == " -Embedding";
    let p = Path::new(path);
    let plain = !path.is_empty() && !path.chars().any(char::is_control) && !crate::scripts::is_network_path(path);
    (shape && plain && p.is_absolute()).then(|| p.to_path_buf())
}

/// P11 (Task 5 review, Important 2): the entry that points Termurion's handoff class at this exe again
/// when it moved. Only while the delegation pair is Termurion's, only for the value of Termurion's own
/// class in the shape Termurion writes, and only when the exe it names is not this one and `gone`
/// says it no longer exists (the caller judges that path before probing it). An older Termurion's
/// value for this very exe (ending in its own ` -Embedding`) takes the plain form.
pub fn server_repair(r: &dyn Registry, exe: &Path, gone: &dyn Fn(&Path) -> bool) -> Vec<(String, String, String)> {
    let key = format!(r"{}\LocalServer32", class_key());
    let now = server_value(exe);
    let Some(value) = r.get(&key, "").filter(|_| is_termurion(&current(r))) else { return Vec::new() };
    let Some(recorded) = server_exe(&value) else { return Vec::new() };
    let same = recorded.to_string_lossy().eq_ignore_ascii_case(&exe.to_string_lossy());
    let stale = if same { value != now } else { gone(&recorded) };
    if stale { vec![entry(&key, "", &now)] } else { Vec::new() }
}

/// Startup: `server_repair` on the real registry. The recorded exe path is saved state, so it is
/// judged before it is probed: a path the judge refuses counts as still there.
#[cfg(windows)]
pub fn repair_at_start() {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut r = crate::reg::HkcuRegistry;
    let fix = server_repair(&r, &exe, &|p| crate::localfs::judged(p) && !crate::localfs::local_exists(p));
    if !fix.is_empty()
        && let Err(e) = crate::reg::apply(&mut r, &fix) {
            crate::host::log(&format!("default terminal: could not repair the handoff class: {e}"));
        }
}

/// The pair put back: the saved one, else "Let Windows decide".
pub fn restore_entries(saved: Option<Pair>) -> Vec<(String, String, String)> {
    let p = saved.unwrap_or(Pair { console: CLSID_DEFAULT.into(), terminal: CLSID_DEFAULT.into() });
    vec![entry(STARTUP_KEY, CONSOLE_VALUE, &p.console), entry(STARTUP_KEY, TERMINAL_VALUE, &p.terminal)]
}

pub fn save_entries(p: &Pair) -> Vec<(String, String, String)> {
    vec![entry(SAVED_KEY, "Console", &p.console), entry(SAVED_KEY, "Terminal", &p.terminal)]
}

pub fn saved(r: &dyn Registry) -> Option<Pair> {
    Some(Pair { console: r.get(SAVED_KEY, "Console")?, terminal: r.get(SAVED_KEY, "Terminal")? })
}

/// The old pair is never overwritten while the current pair is already Termurion's.
pub fn should_save(current: &Pair) -> bool {
    !is_termurion(current)
}

/// Windows Terminal's `wt.exe` alias is in `windows_apps`. The alias is an app-execution link,
/// so the entry itself is looked at, never followed; the path goes through the judge first.
pub fn console_host_present(windows_apps: &Path) -> bool {
    crate::localfs::judge(windows_apps.join("wt.exe")).is_some_and(|p| crate::localfs::lstat(&p).is_ok())
}

/// The alert to show instead of making Termurion the default (Windows Terminal first).
pub fn make_blocked(wt_present: bool, pack_present: bool) -> Option<&'static str> {
    if !wt_present {
        Some(NEEDS_WT_TEXT)
    } else if !pack_present {
        Some(pack_missing_text())
    } else {
        None
    }
}

// ---- the handoff gate (B9): everything a hostile local caller could try ----

pub const MAX_TITLE_UNITS: usize = 256;
pub const MAX_LIVE: usize = 16;
pub const PER_SECOND: usize = 4;
pub const IDLE_EXIT: Duration = Duration::from_secs(15);

/// Why a handoff is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Caller,
    Handle,
    Startup,
    Flood,
    Full,
    Closing,
}

impl Refusal {
    /// The HRESULT the caller gets.
    pub fn hresult(self) -> i32 {
        (match self {
            Refusal::Caller => 0x8007_0005u32,          // E_ACCESSDENIED
            Refusal::Handle | Refusal::Startup => 0x8007_0057, // E_INVALIDARG
            Refusal::Flood | Refusal::Full => 0x8007_00AA, // ERROR_BUSY
            Refusal::Closing => 0x8000_4004,            // E_ABORT
        }) as i32
    }
}

/// What the call carried that can be judged without touching a handle.
#[derive(Clone, Copy, Debug)]
pub struct Args {
    pub signal: isize,
    pub reference: isize,
    pub server: isize,
    pub client: isize,
    /// Both out pointers were given.
    pub outputs: bool,
    /// The startup info pointer was given.
    pub startup: bool,
}

/// The rate limit and the count of refused calls (for one banner).
#[derive(Default)]
pub struct Gate {
    stamps: VecDeque<Instant>,
    ignored: usize,
}

impl Gate {
    pub fn ignored(&self) -> usize {
        self.ignored
    }

    pub fn take_ignored(&mut self) -> usize {
        std::mem::take(&mut self.ignored)
    }
}

/// Judges one call: the caller first (`api_peer::judge_peer`; a foreign caller costs no budget), then the pointers and
/// handles, a closing Termurion, the live count and the rate. `Ok` spends one slot of the rate.
pub fn vet(caller: Option<&crate::api_peer::Peer>, mine: &crate::api_peer::Peer, a: &Args, gate: &mut Gate, now: Instant, live: usize, closing: bool) -> Result<(), Refusal> {
    // The one peer rule: same user, session, elevation and integrity, not sandboxed (review I1).
    if !caller.is_some_and(|c| crate::api_peer::judge_peer(c, mine).is_ok()) {
        return Err(Refusal::Caller);
    }
    if !a.outputs || !a.startup {
        return Err(Refusal::Startup);
    }
    let hs = [a.signal, a.reference, a.server, a.client];
    if hs.iter().any(|h| *h == 0 || *h == -1) || (0..4).any(|i| (i + 1..4).any(|j| hs[i] == hs[j])) {
        return Err(Refusal::Handle);
    }
    if closing {
        return Err(Refusal::Closing);
    }
    if live >= MAX_LIVE {
        gate.ignored += 1;
        return Err(Refusal::Full);
    }
    while gate.stamps.front().is_some_and(|t| now.saturating_duration_since(*t) >= Duration::from_secs(1)) {
        gate.stamps.pop_front();
    }
    if gate.stamps.len() >= PER_SECOND {
        gate.ignored += 1;
        return Err(Refusal::Flood);
    }
    gate.stamps.push_back(now);
    Ok(())
}

pub fn handoff_banner(n: usize) -> String {
    format!("{n} terminal hand-off{} ignored", if n == 1 { "" } else { "s" })
}

/// The startup title as text: at most `MAX_TITLE_UNITS` UTF-16 units (never cut inside a pair),
/// valid UTF-16, with no control, bidi or zero-width character. None means "use the profile's name".
pub fn clean_title(units: &[u16]) -> Option<String> {
    let mut n = units.len().min(MAX_TITLE_UNITS);
    if n < units.len() && n > 0 && (0xD800..0xDC00).contains(&units[n - 1]) {
        n -= 1;
    } else if n == units.len() && n > 0 && (0xD800..0xDC00).contains(&units[n - 1]) {
        return None;
    }
    let text = String::from_utf16(&units[..n]).ok()?;
    (!text.is_empty()).then_some(text).filter(|t| crate::pathnav::safe_text(t).is_some())
}

// ---- `-Embedding` (B9) ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EmbeddingStart {
    Run,
    Exit(&'static str),
}

/// What an `-Embedding` start does: it runs only while Termurion is the default terminal with its
/// own conpty.dll and no Termurion of this user already answers on the launch pipe (it never
/// forwards anything: COM starts it only to take one handoff).
pub fn embedding_start(pair_is_termurion: bool, pack_present: bool, running: &crate::url_launch::Handed) -> EmbeddingStart {
    use crate::url_launch::Handed;
    if !pair_is_termurion {
        return EmbeddingStart::Exit("Termurion is not the default terminal");
    }
    if !pack_present {
        return EmbeddingStart::Exit("no conpty.dll with ConptyPackPseudoConsole");
    }
    match running {
        Handed::NoServer | Handed::OtherOwner(_) => EmbeddingStart::Run,
        Handed::Taken | Handed::Failed(_) => EmbeddingStart::Exit("a running Termurion answers on the launch pipe"),
    }
}

/// What an `-Embedding` start that has no window yet does next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    /// Nothing yet: wait until this moment.
    Wait(Instant),
    /// A handoff or work from the pipe arrived: run as the main instance (and serve the pipe).
    BecomeMain,
    /// `IDLE_EXIT` passed with nothing: stop serving the launch pipe and the handoff class, and look once more.
    StopServing,
    /// Nothing arrived, nothing is served: end.
    Exit,
}

/// The next step of an `-Embedding` start. `waiting` counts the requests it accepted on the pipe
/// that wait for a window; it never ends with one waiting, and it ends only after it stopped
/// serving, so a request the pipe took just before is seen first.
pub fn embedding_next(started: Instant, now: Instant, handoffs: usize, waiting: usize, serving: bool) -> Idle {
    if handoffs > 0 || waiting > 0 {
        Idle::BecomeMain
    } else if now.saturating_duration_since(started) < IDLE_EXIT {
        Idle::Wait(started + IDLE_EXIT)
    } else if serving {
        Idle::StopServing
    } else {
        Idle::Exit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reg::{FakeRegistry, Registry, apply};
    use std::path::Path;
    use std::time::{Duration, Instant};

    fn pair(c: &str, t: &str) -> Pair {
        Pair { console: c.into(), terminal: t.into() }
    }

    #[test]
    fn delegation_follows_windows() {
        let mut r = FakeRegistry::default();
        assert_eq!(current(&r), pair(CLSID_DEFAULT, CLSID_DEFAULT));
        assert!(make_enabled(&r));
        apply(&mut r, &make_termurion_entries(Path::new(r"C:\T\termurion.exe"))).unwrap();
        assert_eq!(current(&r), pair(CLSID_WT_CONSOLE, CLSID_TERMURION_TERMINAL));
        assert!(is_termurion(&current(&r)));
        assert!(!make_enabled(&r));
        let mut r = FakeRegistry::default();
        apply(&mut r, &restore_entries(None)).unwrap();
        assert_eq!(current(&r), pair(CLSID_DEFAULT, CLSID_DEFAULT));
        apply(&mut r, &restore_entries(Some(pair(CLSID_CONHOST, CLSID_CONHOST)))).unwrap();
        assert_eq!(current(&r), pair(CLSID_CONHOST, CLSID_CONHOST));
        let dir = std::env::temp_dir().join(format!("termurion-wt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!console_host_present(&dir));
        std::fs::write(dir.join("wt.exe"), b"").unwrap();
        assert!(console_host_present(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_previous_pair_is_saved_once_and_restored() {
        let mut r = FakeRegistry::default();
        apply(&mut r, &restore_entries(Some(pair(CLSID_CONHOST, CLSID_CONHOST)))).unwrap();
        let now = current(&r);
        apply(&mut r, &save_entries(&now)).unwrap();
        assert_eq!(saved(&r), Some(pair(CLSID_CONHOST, CLSID_CONHOST)));
        apply(&mut r, &make_termurion_entries(Path::new(r"C:\T\termurion.exe"))).unwrap();
        // Already Termurion's: nothing is saved over the old pair.
        assert!(should_save(&pair(CLSID_CONHOST, CLSID_CONHOST)));
        assert!(!should_save(&current(&r)));
        assert_eq!(saved(&r), Some(pair(CLSID_CONHOST, CLSID_CONHOST)));
        // Only Termurion's own class tree is deleted (P9).
        assert!(crate::reg::deletable(&class_key()));
        assert!(r.delete_tree(&class_key()).is_ok());
        assert!(r.delete_tree(r"Console\%%Startup").is_err());
    }

    #[test]
    fn make_needs_pack() {
        assert_eq!(make_blocked(true, true), None);
        assert_eq!(make_blocked(false, true), Some(NEEDS_WT_TEXT));
        assert_eq!(make_blocked(true, false), Some(pack_missing_text()));
        assert_eq!(make_blocked(false, false), Some(NEEDS_WT_TEXT));
    }

    #[test]
    fn registration_follows_com() {
        let e = make_termurion_entries(Path::new(r"C:\T\termurion.exe"));
        let t = |k: &str, n: &str, v: &str| (k.to_string(), n.to_string(), v.to_string());
        let key = r"Software\Classes\CLSID\{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}";
        assert!(e.contains(&t(key, "", "Termurion Terminal Handoff")));
        // COM appends ` -Embedding` itself (LocalServer32 doc): the value is the quoted exe only.
        assert!(e.contains(&t(&format!(r"{key}\LocalServer32"), "", "\"C:\\T\\termurion.exe\"")));
        assert!(e.contains(&t(STARTUP_KEY, CONSOLE_VALUE, CLSID_WT_CONSOLE)));
        assert!(e.contains(&t(STARTUP_KEY, TERMINAL_VALUE, CLSID_TERMURION_TERMINAL)));
        assert_eq!(NEEDS_WT_HEADING, "Windows Terminal Required");
        assert!(NEEDS_WT_TEXT.starts_with("Windows hands console programs") && NEEDS_WT_TEXT.ends_with("choose this again."));
        // HKCU only: no entry is outside the three allowed places.
        assert!(e.iter().all(|(k, _, _)| k.starts_with(r"Software\Classes\CLSID\{6A1E") || k == STARTUP_KEY));
    }

    fn sid(s: &str) -> crate::api_peer::Peer {
        let token = crate::api_peer::Token { sid: s.into(), elevated: false, integrity: crate::api_peer::MEDIUM_RID, app_container: false, restricted: false };
        crate::api_peer::Peer { token, session: Some(1) }
    }

    fn me() -> crate::api_peer::Peer {
        sid("S-1-5-21-1")
    }

    fn args() -> Args {
        Args { signal: 0x10, reference: 0x14, server: 0x18, client: 0x1c, outputs: true, startup: true }
    }

    /// B9: the caller must be this user; checked before anything else, so a foreign caller costs no budget.
    #[test]
    fn handoff_refuses_a_foreign_caller() {
        let mut g = Gate::default();
        let now = Instant::now();
        assert_eq!(vet(Some(&sid("S-1-5-21-2")), &sid("S-1-5-21-1"), &args(), &mut g, now, 0, false), Err(Refusal::Caller));
        assert_eq!(vet(None, &sid("S-1-5-21-1"), &args(), &mut g, now, 0, false), Err(Refusal::Caller));
        for _ in 0..PER_SECOND {
            assert_eq!(vet(Some(&sid("S-1-5-21-2")), &sid("S-1-5-21-1"), &args(), &mut g, now, 0, false), Err(Refusal::Caller));
        }
        assert_eq!(vet(Some(&sid("S-1-5-21-1")), &sid("S-1-5-21-1"), &args(), &mut g, now, 0, false), Ok(()));
        assert_eq!(Refusal::Caller.hresult() as u32, 0x8007_0005);
    }

    /// Task 5 review, Important 1: the caller is judged by the one peer rule (`api_peer::judge_peer`):
    /// the same user, session and integrity as Termurion, not elevated when Termurion is not, never
    /// sandboxed. An elevated console host of the same split-token user is refused (its console would
    /// be driven by a medium Termurion any medium program can reach), and so is one of another session.
    #[test]
    fn handoff_caller_matches_user_session_and_integrity() {
        use crate::api_peer::{HIGH_RID, LOW_RID, Peer, Token};
        let mut g = Gate::default();
        let now = Instant::now();
        let t = |token: Token, session| Peer { token, session };
        let base = me().token;
        for (why, bad) in [
            ("elevated, high integrity", t(Token { elevated: true, integrity: HIGH_RID, ..base.clone() }, Some(1))),
            ("high integrity", t(Token { integrity: HIGH_RID, ..base.clone() }, Some(1))),
            ("low integrity", t(Token { integrity: LOW_RID, ..base.clone() }, Some(1))),
            ("another session", t(base.clone(), Some(2))),
            ("an unreadable session", t(base.clone(), None)),
            ("an app container", t(Token { app_container: true, ..base.clone() }, Some(1))),
            ("a restricted token", t(Token { restricted: true, ..base.clone() }, Some(1))),
        ] {
            assert_eq!(vet(Some(&bad), &me(), &args(), &mut g, now, 0, false), Err(Refusal::Caller), "{why}");
        }
        assert_eq!(g.ignored(), 0, "a refused caller costs no budget");
        assert_eq!(vet(Some(&me()), &me(), &args(), &mut g, now, 0, false), Ok(()));
        // The pipe's server check is the same rule.
        let src = include_str!("url_launch.rs");
        assert!(src.contains("crate::api_peer::judge_peer("), "server_verdict uses the one peer judge");
        let hw = include_str!("handoff_win.rs");
        assert!(hw.contains("winpipe::thread_peer()"), "the caller's token and session are read while impersonating");
    }

    /// Task 5 review, Important 2 (P11): when Termurion's exe moved, startup rewrites only the
    /// `LocalServer32` value of Termurion's own class, only while the pair is Termurion's, and only when the
    /// recorded exe is gone (judged before it is probed, by the caller's `gone`).
    #[test]
    fn a_moved_exe_gets_its_handoff_server_repaired() {
        let old = Path::new(r"C:\Old\termurion.exe");
        let new = Path::new(r"D:\New\termurion.exe");
        let server = format!(r"{}\LocalServer32", class_key());
        let mut r = FakeRegistry::default();
        apply(&mut r, &make_termurion_entries(old)).unwrap();
        assert!(server_repair(&r, new, &|_| false).is_empty(), "another live copy keeps its registration");
        let asked = std::cell::RefCell::new(Vec::new());
        let gone = |p: &Path| {
            asked.borrow_mut().push(p.to_path_buf());
            p == old
        };
        assert_eq!(server_repair(&r, new, &gone), vec![(server.clone(), String::new(), server_value(new))]);
        assert_eq!(asked.borrow().as_slice(), [old.to_path_buf()]);
        let fix = server_repair(&r, new, &gone);
        apply(&mut r, &fix).unwrap();
        assert_eq!(r.get(&server, "").as_deref(), Some(server_value(new).as_str()));
        assert!(server_repair(&r, new, &|_| true).is_empty(), "this exe's own value needs nothing");
        // An older Termurion's value for this very exe (with its own -Embedding) takes the plain form.
        r.set(&server, "", &format!("{} -Embedding", server_value(new))).unwrap();
        assert_eq!(server_repair(&r, new, &|_| false), vec![(server.clone(), String::new(), server_value(new))]);
        // Not the default terminal any more: nothing is touched.
        let mut r = FakeRegistry::default();
        apply(&mut r, &make_termurion_entries(old)).unwrap();
        apply(&mut r, &restore_entries(None)).unwrap();
        assert!(server_repair(&r, new, &|_| true).is_empty());
        // A value that is not the shape Termurion writes is never rewritten, and its path never probed.
        let mut r = FakeRegistry::default();
        apply(&mut r, &make_termurion_entries(old)).unwrap();
        for v in ["", "C:\\Old\\termurion.exe", "\"C:\\Old\\termurion.exe\" /other", "\"relative\\termurion.exe\"", "\"\\\\host\\s\\termurion.exe\" x"] {
            r.set(&server, "", v).unwrap();
            let asked = std::cell::Cell::new(false);
            assert!(server_repair(&r, new, &|_| { asked.set(true); true }).is_empty(), "{v}");
            assert!(!asked.get(), "{v}: nothing is probed");
        }
        let run = include_str!("app/mod.rs");
        assert!(run.contains("crate::default_term::repair_at_start()"), "startup runs the repair");
    }

    #[test]
    fn handoff_refuses_bad_handles_and_pointers() {
        let mut g = Gate::default();
        let ok = |a: &Args, g: &mut Gate| vet(Some(&me()), &me(), a, g, Instant::now(), 0, false);
        for bad in [
            Args { signal: 0, ..args() },
            Args { reference: 0, ..args() },
            Args { server: -1, ..args() },
            Args { client: 0, ..args() },
            Args { signal: 0x10, reference: 0x10, ..args() },
            Args { server: 0x1c, ..args() },
            Args { outputs: false, ..args() },
            Args { startup: false, ..args() },
        ] {
            assert!(matches!(ok(&bad, &mut g), Err(Refusal::Handle | Refusal::Startup)), "{bad:?}");
        }
        assert_eq!(ok(&args(), &mut g), Ok(()));
    }

    #[test]
    fn handoff_title_is_bounded_and_clean() {
        let u = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
        assert_eq!(clean_title(&u("cmd.exe")).as_deref(), Some("cmd.exe"));
        assert_eq!(clean_title(&[]), None);
        for bad in ["a\u{1b}[31m", "a\nb", "a\u{202e}b", "a\u{200b}b", "a\u{7f}", "a\u{85}"] {
            assert_eq!(clean_title(&u(bad)), None, "{bad:?}");
        }
        let long = vec![b'x' as u16; 1_000_000];
        assert_eq!(clean_title(&long).map(|t| t.chars().count()), Some(MAX_TITLE_UNITS));
        // The cap never splits a surrogate pair.
        let mut pair_at_end = vec![b'x' as u16; MAX_TITLE_UNITS - 1];
        pair_at_end.extend(u("\u{1F600}"));
        let t = clean_title(&pair_at_end).unwrap();
        assert_eq!(t.chars().count(), MAX_TITLE_UNITS - 1);
        assert_eq!(clean_title(&[0xD800, 0x41]), None, "a lone surrogate is not text");
    }

    #[test]
    fn handoff_flood_is_capped() {
        let mut g = Gate::default();
        let t0 = Instant::now();
        let mut done = 0;
        for _ in 0..50 {
            if vet(Some(&me()), &me(), &args(), &mut g, t0, 0, false).is_ok() {
                done += 1;
            }
        }
        assert_eq!(done, PER_SECOND);
        assert_eq!(g.ignored(), 50 - PER_SECOND);
        assert_eq!(vet(Some(&me()), &me(), &args(), &mut g, t0, 0, false), Err(Refusal::Flood));
        // A second later the window has passed.
        assert_eq!(vet(Some(&me()), &me(), &args(), &mut g, t0 + Duration::from_millis(1100), 0, false), Ok(()));
        // At most MAX_LIVE panes stay open.
        let mut g = Gate::default();
        assert_eq!(vet(Some(&me()), &me(), &args(), &mut g, t0, MAX_LIVE, false), Err(Refusal::Full));
        assert_eq!(handoff_banner(1), "1 terminal hand-off ignored");
        assert_eq!(handoff_banner(3), "3 terminal hand-offs ignored");
    }

    #[test]
    fn a_handoff_while_closing_is_refused_cleanly() {
        let mut g = Gate::default();
        assert_eq!(vet(Some(&me()), &me(), &args(), &mut g, Instant::now(), 0, true), Err(Refusal::Closing));
        assert_eq!(g.ignored(), 0, "a closing Termurion does not count the call as a flood");
    }

    #[test]
    fn embedding_is_only_the_sole_argument() {
        let cwd = Path::new(r"C:\");
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(crate::launch::parse(&a(&["-Embedding"]), cwd), Ok(crate::launch::Request::Embedding));
        for bad in [&["-Embedding", "x.ps1"][..], &["x.ps1", "-Embedding"], &["--new-tab", "-Embedding"], &["-Embedding", "--url", "ssh://h"]] {
            assert!(crate::launch::parse(&a(bad), cwd).is_err(), "{bad:?}");
        }
    }

    /// Task 5 review, Critical: COM starts the class's server with the `LocalServer32` value plus
    /// ` -Embedding` (learn.microsoft.com/windows/win32/com/localserver32). The command line it makes
    /// from the registered value, split by Windows' argv rules, parses as `Embedding`. A value written
    /// by the old build (with its own ` -Embedding`) gives the flag twice, which is still one start.
    #[test]
    fn the_command_line_com_makes_is_an_embedding_start() {
        let cwd = Path::new(r"C:\");
        let exe = Path::new(r"C:\Program Files\Termurion\termurion.exe");
        let value = make_termurion_entries(exe).into_iter().find(|(k, _, _)| k.ends_with(r"\LocalServer32")).unwrap().2;
        let com_line = format!("{value} -Embedding");
        let argv = crate::semantic::split_command_line(&com_line);
        assert_eq!(argv[0], exe.to_string_lossy());
        assert_eq!(crate::launch::parse(&argv[1..], cwd), Ok(crate::launch::Request::Embedding), "{com_line}");
        let old = format!("\"{}\" -Embedding -Embedding", exe.display());
        let argv = crate::semantic::split_command_line(&old);
        assert_eq!(crate::launch::parse(&argv[1..], cwd), Ok(crate::launch::Request::Embedding), "{old}");
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(crate::launch::parse(&a(&["/Embedding", "-embedding"]), cwd), Ok(crate::launch::Request::Embedding));
        assert!(crate::launch::parse(&a(&["-Embedding", "-Embedding", "-Embedding"]), cwd).is_err());
    }

    #[test]
    fn embedding_with_a_running_termurion_exits() {
        use crate::url_launch::Handed;
        let run = |termurion, pack, h| embedding_start(termurion, pack, &h);
        assert_eq!(run(true, true, Handed::NoServer), EmbeddingStart::Run);
        assert_eq!(run(true, true, Handed::OtherOwner("x".into())), EmbeddingStart::Run);
        assert!(matches!(run(true, true, Handed::Taken), EmbeddingStart::Exit(_)));
        assert!(matches!(run(true, true, Handed::Failed("busy".into())), EmbeddingStart::Exit(_)));
        // Not the default terminal, or no pack: nothing can use a handoff, so it ends at once.
        assert!(matches!(run(false, true, Handed::NoServer), EmbeddingStart::Exit(_)));
        assert!(matches!(run(true, false, Handed::NoServer), EmbeddingStart::Exit(_)));
    }

    /// Task 5 review, Important 3: an `-Embedding` start that holds the launch pipe never drops a
    /// request it accepted. Work waiting (a launch, a link, files to open) or a handoff makes it the
    /// main instance at once; with none it stops serving the pipe after 15 s, and ends only on the
    /// pass after that, when what the pipe took before it stopped has been seen.
    #[test]
    fn an_idle_embedding_start_ends_itself() {
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        assert_eq!(embedding_next(t0, at(14), 0, 0, true), Idle::Wait(at(15)));
        assert_eq!(embedding_next(t0, at(15), 0, 0, true), Idle::StopServing);
        assert_eq!(embedding_next(t0, at(16), 0, 0, false), Idle::Exit);
        assert_eq!(embedding_next(t0, at(1), 1, 0, true), Idle::BecomeMain, "a handoff arrived");
        for (now, serving) in [(1, true), (15, true), (60, true), (60, false)] {
            assert_eq!(embedding_next(t0, at(now), 0, 1, serving), Idle::BecomeMain, "work waits at {now} s: never an exit");
        }
        let ui = include_str!("app/mod.rs");
        let wait = &ui[ui.find("fn about_to_wait").unwrap()..];
        let wait = &wait[..wait.find("self.handoff_banner_tick()").unwrap()];
        assert!(wait.contains("embedding_next(") && wait.contains("self.embedding_waiting_work()"));
        let dt = include_str!("app/default_term_ui.rs");
        let work = &dt[dt.find("fn embedding_waiting_work").unwrap()..];
        let work = &work[..work.find("\n    }\n").unwrap()];
        for q in ["self.launch_queue", "self.pending_urls_at_start", "self.open_batch", "self.handoff_queue"] {
            assert!(work.contains(q), "{q} counts as waiting work");
        }
    }
}
