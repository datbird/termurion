//! Session hotkeys (iTerm2's `iTermSessionHotkeyController` and `iTermShortcut` short strings): one
//! system-wide registration per chord with the sessions that use it, which session a press picks,
//! the chord rules a session hotkey must pass, and the short string tmux's `@hotkeys` keeps.
//!
//! A session hotkey is system-wide, so the program that set it is never trusted with it: the
//! rules here (a modifier, nothing Windows keeps for itself, no clash, a cap) bind every chord,
//! and the App registers only what the person set or allowed.

use crate::hotkey::{self, Chord, Shortcut};
use crate::keystroke::{COMMAND, CONTROL, OPTION};
use crate::layout::PaneId;

/// The first hotkey id of a session hotkey group; profile hotkeys count from 1 and stay below it.
pub const SESSION_HOTKEY_BASE: u32 = 0x1000;

/// At most this many session chords are registered at once.
pub const MAX_CHORDS: usize = 16;

/// A hotkey id of a session hotkey group (the app and Open Quickly ids are far from it).
pub fn is_session_id(id: u32) -> bool {
    (SESSION_HOTKEY_BASE..SESSION_HOTKEY_BASE + MAX_CHORDS as u32).contains(&id)
}

/// The key that `HotkeyUi::errors` holds a session's registration error under.
pub fn error_key(pane: PaneId) -> String {
    format!("session:{pane}")
}

/// The error keys of the sessions a refused session hotkey id covers (`ids`: the sessions of each
/// registered group, in id order); none for an id that is not a session's.
pub fn error_keys(id: u32, ids: &[Vec<PaneId>]) -> Vec<String> {
    if !is_session_id(id) {
        return Vec::new();
    }
    ids.get((id - SESSION_HOTKEY_BASE) as usize).map(|panes| panes.iter().map(|p| error_key(*p)).collect()).unwrap_or_default()
}

/// One registration per chord, its sessions in creation order (`sessions` is in that order).
pub fn groups(sessions: &[(PaneId, Shortcut)]) -> Vec<(Chord, Vec<PaneId>)> {
    let mut out: Vec<(Chord, Vec<PaneId>)> = Vec::new();
    for (pane, s) in sessions {
        let Some(c) = hotkey::chord(s) else { continue };
        match out.iter_mut().find(|(k, _)| *k == c) {
            Some((_, panes)) => panes.push(*pane),
            None => out.push((c, vec![*pane])),
        }
    }
    out
}

/// The session a press reveals (`iTermSessionHotkeyRegistration hotkeyPressed:siblings:`): the one
/// after the first responder (wrapping), else one that is active in a window that is not the key
/// window, else the first.
pub fn pick(group: &[PaneId], first_responder: Option<PaneId>, active_in_background: &[PaneId]) -> Option<PaneId> {
    if let Some(i) = first_responder.and_then(|f| group.iter().position(|p| *p == f)) {
        return Some(group[(i + 1) % group.len()]);
    }
    group.iter().find(|p| active_in_background.contains(p)).or(group.first()).copied()
}

/// What the windows say about their sessions, for `pick`: each window as (it is the key window,
/// the session active in its selected tab). The first responder is the key window's active
/// session (iTerm2 also needs the app active, which the caller gives by marking no window key);
/// the others' active sessions are the background ones.
pub fn focus_state(windows: &[(bool, Option<PaneId>)]) -> (Option<PaneId>, Vec<PaneId>) {
    let first = windows.iter().find(|(key, _)| *key).and_then(|(_, p)| *p);
    let background = windows.iter().filter(|(key, _)| !*key).filter_map(|(_, p)| *p).collect();
    (first, background)
}

/// Why a session chord is not registered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No Control, Alt or Win: a bare or Shift-only key would steal typing system-wide.
    NoModifier,
    /// A Win chord the Windows shell keeps for itself.
    ShellOwned,
    /// The app hotkey, Open Quickly's or a profile hotkey uses it.
    Taken,
    /// More than `MAX_CHORDS` chords.
    TooMany,
    /// The key has no Windows chord.
    NoKey,
}

