//! OSC 1337 blocks in the window: the gutter bar, the hover pill with copy and fold, a
//! click in the left margin to fold, and copying a block's text.

use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::Line;

use super::*;
use crate::blocks::{self, PillButton};

/// A drawn pill button: where, which pane and block, and what it does.
pub(super) type BlockButton = (Rect, PaneId, u32, PillButton);

impl App {
    /// The block under the pointer, for the red bar and the pill.
    pub(super) fn update_block_hover(&mut self) {
        let (x, y) = self.cursor;
        let hover = self.pane_at(x, y).and_then(|(id, rect)| {
            let (_, row, _) = self.cell_at(id, rect, x, y);
            let s = &self.panes.get(&id)?.screen;
            let line = row as i32 - s.term().grid().display_offset() as i32;
            s.block_at_line(line).map(|(n, first, last)| (id, n, first, last))
        });
        // The pointer on the pill keeps the block hovered.
        let on_pill = self.block_buttons.iter().any(|(r, ..)| r.contains(x, y));
        if hover != self.block_hover && !on_pill {
            self.block_hover = hover;
            self.redraw();
        }
    }

    /// View rows of a pane that belong to a block (mark bars give way to block bars there).
    pub(super) fn block_view_rows(&self, id: PaneId) -> Vec<usize> {
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        let g = p.screen.term().grid();
        let off = g.display_offset() as i32;
        (0..g.screen_lines()).filter(|r| g[Line(*r as i32 - off)].block() != 0).collect()
    }

