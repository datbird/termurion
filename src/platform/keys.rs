//! The keyboard layer: the low-level hook and system-wide hotkeys (Windows), and the chord rules
//! that need no OS. On Unix the Cmd and Super keys come from winit's modifiers, so there is no hook to
//! install and the hook calls are no-ops.

use crate::menus::{Chord, ChordKey};

#[cfg(unix)]
use super::unix::keys as imp;
#[cfg(windows)]
use super::windows::keys as imp;

pub use imp::{install, mask_start_menu, mouse_buttons, remove_window, set_hotkey_sink, set_hotkeys, set_remap_global, set_web_focus, text_for, uninstall, vk_of};

/// The chord key of a Windows virtual key code, or None when it has none.
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
        // Punctuation depends on the layout: ask the system which character the key types.
        0xBA..=0xC0 | 0xDB..=0xDF => ChordKey::Char(imp::punctuation(vk)?),
        _ => return None,
    })
}

/// The one chord the Cockpit window takes: Window > Cockpit, Cmd+Option+Shift+C (Alt+Shift+F8).
pub fn cockpit_chord(c: &Chord) -> bool {
    c.cmd && c.alt && c.shift && !c.ctrl && matches!(c.key, ChordKey::Char('c'))
}

/// The chords a password manager window takes: Cmd+B, Cmd+U, Cmd+C with no other modifier
/// (its key equivalents, `WX:138-219`), pressed as the shortcut key.
pub fn pwm_chord(c: &Chord) -> bool {
    matches!(c.key, ChordKey::Char('b' | 'u' | 'c')) && c.cmd && !c.alt && !c.shift && !c.ctrl
}

/// The chords the AI Chat window takes, with no other modifier: Cmd+W (close), Cmd+Shift+Y
/// (focus) (`CWC:481-501`), and the find chords Cmd+F, Cmd+G, Cmd+Shift+G and Cmd+E
/// (`CVC:2917-2960`), pressed as the shortcut key.
pub fn chat_chord(c: &Chord) -> bool {
    c.cmd
        && !c.alt
        && !c.ctrl
        && matches!((&c.key, c.shift), (ChordKey::Char('w' | 'f' | 'e'), false) | (ChordKey::Char('y'), true) | (ChordKey::Char('g'), _))
}

/// Win chords the Windows shell keeps for itself (RegisterHotKey refuses them): only these
/// go to the keyboard hook. Another app's chord is reported, not taken.
pub fn shell_owned(chord: crate::hotkey::Chord) -> bool {
    use crate::hotkey::{ChordKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
    let ChordKey::Scan(scan) = chord.key else { return false };
    if chord.mods & MOD_WIN == 0 {
        return false;
    }
    let shift = chord.mods & MOD_SHIFT != 0;
    let extra = chord.mods & !(MOD_WIN | MOD_SHIFT);
    // Win (+Shift) with a letter but Y, a digit, or the shell's other keys.
    const LETTERS: &[u32] = &[0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x2C];
    const OTHERS: &[u32] = &[0x39, 0x0F, 0x33, 0x34, 0x27, 0x1C, 0x0C, 0x0D, 0x01, 0xE04B, 0xE04D, 0xE048, 0xE050, 0xE047, 0xE037, 0x45];
    // Win+1..0, and Win+Ctrl or Win+Alt with 1..9 (the pinned apps' last window and jump lists).
    let digit = (0x02..=0x0B).contains(&scan);
    let digit9 = (0x02..=0x0A).contains(&scan);
    // The modifiers match exactly, as iTerm2's symbolic hotkeys do: Win+Shift is the shell's for
    // Snipping (S), Restore (M), Notifications (V), Cycle taskbar (T), Reverse language (Space),
    // new instance (1..0) and moving, stretching or restoring a window (Left, Right, Up, Down), not
    // every letter.
    match (extra, shift) {
        (0, false) => LETTERS.contains(&scan) || digit || OTHERS.contains(&scan),
        (0, true) => matches!(scan, 0x1F | 0x32 | 0x2F | 0x14 | 0x39 | 0xE04B | 0xE04D | 0xE048 | 0xE050) || digit,
        // Virtual desktops (Win+Ctrl+D, F4, Left, Right), Quick Assist (Q), Narrator (N), speech
        // (S), sound output (V), the input language (Space), and the like.
        (MOD_CONTROL, false) => matches!(scan, 0x20 | 0x3E | 0xE04B | 0xE04D | 0x1C | 0x21 | 0x30 | 0x2E | 0x10 | 0x31 | 0x1F | 0x2F | 0x39) || digit9,
        // Reset the graphics driver (Win+Ctrl+Shift+B).
        (MOD_CONTROL, true) => scan == 0x30,
        // Game Bar and pen shortcuts (Win+Alt+G, R, T, M, B, D, K, H, PrtSc) and jump lists.
        (MOD_ALT, false) => matches!(scan, 0x22 | 0x13 | 0x14 | 0x32 | 0x30 | 0x20 | 0x25 | 0x23 | 0xE037) || digit9,
        _ => false,
    }
}
