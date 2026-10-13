//! Termurion > Suppressed Alerts on the app side: the one window (app-wide), un-suppressing, the
//! "Always show alerts with remembered selections" mode, and the menu item's title in every
//! window's menu bar (`iTermApplicationDelegate.m:548-566`).

use std::time::{Duration, Instant};

use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::window::WindowId;

use super::App;
use super::warnings_app::unix_now;
use crate::suppressed_alerts_ui::{self as sa, SuppressedEvent, SuppressedWindow};

/// The menu title's "ago" is refreshed this often while something is suppressed (muda has
/// no menu-will-open hook, where iTerm2 retitles).
const RETITLE_EVERY: Duration = Duration::from_secs(60);

impl App {
    /// Termurion > Suppressed Alerts: opens the window, or brings the open one forward.
    pub(super) fn show_suppressed_alerts(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(w) = &self.suppressed_window {
            return w.focus();
        }
        let rows = self.suppressed_rows();
        let Some(g) = self.gpu.as_ref() else { return };
        match SuppressedWindow::open(event_loop, &g.instance, &g.adapter, &g.device, &g.queue, rows, self.show_remembered) {
            Ok(w) => self.suppressed_window = Some(w),
            Err(e) => {
                self.banners.set("window", format!("Could not open the window: {e}"));
                self.redraw();
            }
        }
    }

    /// The window's rows now (lapsed silences pruned, newest first).
    fn suppressed_rows(&mut self) -> Vec<sa::Row> {
        let now = unix_now();
        sa::rows(&self.suppressed.current(&self.globals, now), now)
    }

    /// Events for the Suppressed Alerts window. False for other windows.
    pub(super) fn suppressed_event(&mut self, window: WindowId, event: &WindowEvent) -> bool {
        let Some(w) = self.suppressed_window.as_mut().filter(|w| w.id() == window) else { return false };
        let answer = match event {
            WindowEvent::CloseRequested => SuppressedEvent::Close,
            WindowEvent::RedrawRequested => w.redraw(),
            other => {
                w.event(other);
                SuppressedEvent::None
            }
        };
        match answer {
            SuppressedEvent::None => {}
            SuppressedEvent::Close => self.suppressed_window = None,
            // Un-suppressing writes the catalog at once (`unsuppressIdentifier:`, `SA:189-211`).
            SuppressedEvent::Unsuppress(ids) => {
                for id in &ids {
                    self.suppressed.unsuppress(&mut self.globals, id);
                }
                self.catalog_write();
            }
            SuppressedEvent::UnsuppressAll => {
                self.suppressed.unsuppress_all(&mut self.globals);
                self.catalog_write();
            }
            // Not saved: iTerm2's is a process global.
            SuppressedEvent::ShowRemembered(on) => self.show_remembered = on,
        }
        true
    }

    /// The catalog changed or a minute passed: the menu item's title in every window's menu
    /// bar (the current one and each parked window's) and the open window's rows follow it.
    pub(super) fn retitle_suppressed_menu(&mut self) {
        let now = unix_now();
        let current = self.suppressed.current(&self.globals, now);
        let title = sa::menu_title(current.len(), current.first().map(|e| now - e.last));
        for bar in self.menu_bars() {
            if let Some(item) = &bar.suppressed {
                item.set_text(&title);
            }
        }
        if let Some(w) = self.suppressed_window.as_mut() {
            w.set_rows(sa::rows(&current, now));
        }
        // Only a title with a time in it goes stale.
        self.suppressed_retitle_due = (!current.is_empty()).then(|| Instant::now() + RETITLE_EVERY);
    }

    /// Each pass of the event loop: the title is refreshed once its minute is up.
    pub(super) fn suppressed_tick(&mut self) {
        if self.suppressed_retitle_due.is_some_and(|t| Instant::now() >= t) {
            self.retitle_suppressed_menu();
        }
    }
}