    /// Draws a pane's block bars and, for the hovered block, its pill. The pill's buttons.
    pub(super) fn draw_blocks(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32)) -> Vec<BlockButton> {
        let Some(p) = self.panes.get(&id) else { return Vec::new() };
        let s = &p.screen;
        let g = s.term().grid();
        let (off, lines, cols) = (g.display_offset() as i32, g.screen_lines(), g.columns());
        let hovered = self.block_hover.filter(|h| h.0 == id);
        let mut bars = Vec::new();
        for r in 0..lines {
            let row = &g[Line(r as i32 - off)];
            let n = row.block();
            if n == 0 {
                continue;
            }
            let red = hovered.is_some_and(|(_, hn, first, last)| hn == n && (last - first + 1 >= 3 || row.fold() != 0));
            bars.push((r, row.fold() != 0, red));
        }
        let inner = self.inner(rect);
        let pad = self.margins().0;
        scene::block_bars(frame, &bars, inner, cell.1, pad, s.palette.fg, s.palette.ansi[1]);
        let Some((_, n, first, last)) = hovered else { return Vec::new() };
        let folded = s.blocks.folds.contains_key(&n);
        let Some((row, buttons)) = blocks::pill(first + off, last + off, lines, cols, last - first + 1 >= 3 || folded) else { return Vec::new() };
        let y = inner.y + row as f32 * cell.1;
        let rects: Vec<(Rect, PillButton)> = buttons.iter().map(|(b, col)| (Rect { x: inner.x + *col as f32 * cell.0, y, w: 2.0 * cell.0, h: cell.1 }, *b)).collect();
        scene::block_pill(frame, &rects, folded, s.palette.bg, s.palette.fg);
        rects.into_iter().map(|(r, b)| (r, id, n, b)).collect()
    }

    /// Draws the inline buttons (OSC 1337 Button) on a pane's rows in view.
    pub(super) fn draw_inline_buttons(&self, frame: &mut Frame, id: PaneId, rect: Rect, cell: (f32, f32)) {
        let Some(p) = self.panes.get(&id) else { return };
        let s = &p.screen;
        let g = s.term().grid();
        let off = g.display_offset() as i32;
        let inner = self.inner(rect);
        let mut faces = Vec::new();
        for r in 0..g.screen_lines() {
            for b in s.buttons_on_line(r as i32 - off) {
                let face = match &b.kind {
                    blocks::ButtonKind::Copy { .. } => scene::ButtonFace::Copy,
                    blocks::ButtonKind::Custom { icon, .. } => blocks::icon_glyph(icon).map_or(scene::ButtonFace::Blank, scene::ButtonFace::Icon),
                };
                let at = Rect { x: inner.x + b.col as f32 * cell.0, y: inner.y + r as f32 * cell.1, w: 2.0 * cell.0, h: cell.1 };
                faces.push((at, face, b.valid));
            }
        }
        scene::inline_buttons(frame, &faces, s.palette.bg, s.palette.fg);
    }

    /// A click on an inline button: copy its block, or report a custom button's code to a
    /// trusted program. Inert buttons let the click through. True when used.
    fn inline_button_press(&mut self, x: f32, y: f32) -> bool {
        let Some((id, rect)) = self.pane_at(x, y) else { return false };
        let (col, row, _) = self.cell_at(id, rect, x, y);
        let Some(p) = self.panes.get_mut(&id) else { return false };
        let line = row as i32 - p.screen.term().grid().display_offset() as i32;
        let buttons = p.screen.buttons_on_line(line);
        let Some(b) = blocks::button_hit(&buttons, col as usize).filter(|b| b.valid) else { return false };
        match blocks::click(b, p.screen.trusted) {
            blocks::Click::CopyBlock(block) => self.copy_block(id, &block),
            blocks::Click::Report(bytes) => p.reply(&bytes),
            blocks::Click::Nothing => {}
        }
        true
    }

    /// A click on a pill button, an inline button, or in a block row's left margin. True
    /// when used.
    pub(super) fn block_press(&mut self, x: f32, y: f32) -> bool {
        if self.inline_button_press(x, y) {
            return true;
        }
        if let Some(&(_, pane, n, button)) = self.block_buttons.iter().find(|(r, ..)| r.contains(x, y)) {
            match button {
                PillButton::Copy => {
                    if let Some(id) = self.block_id(pane, n) {
                        self.copy_block(pane, &id);
                    }
                }
                PillButton::Fold => self.toggle_block_fold(pane, n),
            }
            return true;
        }
        // iTerm2: a click in the left margin on a block's line folds it.
        let Some((id, rect)) = self.pane_at(x, y) else { return false };
        if x >= self.inner(rect).x {
            return false;
        }
        let (_, row, _) = self.cell_at(id, rect, x, y);
        let Some(p) = self.panes.get(&id) else { return false };
        let line = row as i32 - p.screen.term().grid().display_offset() as i32;
        let Some((n, first, last)) = p.screen.block_at_line(line) else { return false };
        if last > first && !p.screen.blocks.folds.contains_key(&n) {
            self.toggle_block_fold(id, n);
            return true;
        }
        false
    }

    fn block_id(&self, pane: PaneId, n: u32) -> Option<String> {
        self.panes.get(&pane)?.screen.blocks.by_id.iter().find(|(_, v)| **v == n).map(|(k, _)| k.clone())
    }

    fn toggle_block_fold(&mut self, pane: PaneId, n: u32) {
        let Some(id) = self.block_id(pane, n) else { return };
        if let Some(p) = self.panes.get_mut(&pane) {
            if !p.screen.unfold_block(&id) {
                p.screen.fold_block(&id);
            }
        }
        self.block_hover = None;
        self.redraw();
    }

    /// The block's text on the clipboard: plain, each line's trailing spaces trimmed, ending
    /// with a newline.
    fn copy_block(&mut self, pane: PaneId, block: &str) {
        let Some(p) = self.panes.get(&pane) else { return };
        let Some(text) = p.screen.block_text(block) else { return };
        if let Some(cb) = &mut self.clipboard {
            match cb.set_text(text) {
                Ok(()) => self.show_toast("Copied"),
                Err(e) => self.banners.set("clipboard", format!("Copy failed: {e}")),
            }
        }
    }
}
