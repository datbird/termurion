//! Win chords for Tandem. Windows keeps many Win chords for itself (Win+D, Win+T), so
//! a low-level keyboard hook takes Win chords while a Tandem window has focus and sends
//! them to the app as Cmd chords. Only chords with an enabled Tandem command are taken;
//! the rest stay with Windows. Win+L is never taken (Windows does not allow it), and a
//! harmless key is sent with each taken chord, so the Start menu does not open after it.
//! The same hook remaps modifiers (Keys > Remap Modifiers) while a Tandem window is in front.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
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

/// The Windows virtual key code of a physical key (0 when it has none).
pub fn vk_of(key: winit::keyboard::PhysicalKey) -> u16 {
    use winit::platform::scancode::PhysicalKeyExtScancode;
    const MAPVK_VSC_TO_VK_EX: u32 = 3;
    match key.to_scancode() {
        Some(scan) => unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK_EX) as u16 },
        None => 0,
    }
}

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
/// Tandem's terminal windows.
static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn is_tandem(hwnd: isize) -> bool {
    WINDOWS.lock().is_ok_and(|w| w.contains(&hwnd))
}

/// A terminal window closed.
pub fn remove_window(hwnd: isize) {
    if let Ok(mut w) = WINDOWS.lock() {
        w.retain(|h| *h != hwnd);
    }
}
/// Password manager windows: Win+B, Win+U and Win+C go to them (Windows keeps those chords
/// for itself otherwise).
static PWM_WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

pub fn add_pwm_window(hwnd: isize) {
    if let Ok(mut w) = PWM_WINDOWS.lock() {
        if !w.contains(&hwnd) {
            w.push(hwnd);
        }
    }
}

pub fn remove_pwm_window(hwnd: isize) {
    if let Ok(mut w) = PWM_WINDOWS.lock() {
        w.retain(|h| *h != hwnd);
    }
}

/// The AI Chat window: Win+W and Shift+Win+Y go to it (Windows keeps Win+W for itself).
static CHAT_WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

pub fn add_chat_window(hwnd: isize) {
    if let Ok(mut w) = CHAT_WINDOWS.lock() {
        if !w.contains(&hwnd) {
            w.push(hwnd);
        }
    }
}

pub fn remove_chat_window(hwnd: isize) {
    if let Ok(mut w) = CHAT_WINDOWS.lock() {
        w.retain(|h| *h != hwnd);
    }
}

fn is_chat(hwnd: isize) -> bool {
    CHAT_WINDOWS.lock().is_ok_and(|w| w.contains(&hwnd))
}

/// The window in front when the hook last took a chord, read by the app when the chord arrives
/// (the focus may have moved by then).
static CHORD_FOREGROUND: AtomicIsize = AtomicIsize::new(0);

pub fn chord_foreground() -> isize {
    CHORD_FOREGROUND.load(Ordering::Acquire)
}

fn is_pwm(hwnd: isize) -> bool {
    PWM_WINDOWS.lock().is_ok_and(|w| w.contains(&hwnd))
}

/// The chords a password manager window takes: Win+B, Win+U, Win+C with no other modifier
/// (its key equivalents, `WX:138-219`).
pub fn pwm_chord(c: &Chord) -> bool {
    matches!(c.key, ChordKey::Char('b' | 'u' | 'c')) && c.cmd && !c.alt && !c.shift && !c.ctrl
}

/// The chords the AI Chat window takes, with no other modifier: Win+W (close), Shift+Win+Y
/// (focus) (`CWC:481-501`), and the find chords Win+F, Win+G, Shift+Win+G and Win+E
/// (`CVC:2917-2960`).
pub fn chat_chord(c: &Chord) -> bool {
    c.cmd
        && !c.alt
        && !c.ctrl
        && matches!((&c.key, c.shift), (ChordKey::Char('w' | 'f' | 'e'), false) | (ChordKey::Char('y'), true) | (ChordKey::Char('g'), _))
}
static SETTINGS: AtomicIsize = AtomicIsize::new(0);
/// The settings window is recording a keystroke: every Win chord goes to it.
static RECORDING: AtomicBool = AtomicBool::new(false);