impl Refusal {
    /// The warning shown beside the Session Hotkey row.
    pub fn text(self) -> &'static str {
        match self {
            Refusal::NoModifier => "A session hotkey needs Ctrl, Alt or Win",
            Refusal::ShellOwned => "Windows keeps this shortcut for itself",
            Refusal::Taken => "Another hotkey uses this shortcut",
            Refusal::TooMany => "At most 16 session hotkeys can be on at once",
            Refusal::NoKey => "This key cannot be a hotkey",
        }
    }
}

/// The chord rules for one session shortcut: a modifier, not the shell's, not in `reserved`.
pub fn check(s: &Shortcut, reserved: &[Chord]) -> Result<Chord, Refusal> {
    if s.flags & (CONTROL | OPTION | COMMAND) == 0 {
        return Err(Refusal::NoModifier);
    }
    let c = hotkey::chord(s).ok_or(Refusal::NoKey)?;
    if crate::winkeys::shell_owned(c) {
        return Err(Refusal::ShellOwned);
    }
    if reserved.contains(&c) {
        return Err(Refusal::Taken);
    }
    Ok(c)
}

/// What registers: the groups of the sessions whose chords pass `check`, at most `MAX_CHORDS`
/// of them, and each refused session with its reason.
pub struct Plan {
    pub groups: Vec<(Chord, Vec<PaneId>)>,
    pub refused: Vec<(PaneId, Refusal)>,
}

pub fn plan(sessions: &[(PaneId, Shortcut)], reserved: &[Chord]) -> Plan {
    let mut ok = Vec::new();
    let mut refused = Vec::new();
    for (pane, s) in sessions {
        match check(s, reserved) {
            Ok(_) => ok.push((*pane, s.clone())),
            Err(r) => refused.push((*pane, r)),
        }
    }
    let mut groups = groups(&ok);
    for (_, panes) in groups.drain(MAX_CHORDS.min(groups.len())..) {
        refused.extend(panes.into_iter().map(|p| (p, Refusal::TooMany)));
    }
    Plan { groups, refused }
}

/// What a hotkey press does after its ids are split: the session ids are routed first (they
/// bring another window forward), and only the others take part in the "return to the window
/// before" tail of `hotkey_pressed`.
pub struct Route {
    pub sessions: Vec<usize>,
    pub others: Vec<u32>,
}

pub fn route(ids: &[u32]) -> Route {
    let mut r = Route { sessions: Vec::new(), others: Vec::new() };
    for id in ids {
        if is_session_id(*id) {
            r.sessions.push((*id - SESSION_HOTKEY_BASE) as usize);
        } else {
            r.others.push(*id);
        }
    }
    r
}

impl Route {
    /// The tail that returns to the window before runs only for a press that had a hotkey window
    /// to show or hide: a session press that brought another window forward must stay there.
    pub fn tail_applies(&self) -> bool {
        !self.others.is_empty()
    }
}

/// Where a session hotkey that the person did not set came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A script, the API, an escape sequence, an arrangement or an import.
    Program,
    /// The tmux session's `@hotkeys`: the server's host (its gateway's client name) and the session name.
    Tmux { host: String, name: String },
}

/// How a chord reads ("Ctrl+Alt+1"): its modifiers, then its key (the key at that scan code's place on
/// a U.S. keyboard). The one place a chord is named for the person, so the consent question can only
/// show what would register.
pub fn chord_label(c: &Chord) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (bit, name) in [(hotkey::MOD_WIN, "Win"), (hotkey::MOD_CONTROL, "Ctrl"), (hotkey::MOD_ALT, "Alt"), (hotkey::MOD_SHIFT, "Shift")] {
        if c.mods & bit != 0 {
            parts.push(name.into());
        }
    }
    parts.push(match c.key {
        hotkey::ChordKey::Char(ch) => ch.to_uppercase().to_string(),
        hotkey::ChordKey::Scan(scan) => scan_name(scan),
    });
    parts.join("+")
}

