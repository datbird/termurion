//! A window's border and corners through DWM (`chrome` decides them).

//! A window's border, corners, menu bar and Theme (`chrome` decides them); the Win32 calls are in
//! `platform::chrome`.

use super::*;
use crate::chrome;
use crate::platform::chrome::{apply_menu_theme as platform_menu_theme, guard_theme, hide_menu, invisible_top_bottom, menu_attached, set_border, set_corners, shift_down, show_menu, theme_reset_handled};

/// Whether the window's menu bar has to change to be as wanted: a bar that should be hidden but
/// is attached, or one that should show but is not.
pub(super) fn menu_needs_change(hide: bool, attached: bool) -> bool {
    hide == attached
}

/// Moves and sizes a window to an outer frame; the inner size leaves the borders out.
pub(super) fn set_outer_frame(w: &winit::window::Window, r: Rect) {
    let (outer, inner) = (w.outer_size(), w.inner_size());
    let (dw, dh) = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
    w.set_outer_position(winit::dpi::PhysicalPosition::new(r.x.round() as i32, r.y.round() as i32));
    let _ = crate::platform::caption::request_inner_size(w, winit::dpi::PhysicalSize::new((r.w.round() as u32).saturating_sub(dw).max(1), (r.h.round() as u32).saturating_sub(dh).max(1)));
}

impl App {
    /// Window > Zoom (`PT:7055-7110`): a zoomed window goes back (a maximized one restores,
    /// a vertically zoomed one takes its earlier frame); otherwise `chrome::zoom_frame`
    /// decides: the whole work area maximizes as Windows does, a vertical zoom spans the work
    /// area's height and keeps the frame before it. Its shortcut is a key Zoom, where
    /// MaxVertically alone decides; a click on the menu item is not, and Shift held inverts
    /// MaxVertically. Termurion has no zoom button (the caption's maximize button stays Windows'
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
        let shift = shift_down();
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

    /// Whether the current window's menu bar is to be hidden: in full screen with
    /// `HideMenuBarInFullscreen` (`menu_should_hide`), and always in a Minimal or Compact window,
    /// which has no room for the native bar: its caption strip is where the title bar and menu
    /// bar were (`caption_ui`).
    pub(super) fn want_menu_hidden(&self) -> bool {
        (menu_should_hide(self.full_screen(), self.globals.bool("HideMenuBarInFullscreen")) && !self.menu_peek.shown()) || self.compact_window()
    }

    /// Whether the bar is hidden because of full screen, so Alt or the top edge can bring it back.
    fn full_screen_hides_menu(&self) -> bool {
        menu_should_hide(self.full_screen(), self.globals.bool("HideMenuBarInFullscreen")) && !self.compact_window()
    }

    /// Alt pressed and released alone brings the menu bar back in full screen, and a second tap
    /// sends it away (`menu_peek`): without it a person who cannot press the full screen chord has
    /// no way out but the mouse, and a hidden bar has no mouse way.
    pub(super) fn menu_peek_key(&mut self, ev: &KeyEvent) {
        if !self.full_screen_hides_menu() && !self.menu_peek.shown() {
            self.menu_peek.clear();
            return;
        }
        let is_alt = matches!(ev.logical_key, winit::keyboard::Key::Named(winit::keyboard::NamedKey::Alt));
        if self.menu_peek.key(is_alt, ev.state == ElementState::Pressed, ev.repeat) {
            self.menu_peek_changed();
        }
    }

    /// The pointer moved: at the very top edge of a full screen window the bar comes back, and it
    /// goes when the pointer is back in the window. Never during a drag (a selection dragged to the
    /// top edge scrolls, it does not open a menu).
    pub(super) fn menu_peek_pointer(&mut self, y: f32) {
        let can_peek = self.full_screen_hides_menu() && matches!(self.drag, Drag::None);
        if self.menu_peek.pointer(y, can_peek) {
            self.menu_peek_changed();
        }
    }

    fn menu_peek_changed(&mut self) {
        self.sync_menu_hidden();
        self.relayout();
        self.redraw();
    }

    /// Makes the current window's menu bar as `want_menu_hidden` says, by what the window has
    /// now. Every way in or out of full screen draws a frame after it, so this runs from
    /// `render`, and from a resize, where a bar attached behind the app's back would otherwise be
    /// painted over the client (muda draws the dark bar on the window's whole DC). A bar that is
    /// built or rebuilt is hidden as it is attached (`menu::replace_menu_bar`).
    pub(super) fn sync_menu_hidden(&mut self) {
        // Leaving full screen ends any peek at the bar.
        if self.menu_peek.shown() && !self.full_screen() {
            self.menu_peek.clear();
        }
        let hide = self.want_menu_hidden();
        let (Some(hwnd), Some(bar)) = (self.hwnd(), self.menu_bar.as_ref()) else { return };
        if !menu_needs_change(hide, menu_attached(hwnd)) || self.menu_refused == Some(hide) {
            return;
        }
        let done = if hide { hide_menu(&bar.menu, hwnd) } else { show_menu(&bar.menu, hwnd) };
        // Noted when it fails, so a refused call is not retried every frame.
        self.menu_refused = done.is_err().then_some(hide);
        if let Err(e) = done {
            let what = if hide { "hide" } else { "show" };
            self.banners.set("menu", format!("Could not {what} the menu bar: {e}"));
        }
    }

