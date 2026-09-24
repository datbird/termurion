//! Win chords for Tandem. Windows keeps many Win chords for itself (Win+D, Win+T), so
//! a low-level keyboard hook takes Win chords while a Tandem window has focus and sends
//! them to the app as Cmd chords. Only chords with an enabled Tandem command are taken;
//! the rest stay with Windows. Win+L is never taken (Windows does not allow it), and a
//! harmless key is sent with each taken chord, so the Start menu does not open after it.
//! The same hook remaps modifiers (Keys > Remap Modifiers) while a Tandem window is in front.

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_CHAR,
    MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN,
};

use crate::menus::{Chord, ChordKey};
use crate::mouse::Mods;
use crate::remap::{Remapper, Tags};

const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;
/// An unassigned virtual key. Sending it while Win is down keeps the Start menu closed.
const VK_MASK: u16 = 0xE8;

pub fn vk_to_key(vk: u16) -> Option<ChordKey> {
    Some(match vk {
        0x41..=0x5A => ChordKey::Char((vk as u8 + 32) as char),
        0x30..=0x39 => ChordKey::Char(vk as u8 as char),
        0x26 => ChordKey::Up,
        0x28 => ChordKey::Down,
        0x25 => ChordKey::Left,
        0x27 => ChordKey::Right,
        0x24 => ChordKey::Home,
        0x23 => ChordKey::End,
        0x21 => ChordKey::PageUp,
        0x22 => ChordKey::PageDown,
        0x2E => ChordKey::Delete,
        0x0D => ChordKey::Enter,
        0x09 => ChordKey::Tab,
        0x1B => ChordKey::Escape,
        0x08 => ChordKey::Backspace,
        0x20 => ChordKey::Space,
        0x70..=0x7B => ChordKey::F((vk - 0x70 + 1) as u8),
        // Punctuation depends on the layout: ask Windows which character the key types.
        0xBA..=0xC0 | 0xDB..=0xDF => {
            let c = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_CHAR) } & 0x7FFF;
            ChordKey::Char(char::from_u32(c)?.to_ascii_lowercase())
        }
        _ => return None,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Let Windows have the key.
    Pass,
    /// Tandem takes the key; on a key down, this is the chord to run.
    Take(Option<Chord>),
}

/// One key event as the hook sees it, with the live state of Win and the other modifiers.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    pub vk: u16,
    pub down: bool,
    /// Win is physically down now (read live, so a Win release the hook missed,
    /// after Win+L for example, cannot stick).
    pub win_held: bool,
    /// A Tandem window is in front.
    pub focused: bool,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// The hook's decisions, kept apart from Win32 so they can be tested.
#[derive(Default)]
pub struct Gate {
    /// Keys whose key-down Tandem took; their key-ups are taken too.
    taken: std::collections::HashSet<u16>,
}

impl Gate {
    /// `bound` says whether Tandem has an enabled command for a chord. Unbound Win chords
    /// (Win+Shift+S, snapping, virtual desktops) stay with Windows.
    pub fn on_key(&mut self, e: KeyEvent, bound: &dyn Fn(&Chord) -> bool) -> Verdict {
        if e.vk == VK_LWIN || e.vk == VK_RWIN {
            return Verdict::Pass;
        }
        if !e.down {
            return if self.taken.remove(&e.vk) { Verdict::Take(None) } else { Verdict::Pass };
        }
        // Win+L locks the workstation; Windows never lets an app take it.
        if !e.win_held || !e.focused || e.vk == 0x4C {
            return Verdict::Pass;
        }
        let Some(key) = vk_to_key(e.vk) else { return Verdict::Pass };
        let chord = Chord { key, cmd: true, alt: e.alt, shift: e.shift, ctrl: e.ctrl };
        if !bound(&chord) {
            return Verdict::Pass;
        }
        self.taken.insert(e.vk);
        Verdict::Take(Some(chord))
    }
}

