//! Password prompt detection and what hangs off it: the console input mode rule
//! (`PTYTask.m` passwordInput: echo off with line input on), iTerm2's debounce
//! (`iTermPasswordInputDebouncer.swift`), the Open Password Manager trigger's parameter and
//! pop-up (`PasswordTrigger.m`), the click on the cursor (`PTYMouseHandler.m:644-659`) and the
//! key cursor (`iTermTextDrawingHelper.m:4053-4068`).

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// `ENABLE_ECHO_INPUT` and `ENABLE_LINE_INPUT` of a console input buffer's mode (the
/// termios `ECHO` and `ICANON` of iTerm2's rule).
pub const ENABLE_ECHO_INPUT: u32 = 0x4;
pub const ENABLE_LINE_INPUT: u32 = 0x2;

/// The globals: detection on (`detectPasswordInput`, "Show key at cursor at password prompt?")
/// and how long echo must stay off first (`detectPasswordInputDebounce`).
pub const DETECT_KEY: &str = "DetectPasswordInput";
pub const DEBOUNCE_KEY: &str = "DetectPasswordInputDebounce";
pub const DEFAULT_DEBOUNCE: f64 = 0.2;

/// The profile setting that opens the manager at a password prompt.
pub const AUTO_OPEN_KEY: &str = "Open Password Manager Automatically";

/// The debounce the prompt samplers run with: the configured value, and ALWAYS something, so the
/// screen reader's privacy gate never depends on the "Show key at cursor" display setting
/// (`detect_on`): that setting is a display preference, not a privacy switch.
pub fn sampling_debounce(_detect_on: bool, configured: Option<f64>) -> Option<f64> {
    Some(configured.unwrap_or(DEFAULT_DEBOUNCE))
}

/// A sampled prompt state as (shown, secure): `shown` drives the key cursor and the manager
/// (the setting gates it), `secure` drives what the screen reader may read (nothing gates it).
pub fn states(raw: bool, detect_on: bool) -> (bool, bool) {
    (raw && detect_on, raw)
}

/// A console at a password prompt: echo off with line input on. Raw mode (line input off)
/// is not a prompt.
pub fn at_prompt(mode: u32) -> bool {
    mode & ENABLE_LINE_INPUT != 0 && mode & ENABLE_ECHO_INPUT == 0
}

/// A session's prompt state with the conductor's (`iTermPasswordInputDebouncer.swift:15-23`):
/// the conductor's own report is authoritative and counts at once, without the debounce.
pub fn with_conductor(local_debounced: bool, conductor: bool) -> bool {
    conductor || local_debounced
}

/// One console's samples, debounced: a run of samples that say "prompt" counts once a
/// sample at least the debounce after the run's first one still says so (so a script's brief
/// echo off is not a prompt, however the samples fall); any other sample ends it at once.
/// The watcher (local panes) and tandem-host (hosted panes) keep one per shell and feed it
/// each real reading, so a recheck never re-reads an old sample.
#[derive(Clone, Copy, Debug, Default)]
pub struct Debounce {
    since: Option<Instant>,
    pub on: bool,
}

impl Debounce {
    /// One sample taken at `at`: `prompt` is the console rule's answer (false when the mode
    /// could not be read). Some(new state) when it changed.
    pub fn sample(&mut self, prompt: bool, at: Instant, debounce: f64) -> Option<bool> {
        let on = if !prompt {
            self.since = None;
            false
        } else if debounce <= 0.0 {
            true
        } else {
            let since = *self.since.get_or_insert(at);
            at.saturating_duration_since(since) >= Duration::from_secs_f64(debounce)
        };
        (on != self.on).then(|| {
            self.on = on;
            on
        })
    }
}

/// Whether a change of a session's prompt state opens the manager by itself
/// (`didBeginPasswordInput`): only on the rising edge, only with the profile's "Open Password
/// Manager Automatically", and not while Instant Replay shows (a replaying session drops input).
pub fn opens_automatically(was: bool, now: bool, profile_on: bool, replaying: bool) -> bool {
    !was && now && profile_on && !replaying
}

/// The trigger's placeholder parameter: open without an account.
pub const UNLOCK: &str = "Open Password Manager to Unlock";

/// The account a trigger's parameter names: none for the placeholder or an empty one.
pub fn trigger_param(value: &str) -> Option<String> {
    (!value.is_empty() && value != UNLOCK).then(|| value.to_string())
}

