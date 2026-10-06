//! The taskbar button's bell count and the bell's small decisions (`iTermDockBadgeController.swift`,
//! `PTYSession.m`), plus the one place a sound name is judged (B2). Pure: the clock, the registry
//! and the file system are given by the caller.
//!
//! The count is the bells rung while no Tandem window is active plus the sessions whose status
//! is the highest-priority pattern (waiting), until Tandem is active or their tab is selected.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::localfs::LocalPath;

/// What is counted on the taskbar button.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Badge {
    bells: u32,
    waiting: BTreeSet<String>,
}

/// The most sessions kept as waiting (a program cannot grow the set without bound).
const MAX_WAITING: usize = 4096;

impl Badge {
    /// A bell rang: counted only while the key is on and no Tandem window is active
    /// (`incrementBellCount`). True when it was counted.
    pub fn bell(&mut self, enabled: bool, active: bool) -> bool {
        if !enabled || active {
            return false;
        }
        self.bells = self.bells.saturating_add(1);
        true
    }

    /// A Tandem window became key or was restored (`resetBellCount`).
    pub fn reset_bells(&mut self) {
        self.bells = 0;
    }

    /// A session's status changed: it is waiting when its status is the highest-priority
    /// pattern; counted only while Tandem is not active, and leaving always removes it
    /// (`sessionDidEnterWaiting`, `sessionDidLeaveWaiting`).
    pub fn waiting(&mut self, session: &str, is_waiting: bool, active: bool) {
        if !is_waiting {
            self.waiting.remove(session);
        } else if !active && (self.waiting.len() < MAX_WAITING || self.waiting.contains(session)) {
            self.waiting.insert(session.to_string());
        }
    }

    /// A tab was selected: its sessions are no longer waiting (`tabWasSelected`).
    pub fn tab_selected(&mut self, sessions: &[String]) {
        for s in sessions {
            self.waiting.remove(s);
        }
    }

    /// Tandem came to the front: the waiting sessions clear, the bells keep (`applicationDidBecomeActive`).
    pub fn became_active(&mut self) {
        self.waiting.clear();
    }

    /// A session is gone.
    pub fn forget(&mut self, session: &str) {
        self.waiting.remove(session);
    }

    /// The total as text; "" at 0.
    pub fn label(&self) -> String {
        let total = self.bells as usize + self.waiting.len();
        if total > 0 { total.to_string() } else { String::new() }
    }
}

/// Whether a status is the highest-priority pattern, the first one (`isHighestPriority`,
/// `StatusPrioritySettings.swift:79-97`): none when there are no patterns or no text.
pub fn is_waiting(status: Option<&str>, priorities: &[&str]) -> bool {
    let Some(text) = status else { return false };
    let lower = text.to_lowercase();
    // The first pattern to match decides, in order: only a match on index 0 is "waiting".
    priorities.iter().position(|p| lower.contains(&p.to_lowercase())) == Some(0)
}

/// Whether the taskbar button flashes for a bell (`PTYSession.m:19114`): something was shown or
/// counted, no Tandem window is active, and `BounceOnInactiveBell` is on.
pub fn bounce(notified: bool, active: bool, on: bool) -> bool {
    notified && !active && on
}

/// What clears a session's bell icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearOn {
    /// The session becomes the one in view.
    Activation,
    /// Only a key sent to the session (`PTYSession.m:4180-4189`).
    Typing,
}

/// `ClearBellIconAggressively`.
pub fn clear_on(aggressive: bool) -> ClearOn {
    if aggressive { ClearOn::Activation } else { ClearOn::Typing }
}

/// The sound named for a mark's result; None for an empty name (the default beep).
pub fn sound_for(failed: bool, success: &str, error: &str) -> Option<String> {
    let name = if failed { error } else { success };
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether the system beep follows a mark sound: only when the sound was refused or unresolved
/// (`None`). A resolved sound that did not start (`Some(false)`: `SND_NOSTOP` while another sound
/// plays) must not be stacked with a beep.
pub fn beeps_after(outcome: Option<bool>) -> bool {
    outcome.is_none()
}

/// The icon's text: the total, "99+" above 99. Digits only, at most three characters, so
/// nothing a program wrote can reach the icon.
pub fn icon_text(label: &str) -> String {
    let digits: String = label.chars().filter(char::is_ascii_digit).take(6).collect();
    match digits.parse::<u32>() {
        Ok(n) if n > 99 => "99+".to_string(),
        Ok(n) if n > 0 => n.to_string(),
        _ => String::new(),
    }
}

/// What the taskbar label does this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BadgeStep {
    /// Nothing to do and nothing to wait for.
    Idle,
    /// Push the label now.
    Push,
    /// The label changed inside the throttle window: push it, and wake, then.
    Wait(std::time::Instant),
}