struct Hook {
    gate: Mutex<Gate>,
    send: Box<dyn Fn(Chord) + Send + Sync>,
    bound: Box<dyn Fn(&Chord) -> bool + Send + Sync>,
    remapper: Mutex<Remapper>,
    tags: Arc<RwLock<Tags>>,
    /// True when a key with these physical modifiers is bound to Do Not Remap Modifiers.
    exempt: Box<dyn Fn(u16, Mods) -> bool + Send + Sync>,
    /// The key whose press went through with its physical modifiers.
    exempting: Mutex<Option<u16>>,
}

static HOOK: OnceLock<Hook> = OnceLock::new();
static WINDOW: AtomicIsize = AtomicIsize::new(0);
static SETTINGS: AtomicIsize = AtomicIsize::new(0);

/// The settings window counts as Tandem for remapping.
pub fn set_settings_window(hwnd: isize) {
    SETTINGS.store(hwnd, Ordering::Relaxed);
}
/// Marks the mask key Tandem sends, so the hook passes it and nothing else injected.
/// Other software (a KVM tool such as bumbles) injects real keystrokes; those are handled.
const MAGIC: usize = 0x5441_4E44;

fn held(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

/// Sends a harmless key while Win is down, so Windows does not open the Start menu when
/// Win comes up after a chord Tandem took (the pattern AutoHotkey uses).
fn mask_start_menu() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MASK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

/// Injects keys, marked so the hook passes them. Right-side and Win keys are extended keys.
fn inject(keys: &[(u16, bool)]) {
    let inputs: Vec<INPUT> = keys
        .iter()
        .map(|&(vk, down)| {
            let extended = matches!(vk, 0xA3 | 0xA5 | VK_LWIN | VK_RWIN);
            let flags = if down { 0 } else { KEYEVENTF_KEYUP } | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 };
            let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
            INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: MAGIC } } }
        })
        .collect();
    if !inputs.is_empty() {
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
    }
}

fn is_modifier(vk: u16) -> bool {
    matches!(vk, 0x10..=0x12 | 0xA0..=0xA5 | VK_LWIN | VK_RWIN)
}

