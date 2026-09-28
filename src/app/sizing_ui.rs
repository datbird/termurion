//! Resizing a window by its edges: the snap to the focused pane's cell grid while the user
//! drags (`chrome::snap`), live resize, through a subclass of each terminal window that
//! answers `WM_SIZING`. The subclass runs inside winit's window procedure, so it never
//! touches `App`: each frame publishes the window's grid to a thread-local map it reads.

use super::*;
use crate::chrome::{self, Snap};
use std::cell::RefCell;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, WMSZ_BOTTOM, WMSZ_BOTTOMLEFT, WMSZ_BOTTOMRIGHT, WMSZ_LEFT, WMSZ_RIGHT, WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT, WM_NCDESTROY, WM_SIZING,
};

const SUBCLASS_ID: usize = 0x7473_697A;

thread_local! {
    /// Each terminal window's grid, from its last frame; None while it does not snap.
    static GRIDS: RefCell<HashMap<isize, Option<Snap>>> = RefCell::new(HashMap::new());
}

/// Subclasses a new terminal window so edge drags snap.
pub(super) fn install(hwnd: isize) {
    unsafe { SetWindowSubclass(hwnd as HWND, Some(sizing), SUBCLASS_ID, 0) };
}

/// The window's grid for the next drag, or None when it keeps the size the user drags to.
pub(super) fn publish(hwnd: isize, snap: Option<Snap>) {
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

/// The monitor's work area size, which a snapped size may round up to fill.
fn work_size(hwnd: HWND) -> Option<(f32, f32)> {
    let mut info: MONITORINFO = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut info) } == 0 {
        return None;
    }
    let r = info.rcWork;
    Some(((r.right - r.left) as f32, (r.bottom - r.top) as f32))
}

unsafe extern "system" fn sizing(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
    match msg {
        WM_SIZING => {
            let answer = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            let Some(grid) = GRIDS.with(|g| g.borrow().get(&(hwnd as isize)).copied().flatten()) else { return answer };
            let edge = wparam as u32;
            let left = matches!(edge, WMSZ_LEFT | WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT);
            let right = matches!(edge, WMSZ_RIGHT | WMSZ_TOPRIGHT | WMSZ_BOTTOMRIGHT);
            let top = matches!(edge, WMSZ_TOP | WMSZ_TOPLEFT | WMSZ_TOPRIGHT);
            let bottom = matches!(edge, WMSZ_BOTTOM | WMSZ_BOTTOMLEFT | WMSZ_BOTTOMRIGHT);
            let mut now: RECT = unsafe { std::mem::zeroed() };
            if unsafe { GetWindowRect(hwnd, &mut now) } == 0 {
                return answer;
            }
            let rect = unsafe { &mut *(lparam as *mut RECT) };
            let current = ((now.right - now.left) as f32, (now.bottom - now.top) as f32);
            let proposed = ((rect.right - rect.left) as f32, (rect.bottom - rect.top) as f32);
            // Only the moving edges change: an edge a window is attached to stays put, which
            // covers iTerm2's percentage-window rule as well.
            let ctrl = ctrl_only();
            let grid = Snap { snap_w: grid.snap_w && !ctrl && (left || right), snap_h: grid.snap_h && !ctrl && (top || bottom), ..grid };
            let (mut w, mut h) = chrome::snap(&grid, current, proposed);
            if let Some((sw, sh)) = work_size(hwnd) {
                w = chrome::fill_screen(grid.snap_w, w, proposed.0, grid.cell.0, sw);
                h = chrome::fill_screen(grid.snap_h, h, proposed.1, grid.cell.1, sh);
            }
            let (w, h) = (w.round() as i32, h.round() as i32);
            if left {
                rect.left = rect.right - w;
            } else if right {
                rect.right = rect.left + w;
            }
            if top {
                rect.top = rect.bottom - h;
            } else if bottom {
                rect.bottom = rect.top + h;
            }
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

impl App {
    /// The current window's grid for `sizing_ui` (`PT:5784-5935`): None when a drag keeps
    /// its size (maximized, full screen or size-locked, where iTerm2 returns before the
    /// snap); else the focused pane's cell, the decoration around the tab's pane area, the
    /// padding, the smallest pane area and the axes `chrome::snap_axes` allows.
    pub(super) fn snap_grid(&self) -> Option<Snap> {
        let g = self.gpu.as_ref()?;
        let (maximized, full_screen, locked) = (g.window.is_maximized(), self.full_screen(), self.win_locks.size);
        if maximized || full_screen || locked || matches!(self.win_style, crate::frames::MAXIMIZED | crate::frames::COMPACT_MAXIMIZED) {
            return None;
        }
        let (snap_w, snap_h) = chrome::snap_axes(self.win_style, self.globals.bool("DisableWindowSizeSnap"), false, maximized, full_screen, locked, true, true);
        let cell = self.cell();
        let outer = g.window.outer_size();
        let area = self.content_area();
        let pad = 2.0 * PAD * self.scale();
        Some(Snap {
            cell,
            decoration: (outer.width as f32 - area.w, outer.height as f32 - area.h),
            margins: (pad, pad),
            // `grid_size`'s floor: two columns and one row.
            min: (2.0 * cell.0 + pad, cell.1 + pad),
            snap_w,
            snap_h,
        })
    }

    /// Publishes the current window's grid for its next edge drag.
    pub(super) fn publish_snap(&self) {
        if let Some(hwnd) = self.hwnd() {
            publish(hwnd, self.snap_grid());
        }
    }

    /// Lets the user drag the current window's edges unless its size is locked or
    /// `AllowLiveResize` is off (`chrome::resizable`).
    pub(super) fn apply_resizable(&self) {
        if let Some(g) = &self.gpu {
            g.window.set_resizable(chrome::resizable(self.win_locks.size, self.globals.bool("AllowLiveResize")));
        }
    }

    /// Every window's edges, after a setting changed.
    pub(super) fn apply_resizable_everywhere(&mut self) {
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if self.enter(id) {
                self.apply_resizable();
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }
}