/// The name of the key at a scan code's place on a U.S. keyboard.
fn scan_name(scan: u32) -> String {
    let row = |first: u32, keys: &str| keys.chars().nth(scan.wrapping_sub(first) as usize).filter(|_| scan >= first).map(|c| c.to_string());
    let fixed = match scan {
        0x01 => "Esc",
        0x0E => "Backspace",
        0x0F => "Tab",
        0x1C => "Enter",
        0x39 => "Space",
        0x57 => "F11",
        0x58 => "F12",
        0xE047 => "Home",
        0xE048 => "Up",
        0xE049 => "PgUp",
        0xE04B => "Left",
        0xE04D => "Right",
        0xE04F => "End",
        0xE050 => "Down",
        0xE051 => "PgDn",
        0xE052 => "Insert",
        0xE053 => "Delete",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.into();
    }
    if (0x3B..=0x44).contains(&scan) {
        return format!("F{}", scan - 0x3A);
    }
    let named = match scan {
        0x02..=0x0B => row(0x02, "1234567890"),
        0x0C => Some("-".into()),
        0x0D => Some("=".into()),
        0x10..=0x19 => row(0x10, "QWERTYUIOP"),
        0x1A => Some("[".into()),
        0x1B => Some("]".into()),
        0x1E..=0x26 => row(0x1E, "ASDFGHJKL"),
        0x27 => Some(";".into()),
        0x28 => Some("'".into()),
        0x29 => Some("`".into()),
        0x2B => Some("\\".into()),
        0x2C..=0x32 => row(0x2C, "ZXCVBNM"),
        0x33 => Some(",".into()),
        0x34 => Some(".".into()),
        0x35 => Some("/".into()),
        _ => None,
    };
    named.unwrap_or_else(|| format!("Key 0x{scan:X}"))
}

/// The chord a shortcut registers as and how it reads, from the same `hotkey::chord` that registers
/// it (the keycode wins over the characters, and flags beyond the four modifiers are ignored).
pub fn chord_and_label(s: &Shortcut) -> Option<(Chord, String)> {
    let c = hotkey::chord(s)?;
    let label = chord_label(&c);
    Some((c, label))
}

/// The question text of a program-supplied session hotkey (`label` is the chord, "Ctrl+Alt+1").
pub fn consent_text(origin: &Origin, label: &str) -> String {
    let who = match origin {
        Origin::Program => "A program in a session".to_string(),
        Origin::Tmux { name, .. } => format!("tmux session \u{201c}{name}\u{201d}"),
    };
    format!("{who} asks to register {label} as a hotkey that works in every app.")
}

/// What a program's write did, for the app: the question to ask (None when it needs none) and the
/// question of this pane that the write replaced (its still-queued alert is dropped).
#[derive(Debug, Default, PartialEq)]
pub struct Written {
    pub ask: Option<(PaneId, Shortcut, Origin)>,
    pub stale: Option<Shortcut>,
}

/// The most tmux decisions the book keeps (a stranger's server names stay bounded).
const MAX_DECIDED: usize = 64;

/// The session hotkeys that are on (`live`: the person set or allowed them) and those that wait
/// for the person (`pending`). Only `live` ever registers; a value that a program wrote is
/// stored with its session but sits in `pending` until it is allowed.
///
/// A program cannot flood the person with questions: a session has at most one question out, a
/// session whose question was answered Ignore is not asked again until it ends, a program writing
/// the value that is already live or waiting changes nothing, and a tmux session's answer for a
/// value stands for every pane that tmux session brings with the same value.
#[derive(Default)]
pub struct Book {
    live: Vec<(PaneId, Shortcut)>,
    pending: Vec<(PaneId, Shortcut, Origin)>,
    /// Sessions that said Ignore: their program writes are refused silently until they end.
    ignored: Vec<PaneId>,
    /// A tmux session's answer ((host, name), value, allowed), for the next pane with the same value.
    decided: Vec<((String, String), Shortcut, bool)>,
}

impl Book {
    pub fn live(&self) -> &[(PaneId, Shortcut)] {
        &self.live
    }

    pub fn pending(&self) -> &[(PaneId, Shortcut, Origin)] {
        &self.pending
    }

    /// The value is on, waiting for an answer, or its session said Ignore: nothing more to decide.
    pub fn knows(&self, pane: PaneId, s: &Shortcut) -> bool {
        self.live.iter().any(|(p, x)| *p == pane && x == s) || self.pending.iter().any(|(p, x, _)| *p == pane && x == s) || self.ignored.contains(&pane)
    }