/// The throttle (`every` between pushes) without ever leaving a changed label unpushed: a
/// change held back by it comes with the moment to come back at.
pub fn badge_step(pushed: Option<(&str, std::time::Instant)>, label: &str, now: std::time::Instant, every: std::time::Duration) -> BadgeStep {
    match pushed {
        None => BadgeStep::Push,
        Some((l, _)) if l == label => BadgeStep::Idle,
        Some((_, t)) if now.saturating_duration_since(t) >= every => BadgeStep::Push,
        Some((_, t)) => BadgeStep::Wait(t + every),
    }
}

/// A sound that passed the judge: only this type reaches `PlaySoundW`. Its field is private,
/// so only `resolve` can make one (B2 by type, as `LocalPath`).
#[derive(Debug)]
pub struct Sound(Inner);

#[derive(Debug)]
enum Inner {
    /// A name registered under `HKCU\AppEvents\EventLabels`.
    Alias(String),
    /// A `.wav` file the path judge found local.
    File(LocalPath),
}

impl Sound {
    /// The registered name, for `SND_ALIAS`.
    pub fn alias(&self) -> Option<&str> {
        match &self.0 {
            Inner::Alias(n) => Some(n),
            Inner::File(_) => None,
        }
    }

    /// The judged file, for `SND_FILENAME`.
    pub fn file(&self) -> Option<&Path> {
        match &self.0 {
            Inner::File(p) => Some(p.path()),
            Inner::Alias(_) => None,
        }
    }
}

/// The largest sound file played.
pub const MAX_SOUND_BYTES: u64 = 5 * 1024 * 1024;
const MAX_NAME: usize = 128;
const MAX_PATH_TEXT: usize = 260;

fn has_dotdot(name: &str) -> bool {
    name.split(['\\', '/']).any(|c| c == "..")
}

fn is_drive_path(name: &str) -> bool {
    let b = name.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// The sound a name means (B2): empty is None (the default beeps stay); a share or device path
/// never; a drive path only through `accept` (the judge, `.wav`, a regular file of at most 5 MiB);
/// a bare name is a registered alias, else `<windir>\Media\<name>` or `<name>.wav` there through
/// `accept`; any other name (a separator, `:`, `..`, a control character, over 128 characters)
/// plays only when it is a registered alias, never a path.
pub fn resolve(name: &str, windir: &Path, is_alias: &dyn Fn(&str) -> bool, accept: &dyn Fn(&Path) -> Option<LocalPath>) -> Option<Sound> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > MAX_PATH_TEXT || name.chars().any(char::is_control) {
        return None;
    }
    if name.starts_with("\\\\") || name.starts_with("//") {
        return None;
    }
    if is_drive_path(name) {
        return if has_dotdot(name) { None } else { accept(Path::new(name)).map(|p| Sound(Inner::File(p))) };
    }
    let plain = name.chars().count() <= MAX_NAME && !name.contains(['\\', '/', ':']) && !name.contains("..");
    if is_alias(name) && name.chars().count() <= MAX_NAME {
        return Some(Sound(Inner::Alias(name.to_string())));
    }
    if !plain {
        return None;
    }
    let media: PathBuf = windir.join("Media");
    [name.to_string(), format!("{name}.wav")].iter().find_map(|cand| accept(&media.join(cand)).map(|p| Sound(Inner::File(p))))
}

