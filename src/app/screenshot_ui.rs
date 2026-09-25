//! Session > Make Screenshot... in the window: rendering a range of a frozen copy of the
//! pane into an image.

use super::*;
use crate::screenshot;

impl App {
    /// Renders buffer lines `first..=last` (0 is the oldest line) of `screen` as one RGBA
    /// image: the pane's full width with its side padding, no top or bottom margin, opaque,
    /// with the pane's background image laid out over the whole image's height.
    pub(super) fn render_lines(&mut self, pane: PaneId, screen: &crate::screen::Screen, first: usize, last: usize) -> Result<(u32, u32, Vec<u8>), String> {
        use alacritty_terminal::grid::Dimensions;
        let cell = self.cell();
        let pad = (PAD * self.scale()).round();
        let cols = screen.term().columns();
        let width = (cols as f32 * cell.0 + 2.0 * pad).ceil();
        let lines = last + 1 - first;
        let total_h = lines as f32 * cell.1;
        let history = screen.term().grid().history_size() as i32;
        let background = self.background_of(pane);
        // A batch must fit in one texture; iTerm2 renders 1000 lines at a time.
        let max_rows = ((8192.0 / cell.1).floor() as usize).clamp(1, 1000);
        let bg = screen.palette.bg;
        let mut out = Vec::with_capacity((width as usize) * (total_h as usize) * 4);
        for (b0, b1) in screenshot::batches(first, last, max_rows) {
            let count = b1 + 1 - b0;
            let batch = screenshot::slice(screen, b0 as i32 - history, count);
            let h = count as f32 * cell.1;
            let mut frame = Frame { clear: rgba(bg, 1.0), ..Default::default() };
            let area = Rect { x: 0.0, y: 0.0, w: width, h };
            if let Some((image, mode, blend)) = &background {
                // The image is laid out over the whole screenshot, and this batch shows its slice.
                let container = Rect { x: 0.0, y: -((b0 - first) as f32 * cell.1), w: width, h: total_h };
                let pieces = crate::bgimage::quads(*mode, image.size, container, area);
                scene::background(&mut frame, area, &pieces, image, bg, *blend);
            }
            scene::pane_with(&mut frame, &batch, Rect { x: pad, y: 0.0, w: cols as f32 * cell.0, h }, cell, false, false, None);
            let gpu = self.gpu.as_mut().ok_or("no window")?;
            out.extend(gpu.renderer.draw_offscreen(&frame, width as u32, h as u32)?);
        }
        Ok((width as u32, total_h as u32, out))
    }
}
