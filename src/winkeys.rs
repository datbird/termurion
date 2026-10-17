//! The low-level keyboard hook. Termurion takes no chords with it: no shortcut uses the Win key,
//! which Windows keeps for itself (FIXES 13), and every shortcut is read from the window's own
//! key events (`shortcut_key`). The hook stays for what a window cannot do: the modifier remap
//! (Keys > Remap Modifiers) while a Termurion window is in front, and the system-wide hotkeys.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_CHAR,
    MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetForegroundWindow, GetMessageW, GetSystemMetrics, SM_SWAPBUTTON, HC_ACTION, KBDLLHOOKSTRUCT, MSG, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
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

struct Hook {
    remapper: Mutex<Remapper>,
    tags: Arc<RwLock<Tags>>,
    /// True when a key with these physical modifiers is bound to Do Not Remap Modifiers.
    exempt: Box<dyn Fn(u16, Mods) -> bool + Send + Sync>,
    /// The key whose press went through with its physical modifiers.
    exempting: Mutex<Option<u16>>,
}

static HOOK: OnceLock<Hook> = OnceLock::new();
/// Termurion's terminal windows.
static WINDOWS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

fn is_termurion(hwnd: isize) -> bool {
    locked(&WINDOWS).contains(&hwnd)
}

/// The window whose web view holds the keyboard (13x Task 1a), 0 for none.
static WEB_FOCUS: AtomicIsize = AtomicIsize::new(0);

/// The app says which window's web view has keyboard focus (0: none).
pub fn set_web_focus(hwnd: isize) {
    WEB_FOCUS.store(hwnd, Ordering::Release);
}

/// Whether the terminal has the keyboard: `foreground` is a Termurion window and no web view of it holds
/// the focus. A page owns the keyboard (as in iTerm2), so Termurion's Win-chord commands and remaps
/// stay out of it; only the registered global hotkeys still act.
fn terminal_has_keyboard(foreground_is_termurion: bool, foreground: isize, web_focus: isize) -> bool {
    foreground_is_termurion && !(web_focus != 0 && web_focus == foreground)
}

/// `terminal_has_keyboard` for the window in front now: the one predicate the hook and the remapper ask.
fn termurion_has_keyboard(foreground: isize) -> bool {
    terminal_has_keyboard(is_termurion(foreground), foreground, WEB_FOCUS.load(Ordering::Acquire))
}

/// A terminal window closed.
pub fn remove_window(hwnd: isize) {
    locked(&WINDOWS).retain(|h| *h != hwnd);
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

/// The settings window counts as Termurion for remapping.
pub fn set_settings_window(hwnd: isize) {
    SETTINGS.store(hwnd, Ordering::Relaxed);
}
/// Remap modifiers globally (iTerm2's RemapModifiersGlobally, default on). Read by the hook, so an atomic.
static REMAP_GLOBAL: AtomicBool = AtomicBool::new(true);

/// Marks the mask key Termurion sends, so the hook passes it and nothing else injected.
/// Other software (a KVM tool) injects real keystrokes; those are handled.
const MAGIC: usize = 0x5441_4E44;

fn held(vk: i32) -> bool {
    unsafe { GetAsyncKeyState(vk) as u16 & 0x8000 != 0 }
}

/// The physical mouse buttons right now: (buttons swapped by `SM_SWAPBUTTON`, left down, right
/// down). `GetAsyncKeyState` reads physical buttons, so a caller that wants the primary one
/// (a drag whose capture was lost sees it up) picks by the swap flag.
pub fn mouse_buttons() -> (bool, bool, bool) {
    (unsafe { GetSystemMetrics(SM_SWAPBUTTON) } != 0, held(0x01), held(0x02))
}

/// Sends a harmless key while Win is down, so Windows does not open the Start menu when
/// Win comes up after a chord Termurion took (the pattern AutoHotkey uses).
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

/// The text a key types under `mods` in the keyboard layout `hkl` (0: the calling thread's), or
/// None for a key that types nothing or a dead key. The keyboard state is built from `mods`, not
/// from the keys held, so a remapped modifier types what the key it stands for would. Flag 0x4
/// leaves the layout's pending dead key alone, and a dead key itself yields None.
pub fn text_for_layout(hkl: isize, vk: u16, scan: u32, mods: Mods) -> Option<String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, ToUnicodeEx};
    let mut state = [0u8; 256];
    for (on, key) in [(mods.shift, 0x10), (mods.ctrl, 0x11), (mods.alt, 0x12)] {
        if on {
            state[key] = 0x80;
        }
    }
    let hkl = if hkl == 0 { unsafe { GetKeyboardLayout(0) } } else { hkl as *mut std::ffi::c_void };
    let mut buf = [0u16; 8];
    let n = unsafe { ToUnicodeEx(vk as u32, scan, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 0x4, hkl) };
    if n <= 0 {
        return None;
    }
    String::from_utf16(&buf[..n as usize]).ok()
}

