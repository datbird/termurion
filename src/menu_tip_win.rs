//! Menu tips on Windows (`MainMenu/MenuItemTipController.swift`): a subclass on the window sees
//! the menu loop's `WM_MENUSELECT` (the highlighted item), `WM_TIMER` (the delay) and
//! `WM_EXITMENULOOP`, and shows `menu_tips`' text in one tracking tooltip beside the item. It
//! runs inside the menu's modal loop on the window thread and never touches `App`: the delay is a
//! process static `install` sets. Subclass id 0x7469_7073 ("tips").
//!
//! The item is found by its shown label and its submenu's label, read from the live menu bar
//! (`GetMenu`, walked each highlight), so a rebuilt dynamic menu needs no registry.

use std::sync::Mutex;
use std::time::Duration;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::Controls::{
    InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, TTTOOLINFOW, TTF_ABSOLUTE, TTF_TRACK, TTM_ADDTOOLW, TTM_GETBUBBLESIZE, TTM_SETMAXTIPWIDTH, TTM_TRACKACTIVATE, TTM_TRACKPOSITION, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, GetMenu, GetMenuItemCount, GetMenuItemID, GetMenuItemRect, GetMenuStringW, GetSubMenu, KillTimer, SendMessageW, SetTimer, MF_BYPOSITION, MF_POPUP, MF_SEPARATOR, WM_EXITMENULOOP, WM_MENUSELECT, WM_NCDESTROY, WM_TIMER, WS_EX_NOACTIVATE,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::menu_tips::{self, Step};

const SUBCLASS_ID: usize = 0x7469_7073;
const TIMER_ID: usize = 0x7469_7001;
/// Tip width cap, in pixels.
const MAX_WIDTH: isize = 250;

/// The one tip window of the process and what the delay timer will show.
struct State {
    delay: Duration,
    /// The tooltip window and the window that owns it.
    tip: Option<(isize, isize)>,
    showing: bool,
    /// The item waiting for the delay: (menu, position, text).
    pending: Option<(isize, u32, &'static str)>,
}

static STATE: Mutex<State> = Mutex::new(State { delay: Duration::from_secs(1), tip: None, showing: false, pending: None });

/// A highlighted item with a tip: its menu, position and text.
#[derive(Debug, PartialEq, Eq)]
pub struct Hit {
    pub menu: isize,
    pub pos: u32,
    pub text: &'static str,
}

/// Starts the tips on a window, once; `delay` is `MenuTipDelay`.
pub fn install(hwnd: isize, delay: Duration) {
    if let Ok(mut s) = STATE.lock() {
        s.delay = delay;
    }
    // SAFETY: the HWND is a live window of this process; the proc only reads menus and forwards.
    unsafe { SetWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID, 0) };
}

/// Stops the tips on a window and removes its subclass.
pub fn uninstall(hwnd: isize) {
    hide(hwnd);
    // SAFETY: removing this proc from a window of this process (a no-op if it is not there).
    unsafe { RemoveWindowSubclass(hwnd as HWND, Some(proc), SUBCLASS_ID) };
}

fn menu_string(menu: isize, pos: u32) -> String {
    let mut buf = [0u16; 256];
    // SAFETY: the buffer is 256 units and the length passed is its size.
    let n = unsafe { GetMenuStringW(menu as _, pos, buf.as_mut_ptr(), buf.len() as i32, MF_BYPOSITION) };
    menu_tips::shown_from_menu_text(&String::from_utf16_lossy(&buf[..n.max(0) as usize]))
}

/// The shown label of the submenu `target` in the tree under `menu` ("" for the bar itself).
fn label_of(menu: isize, target: isize, depth: u32) -> Option<String> {
    if menu == target {
        return Some(String::new());
    }
    if depth > 6 {
        return None;
    }
    // SAFETY: plain queries on menu handles of this process.
    let count = unsafe { GetMenuItemCount(menu as _) };
    for i in 0..count.max(0) as u32 {
        let sub = unsafe { GetSubMenu(menu as _, i as i32) } as isize;
        if sub == 0 {
            continue;
        }
        if sub == target {
            return Some(menu_string(menu, i));
        }
        if let Some(l) = label_of(sub, target, depth + 1) {
            return Some(l);
        }
    }
    None
}

/// The tip for a `WM_MENUSELECT` (`wparam`, and the menu in `lparam`), if the highlighted item has one.
pub fn resolve(hwnd: isize, wparam: usize, menu: isize) -> Option<Hit> {
    let (low, flags) = ((wparam & 0xFFFF) as u32, ((wparam >> 16) & 0xFFFF) as u32);
    if menu == 0 || flags & MF_SEPARATOR != 0 {
        return None;
    }
    let pos = if flags & MF_POPUP != 0 {
        low
    } else {
        // SAFETY: plain queries on this menu.
        let count = unsafe { GetMenuItemCount(menu as _) }.max(0) as u32;
        (0..count).find(|i| unsafe { GetMenuItemID(menu as _, *i as i32) } == low)?
    };
    // SAFETY: a plain query.
    let root = unsafe { GetMenu(hwnd as HWND) } as isize;
    let parent = label_of(root, menu, 0)?;
    let id = menu_tips::identifier_for(&menu_string(menu, pos), &parent)?;
    Some(Hit { menu, pos, text: menu_tips::tip_for(&id)? })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// The process's tooltip window, made for `owner` (re-made when another window owns it).
fn tooltip(state: &mut State, owner: isize) -> Option<isize> {
    if let Some((tip, o)) = state.tip {
        if o == owner {
            return Some(tip);
        }
        // SAFETY: destroying a window this module made, on the thread that made it.
        unsafe { DestroyWindow(tip as HWND) };
        state.tip = None;
    }
    let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_WIN95_CLASSES };
    // SAFETY: the struct is valid for the call; the window is a popup owned by `owner`, never activated.
    let tip = unsafe {
        InitCommonControlsEx(&icc);
        let class = wide("tooltips_class32");
        CreateWindowExW(WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT, class.as_ptr(), std::ptr::null(), WS_POPUP | TTS_NOPREFIX | TTS_ALWAYSTIP, 0, 0, 0, 0, owner as HWND, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null())
    };
    if tip.is_null() {
        return None;
    }
    let mut info = toolinfo(owner, std::ptr::null_mut());
    // SAFETY: a valid TTTOOLINFOW; the tooltip copies what it needs.
    unsafe {
        SendMessageW(tip, TTM_ADDTOOLW, 0, &mut info as *mut _ as LPARAM);
        SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, MAX_WIDTH);
    }
    state.tip = Some((tip as isize, owner));
    Some(tip as isize)
}

