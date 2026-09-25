//! Inline images: decoded pictures kept by id, which grid cells show a tile of (iTerm2
//! keeps images in the grid the same way, as cells that carry an image number).

use std::collections::HashMap;
use std::sync::Arc;

use alacritty_terminal::term::cell::Tile;

use crate::screen::Screen;

/// One picture (a GIF has several) in RGBA8.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub rgba: Arc<Vec<u8>>,
    pub delay_ms: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width_px: u32,
    pub height_px: u32,
    /// Its size in cells.
    pub cols: usize,
    pub rows: usize,
    pub frames: Vec<Frame>,
    /// The file did not decode; this is the placeholder.
    pub broken: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Store {
    pub by_id: HashMap<u32, Image>,
    next: u32,
}

impl Store {
    pub fn add(&mut self, image: Image) -> u32 {
        self.next += 1;
        self.by_id.insert(self.next, image);
        self.next
    }
}

impl Screen {
    /// Writes `image`'s tiles from the cursor (scrolling as needed): iTerm2 leaves the
    /// cursor on the image's last row, moved right by its width; with `move_cursor` off it
    /// goes back where it was.
    pub fn place_image(&mut self, image: Image, move_cursor: bool) -> u32 {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        use alacritty_terminal::term::cell::Cell;
        use alacritty_terminal::vte::ansi::Handler;
        let (cols, rows) = (image.cols.max(1), image.rows.max(1));
        let id = self.images.add(image);
        let stamp = self.term().stamp;
        let start = self.term().grid().cursor.point;
        let columns = self.term().columns();
        let mut scrolled = 0i32;
        for y in 0..rows {
            if y > 0 {
                let before = self.term().grid().cursor.point.line;
                self.term_mut().linefeed();
                if self.term().grid().cursor.point.line == before {
                    scrolled += 1;
                }
            }
            let line = self.term().grid().cursor.point.line;
            let grid = self.term_mut().grid_mut();
            grid[line].set_time(stamp);
            for x in 0..cols {
                let col = start.column.0 + x;
                if col >= columns {
                    break;
                }
                let cell = &mut grid[line][Column(col)];
                *cell = Cell::default();
                cell.set_image(Some(Tile { image: id, x: x as u16, y: y as u16 }));
            }
        }
        let grid = self.term_mut().grid_mut();
        grid.cursor.input_needs_wrap = false;
        if move_cursor {
            grid.cursor.point.column = Column((start.column.0 + cols).min(columns - 1));
        } else {
            grid.cursor.point = alacritty_terminal::index::Point::new(Line((start.line.0 - scrolled).max(0)), start.column);
        }
        id
    }

    /// Forgets images no cell shows any more.
    pub fn prune_images(&mut self) {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        if self.images.by_id.is_empty() {
            return;
        }
        let mut used = std::collections::HashSet::new();
        for grid in [self.term().primary_grid(), self.term().grid()] {
            let top = -(grid.history_size() as i32);
            for l in top..grid.screen_lines() as i32 {
                let row = &grid[Line(l)];
                for c in 0..grid.columns() {
                    if let Some(t) = row[Column(c)].image() {
                        used.insert(t.image);
                    }
                }
            }
        }
        self.images.by_id.retain(|id, _| used.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::grid::Dimensions;
    use alacritty_terminal::index::{Column, Line};

    fn picture(cols: usize, rows: usize) -> Image {
        Image { width_px: 10, height_px: 10, cols, rows, frames: vec![Frame { rgba: Arc::new(vec![0; 400]), delay_ms: 0 }], broken: false }
    }

    fn tile(s: &Screen, line: i32, col: usize) -> Option<Tile> {
        s.term().grid()[Line(line)][Column(col)].image()
    }

    #[test]
    fn placing_an_image_writes_tiles_and_moves_the_cursor() {
        let mut s = Screen::new(20, 6);
        s.feed(b"ab");
        let id = s.place_image(picture(3, 2), true);
        assert_eq!(tile(&s, 0, 2), Some(Tile { image: id, x: 0, y: 0 }));
        assert_eq!(tile(&s, 1, 4), Some(Tile { image: id, x: 2, y: 1 }));
        assert_eq!(tile(&s, 0, 5), None);
        let c = s.term().grid().cursor.point;
        assert_eq!((c.line.0, c.column.0), (1, 5));
        let mut t = Screen::new(20, 6);
        t.feed(b"ab");
        t.place_image(picture(3, 2), false);
        let c = t.term().grid().cursor.point;
        assert_eq!((c.line.0, c.column.0), (0, 2));
    }

    #[test]
    fn an_image_at_the_bottom_scrolls_the_screen() {
        let mut s = Screen::new(20, 3);
        s.feed(b"x\r\ny\r\nz");
        let id = s.place_image(picture(2, 3), true);
        let hist = s.term().grid().history_size() as i32;
        assert_eq!(hist, 2);
        assert_eq!(tile(&s, 0, 1), Some(Tile { image: id, x: 0, y: 0 }));
        assert_eq!(tile(&s, 2, 2), Some(Tile { image: id, x: 1, y: 2 }));
    }

    #[test]
    fn image_cells_copy_as_no_text() {
        let mut s = Screen::new(20, 4);
        s.feed(b"a");
        s.place_image(picture(2, 1), true);
        s.feed(b"b");
        assert_eq!(s.line_text(0), "a  b");
    }

    #[test]
    fn text_over_an_image_erases_its_tiles() {
        let mut s = Screen::new(20, 4);
        s.place_image(picture(3, 1), true);
        s.feed(b"\r\x1b[2K");
        assert_eq!(tile(&s, 0, 1), None);
    }

    #[test]
    fn image_rows_survive_reflow() {
        let mut s = Screen::new(20, 4);
        s.feed(b"0123456789012345");
        let id = s.place_image(picture(3, 1), true);
        s.resize(10, 4);
        s.resize(20, 4);
        let found = (-(s.term().grid().history_size() as i32)..4).any(|l| (0..s.term().columns()).any(|c| tile(&s, l, c) == Some(Tile { image: id, x: 2, y: 0 })));
        assert!(found);
    }

    #[test]
    fn unused_images_are_dropped() {
        let mut s = Screen::with_scrollback(20, 4, 100);
        s.place_image(picture(2, 2), true);
        s.prune_images();
        assert_eq!(s.images.by_id.len(), 1);
        s.clear(false);
        // 2J moves the screen into the scrollback; 3J clears that.
        s.feed(b"\x1b[2J\x1b[3J");
        s.prune_images();
        assert!(s.images.by_id.is_empty());
    }

    #[test]
    fn da1_reports_sixel() {
        let mut s = Screen::new(20, 4);
        let reply = s.feed(b"\x1b[c");
        assert_eq!(String::from_utf8(reply).unwrap(), "\x1b[?62;4;22c");
    }
}
