//! Opening a menu above a point: while a `Lift` lives, the first menu window this thread
//! creates (class `#32768`) is subclassed, and each move it gets puts its bottom on the
//! point (never above the top of the monitor's work area). Submenus open where Windows
//! puts them.

use std::cell::Cell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetClassNameW, GetWindowRect, SetWindowsHookExW, UnhookWindowsHookEx, HCBT_CREATEWND, HHOOK, SWP_NOMOVE, SWP_NOSIZE, WH_CBT, WINDOWPOS, WM_NCDESTROY,
    WM_WINDOWPOSCHANGING,
};

const SUBCLASS_ID: usize = 0x7462_6D75;

thread_local! {
    /// The screen y the menu's bottom goes to, while a `Lift` lives.
    static BOTTOM: Cell<Option<i32>> = const { Cell::new(None) };
    /// The menu window being lifted, once created.
    static MENU: Cell<Option<isize>> = const { Cell::new(None) };
}

pub struct Lift {
    hook: HHOOK,
}

impl Lift {
    /// `x`, `y`: the point in `hwnd`'s client pixels.
    pub fn new(hwnd: isize, x: f32, y: f32) -> Lift {
        let mut pt = POINT { x: x as i32, y: y as i32 };
        unsafe { ClientToScreen(hwnd as HWND, &mut pt) };
        BOTTOM.with(|b| b.set(Some(pt.y)));
        MENU.with(|m| m.set(None));
        let hook = unsafe { SetWindowsHookExW(WH_CBT, Some(cbt), std::ptr::null_mut(), GetCurrentThreadId()) };
        Lift { hook }
    }
}

impl Drop for Lift {
    fn drop(&mut self) {
        if !self.hook.is_null() {
            unsafe { UnhookWindowsHookEx(self.hook) };
        }
        BOTTOM.with(|b| b.set(None));
        MENU.with(|m| m.set(None));
    }
}

unsafe extern "system" fn cbt(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND as i32 && BOTTOM.with(Cell::get).is_some() && MENU.with(Cell::get).is_none() {
        let hwnd = wparam as HWND;
        let mut name = [0u16; 16];
        let n = unsafe { GetClassNameW(hwnd, name.as_mut_ptr(), name.len() as i32) };
        if name[..n.max(0) as usize] == *"#32768".encode_utf16().collect::<Vec<u16>>() {
            MENU.with(|m| m.set(Some(hwnd as isize)));
            unsafe { SetWindowSubclass(hwnd, Some(place), SUBCLASS_ID, 0) };
        }
    }
    unsafe { CallNextHookEx(std::ptr::null_mut(), code, wparam, lparam) }
}

unsafe extern "system" fn place(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_WINDOWPOSCHANGING {
        let wp = unsafe { &mut *(lparam as *mut WINDOWPOS) };
        if let Some(bottom) = BOTTOM.with(Cell::get).filter(|_| wp.flags & SWP_NOMOVE == 0) {
            let cy = if wp.flags & SWP_NOSIZE == 0 {
                wp.cy
            } else {
                let mut r: RECT = unsafe { std::mem::zeroed() };
                unsafe { GetWindowRect(hwnd, &mut r) };
                r.bottom - r.top
            };
            let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            let top = if unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) } != 0 { info.rcWork.top } else { i32::MIN };
            wp.y = (bottom - cy).max(top);
        }
    } else if msg == WM_NCDESTROY {
        unsafe { RemoveWindowSubclass(hwnd, Some(place), SUBCLASS_ID) };
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