/// `text_for_layout` in the calling thread's current layout. UI thread only, never the hook's.
pub fn text_for(vk: u16, scan: u32, mods: Mods) -> Option<String> {
    text_for_layout(0, vk, scan, mods)
}

/// Turns Remap modifiers globally on or off. Turning it off hands back every key the hook is
/// still holding down for a remapped modifier, so none stays down in Windows. The hook reads the
/// flag atomically; this takes the remapper's lock only for the release.
pub fn set_remap_global(on: bool) {
    if REMAP_GLOBAL.swap(on, Ordering::Relaxed) == on || on {
        return;
    }
    if let Some(h) = HOOK.get() {
        let keys = locked(&h.remapper).flush();
        inject(&keys);
    }
}

/// Removes the keyboard hook and the hotkeys, first releasing any remapped key still held. Called
/// when Termurion ends and from the panic hook; a crash or kill needs nothing, as Windows removes a
/// low-level hook and its hotkeys when the process dies. Waits a moment for the hook thread.
pub fn uninstall() {
    let thread = HOOK_THREAD.swap(0, Ordering::AcqRel);
    if thread == 0 {
        return;
    }
    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostThreadMessageW(thread, WM_APP_UNHOOK, 0, 0) } == 0 {
        return;
    }
    for _ in 0..40 {
        if UNHOOKED.load(Ordering::Acquire) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Locks a hook mutex through a poison: after a panic in the hook (caught by `guarded`) the data
/// is still consistent for a key press, and unwrapping here would panic on every later keystroke.
/// Every lock in this module is taken here (a source test holds it).
fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The Remap Modifiers settings, read through a poison (a panic while the settings side held the
/// write lock leaves the last whole value in place).
fn read_tags(t: &RwLock<Tags>) -> Tags {
    *t.read().unwrap_or_else(|e| e.into_inner())
}

/// Remapping, and Do Not Remap Modifiers. True when the event was swallowed.
fn remap(h: &Hook, kb: &KBDLLHOOKSTRUCT, down: bool) -> bool {
    let vk = kb.vkCode as u16;
    let fg = unsafe { GetForegroundWindow() } as isize;
    let focused = termurion_has_keyboard(fg) || fg == SETTINGS.load(Ordering::Relaxed);
    let tags = read_tags(&h.tags);
    let mut r = locked(&h.remapper);
    if let crate::remap::Verdict::Replace(keys) = r.on_key(vk, kb.scanCode, down, focused, &tags, REMAP_GLOBAL.load(Ordering::Relaxed)) {
        inject(&keys);
        return true;
    }
    if is_modifier(vk) {
        return false;
    }
    let mut exempting = locked(&h.exempting);
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
    // A panic must never unwind out of this callback (that aborts the process with a remapped key
    // still down): the key goes on to Windows and every key held for a remap is released.
    guarded(|| unsafe { hook_body(code, wparam, lparam) }, || {
        release_remapped();
        unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
    })
}

/// Whether the hook thread's message loop goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Go,
    Stop,
}

/// Runs one message-loop arm; a panic in it is caught and the loop goes on.
fn pump_guarded(arm: impl FnOnce() -> Flow) -> Flow {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(arm)).unwrap_or(Flow::Go)
}

/// Runs its closure when dropped: on a normal end and while unwinding. A panic in the closure is
/// caught (a second panic during an unwind would abort).
struct OnEnd<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> OnEnd<F> {
    fn new(f: F) -> Self {
        OnEnd(Some(f))
    }
}

impl<F: FnOnce()> Drop for OnEnd<F> {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        }
    }
}

/// Runs `body`; a panic in it runs `recover` instead.
fn guarded(body: impl FnOnce() -> LRESULT, recover: impl FnOnce() -> LRESULT) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or_else(|_| recover())
}