pub fn set_recording(on: bool) {
    // A change pauses or restores the system-wide hotkeys (on the hook thread).
    if RECORDING.swap(on, Ordering::Relaxed) != on {
        let thread = HOOK_THREAD.load(Ordering::Acquire);
        if thread != 0 {
            unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(thread, WM_APP_RECORDING, 0, 0) };
        }
    }
}

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
pub fn mask_start_menu() {
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
    let focused = is_tandem(fg) || fg == SETTINGS.load(Ordering::Relaxed);
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
            // System-wide hotkeys first: they act whatever window is in front, and a remap
            // must not rewrite the physical chord.
            if kb.dwExtraInfo != MAGIC {
                match global_hotkeys(kb, down) {
                    HookVerdict::Swallow { .. } => return 1,
                    // Windows delivers a registered chord; no remap or Win-chord gate may take it.
                    HookVerdict::ToSystem => return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) },
                    HookVerdict::Continue => {}
                }
            }
            if kb.dwExtraInfo != MAGIC && remap(h, kb, down) {
                return 1;
            }
            if kb.dwExtraInfo != MAGIC {
                let fg = unsafe { GetForegroundWindow() } as isize;
                let recording = RECORDING.load(Ordering::Relaxed) && fg == SETTINGS.load(Ordering::Relaxed);
                let pwm = is_pwm(fg);
                let chat = is_chat(fg);
                let e = KeyEvent {
                    vk: kb.vkCode as u16,
                    down,
                    win_held: held(VK_LWIN as i32) || held(VK_RWIN as i32),
                    focused: is_tandem(fg) || recording || pwm || chat,
                    shift: held(0x10),
                    alt: held(0x12),
                    ctrl: held(0x11),
                };
                let bound = |c: &Chord| recording || if pwm { pwm_chord(c) } else if chat { chat_chord(c) } else { (h.bound)(c) };
                let verdict = h.gate.lock().unwrap().on_key(e, &bound);
                if let Verdict::Take(chord) = verdict {
                    if let Some(c) = chord {
                        mask_start_menu();
                        CHORD_FOREGROUND.store(fg, Ordering::Release);
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
    if let Ok(mut w) = WINDOWS.lock() {
        if !w.contains(&hwnd) {
            w.push(hwnd);
        }
    }
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
            HOOK_THREAD.store(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }, Ordering::Release);
            let _ = tx.send(Ok(()));
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // Registered hotkeys: (RegisterHotKey id, hotkey ids).
            let mut registered: Vec<(i32, Vec<u32>)> = Vec::new();
            let mut last: Vec<(u32, crate::hotkey::Chord)> = Vec::new();
            const WM_HOTKEY: u32 = 0x0312;
            while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                match msg.message {
                    WM_APP_HOTKEYS => apply_pending(&mut registered, &mut last),
                    WM_APP_RECORDING => {
                        let _ = register(&mut registered, &last, RECORDING.load(Ordering::Relaxed));
                    }
                    WM_HOTKEY => {
                        if let Some((_, ids)) = registered.iter().find(|(rid, _)| *rid as usize == msg.wParam) {
                            fire(ids);
                        }
                    }
                    _ => {}
                }
            }
        })
        .map_err(|e| e.to_string())?;
    rx.recv().map_err(|e| e.to_string())?
}

/// System-wide hotkeys (hotkey windows, the app hotkey): one registration per chord, and
/// the ids of every hotkey that uses it (siblings share one chord, as in iTerm2).
pub fn group_chords(bindings: &[(u32, crate::hotkey::Chord)]) -> Vec<(crate::hotkey::Chord, Vec<u32>)> {
    let mut out: Vec<(crate::hotkey::Chord, Vec<u32>)> = Vec::new();
    for (id, c) in bindings {
        match out.iter_mut().find(|(k, _)| k == c) {
            Some((_, ids)) => ids.push(*id),
            None => out.push((*c, vec![*id])),
        }
    }
    out
}

/// Where a chord goes when RegisterHotKey refused it with `error` (a Win32 error code): a
/// Win chord the shell owns goes to the keyboard hook; anything else is an error to show.
pub fn fallback(chord: crate::hotkey::Chord, error: u32) -> Result<(), String> {
    const ERROR_HOTKEY_ALREADY_REGISTERED: u32 = 1409;
    match error {
        ERROR_HOTKEY_ALREADY_REGISTERED if shell_owned(chord) => Ok(()),
        ERROR_HOTKEY_ALREADY_REGISTERED => Err("Another app uses this shortcut".into()),
        e => Err(format!("Windows refused the shortcut: {}", std::io::Error::from_raw_os_error(e as i32))),
    }
}

