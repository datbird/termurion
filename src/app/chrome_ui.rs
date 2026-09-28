//! A window's border and corners through DWM (`chrome` decides them).

use super::*;
use crate::chrome;
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_WINDOW_CORNER_PREFERENCE, DwmGetWindowAttribute, DwmSetWindowAttribute};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;

/// Gives the window's frame a border color (a COLORREF, or `chrome::COLOR_DEFAULT`). Only
/// Windows 11 (build 22000 on) takes the attribute; Windows 10's refusal is ignored and the
/// window keeps the system's border.
pub(super) fn set_border(hwnd: isize, color: u32) {
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_BORDER_COLOR as u32, (&color as *const u32).cast(), 4) };
}

/// Sets how DWM rounds the window's corners (`chrome::corner_preference`).
pub(super) fn set_corners(hwnd: isize, preference: i32) {
    unsafe { DwmSetWindowAttribute(hwnd as _, DWMWA_WINDOW_CORNER_PREFERENCE as u32, (&preference as *const i32).cast(), 4) };
}

/// Takes the menu bar off the window (`SetMenu` with no menu); the window keeps its outer
/// size, so the client area grows by the bar and a Resized event lays the window out.
pub(super) fn hide_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    unsafe { menu.hide_for_hwnd(hwnd) }.map_err(|e| e.to_string())
}

/// Puts the menu bar back on the window.
pub(super) fn show_menu(menu: &muda::Menu, hwnd: isize) -> Result<(), String> {
    unsafe { menu.show_for_hwnd(hwnd) }.map_err(|e| e.to_string())
}

/// How far the outer frame reaches past the visible one at the top and the bottom: DWM's
/// invisible resize borders (0 without DWM's bounds).
fn invisible_top_bottom(hwnd: isize) -> (f32, f32) {
    let (mut outer, mut visible): (RECT, RECT) = unsafe { (std::mem::zeroed(), std::mem::zeroed()) };
    if unsafe { GetWindowRect(hwnd as _, &mut outer) } == 0 {
        return (0.0, 0.0);
    }
    if unsafe { DwmGetWindowAttribute(hwnd as _, DWMWA_EXTENDED_FRAME_BOUNDS as u32, (&mut visible as *mut RECT).cast(), std::mem::size_of::<RECT>() as u32) } != 0 {
        return (0.0, 0.0);
    }
    ((visible.top - outer.top).max(0) as f32, (outer.bottom - visible.bottom).max(0) as f32)
}

/// Moves and sizes a window to an outer frame; the inner size leaves the borders out.
fn set_outer_frame(w: &winit::window::Window, r: Rect) {
    let (outer, inner) = (w.outer_size(), w.inner_size());
    let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
    w.set_outer_position(winit::dpi::PhysicalPosition::new(r.x.round() as i32, r.y.round() as i32));
    let _ = w.request_inner_size(winit::dpi::PhysicalSize::new((r.w.round() as u32).saturating_sub(dw).max(1), (r.h.round() as u32).saturating_sub(dh).max(1)));
}

impl App {
    /// Window > Zoom (`PT:7055-7110`): a zoomed window goes back (a maximized one restores,
    /// a vertically zoomed one takes its earlier frame); otherwise `chrome::zoom_frame`
    /// decides: the whole work area maximizes as Windows does, a vertical zoom spans the work
    /// area's height and keeps the frame before it. Its shortcut is a key Zoom, where
    /// MaxVertically alone decides; a click on the menu item is not, and Shift held inverts
    /// MaxVertically. Tandem has no zoom button (the caption's maximize button stays Windows'
    /// own). Full screen has nothing to zoom.
    pub(super) fn zoom_window(&mut self) {
        if self.full_screen() {
            return;
        }
        let Some(g) = self.gpu.as_ref() else { return };
        let w = g.window.clone();
        let current = Self::outer_rect(g);
        if w.is_maximized() {
            self.zoom_restore = None;
            return w.set_maximized(false);
        }
        if let Some(before) = self.zoom_restore.take() {
            return set_outer_frame(&w, before);
        }
        let Some((work, _)) = self.monitor_rects() else { return };
        let shift = unsafe { GetKeyState(i32::from(VK_SHIFT)) } < 0;
        // A Zoom chosen while a key press is routed is a key Zoom; a menu click (or a pointer
        // action or the API) is not, and Shift then inverts MaxVertically (`PT:7065-7078`).
        let target = chrome::zoom_frame(current, work, self.globals.bool("MaxVertically"), self.key_down, shift);
        if target == work {
            return w.set_maximized(true);
        }
        // The visible frame fills the work area's height; DWM's invisible borders lie outside it.
        let (top, bottom) = self.hwnd().map_or((0.0, 0.0), invisible_top_bottom);
        self.zoom_restore = Some(current);
        set_outer_frame(&w, Rect { y: target.y - top, h: target.h + top + bottom, ..target });
    }

    /// Hides the current window's menu bar in full screen with `HideMenuBarInFullscreen`, and
    /// shows it again otherwise (`menu_should_hide`). Every way in or out of full screen draws
    /// a frame after it, so this runs from `render`; the setting's change reaches each window
    /// on its next frame.
    pub(super) fn sync_menu_hidden(&mut self) {
        let hide = menu_should_hide(self.full_screen(), self.globals.bool("HideMenuBarInFullscreen"));
        if hide == self.menu_hidden {
            return;
        }
        // Noted even when it fails, so a refused call is not retried every frame.
        self.menu_hidden = hide;
        let (Some(hwnd), Some(bar)) = (self.hwnd(), self.menu_bar.as_ref()) else { return };
        let done = if hide { hide_menu(&bar.menu, hwnd) } else { show_menu(&bar.menu, hwnd) };
        if let Err(e) = done {
            let what = if hide { "hide" } else { "show" };
            self.banners.set("menu", format!("Could not {what} the menu bar: {e}"));
        }
    }

    /// Whether a pane of the current window is see-through (`anyPaneIsTransparent`,
    /// `PT:9215-9225`: a session of the current tab drawn with alpha below 1). Tandem draws
    /// every pane opaque until 13g adds transparency, which replaces this body.
    pub(super) fn any_pane_is_transparent(&self) -> bool {
        false
    }

    /// Gives the current window the border `chrome::show_border` asks for, in its focused or
    /// unfocused color, or the system's border; DWM is told only when the color changes.
    pub(super) fn apply_border(&mut self) {
        let Some(hwnd) = self.hwnd() else { return };
        let g = &self.globals;
        let show = chrome::show_border(g.bool("UseBorder"), self.any_pane_is_transparent(), g.bool("BordersOnlyInLightMode"), chrome::is_dark_style(self.resolved_style()), self.full_screen());
        let color = if show { chrome::colorref(chrome::border_color(self.focused, &g.string("WindowBorderColor"), &g.string("WindowBorderColorUnfocused"))) } else { chrome::COLOR_DEFAULT };
        if self.border_applied == Some(color) {
            return;
        }
        set_border(hwnd, color);
        self.border_applied = Some(color);
    }

    /// Every window's border, after a setting changed.
    pub(super) fn apply_border_everywhere(&mut self) {
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if self.enter(id) {
                self.apply_border();
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }

    /// Square corners for a new window when `SquareWindowCorners` is on; iTerm2 reads the
    /// setting once (it needs a restart), and the default rounding needs no call.
    pub(super) fn apply_corners(&self) {
        if !self.globals.bool("SquareWindowCorners") {
            return;
        }
        if let Some(hwnd) = self.hwnd() {
            set_corners(hwnd, chrome::corner_preference(true));
        }
    }
}
