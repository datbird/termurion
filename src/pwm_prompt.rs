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
pub const DETECT_KEY: &str = "detectPasswordInput";
pub const DEBOUNCE_KEY: &str = "detectPasswordInputDebounce";
pub const DEFAULT_DEBOUNCE: f64 = 0.2;

/// The profile setting that opens the manager at a password prompt.
pub const AUTO_OPEN_KEY: &str = "Open Password Manager Automatically";

/// A console at a password prompt: echo off with line input on. Raw mode (line input off)
/// is not a prompt.
pub fn at_prompt(mode: u32) -> bool {
    mode & ENABLE_LINE_INPUT != 0 && mode & ENABLE_ECHO_INPUT == 0
}

/// A session's latched password prompt state: a reading that says "prompt" counts once it
/// has held for the debounce (so a script's brief echo off is not a prompt); any other
/// reading ends it at once.
#[derive(Debug, Default)]
pub struct Debounce {
    since: Option<Instant>,
    pub on: bool,
}

impl Debounce {
    /// One reading. `detect` is the global switch, `raw` the console rule's answer (false
    /// when the mode could not be read). Some(new state) when it changed.
    pub fn update(&mut self, detect: bool, raw: bool, debounce: f64, now: Instant) -> Option<bool> {
        let now_on = if !detect || !raw {
            self.since = None;
            false
        } else if debounce <= 0.0 {
            true
        } else {
            let since = *self.since.get_or_insert(now);
            now.saturating_duration_since(since) >= Duration::from_secs_f64(debounce)
        };
        (now_on != self.on).then(|| {
            self.on = now_on;
            now_on
        })
    }

    /// When a held reading turns into a prompt, while one waits.
    pub fn recheck_at(&self, debounce: f64) -> Option<Instant> {
        match self.since {
            Some(t) if !self.on && debounce > 0.0 => Some(t + Duration::from_secs_f64(debounce)),
            _ => None,
        }
    }
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
    fn prompt_detection_follows_iterm2() {
        assert!(at_prompt(ENABLE_LINE_INPUT));
        assert!(!at_prompt(ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT), "echo on is not a prompt");
        assert!(!at_prompt(0), "raw mode is not a prompt");
        assert!(!at_prompt(ENABLE_ECHO_INPUT));
        assert!(at_prompt(ENABLE_LINE_INPUT | 0x1 | 0x200), "other flags do not matter");

        let t0 = Instant::now();
        let s = |x: f64| t0 + Duration::from_secs_f64(x);
        let mut d = Debounce::default();
        assert_eq!(d.update(true, true, 0.2, t0), None, "held, not yet a prompt");
        assert_eq!(d.recheck_at(0.2), Some(s(0.2)));
        assert_eq!(d.update(true, true, 0.2, s(0.1)), None);
        assert_eq!(d.update(true, true, 0.2, s(0.2)), Some(true), "a prompt once it held for the debounce");
        assert!(d.on);
        assert_eq!(d.recheck_at(0.2), None, "nothing waits once on");
        assert_eq!(d.update(true, true, 0.2, s(0.3)), None, "no change");
        assert_eq!(d.update(true, false, 0.2, s(0.35)), Some(false), "a reading off ends it at once");
        assert_eq!(d.recheck_at(0.2), None);
        // Starting over: the hold starts at the new reading.
        assert_eq!(d.update(true, true, 0.2, s(1.0)), None);
        assert_eq!(d.update(true, false, 0.2, s(1.1)), None, "a brief echo off never turned on");
        assert_eq!(d.update(true, true, 0.2, s(1.15)), None);
        assert_eq!(d.update(true, true, 0.2, s(1.3)), None, "the hold restarted at 1.15");
        assert_eq!(d.update(true, true, 0.2, s(1.35)), Some(true));

        let mut off = Debounce::default();
        for i in 0..5 {
            assert_eq!(off.update(false, true, 0.2, s(i as f64)), None, "detection off never turns on");
        }
        assert_eq!(off.recheck_at(0.2), None);
        let mut was_on = Debounce::default();
        assert_eq!(was_on.update(true, true, 0.0, t0), Some(true), "debounce 0 turns on at once");
        assert_eq!(was_on.update(false, true, 0.0, t0), Some(false), "turning detection off ends it");
        let mut negative = Debounce::default();
        assert_eq!(negative.update(true, true, -1.0, t0), Some(true), "a negative debounce is none");

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
}
