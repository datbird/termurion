//! Resizing a window by its edges: the grid the snap uses and the resizable flag. The subclass that
//! snaps a drag is `platform::sizing`.

use super::*;
use crate::chrome::{self, Snap};
use crate::platform::sizing::{Grid, install, publish};

impl App {
    /// The current window's grid for `sizing_ui` (`PT:5784-5935`): None when a drag keeps
    /// its size (maximized, full screen or size-locked, where iTerm2 returns before the
    /// snap); else the focused pane's cell, the decoration around the tab's pane area, the
    /// padding, the smallest pane area and the axes `chrome::snap_axes` allows at rest (a
    /// drag decides them again from what it changes).
    pub(super) fn snap_grid(&self) -> Option<Snap> {
        let g = self.gpu.as_ref()?;
        let (maximized, full_screen, locked) = (g.window.is_maximized(), self.full_screen(), self.win_locks.size);
        if maximized || full_screen || locked || matches!(self.win_style, crate::frames::MAXIMIZED | crate::frames::COMPACT_MAXIMIZED) {
            return None;
        }
        let (snap_w, snap_h) = chrome::snap_axes(self.win_style, self.globals.bool("DisableWindowSizeSnap"), false, maximized, full_screen, locked, true, true);
        // The focused pane's cell: the grid the window holds is that pane's (P14).
        let cell = self.focused_cell();
        let outer = g.window.outer_size();
        let area = self.content_area();
        let (side, vert) = self.margins();
        // The strip a legacy scrollbar takes is room around the grid, counted once.
        let (padx, pady) = (2.0 * side + self.scroll_room(), 2.0 * vert);
        Some(Snap {
            cell,
            decoration: (outer.width as f32 - area.w, outer.height as f32 - area.h),
            margins: (padx, pady),
            // `grid_size`'s floor: two columns and one row.
            min: (2.0 * cell.0 + padx, cell.1 + pady),
            snap_w,
            snap_h,
        })
    }

    /// Publishes the current window's grid for its next edge drag.
    pub(super) fn publish_snap(&self) {
        if let Some(hwnd) = self.hwnd() {
            let disabled = self.globals.bool("DisableWindowSizeSnap");
            publish(hwnd, self.snap_grid().map(|snap| Grid { snap, style: self.win_style, disabled }));
        }
    }

    /// A new terminal window's edges, once it is current: the snap subclass, and live resize
    /// for every window, hotkey windows too (iTerm2 drops the resizable mask for every
    /// window type, `PTWS:1063-1110`).
    pub(super) fn install_sizing(&self) {
        if let Some(hwnd) = self.hwnd() {
            install(hwnd);
        }
        self.apply_resizable();
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