/// Whether a hook key-down (its scan code, extended flag, and the modifiers held) is `chord`.
pub fn hook_matches(chord: crate::hotkey::Chord, scan: u32, extended: bool, mods: u32, vk_of_char: &dyn Fn(char) -> Option<u32>, vk: u32) -> bool {
    if chord.mods != mods {
        return false;
    }
    match chord.key {
        crate::hotkey::ChordKey::Scan(s) => s & 0xFF == scan & 0xFF && (s & 0xE000 != 0) == extended,
        crate::hotkey::ChordKey::Char(c) => vk_of_char(c) == Some(vk),
    }
}

/// One key event for the hook's hotkeys.
#[derive(Clone, Copy, Debug)]
pub struct HookKey {
    pub vk: u32,
    pub scan: u32,
    pub extended: bool,
    pub down: bool,
    pub time: std::time::Duration,
    /// The modifiers held (MOD_* bits).
    pub mods: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookVerdict {
    /// Not a hotkey: the rest of the hook decides.
    Continue,
    /// A hotkey took it; `mask` sends the Start-menu mask (a Win chord).
    Swallow { mask: bool },
    /// A chord registered with RegisterHotKey: pass it on untouched so Windows delivers it.
    ToSystem,
}

/// System-wide hotkeys as the hook sees them: chords only the hook can take (the shell owns
/// them), chords Windows delivers (so nothing else in the hook may take them), and double
/// taps. Paused while Settings records a shortcut.
#[derive(Default)]
pub struct HookHotkeys {
    pub hooked: Vec<(crate::hotkey::Chord, Vec<u32>)>,
    pub system: Vec<crate::hotkey::Chord>,
    /// Keys whose key-down a hotkey took; their repeats and key-ups are taken too.
    pub taken: std::collections::HashSet<u32>,
    pub taps: Vec<(crate::doubletap::Machine, u32)>,
    pub paused: bool,
}

impl HookHotkeys {
    /// The verdict for one key event, and the hotkeys it fired.
    pub fn on_key(&mut self, k: HookKey, vk_of_char: &dyn Fn(char) -> Option<u32>, down_now: &dyn Fn(u32) -> bool) -> (HookVerdict, Vec<u32>) {
        if self.paused {
            return (HookVerdict::Continue, Vec::new());
        }
        let mut fired = Vec::new();
        for (m, id) in self.taps.iter_mut() {
            // A release the hook never saw (Win+L, a UAC prompt) must not leave a key held.
            m.prune(down_now);
            if m.event(crate::doubletap::Ev { vk: k.vk, down: k.down, time: k.time, scan: k.scan }) {
                fired.push(*id);
            }
        }
        if !k.down {
            let taken = self.taken.remove(&k.vk);
            return (if taken { HookVerdict::Swallow { mask: false } } else { HookVerdict::Continue }, fired);
        }
        if is_modifier(k.vk as u16) {
            return (HookVerdict::Continue, fired);
        }
        // A held hotkey repeats: take the repeats, fire once.
        if self.taken.contains(&k.vk) {
            return (HookVerdict::Swallow { mask: false }, fired);
        }
        if let Some((chord, ids)) = self.hooked.iter().find(|(c, _)| hook_matches(*c, k.scan, k.extended, k.mods, vk_of_char, k.vk)) {
            self.taken.insert(k.vk);
            fired.extend(ids.iter().copied());
            return (HookVerdict::Swallow { mask: chord.mods & crate::hotkey::MOD_WIN != 0 }, fired);
        }
        if self.system.iter().any(|c| hook_matches(*c, k.scan, k.extended, k.mods, vk_of_char, k.vk)) {
            return (HookVerdict::ToSystem, fired);
        }
        (HookVerdict::Continue, fired)
    }
}

/// Win chords the Windows shell keeps for itself (RegisterHotKey refuses them): only these
/// go to the keyboard hook. Another app's chord is reported, not taken.
pub fn shell_owned(chord: crate::hotkey::Chord) -> bool {
    use crate::hotkey::{ChordKey, MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN};
    let ChordKey::Scan(scan) = chord.key else { return false };
    if chord.mods & MOD_WIN == 0 {
        return false;
    }
    let extra = chord.mods & !(MOD_WIN | MOD_SHIFT);
    // Win (+Shift) with a letter but Y, a digit, or the shell's other keys.
    const LETTERS: &[u32] = &[0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x2C];
    const OTHERS: &[u32] = &[0x39, 0x0F, 0x33, 0x34, 0x27, 0x1C, 0x0C, 0x0D, 0x01, 0xE04B, 0xE04D, 0xE048, 0xE050, 0xE047, 0xE037, 0x45];
    let digit = (0x02..=0x0B).contains(&scan);
    match extra {
        0 => LETTERS.contains(&scan) || digit || OTHERS.contains(&scan),
        // Virtual desktops (Win+Ctrl+D, F4, Left, Right) and the like.
        MOD_CONTROL => matches!(scan, 0x20 | 0x3E | 0xE04B | 0xE04D | 0x1C | 0x21 | 0x30 | 0x2E),
        // Game Bar and pen shortcuts (Win+Alt+G, R, T, M, B, D, K, H, PrtSc).
        MOD_ALT => matches!(scan, 0x22 | 0x13 | 0x14 | 0x32 | 0x30 | 0x20 | 0x25 | 0x23 | 0xE037),
        _ => false,
    }
}

type Global = HookHotkeys;

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);
static HOTKEY_SINK: OnceLock<Box<dyn Fn(Vec<u32>) + Send + Sync>> = OnceLock::new();
static HOOK_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
type Pending = (Vec<(u32, crate::hotkey::Chord)>, std::sync::mpsc::Sender<Vec<(u32, Result<(), String>)>>);
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
const WM_APP_HOTKEYS: u32 = 0x8000 + 0x48;
const WM_APP_RECORDING: u32 = 0x8000 + 0x49;

