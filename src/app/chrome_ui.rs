//! A window's border and corners through DWM (`chrome` decides them).

use super::*;
use crate::chrome;
use windows_sys::Win32::Graphics::Dwm::{DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DwmSetWindowAttribute};

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

impl App {
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
