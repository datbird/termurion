//! Tandem as the Windows default terminal (m13v1 Task 5), the pure half: the delegation pair, the
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
/// Tandem's own terminal class, fixed.
pub const CLSID_TANDEM_TERMINAL: &str = "{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}";
pub const SAVED_KEY: &str = r"Software\Tandem\PreviousDefaultTerminal";

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
    "Tandem can be the default terminal only with its own conpty.dll next to tandem.exe."
}

/// The registry key of Tandem's COM class (the one tree `unmake` deletes).
pub fn class_key() -> String {
    format!(r"Software\Classes\CLSID\{CLSID_TANDEM_TERMINAL}")
}

fn value_or_default(r: &dyn Registry, key: &str, name: &str) -> String {
    r.get(key, name).filter(|v| !v.is_empty()).unwrap_or_else(|| CLSID_DEFAULT.to_string())
}

/// The delegation pair now (a missing value is "Let Windows decide").
pub fn current(r: &dyn Registry) -> Pair {
    Pair { console: value_or_default(r, STARTUP_KEY, CONSOLE_VALUE), terminal: value_or_default(r, STARTUP_KEY, TERMINAL_VALUE) }
}

pub fn is_tandem(p: &Pair) -> bool {
    p.terminal.eq_ignore_ascii_case(CLSID_TANDEM_TERMINAL)
}

/// `AD:481-482`: the menu item is enabled while Tandem is not already the default.
pub fn make_enabled(r: &dyn Registry) -> bool {
    !is_tandem(&current(r))
}

fn entry(k: &str, n: &str, v: &str) -> (String, String, String) {
    (k.to_string(), n.to_string(), v.to_string())
}