/// The real acceptance: judged local, a `.wav`, a regular file of at most 5 MiB.
pub fn accept_file(p: &Path) -> Option<LocalPath> {
    let ext_ok = p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    if !ext_ok {
        return None;
    }
    let judged = LocalPath::judge(p)?;
    let st = crate::localfs::stat(&judged).ok()?;
    (st.is_file && st.len <= MAX_SOUND_BYTES).then_some(judged)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_resolved_sound_that_is_busy_gets_no_beep() {
        assert!(beeps_after(None), "refused: the system beep");
        assert!(!beeps_after(Some(false)), "resolved but busy: no stacked beep");
        assert!(!beeps_after(Some(true)), "played: no beep");
    }

    /// Task 6 fix r1 item 1: a label that changes inside the throttle window is held until the
    /// window ends, with a wake at that moment, so the taskbar number is never left stale.
    #[test]
    fn a_held_back_badge_change_schedules_its_push() {
        use std::time::{Duration, Instant};
        let (t0, every) = (Instant::now(), Duration::from_millis(250));
        let ms = Duration::from_millis;
        assert_eq!(badge_step(None, "1", t0, every), BadgeStep::Push, "nothing pushed yet");
        assert_eq!(badge_step(Some(("1", t0)), "1", t0 + ms(10), every), BadgeStep::Idle, "unchanged: no wake");
        assert_eq!(badge_step(Some(("1", t0)), "2", t0 + ms(100), every), BadgeStep::Wait(t0 + every), "changed inside the window: wake at its end");
        assert_eq!(badge_step(Some(("1", t0)), "", t0 + ms(100), every), BadgeStep::Wait(t0 + every), "a clear is held the same short time");
        assert_eq!(badge_step(Some(("1", t0)), "2", t0 + every, every), BadgeStep::Push);
        assert_eq!(badge_step(Some(("1", t0)), "2", t0 + ms(900), every), BadgeStep::Push);
    }

    /// Task 6 fix r1 item 2 (B2 by type): a `Sound` is built in `resolve` and nowhere else, so
    /// no unjudged string can reach `PlaySoundW`; no other file names its variants.
    #[test]
    fn a_sound_is_built_only_by_the_judge() {
        fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            let code = text.split("#[cfg(test)]").next().unwrap();
            if f.file_name().is_some_and(|n| n == "bell_badge.rs") {
                let judge = &code[code.find("pub fn resolve(").unwrap()..];
                let judge = &judge[..judge.find("\n}\n").unwrap()];
                let outside = code.replace(judge, "");
                assert!(!outside.contains("Sound(Inner::"), "a Sound is built only inside resolve");
                continue;
            }
            for banned in ["Sound::Alias", "Sound::File", "Sound(Inner", "bell_badge::Inner"] {
                assert!(!code.contains(banned), "{} builds or opens a Sound with {banned}", f.display());
            }
        }
    }

    use super::*;
    use std::cell::RefCell;

    const PRIORITIES: [&str; 3] = crate::tabstatus::PRIORITIES;

    #[test]
    fn badge_count_follows_iterm2() {
        let mut b = Badge::default();
        assert!(b.bell(true, false));
        assert_eq!(b.label(), "1");
        assert!(!b.bell(true, true), "active: not counted");
        assert!(!b.bell(false, false), "key off: not counted");
        assert_eq!(b.label(), "1");
        assert!(b.bell(true, false));
        b.reset_bells();
        assert_eq!(b.label(), "");

        b.waiting("s1", true, false);
        assert_eq!(b.label(), "1");
        b.waiting("s2", true, true);
        assert_eq!(b.label(), "1", "active: a waiting session adds nothing");
        b.tab_selected(&["s1".to_string()]);
        assert_eq!(b.label(), "");

        b.bell(true, false);
        b.waiting("s1", true, false);
        assert_eq!(b.label(), "2");
        b.became_active();
        assert_eq!(b.label(), "1", "waiting clears, bells stay");

        b.waiting("s3", true, false);
        b.waiting("s3", false, true);
        assert_eq!(b.label(), "1", "leaving waiting removes it even when active");

        assert!(is_waiting(Some("Waiting for input"), &PRIORITIES));
        assert!(!is_waiting(Some("working hard"), &PRIORITIES));
        assert!(!is_waiting(None, &PRIORITIES));
        assert!(!is_waiting(Some("wait"), &[]));
        assert!(is_waiting(Some("blocked"), &["blocked", "wait"]));
        assert!(!is_waiting(Some("Waiting"), &["blocked", "wait"]), "the user's first pattern decides");
    }

    #[test]
    fn waiting_set_is_bounded() {
        let mut b = Badge::default();
        for i in 0..(MAX_WAITING + 50) {
            b.waiting(&format!("s{i}"), true, false);
        }
        assert_eq!(b.label(), MAX_WAITING.to_string());
    }

    #[test]
    fn bounce_follows_iterm2() {
        assert!(bounce(true, false, true));
        assert!(!bounce(true, true, true));
        assert!(!bounce(false, false, true));
        assert!(!bounce(true, false, false));
    }

    #[test]
    fn bell_icon_clearing_follows_iterm2() {
        assert_eq!(clear_on(true), ClearOn::Activation);
        assert_eq!(clear_on(false), ClearOn::Typing);
    }

    #[test]
    fn mark_sound_names_follow_iterm2() {
        assert_eq!(sound_for(true, "", "Windows Critical Stop"), Some("Windows Critical Stop".to_string()));
        assert_eq!(sound_for(false, "", "x"), None);
        assert_eq!(sound_for(false, "Ding", ""), Some("Ding".to_string()));
        assert_eq!(sound_for(true, "Ding", ""), None);
    }

    #[test]
    fn icon_text_is_a_bounded_number() {
        assert_eq!(icon_text(""), "");
        assert_eq!(icon_text("7"), "7");
        assert_eq!(icon_text("99"), "99");
        assert_eq!(icon_text("100"), "99+");
        assert_eq!(icon_text("123456789012345"), "99+");
        assert_eq!(icon_text("<b>evil</b>"), "", "no digits, no icon");
        assert_eq!(icon_text("0"), "");
    }

    #[test]
    fn sound_names_are_judged() {
        let asked = RefCell::new(Vec::<String>::new());
        let accept = |p: &Path| -> Option<LocalPath> {
            asked.borrow_mut().push(p.to_string_lossy().into_owned());
            None
        };
        let alias = |n: &str| n == "SystemAsterisk";
        let win = Path::new("C:\\Windows");
        // The refusals: nothing reaches the judge, nothing becomes an alias.
        let long = "a".repeat(129);
        let longer = "a".repeat(300);
        for bad in [
            "\\\\evil\\s\\a.wav",
            "//evil/s/a.wav",
            "\\\\?\\UNC\\h\\s\\a.wav",
            "\\\\.\\pipe\\x",
            "C:\\Media\\..\\..\\x.wav",
            "..\\..\\x.wav",
            "sub\\x.wav",
            "a:b",
            "bell\u{7}",
            long.as_str(),
            longer.as_str(),
            "",
            "   ",
        ] {
            assert!(resolve(bad, win, &alias, &accept).is_none(), "{bad:?}");
        }
        assert!(asked.borrow().is_empty(), "{:?}", asked.borrow());
        // An alias is an alias only by exact name.
        assert!(matches!(resolve("SystemAsterisk", win, &alias, &accept), Some(s) if s.alias() == Some("SystemAsterisk")));
        // A bare name asks the judge for the Media folder, plain then with `.wav`; never anywhere else.
        assert!(resolve("Windows Ding", win, &alias, &accept).is_none());
        assert_eq!(*asked.borrow(), vec!["C:\\Windows\\Media\\Windows Ding".to_string(), "C:\\Windows\\Media\\Windows Ding.wav".to_string()]);
        // A drive path goes to the judge as typed.
        asked.borrow_mut().clear();
        assert!(resolve("D:\\sounds\\ok.wav", win, &alias, &accept).is_none());
        assert_eq!(*asked.borrow(), vec!["D:\\sounds\\ok.wav".to_string()]);
    }

    #[test]
    fn accept_file_wants_a_local_small_wav() {
        let dir = std::env::temp_dir().join(format!("tandem-sound-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wav = dir.join("ok.wav");
        std::fs::write(&wav, b"RIFF").unwrap();
        let txt = dir.join("no.txt");
        std::fs::write(&txt, b"RIFF").unwrap();
        let big = dir.join("big.wav");
        std::fs::write(&big, vec![0u8; MAX_SOUND_BYTES as usize + 1]).unwrap();
        assert!(accept_file(&wav).is_some());
        assert!(accept_file(&txt).is_none(), "not a .wav");
        assert!(accept_file(&big).is_none(), "over 5 MiB");
        assert!(accept_file(&dir).is_none(), "a folder");
        assert!(accept_file(&dir.join("missing.wav")).is_none());
        assert!(accept_file(Path::new("\\\\evil\\share\\a.wav")).is_none(), "a share");
        // The real resolve finds a bare name in a fake Media folder.
        let media = dir.join("Media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("Ding.wav"), b"RIFF").unwrap();
        assert!(matches!(resolve("Ding", &dir, &|_| false, &accept_file), Some(s) if s.file().is_some()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