/// Where hotkey presses go (their ids).
pub fn set_hotkey_sink(sink: Box<dyn Fn(Vec<u32>) + Send + Sync>) {
    let _ = HOTKEY_SINK.set(sink);
}

/// One press, every hotkey it matched: the app handles each sibling group once.
fn fire(ids: &[u32]) {
    if let (Some(s), false) = (HOTKEY_SINK.get(), ids.is_empty()) {
        let mut ids = ids.to_vec();
        ids.sort_unstable();
        ids.dedup();
        s(ids);
    }
}

/// Replaces the system-wide hotkeys: chords (RegisterHotKey on the hook thread, or the hook
/// for chords the shell owns) and double taps (modifier, id). Returns each chord's outcome.
pub fn set_hotkeys(chords: Vec<(u32, crate::hotkey::Chord)>, taps: Vec<(u32, u8)>, min: std::time::Duration, max: std::time::Duration) -> Result<Vec<(u32, Result<(), String>)>, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW;
    let thread = HOOK_THREAD.load(Ordering::Acquire);
    if thread == 0 {
        return Err("the keyboard hook is not running".into());
    }
    let paused = RECORDING.load(Ordering::Relaxed);
    *GLOBAL.lock().unwrap() = Some(Global { taps: taps.into_iter().map(|(id, m)| (crate::doubletap::Machine::new(m, min, max), id)).collect(), paused, ..Default::default() });
    let (tx, rx) = std::sync::mpsc::channel();
    *PENDING.lock().unwrap() = Some((chords, tx));
    if unsafe { PostThreadMessageW(thread, WM_APP_HOTKEYS, 0, 0) } == 0 {
        return Err(format!("could not reach the keyboard hook: {}", std::io::Error::last_os_error()));
    }
    rx.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| "the keyboard hook did not answer".to_string())
}

/// On the hook thread: registers the pending chords (RegisterHotKey is per thread).
fn apply_pending(registered: &mut Vec<(i32, Vec<u32>)>, last: &mut Vec<(u32, crate::hotkey::Chord)>) {
    let Some((chords, reply)) = PENDING.lock().unwrap().take() else { return };
    *last = chords.clone();
    let out = register(registered, &chords, RECORDING.load(Ordering::Relaxed));
    let _ = reply.send(out);
}