    /// A session that a program made (an arrangement, the API, an undone close) already carries a
    /// value: it asks like any program's write, once, unless the book already knows the value.
    pub fn adopt(&mut self, pane: PaneId, s: Option<Shortcut>) -> Written {
        match s {
            Some(s) if !self.knows(pane, &s) => self.program_set(pane, Some(s), Origin::Program),
            _ => Written::default(),
        }
    }

    /// A value is stored for the pane (`has_value`) but is neither live nor waiting for an answer:
    /// the person said Ignore, or a tmux decision refused it. The row shows it as not registered.
    pub fn refused(&self, pane: PaneId, has_value: bool) -> bool {
        has_value && !self.live.iter().any(|(p, _)| *p == pane) && !self.waiting(pane)
    }

    /// The pane's value waits for the person (it shows as not registered, never as live).
    pub fn waiting(&self, pane: PaneId) -> bool {
        self.pending.iter().any(|(p, _, _)| *p == pane)
    }

    /// The person set (Some) or cleared (None) the pane's hotkey.
    pub fn person_set(&mut self, pane: PaneId, s: Option<Shortcut>) {
        self.drop_values(pane);
        if let Some(s) = s {
            self.live.push((pane, s));
            self.live.sort_by_key(|(p, _)| *p);
        }
    }

    /// A program wrote the pane's hotkey (or cleared it): the old one is off, the new one waits.
    pub fn program_set(&mut self, pane: PaneId, s: Option<Shortcut>, origin: Origin) -> Written {
        let Some(s) = s else {
            self.drop_values(pane);
            return Written::default();
        };
        // The value that is already live or waiting: nothing changes, nothing is asked.
        if self.live.iter().any(|(p, x)| *p == pane && *x == s) || self.pending.iter().any(|(p, x, _)| *p == pane && *x == s) {
            return Written::default();
        }
        let stale = self.pending.iter().find(|(p, _, _)| *p == pane).map(|(_, x, _)| x.clone());
        self.drop_values(pane);
        if self.ignored.contains(&pane) {
            return Written { ask: None, stale };
        }
        // A tmux session answered for this value before: the same answer.
        if let Origin::Tmux { host, name } = &origin {
            let key = (host.clone(), name.clone());
            if let Some((_, _, allowed)) = self.decided.iter().find(|(n, x, _)| *n == key && *x == s) {
                if *allowed {
                    self.live.push((pane, s));
                    self.live.sort_by_key(|(p, _)| *p);
                } else {
                    self.ignored.push(pane);
                }
                return Written { ask: None, stale };
            }
        }
        self.pending.push((pane, s.clone(), origin.clone()));
        Written { ask: Some((pane, s, origin)), stale }
    }

    fn decide(&mut self, origin: &Origin, s: &Shortcut, allowed: bool) {
        if let Origin::Tmux { host, name } = origin {
            let key = (host.clone(), name.clone());
            self.decided.retain(|(n, x, _)| !(*n == key && x == s));
            self.decided.push((key, s.clone(), allowed));
            if self.decided.len() > MAX_DECIDED {
                self.decided.remove(0);
            }
        }
    }

    /// The person allowed a waiting hotkey. False when it is no longer waiting.
    pub fn allow(&mut self, pane: PaneId, s: &Shortcut) -> bool {
        let Some(i) = self.pending.iter().position(|(p, x, _)| *p == pane && x == s) else { return false };
        let (_, _, origin) = self.pending.remove(i);
        self.decide(&origin, s, true);
        self.live.push((pane, s.clone()));
        self.live.sort_by_key(|(p, _)| *p);
        true
    }

    /// The person ignored a waiting hotkey: it stays stored, inert, and its session is not asked
    /// again. (The answer to a question that a newer one replaced changes nothing.)
    pub fn ignore(&mut self, pane: PaneId, s: &Shortcut) {
        let Some(i) = self.pending.iter().position(|(p, x, _)| *p == pane && x == s) else { return };
        let (_, _, origin) = self.pending.remove(i);
        self.decide(&origin, s, false);
        if !self.ignored.contains(&pane) {
            self.ignored.push(pane);
        }
    }

    fn drop_values(&mut self, pane: PaneId) {
        self.live.retain(|(p, _)| *p != pane);
        self.pending.retain(|(p, _, _)| *p != pane);
    }

