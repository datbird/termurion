//! Resizing a window by its edges: the snap to the focused pane's cell grid while the user
//! drags (`chrome::snap`), through a subclass that answers `WM_SIZING`. The subclass runs inside
//! winit's window procedure, so it never touches `App`: each frame publishes the window's grid to a
//! thread-local map it reads.

use std::cell::RefCell;
use std::collections::HashMap;

use super::super::sizing::Grid;
use crate::chrome::{self, Snap};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, WMSZ_BOTTOMLEFT, WMSZ_LEFT, WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT, WM_NCDESTROY, WM_SIZING,
};

const SUBCLASS_ID: usize = 0x7473_697A;

thread_local! {
    /// Each terminal window's grid, from its last frame; None while it does not snap.
    static GRIDS: RefCell<HashMap<isize, Option<Grid>>> = RefCell::new(HashMap::new());
}

/// Subclasses a new terminal window so edge drags snap.
pub fn install(hwnd: isize) {
    unsafe { SetWindowSubclass(hwnd as HWND, Some(sizing), SUBCLASS_ID, 0) };
}

/// The window's grid for the next drag, or None when it keeps the size the user drags to.
pub fn publish(hwnd: isize, snap: Option<Grid>) {
    GRIDS.with(|g| g.borrow_mut().insert(hwnd, snap));
}

fn down(key: u16) -> bool {
    let state = unsafe { GetKeyState(i32::from(key)) };
    state < 0
}

/// Ctrl, and only Ctrl, is held (iTerm2's Control without Option, Command or Shift).
fn ctrl_only() -> bool {
    down(VK_CONTROL) && !down(VK_MENU) && !down(VK_SHIFT) && !down(VK_LWIN) && !down(VK_RWIN)
}

/// The monitor's work area size, which a snapped size may round up to fill, in the outer
/// frame's pixels (`chrome::outer_screen`): `outer` is the window's `GetWindowRect` size, and
/// DWM's visible frame leaves out the invisible borders.
fn work_size(hwnd: HWND, outer: (f32, f32)) -> Option<(f32, f32)> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) } == 0 {
        return None;
    }
    let r = info.rcWork;
    let work = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
    let mut v: RECT = unsafe { std::mem::zeroed() };
    let got = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&mut v as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32) };
    // Without DWM's bounds the window has no invisible borders to allow for.
    let visible = if got == 0 { ((v.right - v.left) as f32, (v.bottom - v.top) as f32) } else { outer };
    Some(chrome::outer_screen(work, outer, visible))
}

unsafe extern "system" fn sizing(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_SIZING => {
            let answer = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            let Some(published) = GRIDS.with(|g| g.borrow().get(&(hwnd as isize)).copied().flatten()) else { return answer };
            let edge = wparam as u32;
            let left = matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT);
            let top = matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT);
            let mut now: RECT = unsafe { std::mem::zeroed() };
            if unsafe { GetWindowRect(hwnd, &mut now) } == 0 {
                return answer;
            }
            let rect = unsafe { &mut *(lparam as *mut RECT) };
            let current = ((now.right - now.left) as f32, (now.bottom - now.top) as f32);
            let proposed = ((rect.right - rect.left) as f32, (rect.bottom - rect.top) as f32);
            // Both axes snap, the one the drag does not move too (`PT:5826-5835`), except
            // while Ctrl alone is held and for a percentage window's free axis
            // (`chrome::snap_axes`, `PT:5837-5850`).
            let (snap_w, snap_h) = chrome::snap_axes(published.style, published.disabled, ctrl_only(), false, false, false, proposed.0 != current.0, proposed.1 != current.1);
            let grid = Snap { snap_w, snap_h, ..published.snap };
            let (mut w, mut h) = chrome::snap(&grid, current, proposed);
            if let Some((sw, sh)) = work_size(hwnd, current) {
                w = chrome::fill_screen(grid.snap_w, w, proposed.0, grid.cell.0, sw);
                h = chrome::fill_screen(grid.snap_h, h, proposed.1, grid.cell.1, sh);
            }
            let (l, t, r, b) = chrome::sized_rect((rect.left, rect.top, rect.right, rect.bottom), left, top, (w.round() as i32, h.round() as i32));
            (rect.left, rect.top, rect.right, rect.bottom) = (l, t, r, b);
            1
        }
        WM_NCDESTROY => {
            GRIDS.with(|g| g.borrow_mut().remove(&(hwnd as isize)));
            unsafe { RemoveWindowSubclass(hwnd, Some(sizing), SUBCLASS_ID) };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

