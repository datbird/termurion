//! System-wide hotkeys on macOS, through the global-hotkey crate (Carbon hot keys). One manager,
//! made on the main thread; a press maps back to the ids the app registered the chord under.
//! Double-tap hotkeys need an event tap and the Accessibility permission, so they are refused by name.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};

use crate::hotkey::{Chord, ChordKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};

type Sink = Box<dyn Fn(Vec<u32>) + Send + Sync>;
type Results = Vec<(u32, Result<(), String>)>;

const TAPS_REFUSED: &str = "Double-tap hotkeys are not available on macOS yet";
const TAKEN: &str = "Another app already uses this shortcut";

struct State {
    manager: GlobalHotKeyManager,
    keys: Vec<HotKey>,
    last: Vec<(u32, Chord)>,
    /// The latest answer per id, for a request that comes while the recorder holds the chords.
    answers: HashMap<u32, Result<(), String>>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// Hotkey id (the crate's hash of the chord) to the app's ids for it.
static IDS: Mutex<Option<HashMap<u32, Vec<u32>>>> = Mutex::new(None);
static SINK: Mutex<Option<Sink>> = Mutex::new(None);
/// Set while the shortcut recorder listens: nothing is registered then, so it gets the keys.
static PAUSED: Mutex<bool> = Mutex::new(false);

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The W3C `Code` of a PC scan code (the key's position), None for one with no such key.
pub fn code_of(key: ChordKey) -> Option<Code> {
    match key {
        ChordKey::Scan(s) => scan_code(s),
        ChordKey::Char(c) => char_code(c),
    }
}

fn scan_code(s: u32) -> Option<Code> {
    use Code::*;
    Some(match s {
        0x01 => Escape,
        0x02 => Digit1, 0x03 => Digit2, 0x04 => Digit3, 0x05 => Digit4, 0x06 => Digit5,
        0x07 => Digit6, 0x08 => Digit7, 0x09 => Digit8, 0x0A => Digit9, 0x0B => Digit0,
        0x0C => Minus, 0x0D => Equal, 0x0E => Backspace, 0x0F => Tab,
        0x10 => KeyQ, 0x11 => KeyW, 0x12 => KeyE, 0x13 => KeyR, 0x14 => KeyT,
        0x15 => KeyY, 0x16 => KeyU, 0x17 => KeyI, 0x18 => KeyO, 0x19 => KeyP,
        0x1A => BracketLeft, 0x1B => BracketRight, 0x1C => Enter,
        0x1E => KeyA, 0x1F => KeyS, 0x20 => KeyD, 0x21 => KeyF, 0x22 => KeyG,
        0x23 => KeyH, 0x24 => KeyJ, 0x25 => KeyK, 0x26 => KeyL,
        0x27 => Semicolon, 0x28 => Quote, 0x29 => Backquote, 0x2B => Backslash,
        0x2C => KeyZ, 0x2D => KeyX, 0x2E => KeyC, 0x2F => KeyV, 0x30 => KeyB, 0x31 => KeyN, 0x32 => KeyM,
        0x33 => Comma, 0x34 => Period, 0x35 => Slash,
        0x37 => NumpadMultiply, 0x39 => Space,
        0x3B => F1, 0x3C => F2, 0x3D => F3, 0x3E => F4, 0x3F => F5,
        0x40 => F6, 0x41 => F7, 0x42 => F8, 0x43 => F9, 0x44 => F10,
        0x45 => NumLock,
        0x47 => Numpad7, 0x48 => Numpad8, 0x49 => Numpad9, 0x4A => NumpadSubtract,
        0x4B => Numpad4, 0x4C => Numpad5, 0x4D => Numpad6, 0x4E => NumpadAdd,
        0x4F => Numpad1, 0x50 => Numpad2, 0x51 => Numpad3, 0x52 => Numpad0, 0x53 => NumpadDecimal,
        0x56 => IntlBackslash, 0x57 => F11, 0x58 => F12, 0x59 => NumpadEqual,
        0x64 => F13, 0x65 => F14, 0x66 => F15, 0x67 => F16,
        0x68 => F17, 0x69 => F18, 0x6A => F19, 0x6B => F20,
        0xE01C => NumpadEnter, 0xE035 => NumpadDivide,
        0xE047 => Home, 0xE048 => ArrowUp, 0xE049 => PageUp, 0xE04B => ArrowLeft, 0xE04D => ArrowRight,
        0xE04F => End, 0xE050 => ArrowDown, 0xE051 => PageDown, 0xE052 => Insert, 0xE053 => Delete,
        _ => return None,
    })
}

/// A typed character by its US-layout position: letters, digits and the punctuation keys.
fn char_code(c: char) -> Option<Code> {
    use Code::*;
    Some(match c.to_ascii_lowercase() {
        'a' => KeyA, 'b' => KeyB, 'c' => KeyC, 'd' => KeyD, 'e' => KeyE, 'f' => KeyF, 'g' => KeyG,
        'h' => KeyH, 'i' => KeyI, 'j' => KeyJ, 'k' => KeyK, 'l' => KeyL, 'm' => KeyM, 'n' => KeyN,
        'o' => KeyO, 'p' => KeyP, 'q' => KeyQ, 'r' => KeyR, 's' => KeyS, 't' => KeyT, 'u' => KeyU,
        'v' => KeyV, 'w' => KeyW, 'x' => KeyX, 'y' => KeyY, 'z' => KeyZ,
        '1' | '!' => Digit1, '2' | '@' => Digit2, '3' | '#' => Digit3, '4' | '$' => Digit4, '5' | '%' => Digit5,
        '6' | '^' => Digit6, '7' | '&' => Digit7, '8' | '*' => Digit8, '9' | '(' => Digit9, '0' | ')' => Digit0,
        '-' | '_' => Minus, '=' | '+' => Equal, '[' | '{' => BracketLeft, ']' | '}' => BracketRight,
        '\\' | '|' => Backslash, ';' | ':' => Semicolon, '\'' | '"' => Quote, '`' | '~' => Backquote,
        ',' | '<' => Comma, '.' | '>' => Period, '/' | '?' => Slash, ' ' => Space,
        _ => return None,
    })
}

/// The crate's modifiers for the app's chord bits; Win is Cmd here (SUPER, which the crate maps to Cmd).
#[allow(deprecated)]
pub fn mods_of(mods: u32) -> Modifiers {
    let mut m = Modifiers::empty();
    for (bit, flag) in [(MOD_WIN, Modifiers::SUPER), (MOD_ALT, Modifiers::ALT), (MOD_CONTROL, Modifiers::CONTROL), (MOD_SHIFT, Modifiers::SHIFT)] {
        if mods & bit != 0 {
            m |= flag;
        }
    }
    m
}

/// Every double tap is refused: the answer `set_hotkeys` gives for them.
pub fn tap_results(taps: &[(u32, u8)]) -> Results {
    taps.iter().map(|(id, _)| (*id, Err(TAPS_REFUSED.to_string()))).collect()
}

/// A chord's registration error as the person reads it. Carbon refuses a chord another app owns.
fn refusal_text(e: &global_hotkey::Error) -> String {
    match e {
        global_hotkey::Error::FailedToRegister(m) if m.contains("RegisterEventHotKey") => TAKEN.to_string(),
        global_hotkey::Error::AlreadyRegistered(_) => TAKEN.to_string(),
        e => e.to_string(),
    }
}

/// The key a chord registers as: None when the key has no macOS code.
type Resolved = Option<(Code, Modifiers)>;

fn resolve(c: &Chord) -> Resolved {
    code_of(c.key).map(|code| (code, mods_of(c.mods)))
}

/// Registrations to make, with every id that uses each. Chords that resolve to the same key
/// share one (siblings, or a Char and a Scan chord for the same key); an unmapped key stands alone.
fn plan(chords: &[(u32, Chord)]) -> Vec<(Resolved, Vec<u32>)> {
    let mut out: Vec<(Resolved, Vec<u32>)> = Vec::new();
    for (id, c) in chords {
        let r = resolve(c);
        match out.iter_mut().find(|(k, _)| r.is_some() && *k == r) {
            Some((_, ids)) => ids.push(*id),
            None => out.push((r, vec![*id])),
        }
    }
    out
}

/// Replaces the registered hotkeys with `chords`; the answer holds each chord's and tap's outcome.
pub fn set_hotkeys(chords: Vec<(u32, Chord)>, taps: Vec<(u32, u8)>) -> Result<Results, String> {
    if objc2::MainThreadMarker::new().is_none() {
        return Err("system-wide hotkeys can only be set on the main thread".into());
    }
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        if s.is_none() {
            let manager = GlobalHotKeyManager::new().map_err(|e| e.to_string())?;
            *s = Some(State { manager, keys: Vec::new(), last: Vec::new(), answers: HashMap::new() });
        }
        let st = s.as_mut().expect("made above");
        st.last = chords.clone();
        let mut out = register(st, &chords, *locked(&PAUSED));
        out.extend(tap_results(&taps));
        Ok(out)
    })
}