/// Hands back every key the hook holds down for a remapped modifier (a poisoned lock is read
/// anyway: this runs after a panic).
fn release_remapped() {
    if let Some(h) = HOOK.get() {
        let keys = locked(&h.remapper).flush();
        inject(&keys);
    }
}

unsafe fn hook_body(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        if let Some(h) = HOOK.get() {
            let kb = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
            let down = wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN;
            // System-wide hotkeys first: they act whatever window is in front, and a remap
            // must not rewrite the physical chord.
            if kb.dwExtraInfo != MAGIC {
                match global_hotkeys(kb, down) {
                    HookVerdict::Swallow { .. } => return 1,
                    // Windows delivers a registered chord; no remap may take it.
                    HookVerdict::ToSystem => return unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) },
                    HookVerdict::Continue => {}
                }
            }
            if kb.dwExtraInfo != MAGIC && remap(h, kb, down) {
                return 1;
            }
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

/// Installs the hook for Termurion's main window, on a thread of its own: a low-level hook
/// runs on the installing thread's message loop, and every keystroke on the machine waits
/// for it, so it must never wait for Termurion's drawing. `tags` are the Remap Modifiers
/// settings, and `exempt` says which keys are bound to Do Not Remap Modifiers.
pub fn install(
    hwnd: isize,
    tags: Arc<RwLock<Tags>>,
    exempt: Box<dyn Fn(u16, Mods) -> bool + Send + Sync>,
) -> Result<(), String> {
    {
        let mut w = locked(&WINDOWS);
        if !w.contains(&hwnd) {
            w.push(hwnd);
        }
    }
    let hook = Hook { remapper: Mutex::new(Remapper::default()), tags, exempt, exempting: Mutex::new(None) };
    if HOOK.set(hook).is_err() {
        return Ok(());
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("termurion-winkeys".into())
        .spawn(move || {
            let h = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), std::ptr::null_mut(), 0) };
            if h.is_null() {
                let _ = tx.send(Err(format!("SetWindowsHookExW failed: {}", std::io::Error::last_os_error())));
                return;
            }
            // However this thread ends (the unhook message, a failed GetMessageW, an unwind), the
            // hook goes first: a low-level hook with no thread pumping its messages makes every
            // keystroke on the machine wait for LowLevelHooksTimeout.
            let _unhook = OnEnd::new(move || {
                release_remapped();
                unsafe { UnhookWindowsHookEx(h) };
                HOOK_THREAD.store(0, Ordering::Release);
                UNHOOKED.store(true, Ordering::Release);
            });
            HOOK_THREAD.store(unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }, Ordering::Release);
            let _ = tx.send(Ok(()));
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // Registered hotkeys: (RegisterHotKey id, hotkey ids).
            let mut registered: Vec<(i32, Vec<u32>)> = Vec::new();
            let mut last: Vec<(u32, crate::hotkey::Chord)> = Vec::new();
            const WM_HOTKEY: u32 = 0x0312;
            while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
                // A panic in one message (a registration, a hotkey's sink) must not end the loop
                // while the hook is installed.
                let flow = pump_guarded(|| match msg.message {
                    WM_APP_HOTKEYS => {
                        apply_pending(&mut registered, &mut last);
                        Flow::Go
                    }
                    WM_APP_RECORDING => {
                        let _ = register(&mut registered, &last, RECORDING.load(Ordering::Relaxed));
                        Flow::Go
                    }
                    WM_HOTKEY => {
                        if let Some((_, ids)) = registered.iter().find(|(rid, _)| *rid as usize == msg.wParam) {
                            fire(ids);
                        }
                        Flow::Go
                    }
                    _ => Flow::Go,
                });
                // The unhook message always ends the loop, even when its hotkey cleanup panics (13u
                // minor): the hotkeys go here, then the guard releases held keys and the hook.
                let flow = if msg.message == WM_APP_UNHOOK {
                    let _ = pump_guarded(|| {
                        let _ = register(&mut registered, &last, true);
                        Flow::Go
                    });
                    Flow::Stop
                } else {
                    flow
                };
                if flow == Flow::Stop {
                    break;
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

type Global = HookHotkeys;

static GLOBAL: Mutex<Option<Global>> = Mutex::new(None);
static HOTKEY_SINK: OnceLock<Box<dyn Fn(Vec<u32>) + Send + Sync>> = OnceLock::new();
static HOOK_THREAD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
type Pending = (Vec<(u32, crate::hotkey::Chord)>, std::sync::mpsc::Sender<Vec<(u32, Result<(), String>)>>);
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
const WM_APP_HOTKEYS: u32 = 0x8000 + 0x48;
const WM_APP_RECORDING: u32 = 0x8000 + 0x49;
const WM_APP_UNHOOK: u32 = 0x8000 + 0x4A;
/// The hook thread has removed its hook and hotkeys.
static UNHOOKED: AtomicBool = AtomicBool::new(false);

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
    *locked(&GLOBAL) = Some(Global { taps: taps.into_iter().map(|(id, m)| (crate::doubletap::Machine::new(m, min, max), id)).collect(), paused, ..Default::default() });
    let (tx, rx) = std::sync::mpsc::channel();
    *locked(&PENDING) = Some((chords, tx));
    if unsafe { PostThreadMessageW(thread, WM_APP_HOTKEYS, 0, 0) } == 0 {
        return Err(format!("could not reach the keyboard hook: {}", std::io::Error::last_os_error()));
    }
    rx.recv_timeout(std::time::Duration::from_secs(2)).map_err(|_| "the keyboard hook did not answer".to_string())
}