/// The trigger's pop-up, in the order it shows (sorted case-insensitively): the cached
/// display strings, or, before any list was loaded, the current parameter and the
/// placeholder (`addUnlockToAccountNamesIfNeeded`).
pub fn trigger_choices(cached: &[String], param: &str) -> Vec<String> {
    let mut names: Vec<String> = if cached.iter().any(|n| n != UNLOCK) {
        cached.to_vec()
    } else {
        match trigger_param(param) {
            Some(p) => vec![p, UNLOCK.to_string()],
            None => vec![UNLOCK.to_string()],
        }
    };
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// The display strings of the last account list (`cachedCombinedAccountNames`), app-wide as
/// iTerm2's class property is. Display strings only: never a secret.
static CACHED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn set_cached_names(names: Vec<String>) {
    *CACHED.lock().unwrap_or_else(|e| e.into_inner()) = names;
}

pub fn cached_names() -> Vec<String> {
    CACHED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A plain click opens the manager: at a password prompt, on the cursor's cell, without Alt
/// (Option) or Win (Cmd). Cells are (column, row).
pub fn opens_on_click(at_prompt: bool, alt: bool, win: bool, click: (usize, usize), cursor: (usize, usize)) -> bool {
    at_prompt && !alt && !win && click == cursor
}

/// The key drawn in place of the cursor: Segoe Fluent Icons' Permissions glyph.
pub const KEY_GLYPH: char = '\u{E8D7}';

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_for_the_text_model_ignores_the_setting() {
        assert_eq!(sampling_debounce(false, None), Some(DEFAULT_DEBOUNCE), "sampling runs with the setting off");
        assert_eq!(sampling_debounce(false, Some(0.5)), Some(0.5));
        assert_eq!(sampling_debounce(true, Some(0.5)), Some(0.5));
        assert_eq!(states(true, false), (false, true), "off: no key cursor, still secure");
        assert_eq!(states(true, true), (true, true));
        assert_eq!(states(false, false), (false, false));
        assert_eq!(states(false, true), (false, false));
    }

    #[test]
    fn prompt_detection_follows_iterm2() {
        assert!(at_prompt(ENABLE_LINE_INPUT));
        assert!(!at_prompt(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT), "echo on is not a prompt");
        assert!(!at_prompt(0), "raw mode is not a prompt");
        assert!(!at_prompt(ENABLE_ECHO_INPUT));
        assert!(at_prompt(ENABLE_LINE_INPUT | 0x1 | 0x200), "other flags do not matter");

        let t0 = Instant::now();
        let s = |x: f64| t0 + Duration::from_secs_f64(x);
        let mut d = Debounce::default();
        assert_eq!(d.sample(true, t0, 0.2), None, "one sample at a prompt is not yet one");
        assert!(!d.on, "no later sample confirmed it, however long the UI waits");
        assert_eq!(d.sample(true, s(0.1), 0.2), None, "0.1 s after the first: not held long enough");
        assert_eq!(d.sample(true, s(0.2), 0.2), Some(true), "a later sample 0.2 s after the first confirms it");
        assert!(d.on);
        assert_eq!(d.sample(true, s(0.7), 0.2), None, "no change");
        assert_eq!(d.sample(false, s(1.2), 0.2), Some(false), "a sample off ends it at once");
        // The watcher's 0.5 s rate: a blip seen by one sample never turns on.
        let mut blip = Debounce::default();
        assert_eq!(blip.sample(false, t0, 0.2), None);
        assert_eq!(blip.sample(true, s(0.5), 0.2), None, "a brief echo off, sampled once");
        assert_eq!(blip.sample(false, s(1.0), 0.2), None, "gone at the next sample: never a prompt");
        assert_eq!(blip.sample(true, s(1.5), 0.2), None, "a new run starts over");
        assert_eq!(blip.sample(true, s(2.0), 0.2), Some(true), "two samples 0.5 s apart: a prompt");
        let mut zero = Debounce::default();
        assert_eq!(zero.sample(true, t0, 0.0), Some(true), "debounce 0 turns on at the first sample");
        let mut negative = Debounce::default();
        assert_eq!(negative.sample(true, t0, -1.0), Some(true), "a negative debounce is none");
        let mut unread = Debounce::default();
        for i in 0..4 {
            assert_eq!(unread.sample(false, s(i as f64), 0.2), None, "an unreadable console is never a prompt");
        }

        // Opening by itself: the rising edge only, with the profile setting, not while replaying.
        assert!(opens_automatically(false, true, true, false));
        assert!(!opens_automatically(true, true, true, false), "already on: no edge");
        assert!(!opens_automatically(true, false, true, false), "a falling edge");
        assert!(!opens_automatically(false, false, true, false));
        assert!(!opens_automatically(false, true, false, false), "the profile setting is off");
        assert!(!opens_automatically(false, true, true, true), "Instant Replay shows");

        assert_eq!(trigger_param(UNLOCK), None);
        assert_eq!(trigger_param(""), None);
        assert_eq!(trigger_param("github"), Some("github".into()));

        assert_eq!(trigger_choices(&[], ""), vec![UNLOCK.to_string()]);
        assert_eq!(trigger_choices(&[], UNLOCK), vec![UNLOCK.to_string()], "the placeholder is an empty parameter");
        // Shown sorted, as iTerm2's pop-up sorts its items (`objectsSortedByValueInDict:`).
        assert_eq!(trigger_choices(&[], "x"), vec![UNLOCK.to_string(), "x".to_string()]);
        assert_eq!(trigger_choices(&[], "abc"), vec!["abc".to_string(), UNLOCK.to_string()]);
        let cached: Vec<String> = ["zeta", "Alpha", "beta\u{2002}\u{2014}\u{2002}bob"].iter().map(|s| s.to_string()).collect();
        assert_eq!(trigger_choices(&cached, "x"), vec!["Alpha", "beta\u{2002}\u{2014}\u{2002}bob", "zeta"], "cached names only, sorted case-insensitively");
        assert_eq!(trigger_choices(&[UNLOCK.to_string()], "x"), vec![UNLOCK.to_string(), "x".to_string()], "a list of only the placeholder counts as none");

        assert!(opens_on_click(true, false, false, (3, 4), (3, 4)));
        assert!(!opens_on_click(true, true, false, (3, 4), (3, 4)), "Alt");
        assert!(!opens_on_click(true, false, true, (3, 4), (3, 4)), "Win");
        assert!(!opens_on_click(true, false, false, (4, 4), (3, 4)), "another cell");
        assert!(!opens_on_click(true, false, false, (3, 5), (3, 4)), "another row");
        assert!(!opens_on_click(false, false, false, (3, 4), (3, 4)), "off the prompt");
        assert_eq!(KEY_GLYPH as u32, 0xE8D7);
    }

    #[test]
    fn conductor_prompt_follows_iterm2() {
        // iTermPasswordInputDebouncer.swift:15-23: the conductor's prompt counts at once.
        assert!(with_conductor(false, true));
        assert!(with_conductor(true, false));
        assert!(with_conductor(true, true));
        assert!(!with_conductor(false, false));
    }
}