    /// The session ended: nothing of it stays, and a new session is asked afresh.
    pub fn pane_gone(&mut self, pane: PaneId) {
        self.drop_values(pane);
        self.ignored.retain(|p| *p != pane);
    }
}

/// The most characters a short string's text may hold (bytes), so a stranger's value stays small.
pub const MAX_CHARS_BYTES: usize = 64;

/// `iTermShortcut shortStringForDictionary:`: `keyCode,modifiers,characters,charactersIgnoringModifiers`
/// with backslash, comma and space escaped as `\0`, `\1`, `\2`.
pub fn short_string(s: &Shortcut) -> String {
    format!("{},{},{},{}", s.keycode, s.flags, escape(&s.chars), escape(&s.chars_ignoring))
}

fn escape(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\0"),
            ',' => out.push_str("\\1"),
            ' ' => out.push_str("\\2"),
            c => out.push(c),
        }
    }
    out
}

/// `dictionaryForShortString:`. None with fewer than four parts, or text longer than
/// `MAX_CHARS_BYTES`; a number that does not parse is 0 (iTerm2's `iterm_unsignedIntegerValue`).
pub fn from_short_string(t: &str) -> Option<Shortcut> {
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut esc = false;
    for c in t.chars() {
        if esc {
            if let Some(i) = c.to_digit(10).filter(|i| *i < 3) {
                cur.push(['\\', ',', ' '][i as usize]);
            }
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ',' {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    parts.push(cur);
    if parts.len() < 4 || parts[2].len() > MAX_CHARS_BYTES || parts[3].len() > MAX_CHARS_BYTES {
        return None;
    }
    let number = |s: &str| s.trim().parse::<u64>().unwrap_or(0);
    Some(Shortcut { keycode: number(&parts[0]).min(u16::MAX as u64) as u16, flags: number(&parts[1]), chars: parts[2].clone(), chars_ignoring: parts[3].clone() })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(code: u16, flags: u64, text: &str) -> Shortcut {
        Shortcut { keycode: code, flags, chars: text.into(), chars_ignoring: text.into() }
    }

    const CTRL_ALT: u64 = CONTROL | OPTION;

    #[test]
    fn session_hotkey_cycle_follows_iterm2() {
        let one = sc(18, CTRL_ALT, "1");
        let two = sc(19, CTRL_ALT, "2");
        let (a, b, c) = (1, 2, 3);
        let g = groups(&[(a, one.clone()), (b, one.clone()), (c, two)]);
        assert_eq!(g.len(), 2, "one registration per chord");
        assert_eq!(g[0].1, vec![a, b], "sessions in creation order");
        assert_eq!(g[1].1, vec![c]);
        assert_eq!(pick(&[a, b], Some(a), &[]), Some(b), "the one after the first responder");
        assert_eq!(pick(&[a, b], Some(b), &[]), Some(a), "wrapping");
        assert_eq!(pick(&[a, b], None, &[b]), Some(b), "active in a window that is not key");
        assert_eq!(pick(&[a, b], None, &[]), Some(a), "else the first");
        assert_eq!(pick(&[], None, &[]), None);
        // The key window's active session is the first responder; the others are background.
        assert_eq!(focus_state(&[(false, Some(a)), (true, Some(b))]), (Some(b), vec![a]));
        assert_eq!(focus_state(&[(false, Some(a)), (false, Some(b))]), (None, vec![a, b]), "Tandem not in front");
    }

    #[test]
    fn hotkeys_option_round_trips() {
        let s = sc(18, COMMAND, "1");
        assert_eq!(short_string(&s), "18,1048576,1,1");
        assert_eq!(from_short_string("18,1048576,1,1"), Some(s));
        let comma = Shortcut { keycode: 43, flags: CONTROL, chars: "a,b".into(), chars_ignoring: "a b\\".into() };
        assert_eq!(short_string(&comma), "43,262144,a\\1b,a\\2b\\0");
        assert_eq!(from_short_string(&short_string(&comma)), Some(comma));
        assert_eq!(from_short_string("18,1048576,1"), None, "fewer than four parts");
        assert_eq!(from_short_string(&format!("1,2,{},x", "a".repeat(65))), None, "text is capped");
    }

    #[test]
    fn session_hotkey_chord_rules() {
        let ok = sc(18, CTRL_ALT, "1");
        let c = check(&ok, &[]).expect("Ctrl+Alt+1");
        assert_eq!(check(&sc(18, 0, "1"), &[]), Err(Refusal::NoModifier), "a bare key");
        assert_eq!(check(&sc(18, crate::keystroke::SHIFT, "!"), &[]), Err(Refusal::NoModifier), "Shift alone");
        // Win+E is the shell's.
        assert_eq!(check(&sc(14, COMMAND, "e"), &[]), Err(Refusal::ShellOwned));
        assert_eq!(check(&ok, &[c]), Err(Refusal::Taken), "the app hotkey's chord");
        // The cap: 17 distinct chords register 16.
        let many: Vec<(PaneId, Shortcut)> = (0..17u64).map(|i| (i + 1, sc(0x7A + 0, CTRL_ALT, "a"))).collect();
        assert_eq!(plan(&many, &[]).groups.len(), 1, "one chord, however many sessions");
        let codes = [0x7Au16, 0x78, 0x63, 0x76, 0x60, 0x61, 0x62, 0x64, 0x65, 0x6D, 0x67, 0x6F, 18, 19, 20, 21, 23];
        let distinct: Vec<(PaneId, Shortcut)> = codes.iter().enumerate().map(|(i, k)| (i as u64 + 1, sc(*k, CTRL_ALT, "x"))).collect();
        let p = plan(&distinct, &[]);
        assert_eq!(p.groups.len(), MAX_CHORDS);
        assert_eq!(p.refused, vec![(17, Refusal::TooMany)]);
    }

    #[test]
    fn session_hotkey_error_names_the_session() {
        let ids = vec![vec![1, 2], vec![3]];
        assert_eq!(error_keys(SESSION_HOTKEY_BASE + 1, &ids), vec!["session:3".to_string()]);
        assert_eq!(error_keys(SESSION_HOTKEY_BASE, &ids), vec!["session:1".to_string(), "session:2".to_string()]);
        assert!(error_keys(SESSION_HOTKEY_BASE + 5, &ids).is_empty());
        assert!(error_keys(2, &ids).is_empty(), "a profile id is not a session's");
    }

    #[test]
    fn session_hotkey_press_does_not_return_to_the_previous_window() {
        let r = route(&[SESSION_HOTKEY_BASE + 1]);
        assert_eq!((r.sessions.clone(), r.others.clone()), (vec![1], vec![]));
        assert!(!r.tail_applies(), "a session press stays in the window it revealed");
        let r = route(&[1, SESSION_HOTKEY_BASE]);
        assert!(r.tail_applies());
        assert!(route(&[0, u32::MAX]).tail_applies(), "the app and Open Quickly ids are not session ids");
        assert!(!is_session_id(SESSION_HOTKEY_BASE - 1));
    }

    #[test]
    fn consent_shows_the_chord_that_registers() {
        // The characters say Q, the keycode (18) is the key 1: Ctrl+Alt+1 is what registers.
        let tricky = Shortcut { keycode: 18, flags: CTRL_ALT | crate::keystroke::LEADER | crate::keystroke::NUMPAD, chars: "q".into(), chars_ignoring: "q".into() };
        let (chord, label) = chord_and_label(&tricky).unwrap();
        assert_eq!(label, "Ctrl+Alt+1");
        assert_eq!(chord, check(&tricky, &[]).unwrap(), "the label names the chord `check` and registration use");
        // No keycode of its own: the character is the key.
        let by_char = Shortcut { keycode: 0xFFFF, flags: COMMAND | crate::keystroke::SHIFT, chars: "x".into(), chars_ignoring: "x".into() };
        assert_eq!(chord_and_label(&by_char).unwrap().1, "Win+Shift+X");
        for (code, flags, want) in [(0x7Au16, CONTROL, "Ctrl+F1"), (0x31, OPTION, "Alt+Space"), (0x7B, COMMAND, "Win+Left"), (0x24, CTRL_ALT, "Ctrl+Alt+Enter"), (0x00, CONTROL, "Ctrl+A")] {
            let s = Shortcut { keycode: code, flags, chars: "z".into(), chars_ignoring: "z".into() };
            assert_eq!(chord_and_label(&s).unwrap().1, want);
        }
    }

    #[test]
    fn program_prompts_are_bounded() {
        let a = sc(18, CTRL_ALT, "1");
        let mut b = Book::default();
        // One question per session: a second write replaces the first.
        assert!(b.program_set(1, Some(a.clone()), Origin::Program).ask.is_some());
        let w = b.program_set(1, Some(sc(19, CTRL_ALT, "2")), Origin::Program);
        assert!(w.ask.is_some() && w.stale == Some(a.clone()), "the replaced question is named so its alert is dropped");
        assert_eq!(b.pending().len(), 1);
        // The same value again: nothing changes, nothing is asked.
        assert_eq!(b.program_set(1, Some(sc(19, CTRL_ALT, "2")), Origin::Program), Written::default());
        assert_eq!(b.pending().len(), 1);
        // Ignore: this session is never asked again, however many chords the program tries.
        b.ignore(1, &sc(19, CTRL_ALT, "2"));
        for code in 0..40u16 {
            let w = b.program_set(1, Some(sc(code, CTRL_ALT, "x")), Origin::Program);
            assert!(w.ask.is_none(), "chord {code}");
        }
        assert!(b.pending().is_empty() && b.live().is_empty());
        // Another session is asked as usual; the session ending clears its refusal.
        assert!(b.program_set(2, Some(a.clone()), Origin::Program).ask.is_some());
        b.pane_gone(1);
        assert!(b.program_set(1, Some(a.clone()), Origin::Program).ask.is_some());
        // The answer to a question that a newer one replaced does not silence the session.
        let mut b = Book::default();
        b.program_set(5, Some(a.clone()), Origin::Program);
        b.program_set(5, Some(sc(19, CTRL_ALT, "2")), Origin::Program);
        b.ignore(5, &a);
        assert!(b.program_set(5, Some(sc(20, CTRL_ALT, "3")), Origin::Program).ask.is_some(), "a stale Ignore changes nothing");
        // A live hotkey is not turned off by the same value written again.
        let mut b = Book::default();
        b.person_set(7, Some(a.clone()));
        assert_eq!(b.program_set(7, Some(a.clone()), Origin::Program), Written::default());
        assert_eq!(b.live(), &[(7, a.clone())], "still live");
    }

    #[test]
    fn tmux_answers_stand_for_the_same_session_and_value() {
        let a = sc(18, CTRL_ALT, "1");
        let tmux = |n: &str| Origin::Tmux { host: "me@box".into(), name: n.into() };
        let mut b = Book::default();
        assert!(b.program_set(1, Some(a.clone()), tmux("work")).ask.is_some());
        assert!(b.allow(1, &a));
        // Attached again (new pane ids), the same tmux session and value: allowed without a question.
        b.pane_gone(1);
        assert!(b.program_set(9, Some(a.clone()), tmux("work")).ask.is_none());
        assert_eq!(b.live(), &[(9, a.clone())]);
        // Another tmux session, or another value, is asked.
        assert!(b.program_set(10, Some(a.clone()), tmux("other")).ask.is_some());
        assert!(b.program_set(11, Some(sc(19, CTRL_ALT, "2")), tmux("work")).ask.is_some());
        // An Ignore stands too, and stays inert.
        let mut b = Book::default();
        b.program_set(1, Some(a.clone()), tmux("work"));
        b.ignore(1, &a);
        b.pane_gone(1);
        assert!(b.program_set(2, Some(a.clone()), tmux("work")).ask.is_none());
        assert!(b.live().is_empty() && b.pending().is_empty());
        assert!(b.knows(2, &a), "a refused session is known: nothing more to decide");
        assert!(!b.waiting(2));
    }

    #[test]
    fn a_stored_value_that_is_not_live_is_refused_or_waiting() {
        let a = sc(18, CTRL_ALT, "1");
        let mut b = Book::default();
        // Nothing stored: not refused. Waiting: not refused (the row says waiting). Live: not refused.
        assert!(!b.refused(1, false));
        b.program_set(1, Some(a.clone()), Origin::Program);
        assert!(!b.refused(1, true));
        b.allow(1, &a);
        assert!(!b.refused(1, true));
        // Ignore: stored and inert, so refused. With no stored value there is nothing to refuse.
        b.program_set(2, Some(a.clone()), Origin::Program);
        b.ignore(2, &a);
        assert!(b.refused(2, true) && !b.refused(2, false));
        // A tmux decision that refused the value: the same.
        let t = Origin::Tmux { host: "me@box".into(), name: "h".into() };
        b.program_set(3, Some(a.clone()), t.clone());
        b.ignore(3, &a);
        b.pane_gone(3);
        assert!(b.program_set(4, Some(a.clone()), t).ask.is_none());
        assert!(b.refused(4, true));
        // The person setting a value makes it live again.
        b.person_set(2, Some(a.clone()));
        assert!(!b.refused(2, true));
    }

    #[test]
    fn tmux_answers_do_not_cross_hosts() {
        // tmux's default session names ("0", "1") repeat across servers: an answer is for one host.
        let a = sc(18, CTRL_ALT, "1");
        let on = |h: &str| Origin::Tmux { host: h.into(), name: "0".into() };
        let mut b = Book::default();
        assert!(b.program_set(1, Some(a.clone()), on("me@local")).ask.is_some());
        assert!(b.allow(1, &a));
        b.pane_gone(1);
        assert!(b.program_set(2, Some(a.clone()), on("me@local")).ask.is_none(), "the same host and session stands");
        assert!(b.program_set(3, Some(a.clone()), on("root@remote")).ask.is_some(), "another host with the same session name is asked");
        assert_eq!(b.live(), &[(2, a.clone())]);
    }

    #[test]
    fn restored_and_api_sessions_ask_once() {
        let a = sc(18, CTRL_ALT, "1");
        let mut b = Book::default();
        // A restored arrangement's or API session's value: asked, not live.
        let w = b.adopt(3, Some(a.clone()));
        assert_eq!(w.ask, Some((3, a.clone(), Origin::Program)));
        assert!(b.live().is_empty());
        assert!(b.waiting(3), "the Settings row shows it as waiting, never live");
        // Seen again (another pass): nothing more to ask.
        assert!(b.adopt(3, Some(a.clone())).ask.is_none());
        // Register makes it live; the row no longer waits.
        assert!(b.allow(3, &a));
        assert!(!b.waiting(3) && b.live() == [(3, a.clone())]);
        assert!(b.adopt(3, Some(a.clone())).ask.is_none());
        // Ignore leaves it inert and silent.
        b.adopt(4, Some(a.clone()));
        b.ignore(4, &a);
        assert!(!b.waiting(4) && b.adopt(4, Some(a.clone())).ask.is_none() && b.live().len() == 1);
        // No value, nothing to ask.
        assert!(b.adopt(5, None).ask.is_none());
    }

    #[test]
    fn program_supplied_hotkey_needs_consent() {
        let s = sc(18, CTRL_ALT, "1");
        let mut b = Book::default();
        let ask = b.program_set(3, Some(s.clone()), Origin::Tmux { host: "me@box".into(), name: "x".into() }).ask;
        assert!(ask.is_some(), "the person is asked");
        assert!(b.live().is_empty(), "nothing registers before the answer");
        assert_eq!(consent_text(&Origin::Tmux { host: "me@box".into(), name: "x".into() }, "Ctrl+Alt+1"), "tmux session \u{201c}x\u{201d} asks to register Ctrl+Alt+1 as a hotkey that works in every app.");
        assert!(b.allow(3, &s));
        assert_eq!(b.live(), &[(3, s.clone())]);
        // A program rewriting it turns it off until asked again.
        b.program_set(3, Some(sc(19, CTRL_ALT, "2")), Origin::Program);
        assert!(b.live().is_empty());
        b.ignore(3, &sc(19, CTRL_ALT, "2"));
        assert!(b.pending().is_empty() && b.live().is_empty(), "ignored stays inert");
        // The person's own set registers at once, and the session ending drops it.
        b.person_set(4, Some(s.clone()));
        b.person_set(2, Some(s));
        assert_eq!(b.live().iter().map(|(p, _)| *p).collect::<Vec<_>>(), vec![2, 4], "creation order");
        b.pane_gone(2);
        assert_eq!(b.live().len(), 1);
        b.person_set(4, None);
        assert!(b.live().is_empty());
    }
}
