//! Win chords for Tandem. Windows keeps many Win chords for itself (Win+D, Win+T), so
//! a low-level keyboard hook takes Win chords while a Tandem window has focus and sends
//! them to the app as Cmd chords. Win+L is never taken (Windows does not allow it), and
//! a harmless key is sent before Win comes up, so the Start menu does not open after a
//! chord Tandem used.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, MAPVK_VK_TO_CHAR, MapVirtualKeyW, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED, SetWindowsHookExW, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_SYSKEYDOWN,
};

use crate::menus::{Chord, ChordKey};

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
    /// Let Win come up, but first send the mask key so the Start menu stays closed.
    PassAndMaskStart,
}

/// The hook's decisions, kept apart from Win32 so they can be tested.
#[derive(Default)]
pub struct Gate {
    win_down: bool,
    took_any: bool,
}

impl Gate {
    /// `mods` is (shift, alt, ctrl) as held now.
    pub fn on_key(&mut self, vk: u16, down: bool, foreground_is_tandem: bool, mods: (bool, bool, bool)) -> Verdict {
        if vk == VK_LWIN || vk == VK_RWIN {
            self.win_down = down;
            if !down && std::mem::take(&mut self.took_any) {
                return Verdict::PassAndMaskStart;
            }
            return Verdict::Pass;
        }
        // Win+L locks the workstation; Windows never lets an app take it.
        if !self.win_down || !foreground_is_tandem || vk == 0x4C {
            return Verdict::Pass;
        }
        let Some(key) = vk_to_key(vk) else { return Verdict::Pass };
        if !down {
            return Verdict::Take(None);
        }
        self.took_any = true;
        let (shift, alt, ctrl) = mods;
        Verdict::Take(Some(Chord { key, cmd: true, alt, shift, ctrl }))
    }
}

struct Hook {
    gate: Mutex<Gate>,
    send: Box<dyn Fn(Chord) + Send + Sync>,
}

static HOOK: OnceLock<Hook> = OnceLock::new();
static WINDOW: AtomicIsize = AtomicIsize::new(0);

fn held(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

fn mask_start_menu() {
    let key = |flags| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VK_MASK, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    let inputs = [key(0), key(KEYEVENTF_KEYUP)];
    unsafe { SendInput(2, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32) };
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(h) = HOOK.get() {
            let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            // Our own mask key (and any other injected key) passes untouched.
            if kb.flags & LLKHF_INJECTED == 0 {
                let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
                let focused = unsafe { GetForegroundWindow() } as isize == WINDOW.load(Ordering::Relaxed);
                let mods = (held(0x10), held(0x12), held(0x11));
                let verdict = h.gate.lock().unwrap().on_key(kb.vkCode as u16, down, focused, mods);
                match verdict {
                    Verdict::Pass => {}
                    Verdict::PassAndMaskStart => mask_start_menu(),
                    Verdict::Take(chord) => {
                        if let Some(c) = chord {
                            (h.send)(c);
                        }
                        return 1;
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Installs the hook for Tandem's main window. Chords go to `send`, which must be quick:
/// Windows removes a low-level hook that is slow to answer.
pub fn install(hwnd: isize, send: Box<dyn Fn(Chord) + Send + Sync>) -> Result<(), String> {
    WINDOW.store(hwnd, Ordering::Relaxed);
    if HOOK.set(Hook { gate: Mutex::new(Gate::default()), send }).is_err() {
        return Ok(());
    }
    let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0) };
    if h.is_null() {
        return Err(format!("SetWindowsHookExW failed: {}", std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menus::ChordKey;

    const VK_LWIN: u16 = 0x5B;
    const NONE: (bool, bool, bool) = (false, false, false);

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
    fn only_the_focused_tandem_window_takes_win_chords() {
        let mut g = Gate::default();
        assert_eq!(g.on_key(VK_LWIN, true, true, NONE), Verdict::Pass);
        assert!(matches!(g.on_key(0x44, true, true, NONE), Verdict::Take(Some(_))));
        assert_eq!(g.on_key(0x44, false, true, NONE), Verdict::Take(None));
        let mut other = Gate::default();
        other.on_key(VK_LWIN, true, false, NONE);
        assert_eq!(other.on_key(0x44, true, false, NONE), Verdict::Pass);
    }

    #[test]
    fn win_l_is_never_taken() {
        let mut g = Gate::default();
        g.on_key(VK_LWIN, true, true, NONE);
        assert_eq!(g.on_key(0x4C, true, true, NONE), Verdict::Pass);
    }

    #[test]
    fn start_menu_is_masked_only_after_a_taken_chord() {
        let mut g = Gate::default();
        g.on_key(VK_LWIN, true, true, NONE);
        assert_eq!(g.on_key(VK_LWIN, false, true, NONE), Verdict::Pass);
        g.on_key(VK_LWIN, true, true, NONE);
        g.on_key(0x44, true, true, NONE);
        assert_eq!(g.on_key(VK_LWIN, false, true, NONE), Verdict::PassAndMaskStart);
    }

    #[test]
    fn the_chord_carries_the_other_modifiers() {
        let mut g = Gate::default();
        g.on_key(VK_LWIN, true, true, NONE);
        let Verdict::Take(Some(c)) = g.on_key(0x44, true, true, (true, false, false)) else { panic!() };
        assert_eq!(c, crate::menus::Chord { key: ChordKey::Char('d'), cmd: true, alt: false, shift: true, ctrl: false });
    }
}