/// Drops the earlier set, then registers each key once. While paused nothing is registered and
/// each id gets its latest answer (Ok when it has none), so a refused chord keeps its error.
fn register(st: &mut State, chords: &[(u32, Chord)], paused: bool) -> Results {
    for k in st.keys.drain(..) {
        let _ = st.manager.unregister(k);
    }
    let mut ids = HashMap::new();
    let mut out = Vec::new();
    for (resolved, group_ids) in plan(chords) {
        let result = if paused {
            None
        } else {
            Some(match resolved {
                None => Err("This key has no macOS key code".to_string()),
                Some((code, mods)) => {
                    let key = HotKey::new(if mods.is_empty() { None } else { Some(mods) }, code);
                    match st.manager.register(key) {
                        Ok(()) => {
                            st.keys.push(key);
                            ids.insert(key.id(), group_ids.clone());
                            Ok(())
                        }
                        Err(e) => Err(refusal_text(&e)),
                    }
                }
            })
        };
        for id in group_ids {
            let r = result.clone().unwrap_or_else(|| st.answers.get(&id).cloned().unwrap_or(Ok(())));
            out.push((id, r));
        }
    }
    if !paused {
        st.answers = out.iter().cloned().collect();
    }
    *locked(&IDS) = Some(ids);
    out
}