/// Replaces the registered chords. While Settings records a shortcut nothing is
/// registered, so the recorder gets the keys (iTerm2 sends a press to the recorder).
fn register(registered: &mut Vec<(i32, Vec<u32>)>, chords: &[(u32, crate::hotkey::Chord)], paused: bool) -> Vec<(u32, Result<(), String>)> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, VkKeyScanW};
    for (rid, _) in registered.drain(..) {
        unsafe { UnregisterHotKey(std::ptr::null_mut(), rid) };
    }
    if paused {
        if let Some(g) = GLOBAL.lock().unwrap().as_mut() {
            g.paused = true;
        }
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut hooked = Vec::new();
    let mut system = Vec::new();
    const MAPVK_VSC_TO_VK_EX: u32 = 3;
    const MOD_NOREPEAT: u32 = 0x4000;
    for (i, (chord, ids)) in group_chords(&chords).into_iter().enumerate() {
        let vk = match chord.key {
            crate::hotkey::ChordKey::Scan(s) => unsafe { MapVirtualKeyW(s, MAPVK_VSC_TO_VK_EX) },
            crate::hotkey::ChordKey::Char(c) => (unsafe { VkKeyScanW(c as u16) } as u32) & 0xFF,
        };
        let rid = i as i32 + 1;
        let result = if vk == 0 || vk == 0xFF {
            Err("This key has no Windows key code on this keyboard".to_string())
        } else if unsafe { RegisterHotKey(std::ptr::null_mut(), rid, chord.mods | MOD_NOREPEAT, vk) } != 0 {
            registered.push((rid, ids.clone()));
            system.push(chord);
            Ok(())
        } else {
            let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
            let r = fallback(chord, e);
            if r.is_ok() {
                hooked.push((chord, ids.clone()));
            }
            r
        };
        out.extend(ids.iter().map(|id| (*id, result.clone())));
    }
    if let Some(g) = GLOBAL.lock().unwrap().as_mut() {
        g.hooked = hooked;
        g.system = system;
        g.paused = false;
    }
    out
}