fn toolinfo(owner: isize, text: *mut u16) -> TTTOOLINFOW {
    // SAFETY: TTTOOLINFOW is plain data; all-zero is a valid empty value.
    let mut info: TTTOOLINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<TTTOOLINFOW>() as u32;
    info.uFlags = TTF_TRACK | TTF_ABSOLUTE;
    info.hwnd = owner as HWND;
    info.uId = 1;
    info.lpszText = text;
    info
}

/// The window `GetMenuItemRect` takes for `menu`: the window for its menu bar, null for a popup
/// (a submenu), which the system finds itself; with the owner window the popup's item rect is
/// taken from the wrong window.
fn rect_owner(hwnd: isize, menu: isize) -> HWND {
    // SAFETY: a plain query on this thread's window.
    if unsafe { GetMenu(hwnd as HWND) } as isize == menu {
        hwnd as HWND
    } else {
        std::ptr::null_mut()
    }
}

/// Shows the tip for `hit` beside its item.
fn show(hwnd: isize, hit: &Hit) {
    let Ok(mut state) = STATE.lock() else { return };
    let Some(tip) = tooltip(&mut state, hwnd) else { return };
    let mut item = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    // SAFETY: a plain query with a valid rect.
    if unsafe { GetMenuItemRect(rect_owner(hwnd, hit.menu), hit.menu as _, hit.pos, &mut item) } == 0 || item.right <= item.left {
        return;
    }
    let mut text = wide(hit.text);
    let mut info = toolinfo(hwnd, text.as_mut_ptr());
    // SAFETY: valid structs and a live tooltip window of this thread; the text outlives the calls.
    unsafe {
        SendMessageW(tip as HWND, TTM_UPDATETIPTEXTW, 0, &mut info as *mut _ as LPARAM);
        let bubble = SendMessageW(tip as HWND, TTM_GETBUBBLESIZE, 0, &mut info as *mut _ as LPARAM) as u32;
        let size = ((bubble & 0xFFFF) as i32, (bubble >> 16) as i32);
        let monitor = MonitorFromPoint(windows_sys::Win32::Foundation::POINT { x: item.right, y: item.top }, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let work = if GetMonitorInfoW(monitor, &mut mi) != 0 { (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom) } else { (0, 0, i32::MAX / 2, i32::MAX / 2) };
        let (x, y) = menu_tips::tip_origin((item.left, item.top, item.right, item.bottom), size, work);
        SendMessageW(tip as HWND, TTM_TRACKPOSITION, 0, ((y as u32 as isize) << 16) | (x as u32 as isize & 0xFFFF));
        SendMessageW(tip as HWND, TTM_TRACKACTIVATE, 1, &mut info as *mut _ as LPARAM);
    }
    state.showing = true;
    drop(text);
}

/// Hides the tip and cancels the delay.
fn hide(hwnd: isize) {
    // SAFETY: cancelling this module's timer on the window.
    unsafe { KillTimer(hwnd as HWND, TIMER_ID) };
    let Ok(mut state) = STATE.lock() else { return };
    state.pending = None;
    if let (true, Some((tip, _))) = (state.showing, state.tip) {
        let mut info = toolinfo(hwnd, std::ptr::null_mut());
        // SAFETY: a valid TTTOOLINFOW and the live tooltip window.
        unsafe { SendMessageW(tip as HWND, TTM_TRACKACTIVATE, 0, &mut info as *mut _ as LPARAM) };
    }
    state.showing = false;
}

fn highlighted(hwnd: isize, wparam: usize, lparam: isize) {
    let hit = resolve(hwnd, wparam, lparam);
    let (showing, delay) = STATE.lock().map(|s| (s.showing, s.delay)).unwrap_or((false, Duration::from_secs(1)));
    match menu_tips::on_highlight(showing, hit.is_some()) {
        Step::Hide => hide(hwnd),
        Step::ShowNow => {
            if let Some(hit) = hit {
                show(hwnd, &hit);
            }
        }
        Step::Wait => {
            hide(hwnd);
            if let (Some(hit), Ok(mut s)) = (hit, STATE.lock()) {
                s.pending = Some((hit.menu, hit.pos, hit.text));
                // A new timer on the same id restarts the delay.
                // SAFETY: a timer on this window, handled by the proc below.
                unsafe { SetTimer(hwnd as HWND, TIMER_ID, delay.as_millis().clamp(1, u32::MAX as u128 - 1) as u32, None) };
            }
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    // A panic must never leave the menu loop (it would abort the process): the bodies run guarded.
    let handled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| proc_body(hwnd, msg, wparam, lparam)));
    if let Ok(Some(r)) = handled {
        return r;
    }
    // SAFETY: forwarding the message the system delivered, unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// The messages this subclass acts on; `Some` when it answers one itself.
fn proc_body(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let h = hwnd as isize;
    match msg {
        WM_MENUSELECT => highlighted(h, wparam, lparam),
        WM_TIMER if wparam == TIMER_ID => {
            // SAFETY: cancelling this module's one-shot timer.
            unsafe { KillTimer(hwnd, TIMER_ID) };
            let pending = STATE.lock().ok().and_then(|mut s| s.pending.take());
            if let Some((menu, pos, text)) = pending {
                show(h, &Hit { menu, pos, text });
            }
            return Some(0);
        }
        WM_EXITMENULOOP => hide(h),
        WM_NCDESTROY => {
            hide(h);
            if let Ok(mut s) = STATE.lock() {
                if let Some((tip, owner)) = s.tip.filter(|(_, o)| *o == h) {
                    let _ = owner;
                    // SAFETY: the tip is a child popup of the window being destroyed.
                    unsafe { DestroyWindow(tip as HWND) };
                    s.tip = None;
                }
            }
            // SAFETY: removing this proc from the window being destroyed.
            unsafe { RemoveWindowSubclass(hwnd, Some(proc), SUBCLASS_ID) };
        }
        _ => {}
    }
    None
}

#[cfg(test)]
mod tests {
    /// Final review 10: the proc runs inside the menu loop, so a panic never leaves it.
    #[test]
    fn the_proc_has_a_panic_guard() {
        let src = include_str!("menu_tip_win.rs");
        let at = src.find("unsafe extern \"system\" fn proc(").unwrap();
        assert!(src[at..at + src[at..].find("\n}\n").unwrap()].contains("catch_unwind"));
    }

    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{AppendMenuW, CreateMenu, CreatePopupMenu, SetMenu, MF_STRING};

    /// A hidden top-level window with a menu bar: Shell > the four split items and a separator.
    fn window_with_menu() -> (HWND, isize, isize) {
        let class = wide("STATIC");
        // SAFETY: a hidden window of the system's STATIC class on this thread, with menus this test owns.
        unsafe {
            let w = CreateWindowExW(0, class.as_ptr(), std::ptr::null(), 0, 0, 0, 10, 10, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            assert!(!w.is_null());
            let bar = CreateMenu();
            let shell = CreatePopupMenu();
            let other = CreatePopupMenu();
            AppendMenuW(shell, MF_STRING, 101, wide("Split Vertically with Current Profile\tCtrl+D").as_ptr());
            AppendMenuW(shell, MF_STRING, 102, wide("\u{2502}\u{20de} Split Vertically...").as_ptr());
            AppendMenuW(shell, MF_STRING, 103, wide("No Tip Here").as_ptr());
            AppendMenuW(other, MF_STRING, 201, wide("Split Vertically with Current Profile").as_ptr());
            AppendMenuW(bar, MF_POPUP | MF_STRING, shell as usize, wide("Shell").as_ptr());
            AppendMenuW(bar, MF_POPUP | MF_STRING, other as usize, wide("Edit").as_ptr());
            SetMenu(w, bar);
            (w, shell as isize, other as isize)
        }
    }

    /// m13v2 Task 7 fix: a popup submenu's item rect is asked with a null owner, the bar's with the window.
    #[test]
    fn item_rects_name_the_owner_the_docs_say() {
        let (w, shell, _) = window_with_menu();
        // SAFETY: the window is this thread's.
        let bar = unsafe { GetMenu(w) } as isize;
        assert_eq!(rect_owner(w as isize, bar), w, "the menu bar belongs to the window");
        assert!(rect_owner(w as isize, shell).is_null(), "a popup submenu has no owner window");
        // SAFETY: the window is this thread's.
        unsafe { DestroyWindow(w) };
    }

    #[test]
    fn highlight_finds_the_tip_by_label_and_parent() {
        let (w, shell, other) = window_with_menu();
        let hit = resolve(w as isize, 101, shell).expect("a tip");
        assert_eq!((hit.menu, hit.pos), (shell, 0));
        assert_eq!(hit.text, menu_tips::tip_for("Split Vertically with Current Profile").unwrap());
        // The hint prefix and the chord are not part of the label.
        assert_eq!(resolve(w as isize, 102, shell).map(|h| h.pos), Some(1));
        assert!(resolve(w as isize, 103, shell).is_none(), "an item with no tip");
        assert!(resolve(w as isize, 201, other).is_none(), "the same label under another menu has none");
        assert!(resolve(w as isize, 101, 0).is_none(), "a closed menu");
        // SAFETY: the window is this thread's.
        unsafe { DestroyWindow(w) };
    }

    #[test]
    fn install_and_exit_are_safe() {
        let (w, _, _) = window_with_menu();
        install(w as isize, Duration::from_millis(5));
        // SAFETY: the window is this thread's; the proc runs inside SendMessageW.
        unsafe { SendMessageW(w, WM_EXITMENULOOP, 0, 0) };
        uninstall(w as isize);
        unsafe { DestroyWindow(w) };
    }
}
