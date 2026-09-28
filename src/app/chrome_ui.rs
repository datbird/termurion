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

impl App {
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