/// The COM class and the pair that hands consoles to it: Windows Terminal's console host with
/// Tandem's terminal class. Per user (`HKCU`) only.
pub fn make_tandem_entries(exe: &Path) -> Vec<(String, String, String)> {
    let key = class_key();
    vec![
        entry(&key, "", "Tandem Terminal Handoff"),
        entry(&format!(r"{key}\LocalServer32"), "", &format!("\"{}\" -Embedding", exe.display())),
        entry(STARTUP_KEY, CONSOLE_VALUE, CLSID_WT_CONSOLE),
        entry(STARTUP_KEY, TERMINAL_VALUE, CLSID_TANDEM_TERMINAL),
    ]
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

/// The old pair is never overwritten while the current pair is already Tandem's.
pub fn should_save(current: &Pair) -> bool {
    !is_tandem(current)
}

/// Windows Terminal's `wt.exe` alias is in `windows_apps`. The alias is an app-execution link,
/// so the entry itself is looked at, never followed; the path goes through the judge first.
pub fn console_host_present(windows_apps: &Path) -> bool {
    crate::localfs::judge(windows_apps.join("wt.exe")).is_some_and(|p| crate::localfs::lstat(&p).is_ok())
}

/// The alert to show instead of making Tandem the default (Windows Terminal first).
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

/// Judges one call: the caller first (a foreign caller costs no budget), then the pointers and
/// handles, a closing Tandem, the live count and the rate. `Ok` spends one slot of the rate.
pub fn vet(caller: Option<&str>, mine: &str, a: &Args, gate: &mut Gate, now: Instant, live: usize, closing: bool) -> Result<(), Refusal> {
    if !caller.is_some_and(|c| c.eq_ignore_ascii_case(mine)) {
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

/// What an `-Embedding` start does: it runs only while Tandem is the default terminal with its
/// own conpty.dll and no Tandem of this user already answers on the launch pipe (it never
/// forwards anything: COM starts it only to take one handoff).
pub fn embedding_start(pair_is_tandem: bool, pack_present: bool, running: &crate::url_launch::Handed) -> EmbeddingStart {
    use crate::url_launch::Handed;
    if !pair_is_tandem {
        return EmbeddingStart::Exit("Tandem is not the default terminal");
    }
    if !pack_present {
        return EmbeddingStart::Exit("no conpty.dll with ConptyPackPseudoConsole");
    }
    match running {
        Handed::NoServer | Handed::OtherOwner(_) => EmbeddingStart::Run,
        Handed::Taken | Handed::Failed(_) => EmbeddingStart::Exit("a running Tandem answers on the launch pipe"),
    }
}

/// An `-Embedding` start with no handoff and no window ends itself after `IDLE_EXIT`.
pub fn idle_exit(started: Instant, now: Instant, handoffs: usize, windows: usize) -> bool {
    handoffs == 0 && windows == 0 && now.saturating_duration_since(started) >= IDLE_EXIT
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
        apply(&mut r, &make_tandem_entries(Path::new(r"C:\T\tandem.exe"))).unwrap();
        assert_eq!(current(&r), pair(CLSID_WT_CONSOLE, CLSID_TANDEM_TERMINAL));
        assert!(is_tandem(&current(&r)));
        assert!(!make_enabled(&r));
        let mut r = FakeRegistry::default();
        apply(&mut r, &restore_entries(None)).unwrap();
        assert_eq!(current(&r), pair(CLSID_DEFAULT, CLSID_DEFAULT));
        apply(&mut r, &restore_entries(Some(pair(CLSID_CONHOST, CLSID_CONHOST)))).unwrap();
        assert_eq!(current(&r), pair(CLSID_CONHOST, CLSID_CONHOST));
        let dir = std::env::temp_dir().join(format!("tandem-wt-{}", std::process::id()));
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
        apply(&mut r, &make_tandem_entries(Path::new(r"C:\T\tandem.exe"))).unwrap();
        // Already Tandem's: nothing is saved over the old pair.
        assert!(should_save(&pair(CLSID_CONHOST, CLSID_CONHOST)));
        assert!(!should_save(&current(&r)));
        assert_eq!(saved(&r), Some(pair(CLSID_CONHOST, CLSID_CONHOST)));
        // Only Tandem's own class tree is deleted (P9).
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
        let e = make_tandem_entries(Path::new(r"C:\T\tandem.exe"));
        let t = |k: &str, n: &str, v: &str| (k.to_string(), n.to_string(), v.to_string());
        let key = r"Software\Classes\CLSID\{6A1E8D3C-2B7F-4C9E-9D41-5F0B7A2C8E13}";
        assert!(e.contains(&t(key, "", "Tandem Terminal Handoff")));
        assert!(e.contains(&t(&format!(r"{key}\LocalServer32"), "", "\"C:\\T\\tandem.exe\" -Embedding")));
        assert!(e.contains(&t(STARTUP_KEY, CONSOLE_VALUE, CLSID_WT_CONSOLE)));
        assert!(e.contains(&t(STARTUP_KEY, TERMINAL_VALUE, CLSID_TANDEM_TERMINAL)));
        assert_eq!(NEEDS_WT_HEADING, "Windows Terminal Required");
        assert!(NEEDS_WT_TEXT.starts_with("Windows hands console programs") && NEEDS_WT_TEXT.ends_with("choose this again."));
        // HKCU only: no entry is outside the three allowed places.
        assert!(e.iter().all(|(k, _, _)| k.starts_with(r"Software\Classes\CLSID\{6A1E") || k == STARTUP_KEY));
    }

    fn args() -> Args {
        Args { signal: 0x10, reference: 0x14, server: 0x18, client: 0x1c, outputs: true, startup: true }
    }

    /// B9: the caller must be this user; checked before anything else, so a foreign caller costs no budget.
    #[test]
    fn handoff_refuses_a_foreign_caller() {
        let mut g = Gate::default();
        let now = Instant::now();
        assert_eq!(vet(Some("S-1-5-21-2"), "S-1-5-21-1", &args(), &mut g, now, 0, false), Err(Refusal::Caller));
        assert_eq!(vet(None, "S-1-5-21-1", &args(), &mut g, now, 0, false), Err(Refusal::Caller));
        for _ in 0..PER_SECOND {
            assert_eq!(vet(Some("S-1-5-21-2"), "S-1-5-21-1", &args(), &mut g, now, 0, false), Err(Refusal::Caller));
        }
        assert_eq!(vet(Some("s-1-5-21-1"), "S-1-5-21-1", &args(), &mut g, now, 0, false), Ok(()));
        assert_eq!(Refusal::Caller.hresult() as u32, 0x8007_0005);
    }

    #[test]
    fn handoff_refuses_bad_handles_and_pointers() {
        let mut g = Gate::default();
        let ok = |a: &Args, g: &mut Gate| vet(Some("S"), "S", a, g, Instant::now(), 0, false);
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
            if vet(Some("S"), "S", &args(), &mut g, t0, 0, false).is_ok() {
                done += 1;
            }
        }
        assert_eq!(done, PER_SECOND);
        assert_eq!(g.ignored(), 50 - PER_SECOND);
        assert_eq!(vet(Some("S"), "S", &args(), &mut g, t0, 0, false), Err(Refusal::Flood));
        // A second later the window has passed.
        assert_eq!(vet(Some("S"), "S", &args(), &mut g, t0 + Duration::from_millis(1100), 0, false), Ok(()));
        // At most MAX_LIVE panes stay open.
        let mut g = Gate::default();
        assert_eq!(vet(Some("S"), "S", &args(), &mut g, t0, MAX_LIVE, false), Err(Refusal::Full));
        assert_eq!(handoff_banner(1), "1 terminal hand-off ignored");
        assert_eq!(handoff_banner(3), "3 terminal hand-offs ignored");
    }

    #[test]
    fn a_handoff_while_closing_is_refused_cleanly() {
        let mut g = Gate::default();
        assert_eq!(vet(Some("S"), "S", &args(), &mut g, Instant::now(), 0, true), Err(Refusal::Closing));
        assert_eq!(g.ignored(), 0, "a closing Tandem does not count the call as a flood");
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

    #[test]
    fn embedding_with_a_running_tandem_exits() {
        use crate::url_launch::Handed;
        let run = |tandem, pack, h| embedding_start(tandem, pack, &h);
        assert_eq!(run(true, true, Handed::NoServer), EmbeddingStart::Run);
        assert_eq!(run(true, true, Handed::OtherOwner("x".into())), EmbeddingStart::Run);
        assert!(matches!(run(true, true, Handed::Taken), EmbeddingStart::Exit(_)));
        assert!(matches!(run(true, true, Handed::Failed("busy".into())), EmbeddingStart::Exit(_)));
        // Not the default terminal, or no pack: nothing can use a handoff, so it ends at once.
        assert!(matches!(run(false, true, Handed::NoServer), EmbeddingStart::Exit(_)));
        assert!(matches!(run(true, false, Handed::NoServer), EmbeddingStart::Exit(_)));
    }

    #[test]
    fn an_idle_embedding_start_ends_itself() {
        let t0 = Instant::now();
        assert!(!idle_exit(t0, t0 + Duration::from_secs(14), 0, 0));
        assert!(idle_exit(t0, t0 + Duration::from_secs(15), 0, 0));
        assert!(!idle_exit(t0, t0 + Duration::from_secs(60), 1, 0), "a handoff arrived");
        assert!(!idle_exit(t0, t0 + Duration::from_secs(60), 0, 1), "a window exists");
    }
}