/// The shortcut recorder listens: the chords are released so it gets the keys, then taken back.
pub fn set_recording(on: bool) {
    if std::mem::replace(&mut *locked(&PAUSED), on) == on {
        return;
    }
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            let last = st.last.clone();
            register(st, &last, on);
        }
    });
}

/// Whether the shortcut recorder listens (the key claim gives it every Cmd chord then).
pub fn recording() -> bool {
    *locked(&PAUSED)
}

/// Hands presses to `sink` as the ids their chord was registered under.
pub fn set_hotkey_sink(sink: Sink) {
    *locked(&SINK) = Some(sink);
    GlobalHotKeyEvent::set_event_handler(Some(|e: GlobalHotKeyEvent| {
        if e.state() != HotKeyState::Pressed {
            return;
        }
        let ids = locked(&IDS).as_ref().and_then(|m| m.get(&e.id()).cloned());
        if let (Some(ids), Some(sink)) = (ids, locked(&SINK).as_ref()) {
            sink(ids);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(deprecated)]
    #[test]
    fn every_scan_code_has_a_code() {
        for mac in 0..=0x7Fu16 {
            if let Some(s) = crate::hotkey::scan_code(mac) {
                assert!(code_of(ChordKey::Scan(s)).is_some(), "mac key {mac:#x}, scan {s:#x}");
            }
        }
        assert_eq!(code_of(ChordKey::Scan(0x1E)), Some(Code::KeyA));
        assert_eq!(code_of(ChordKey::Scan(0x3B)), Some(Code::F1));
        assert_eq!(code_of(ChordKey::Scan(0xE048)), Some(Code::ArrowUp));
        assert_eq!(code_of(ChordKey::Scan(0x39)), Some(Code::Space));
        assert_eq!(code_of(ChordKey::Char('Q')), Some(Code::KeyQ));
        assert_eq!(code_of(ChordKey::Char('/')), Some(Code::Slash));
        assert_eq!(mods_of(MOD_WIN | MOD_SHIFT), Modifiers::SUPER | Modifiers::SHIFT);
        assert_eq!(mods_of(MOD_ALT | MOD_CONTROL), Modifiers::ALT | Modifiers::CONTROL);
    }

    #[test]
    fn taps_are_refused_by_name() {
        let r = tap_results(&[(3, 1)]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, 3);
        assert!(r[0].1.as_ref().unwrap_err().contains("Double-tap"));
    }

    #[test]
    fn siblings_share_one_registration() {
        let c = Chord { key: ChordKey::Scan(0x39), mods: MOD_ALT };
        let d = Chord { key: ChordKey::Scan(0x3B), mods: 0 };
        let g = plan(&[(1, c), (2, d), (3, c)]);
        assert_eq!(g, vec![(resolve(&c), vec![1, 3]), (resolve(&d), vec![2])]);
    }

    #[test]
    fn a_char_and_a_scan_chord_for_one_key_share_a_registration() {
        let by_char = Chord { key: ChordKey::Char('a'), mods: MOD_ALT };
        let by_scan = Chord { key: ChordKey::Scan(0x1E), mods: MOD_ALT };
        let g = plan(&[(1, by_char), (2, by_scan)]);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].1, vec![1, 2]);
        let bad = Chord { key: ChordKey::Scan(0x9999), mods: 0 };
        assert_eq!(plan(&[(1, bad), (2, bad)]).len(), 2, "unmapped keys stay apart");
    }
}