/// The hook's part of system-wide hotkeys.
fn global_hotkeys(kb: &KBDLLHOOKSTRUCT, down: bool) -> HookVerdict {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW;
    let mut guard = GLOBAL.lock().unwrap();
    let Some(g) = guard.as_mut() else { return HookVerdict::Continue };
    const LLKHF_EXTENDED: u32 = 0x01;
    let mods = [(held(VK_LWIN as i32) || held(VK_RWIN as i32), crate::hotkey::MOD_WIN), (held(0x12), crate::hotkey::MOD_ALT), (held(0x11), crate::hotkey::MOD_CONTROL), (held(0x10), crate::hotkey::MOD_SHIFT)]
        .iter()
        .filter(|(h, _)| *h)
        .fold(0, |a, (_, m)| a | m);
    let k = HookKey { vk: kb.vkCode, scan: kb.scanCode, extended: kb.flags & LLKHF_EXTENDED != 0, down, time: std::time::Duration::from_millis(kb.time as u64), mods };
    let vk_of_char = |c: char| Some((unsafe { VkKeyScanW(c as u16) } as u32) & 0xFF);
    let (verdict, fired) = g.on_key(k, &vk_of_char, &|vk| held(vk as i32));
    drop(guard);
    if let HookVerdict::Swallow { mask: true } = verdict {
        mask_start_menu();
    }
    fire(&fired);
    verdict
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
    fn registration_plan_splits_hook_and_system() {
        use crate::hotkey::{Chord as HChord, ChordKey as HKey, MOD_ALT, MOD_WIN};
        let space = HChord { key: HKey::Scan(0x39), mods: MOD_ALT };
        let t = HChord { key: HKey::Scan(0x14), mods: MOD_WIN };
        assert_eq!(group_chords(&[(1, space), (2, t), (3, space)]), vec![(space, vec![1, 3]), (t, vec![2])]);
        // ERROR_HOTKEY_ALREADY_REGISTERED (1409).
        assert_eq!(fallback(t, 1409), Ok(()), "Win+T belongs to the shell: the hook takes it");
        assert_eq!(fallback(space, 1409), Err("Another app uses this shortcut".to_string()));
        assert!(fallback(space, 5).unwrap_err().contains("5"), "another error says what it was");
        let f1 = HChord { key: HKey::Scan(0x3B), mods: 0 };
        assert!(hook_matches(f1, 0x3B, false, 0, &|_| None, 0x70));
        assert!(!hook_matches(f1, 0x3B, false, MOD_ALT, &|_| None, 0x70), "extra modifiers");
        let up = HChord { key: HKey::Scan(0xE048), mods: MOD_WIN };
        assert!(hook_matches(up, 0x48, true, MOD_WIN, &|_| None, 0x26));
        assert!(!hook_matches(up, 0x48, false, MOD_WIN, &|_| None, 0x68), "keypad 8 is not Up");
        let x = HChord { key: HKey::Char('x'), mods: MOD_WIN };
        assert!(hook_matches(x, 0x2D, false, MOD_WIN, &|c| (c == 'x').then_some(0x58), 0x58));
    }

    #[test]
    fn hook_hotkeys_follow_the_rules() {
        use crate::hotkey::{Chord as HChord, ChordKey as HKey, MOD_CONTROL, MOD_WIN};
        let t = HChord { key: HKey::Scan(0x14), mods: MOD_WIN };
        let tick = HChord { key: HKey::Scan(0x29), mods: MOD_CONTROL };
        let mut h = HookHotkeys { hooked: vec![(t, vec![7])], system: vec![tick], ..Default::default() };
        let key = |vk, scan, down, mods| HookKey { vk, scan, extended: false, down, time: std::time::Duration::ZERO, mods };
        let none = |_: char| None;
        let up = |_: u32| false;
        assert_eq!(h.on_key(key(0x54, 0x14, true, MOD_WIN), &none, &up), (HookVerdict::Swallow { mask: true }, vec![7]));
        assert_eq!(h.on_key(key(0x54, 0x14, true, MOD_WIN), &none, &up), (HookVerdict::Swallow { mask: false }, vec![]), "a repeat fires nothing");
        assert_eq!(h.on_key(key(0x54, 0x14, false, MOD_WIN), &none, &up).0, HookVerdict::Swallow { mask: false }, "its release is taken too");
        assert_eq!(h.on_key(key(0xC0, 0x29, true, MOD_CONTROL), &none, &up).0, HookVerdict::ToSystem, "Windows delivers a registered chord");
        assert_eq!(h.on_key(key(0x41, 0x1E, true, 0), &none, &up).0, HookVerdict::Continue);
        h.paused = true;
        assert_eq!(h.on_key(key(0x54, 0x14, true, MOD_WIN), &none, &up), (HookVerdict::Continue, vec![]), "recording: the recorder gets it");
        assert!(shell_owned(t), "Win+T");
        assert!(!shell_owned(HChord { key: HKey::Scan(0x15), mods: MOD_WIN }), "Win+Y is free");
        assert!(!shell_owned(HChord { key: HKey::Scan(0x24), mods: MOD_WIN | crate::hotkey::MOD_ALT | MOD_CONTROL }), "Win+Ctrl+Alt+J");
        assert_eq!(fallback(HChord { key: HKey::Scan(0x15), mods: MOD_WIN }, 1409), Err("Another app uses this shortcut".to_string()));
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

    #[test]
    fn password_manager_windows_take_their_win_chords() {
        let c = |k: char, shift: bool| Chord { key: ChordKey::Char(k), cmd: true, alt: false, shift, ctrl: false };
        assert!(pwm_chord(&c('b', false)) && pwm_chord(&c('u', false)) && pwm_chord(&c('c', false)));
        assert!(!pwm_chord(&c('b', true)) && !pwm_chord(&c('x', false)));
        let mut g = Gate::default();
        assert!(matches!(g.on_key(key(0x42, true), &|ch| pwm_chord(ch)), Verdict::Take(Some(_))), "Win+B in the manager");
        assert!(matches!(g.on_key(key(0x42, false), &|ch| pwm_chord(ch)), Verdict::Take(None)));
        assert!(matches!(g.on_key(key(0x44, true), &|ch| pwm_chord(ch)), Verdict::Pass), "Win+D stays with Windows");
        add_pwm_window(77);
        assert!(is_pwm(77));
        remove_pwm_window(77);
        assert!(!is_pwm(77));
    }

    #[test]
    fn the_chat_window_takes_win_w_and_shift_win_y() {
        let c = |k: char, shift: bool| Chord { key: ChordKey::Char(k), cmd: true, alt: false, shift, ctrl: false };
        assert!(chat_chord(&c('w', false)) && chat_chord(&c('y', true)));
        assert!(!chat_chord(&c('w', true)) && !chat_chord(&c('y', false)) && !chat_chord(&c('x', false)));
        assert!(!chat_chord(&Chord { ctrl: true, ..c('w', false) }));
        assert!(matches!(Gate::default().on_key(key(0x57, true), &|ch| chat_chord(ch)), Verdict::Take(Some(_))), "Win+W in the chat window");
        add_chat_window(78);
        assert!(is_chat(78));
        remove_chat_window(78);
        assert!(!is_chat(78));
    }
}