/// On the hook thread: registers the pending chords (RegisterHotKey is per thread).
fn apply_pending(registered: &mut Vec<(i32, Vec<u32>)>, last: &mut Vec<(u32, crate::hotkey::Chord)>) {
    let Some((chords, reply)) = locked(&PENDING).take() else { return };
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
        // A poisoned lock (the app panicked holding it) must not stop the unhook's cleanup.
        if let Some(g) = locked(&GLOBAL).as_mut() {
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
    if let Some(g) = locked(&GLOBAL).as_mut() {
        g.hooked = hooked;
        g.system = system;
        g.paused = false;
    }
    out
}

/// The hook's part of system-wide hotkeys.
fn global_hotkeys(kb: &KBDLLHOOKSTRUCT, down: bool) -> HookVerdict {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::VkKeyScanW;
    let mut guard = locked(&GLOBAL);
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

    #[test]
    fn text_follows_remapped_mods() {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, MapVirtualKeyW};
        // The US layout (id "04090409") from the layouts already installed, or the one this thread
        // already has (a session without a desktop lists none); never loaded here.
        let n = unsafe { GetKeyboardLayoutList(0, std::ptr::null_mut()) };
        let mut list = vec![std::ptr::null_mut(); n.max(0) as usize];
        unsafe { GetKeyboardLayoutList(n, list.as_mut_ptr()) };
        list.push(unsafe { GetKeyboardLayout(0) });
        let Some(hkl) = list.iter().map(|h| *h as isize).find(|h| (*h as usize & 0xFFFF_FFFF) == 0x0409_0409) else {
            eprintln!("US keyboard layout not installed; skipped");
            return;
        };
        let scan = unsafe { MapVirtualKeyW(0x43, 0) };
        let none = Mods::default();
        assert_eq!(text_for_layout(hkl, 0x43, scan, none).as_deref(), Some("c"));
        assert_eq!(text_for_layout(hkl, 0x43, scan, Mods { ctrl: true, ..none }).as_deref(), Some("\u{3}"));
        assert_eq!(text_for_layout(hkl, 0x43, scan, Mods { shift: true, ..none }).as_deref(), Some("C"));
        // A key with no text types nothing.
        assert_eq!(text_for_layout(hkl, 0x10, 0x2A, none), None);
    }

    /// 13u seam (13x Task 1a fix, security): while keyboard focus is inside a web view of the
    /// foreground Termurion window, the terminal does not have the keyboard. The remapper passes every key
    /// on, so a page keeps its own keys.
    #[test]
    fn a_web_view_with_the_keyboard_is_not_the_terminal() {
        // One predicate: a Termurion window in front, and not the one a web view holds.
        assert!(terminal_has_keyboard(true, 10, 0), "no web focus");
        assert!(!terminal_has_keyboard(true, 10, 10), "the web view in front has it");
        assert!(terminal_has_keyboard(true, 10, 20), "another window's web view does not matter");
        assert!(!terminal_has_keyboard(false, 10, 0), "not a Termurion window");
        // The hook and the remapper both ask it; the global hotkeys run before it and ask nothing.
        let src = include_str!("winkeys.rs");
        let hook = &src[src.find("unsafe fn hook_body").unwrap()..];
        let hook = &hook[..hook.find("\n}\n").unwrap()];
        assert!(!hook.contains("is_termurion(fg)"), "no second foreground test beside it");
        assert!(hook.find("global_hotkeys(kb, down)").unwrap() < hook.find("remap(h, kb, down)").unwrap());
        let remap = &src[src.find("fn remap(h: &Hook").unwrap()..];
        assert!(remap[..remap.find("\n}\n").unwrap()].contains("termurion_has_keyboard(fg)"));
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
    fn a_poisoned_hook_lock_is_recovered() {
        let m = Mutex::new(5u8);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = m.lock().unwrap();
            panic!("poison it");
        }));
        assert!(m.is_poisoned());
        // Every later key press still reads it.
        assert_eq!(*locked(&m), 5);
        *locked(&m) = 6;
        assert_eq!(*locked(&m), 6);
    }

    #[test]
    fn a_hook_panic_passes_the_key_on_and_releases_remapped_keys() {
        // A body that completes returns its own answer and recovers nothing.
        let mut recovered = false;
        assert_eq!(guarded(|| 7, || { recovered = true; 9 }), 7);
        assert!(!recovered);
        // A panic in the body never unwinds out: the recovery's answer (the key passed on) stands.
        let released = std::cell::Cell::new(false);
        let r = guarded(|| panic!("forced panic in the hook body"), || { released.set(true); 9 });
        assert_eq!(r, 9);
        assert!(released.get(), "the held remapped keys are released");
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

    /// The chords the windows without menus take are fixed sets of Cmd chords (FIXES 13): the
    /// password manager's Cmd+B, U and C, the chat window's close, focus and find chords, and the
    /// Cockpit's Window > Cockpit.
    #[test]
    fn the_windows_without_menus_take_their_own_chords() {
        let c = |k: char, alt: bool, shift: bool| Chord { key: ChordKey::Char(k), cmd: true, alt, shift, ctrl: false };
        assert!(pwm_chord(&c('b', false, false)) && pwm_chord(&c('u', false, false)) && pwm_chord(&c('c', false, false)));
        assert!(!pwm_chord(&c('b', false, true)) && !pwm_chord(&c('x', false, false)));
        assert!(chat_chord(&c('w', false, false)) && chat_chord(&c('y', false, true)));
        assert!(!chat_chord(&c('w', false, true)) && !chat_chord(&c('y', false, false)) && !chat_chord(&c('x', false, false)));
        assert!(!chat_chord(&Chord { ctrl: true, ..c('w', false, false) }));
        assert!(cockpit_chord(&c('c', true, true)));
        assert!(!cockpit_chord(&c('c', false, true)) && !cockpit_chord(&c('c', true, false)) && !cockpit_chord(&c('x', true, true)));
        assert!(!cockpit_chord(&Chord { ctrl: true, ..c('c', true, true) }));
    }

    /// Leaves `m` poisoned, as a panic while holding it would.
    fn poison<T>(m: &Mutex<T>) {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = m.lock().unwrap_or_else(|e| e.into_inner());
            panic!("poison it");
        }));
        assert!(m.is_poisoned());
    }

    fn kb(vk: u32) -> KBDLLHOOKSTRUCT {
        KBDLLHOOKSTRUCT { vkCode: vk, scanCode: 0x1E, flags: 0, time: 0, dwExtraInfo: 0 }
    }

    #[test]
    fn every_poisoned_hook_thread_lock_still_registers_and_passes_keys() {
        // One test owns the module's statics, so no other test sees them poisoned.
        for m in [&WINDOWS] {
            poison(m);
        }
        poison(&GLOBAL);
        poison(&PENDING);
        // The window list still answers (a poisoned list read as empty would drop every remap).
        locked(&WINDOWS).push(94);
        assert!(is_termurion(94));
        remove_window(94);
        assert!(!is_termurion(94));
        // A key press reads the hotkeys through the poison (empty chords: nothing registers for real).
        *locked(&GLOBAL) = Some(Global::default());
        assert_eq!(global_hotkeys(&kb(0x41), true), HookVerdict::Continue);
        assert_eq!(global_hotkeys(&kb(0x41), false), HookVerdict::Continue);
        // Registration (a settings change) and its pause run on the message loop.
        let mut registered = Vec::new();
        assert!(register(&mut registered, &[], true).is_empty());
        assert!(locked(&GLOBAL).as_ref().is_some_and(|g| g.paused));
        assert!(register(&mut registered, &[], false).is_empty());
        assert!(locked(&GLOBAL).as_ref().is_some_and(|g| !g.paused));
        let (tx, rx) = std::sync::mpsc::channel();
        *locked(&PENDING) = Some((Vec::new(), tx));
        let mut last = Vec::new();
        apply_pending(&mut registered, &mut last);
        assert_eq!(rx.try_recv().ok(), Some(Vec::new()), "the settings side gets its answer");
        *locked(&GLOBAL) = None;
        for m in [&WINDOWS] {
            m.clear_poison();
        }
        GLOBAL.clear_poison();
        PENDING.clear_poison();
    }

    #[test]
    fn poisoned_remap_locks_still_pass_keys() {
        let tags = Arc::new(RwLock::new(Tags::identity()));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = tags.write().unwrap_or_else(|e| e.into_inner());
            panic!("poison it");
        }));
        assert!(tags.is_poisoned());
        let h = Hook {
            remapper: Mutex::new(Remapper::default()),
            tags,
            exempt: Box::new(|_, _| false),
            exempting: Mutex::new(None),
        };
        poison(&h.remapper);
        poison(&h.exempting);
        // Identity tags and a plain letter: nothing is replaced, so nothing is injected.
        assert!(!remap(&h, &kb(0x41), true));
        assert!(!remap(&h, &kb(0x41), false));
        assert_eq!(read_tags(&h.tags), Tags::identity());
    }

    #[test]
    fn the_hook_thread_never_unwraps_a_lock_and_never_outlives_its_pump() {
        let src = include_str!("winkeys.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        // Every lock is taken through `locked` or `read_tags`, which read through a poison.
        for bad in [".lock().unwrap()", ".read().unwrap()", ".write().unwrap()", ".lock().ok()", ".lock().map(", ".read().map", "if let Ok(mut w) = ", ".lock().is_ok_and"] {
            assert!(!code.contains(bad), "winkeys.rs takes a lock with {bad}");
        }
        assert_eq!(code.matches(".lock()").count(), 1, "only `locked` calls lock()");
        assert_eq!(code.matches(".read()").count(), 1, "only `read_tags` calls read()");
        // Each message-loop arm runs inside catch_unwind, and the thread's hook is removed when
        // the thread ends for any reason.
        let pump = code.split("fn install(").nth(1).expect("install");
        assert!(pump.contains("pump_guarded("), "the message loop dispatches through pump_guarded");
        assert!(pump.contains("OnEnd::new("), "the hook thread holds an unhook guard");
    }

    #[test]
    fn the_hook_closures_read_their_locks_through_a_poison() {
        // `exempt` and the hotkey sink run on the hook thread, inside every key.
        let src = include_str!("app/menu.rs");
        let start = src.find("These closures run on the hook thread").expect("the closures");
        let end = src.find("crate::winkeys::set_remap_global(").expect("set_remap_global");
        let closures = &src[start..end];
        assert!(closures.contains("set_hotkey_sink("));
        assert!(!closures.contains(".unwrap()"), "a hook closure unwraps a lock");
        assert!(!closures.contains(".expect("), "a hook closure can panic");
    }

    #[test]
    fn the_unhook_message_stops_the_loop_whatever_its_cleanup_does() {
        let src = include_str!("winkeys.rs").split("#[cfg(test)]").next().unwrap();
        let at = src.find("let flow = if msg.message == WM_APP_UNHOOK").expect("the unhook is decided outside the guarded arms");
        let arm = &src[at..at + 400];
        assert!(arm.contains("Flow::Stop") && arm.contains("pump_guarded("), "{arm}");
        assert!(!src[..at].contains("WM_APP_UNHOOK => {"), "no guarded arm can swallow the stop");
    }

    #[test]
    fn a_panicking_message_arm_keeps_the_loop_and_the_guard_unhooks_on_any_end() {
        assert_eq!(pump_guarded(|| Flow::Go), Flow::Go);
        assert_eq!(pump_guarded(|| Flow::Stop), Flow::Stop);
        assert_eq!(pump_guarded(|| panic!("forced panic in a message arm")), Flow::Go, "the loop goes on");
        let ran = std::cell::Cell::new(0);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _g = OnEnd::new(|| ran.set(ran.get() + 1));
            panic!("the thread unwinds");
        }));
        assert_eq!(ran.get(), 1, "the unhook runs when the thread unwinds");
        {
            let _g = OnEnd::new(|| ran.set(ran.get() + 1));
        }
        assert_eq!(ran.get(), 2, "and when the loop ends normally");
    }
}