/// Remapping, and Do Not Remap Modifiers. True when the event was swallowed.
fn remap(h: &Hook, kb: &KBDLLHOOKSTRUCT, down: bool) -> bool {
    let vk = kb.vkCode as u16;
    let fg = unsafe { GetForegroundWindow() } as isize;
    let focused = fg == WINDOW.load(Ordering::Relaxed) || fg == SETTINGS.load(Ordering::Relaxed);
    let tags = *h.tags.read().unwrap();
    let mut r = h.remapper.lock().unwrap();
    if let crate::remap::Verdict::Replace(keys) = r.on_key(vk, kb.scanCode, down, focused, &tags) {
        inject(&keys);
        return true;
    }
    if is_modifier(vk) {
        return false;
    }
    let mut exempting = h.exempting.lock().unwrap();
    if down && r.active() && (*exempting == Some(vk) || (h.exempt)(vk, r.physical_mods())) {
        *exempting = Some(vk);
        inject(&r.exempt_down(vk));
        return true;
    }
    if !down && *exempting == Some(vk) {
        *exempting = None;
        inject(&r.exempt_up(vk));
        return true;
    }
    false
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(h) = HOOK.get() {
            let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
            if kb.dwExtraInfo != MAGIC && remap(h, kb, down) {
                return 1;
            }
            if kb.dwExtraInfo != MAGIC {
                let e = KeyEvent {
                    vk: kb.vkCode as u16,
                    down,
                    win_held: held(VK_LWIN as i32) || held(VK_RWIN as i32),
                    focused: unsafe { GetForegroundWindow() } as isize == WINDOW.load(Ordering::Relaxed),
                    shift: held(0x10),
                    alt: held(0x12),
                    ctrl: held(0x11),
                };
                let verdict = h.gate.lock().unwrap().on_key(e, &*h.bound);
                if let Verdict::Take(chord) = verdict {
                    if let Some(c) = chord {
                        mask_start_menu();
                        (h.send)(c);
                    }
                    return 1;
                }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Installs the hook for Tandem's main window, on a thread of its own: a low-level hook
/// runs on the installing thread's message loop, and every keystroke on the machine waits
/// for it, so it must never wait for Tandem's drawing. `bound` says which chords Tandem
/// takes; taken chords go to `send`. `tags` are the Remap Modifiers settings, and `exempt`
/// says which keys are bound to Do Not Remap Modifiers.
pub fn install(
    hwnd: isize,
    send: Box<dyn Fn(Chord) + Send + Sync>,
    bound: Box<dyn Fn(&Chord) -> bool + Send + Sync>,
    tags: Arc<RwLock<Tags>>,
    exempt: Box<dyn Fn(u16, Mods) -> bool + Send + Sync>,
) -> Result<(), String> {
    WINDOW.store(hwnd, Ordering::Relaxed);
    let hook = Hook { gate: Mutex::new(Gate::default()), send, bound, remapper: Mutex::new(Remapper::default()), tags, exempt, exempting: Mutex::new(None) };
    if HOOK.set(hook).is_err() {
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("tandem-winkeys".into())
        .spawn(move || {
            let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0) };
            if h.is_null() {
                let _ = tx.send(Err(format!("SetWindowsHookExW failed: {}", std::io::Error::last_os_error())));
                return;
            }
            let _ = tx.send(Ok(()));
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {}
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::ChordKey;

    fn key(vk: u16, down: bool) -> KeyEvent {
        KeyEvent { vk, down, win_held: true, focused: true, shift: false, alt: false, ctrl: false }
    }

    fn all(_: &Chord) -> bool {
        true
    }

    fn none(_: &Chord) -> bool {
        false
    }

    #[test]
    fn keys_map_to_chord_keys() {
        assert_eq!(vk_to_key(0x44), Some(ChordKey::Char('d')));
        assert_eq!(vk_to_key(0x35), Some(ChordKey::Char('5')));
        assert_eq!(vk_to_key(0xDB), Some(ChordKey::Char('['))); // VK_OEM_4 on a US layout
        assert_eq!(vk_to_key(0x26), Some(ChordKey::Up));
        assert_eq!(vk_to_key(0x70), Some(ChordKey::F(1)));
        assert_eq!(vk_to_key(0x10), None); // Shift alone is a modifier, not a key
    }

    #[test]
    fn a_bound_chord_is_taken_and_its_key_up_too() {
        let mut g = Gate::default();
        assert!(matches!(g.on_key(key(0x44, true), &all), Verdict::Take(Some(_))));
        assert_eq!(g.on_key(key(0x44, false), &all), Verdict::Take(None));
    }

    #[test]
    fn only_the_focused_tandem_window_takes_win_chords() {
        let mut g = Gate::default();
        assert_eq!(g.on_key(KeyEvent { focused: false, ..key(0x44, true) }, &all), Verdict::Pass);
    }

    #[test]
    fn a_missed_win_release_does_not_stick() {
        // After Win+L the lock screen eats the Win key-up. The live key state says Win is up.
        let mut g = Gate::default();
        assert_eq!(g.on_key(KeyEvent { win_held: false, ..key(0x44, true) }, &all), Verdict::Pass);
    }

    #[test]
    fn unbound_win_chords_stay_with_windows() {
        // Win+Shift+S (screenshot) and friends: Tandem has no command there.
        let mut g = Gate::default();
        assert_eq!(g.on_key(key(0x53, true), &none), Verdict::Pass);
        assert_eq!(g.on_key(key(0x53, false), &none), Verdict::Pass);
    }

    #[test]
    fn win_l_is_never_taken() {
        let mut g = Gate::default();
        assert_eq!(g.on_key(key(0x4C, true), &all), Verdict::Pass);
    }

    #[test]
    fn win_itself_always_passes() {
        let mut g = Gate::default();
        assert_eq!(g.on_key(key(0x5B, true), &all), Verdict::Pass);
        assert_eq!(g.on_key(key(0x5B, false), &all), Verdict::Pass);
    }

    #[test]
    fn the_chord_carries_the_other_modifiers() {
        let mut g = Gate::default();
        let Verdict::Take(Some(c)) = g.on_key(KeyEvent { shift: true, ..key(0x44, true) }, &all) else { panic!() };
        assert_eq!(c, crate::menus::Chord { key: ChordKey::Char('d'), cmd: true, alt: false, shift: true, ctrl: false });
    }
}