    /// Whether a pane of the current window is see-through (`anyPaneIsTransparent`,
    /// `PT:9215-9225`: a session of the current tab drawn with alpha below 1).
    pub(super) fn any_pane_is_transparent(&self) -> bool {
        let Some(tab) = self.tabs.get(self.active) else { return false };
        crate::transparency::needs_alpha(&self.pane_alphas(tab).iter().map(|(_, a)| *a).collect::<Vec<_>>())
    }

    /// Gives the current window the border `chrome::show_border` asks for, in its focused or
    /// unfocused color, or the system's border; DWM is told only when the color changes.
    pub(super) fn apply_border(&mut self) {
        // Every way a window's chrome changes (a new window, its style, full screen, the Theme)
        // ends here, so the caption frame follows too.
        self.refresh_caption_frame();
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

    /// This window (and its menu bar, password manager and replay panel) follows a fixed Theme's
    /// mode, or Windows' again (`updateAppearance`, `iTermApplication.m:1276-1292`). The menu
    /// bar and drop-downs are told their mode (`apply_menu_theme`). Returns whether the mode the window follows changed. winit's
    /// `Window::set_theme` does not update `Window::theme` on Windows, so the mode is worked out
    /// here (`dark`) and remembered per window (`theme_applied`), never read back from winit.
    pub(super) fn apply_theme(&mut self) -> bool {
        let forced = crate::tab_theme::forced_dark(self.tab_style());
        let Some(g) = self.gpu.as_ref() else { return false };
        g.window.set_theme(crate::tab_theme::window_theme(forced));
        let dark = self.dark();
        let changed = crate::tab_theme::theme_changed(self.theme_applied, dark);
        self.theme_applied = Some(dark);
        self.apply_menu_theme();
        if let Some(w) = &self.pwm_window {
            w.follow_theme(forced);
        }
        if let Some(w) = &self.replay_window {
            w.follow_theme(forced);
        }
        changed
    }

    /// The menu bar and its drop-down menus take the mode `dark_menu::menu_dark` decides: a fixed
    /// Theme's, else Windows' app mode read from the registry (muda's `Auto` needs a window
    /// allowed dark, which winit never does, so the bar is always told Dark or Light). The
    /// drop-downs follow the process app mode, set here too.
    pub(super) fn apply_menu_theme(&mut self) {
        let forced = crate::tab_theme::forced_dark(self.tab_style());
        let hc = crate::tab_theme::system_high_contrast();
        let Some(hwnd) = self.hwnd() else { return };
        if let Err(e) = platform_menu_theme(hwnd, forced, hc, self.menu_bar.as_ref().map(|b| &b.menu)) {
            self.banners.set("menu", format!("Could not set the menu bar's theme: {e}"));
        }
    }

    /// Keeps this window's forced title bar: winit resets a window with no preferred theme to
    /// Windows' mode at every `WM_SETTINGCHANGE` and sends nothing unless the mode changed, so a
    /// subclass asks the app to apply the Theme again after each one.
    pub(super) fn guard_theme(&self) {
        let Some(hwnd) = self.hwnd() else { return };
        let proxy = self.proxy.lock().unwrap().clone();
        guard_theme(hwnd, Box::new(move || {
            let _ = proxy.send_event(UserEvent::ThemeReset);
        }));
    }

    /// After a system setting changed (`UserEvent::ThemeReset`): a fixed Theme's windows take
    /// its mode again. Nothing to do for Regular, Minimal and Compact, which follow Windows.
    pub(super) fn reapply_forced_theme(&mut self) {
        theme_reset_handled();
        // The same broadcast carries "Always show scrollbars" (`DynamicScrollbars`).
        self.refresh_scroll_style();
        if crate::tab_theme::forced_dark(self.tab_style()).is_some() {
            self.apply_theme_everywhere();
        } else {
            // Windows' app mode or high contrast may have changed: every window's menus follow it live.
            self.apply_menu_theme_everywhere();
        }
    }

    /// Every terminal window's menu bar takes the menu mode again (`apply_menu_theme`).
    fn apply_menu_theme_everywhere(&mut self) {
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if self.enter(id) {
                self.apply_menu_theme();
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
    }

    /// Every terminal window, the Settings window and the Profiles window follow the Theme, after
    /// it changed. A window whose mode changed rebuilds its panes' colors.
    pub(super) fn apply_theme_everywhere(&mut self) {
        let forced = crate::tab_theme::forced_dark(self.tab_style());
        for w in [&self.settings, &self.profiles_win].into_iter().flatten() {
            w.follow_theme(forced);
        }
        let back = self.current_window();
        let ids: Vec<WindowId> = self.registry.order().into_iter().filter_map(|k| self.window_id_of_key(k)).collect();
        for id in ids {
            if self.enter(id) && self.apply_theme() {
                self.refresh_panes();
            }
        }
        if let Some(b) = back {
            self.enter(b);
        }
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

#[cfg(test)]
#[path = "menu_hold_tests.rs"]
mod menu_hold_tests;
