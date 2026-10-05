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
    /// Fit inside its cells keeping its shape (iTerm2's preserveAspectRatio).
    pub keep_aspect: bool,
    /// Unique across every screen, for the renderer's texture cache (set by `Store::add`).
    pub uid: u64,
}

static NEXT_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A decoded file: its size and frames.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Frame>,
    pub broken: bool,
}

/// Larger pictures are refused: a GPU texture holds at most 8192 pixels a side.
pub const MAX_SIDE: u32 = 8192;
/// A GIF keeps at most this many frames.
pub const MAX_FRAMES: usize = 1_000;
/// iTerm2 keeps an image's row in 8 bits, so an image is at most this many rows tall.
pub const MAX_ROWS: usize = 255;

/// Decodes a PNG, JPEG, GIF (every frame), BMP, WebP, TIFF, or ICO file to RGBA8.
/// All frames of one image together may hold at most this many bytes of pixels.
pub const FRAME_BUDGET: usize = 256 << 20;

/// `decode`, stopping at `budget` bytes of frames (the first frame always counts).
pub fn decode(bytes: &[u8]) -> Result<Decoded, String> {
    decode_within(bytes, FRAME_BUDGET)
}

pub fn decode_within(bytes: &[u8], budget: usize) -> Result<Decoded, String> {
    use image::AnimationDecoder;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(1 << 30);
    let format = image::guess_format(bytes).map_err(|e| e.to_string())?;
    if format == image::ImageFormat::Gif {
        let mut dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        image::ImageDecoder::set_limits(&mut dec, limits).map_err(|e| e.to_string())?;
        let (width, height) = image::ImageDecoder::dimensions(&dec);
        // A small GIF can hold a large canvas many times over: frames stop at the budget.
        let mut frames = Vec::new();
        let mut total = 0usize;
        for f in dec.into_frames().take(MAX_FRAMES) {
            let f = f.map_err(|e| e.to_string())?;
            let (n, d) = f.delay().numer_denom_ms();
            let rgba = f.into_buffer().into_raw();
            total += rgba.len();
            if !frames.is_empty() && total > budget {
                break;
            }
            frames.push(Frame { delay_ms: n / d.max(1), rgba: Arc::new(rgba) });
        }
        if frames.is_empty() {
            return Err("The GIF has no frames.".into());
        }
        return Ok(Decoded { width, height, frames, broken: false });
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    reader.limits(limits);
    let img = reader.decode().map_err(|e| e.to_string())?.to_rgba8();
    let (width, height) = img.dimensions();
    Ok(Decoded { width, height, frames: vec![Frame { rgba: Arc::new(img.into_raw()), delay_ms: 0 }], broken: false })
}

/// iTerm2's broken-image placeholder: a grey square with a red cross.
pub fn broken() -> Decoded {
    const N: u32 = 32;
    let mut px = vec![0u8; (N * N * 4) as usize];
    for y in 0..N {
        for x in 0..N {
            let edge = x < 2 || y < 2 || x >= N - 2 || y >= N - 2;
            let cross = (x as i32 - y as i32).abs() <= 1 || (x as i32 + y as i32 - (N as i32 - 1)).abs() <= 1;
            let c: [u8; 4] = if cross { [0xd0, 0x30, 0x30, 0xff] } else if edge { [0x80, 0x80, 0x80, 0xff] } else { [0x40, 0x40, 0x40, 0xff] };
            let i = ((y * N + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    }
    Decoded { width: N, height: N, frames: vec![Frame { rgba: Arc::new(px), delay_ms: 0 }], broken: true }
}

/// A requested width or height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dim {
    Auto,
    Cells(u32),
    Pixels(u32),
    Percent(u32),
}

/// An image's size in cells, as iTerm2 works it out: pixels become `ceil(px / cell)`,
/// percent is of the grid, auto keeps the shape; too wide for the space left means the
/// full grid width (the height scaled to match); at most MAX_ROWS rows.
pub fn size_in_cells(img: (u32, u32), w: Dim, h: Dim, keep_aspect: bool, cell: (f32, f32), grid: (usize, usize), cursor_col: usize) -> (usize, usize) {
    let (cw, ch) = (cell.0.max(1.0) as f64, cell.1.max(1.0) as f64);
    let (iw, ih) = (img.0.max(1) as f64, img.1.max(1) as f64);
    let cells = |d: Dim, total: usize, cell: f64| -> Option<f64> {
        match d {
            Dim::Auto => None,
            Dim::Cells(n) => Some(n as f64),
            Dim::Pixels(n) => Some((n as f64 / cell).ceil()),
            Dim::Percent(p) => Some((total as f64 * p as f64 / 100.0).ceil()),
        }
    };
    let (w, h) = match (cells(w, grid.0, cw), cells(h, grid.1, ch)) {
        (None, None) => ((iw / cw).ceil(), (ih / ch).ceil()),
        (Some(w), None) => (w, if keep_aspect { (w * cw * ih / iw / ch).ceil() } else { (ih / ch).ceil() }),
        (None, Some(h)) => (if keep_aspect { (h * ch * iw / ih / cw).ceil() } else { (iw / cw).ceil() }, h),
        (Some(w), Some(h)) => (w, h),
    };
    let (mut w, mut h) = (w.max(1.0), h.max(1.0));
    if w > grid.0.saturating_sub(cursor_col) as f64 {
        let full = grid.0.max(1) as f64;
        if keep_aspect {
            h = (h * full / w).ceil().max(1.0);
        }
        w = full;
    }
    (w as usize, (h as usize).min(MAX_ROWS))
}

#[derive(Clone, Debug, Default)]
pub struct Store {
    pub by_id: HashMap<u32, Image>,
    next: u32,
}

impl Store {
    pub fn add(&mut self, mut image: Image) -> u32 {
        image.uid = NEXT_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        // All images together stay under the budget: drop unused ones, else refuse.
        let size: usize = image.frames.iter().map(|f| f.rgba.len()).sum();
        let held = |s: &Screen| s.images.by_id.values().flat_map(|i| &i.frames).map(|f| f.rgba.len()).sum::<usize>();
        if held(self) + size > self.limits.images {
            self.prune_images();
            if held(self) + size > self.limits.images {
                return 0;
            }
        }
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
        // Folded lines keep their images for when they unfold.
        for row in self.folded_rows() {
            for c in 0..row.len() {
                if let Some(t) = row[Column(c)].image() {
                    used.insert(t.image);
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
        Image { width_px: 10, height_px: 10, cols, rows, frames: vec![Frame { rgba: Arc::new(vec![0; 400]), delay_ms: 0 }], broken: false, keep_aspect: true, uid: 0 }
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
        s.set_rules(crate::emu::Rules { clear_history: Some(false), ..Default::default() });
        // 2J moves the screen into the scrollback; 3J clears that.
        s.feed(b"\x1b[2J\x1b[3J");
        s.prune_images();
        assert!(s.images.by_id.is_empty());
    }

    #[test]
    fn da1_reports_sixel() {
        let mut s = Screen::new(20, 4);
        let reply = s.feed(b"\x1b[c");
        assert_eq!(String::from_utf8(reply).unwrap(), "\x1b[?64;1;2;4;6;17;18;21;22;52c");
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    fn gif() -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for c in [0u8, 255] {
                let img = image::RgbaImage::from_pixel(4, 3, image::Rgba([c, 0, 0, 255]));
                let frame = image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(70, 1));
                enc.encode_frame(frame).unwrap();
            }
        }
        out
    }

    #[test]
    fn decodes_png_and_gif_frames() {
        let d = decode(&png(3, 2)).unwrap();
        assert_eq!((d.width, d.height, d.frames.len(), d.broken), (3, 2, 1, false));
        assert_eq!(&d.frames[0].rgba[..4], &[10, 20, 30, 255]);
        let g = decode(&gif()).unwrap();
        assert_eq!((g.width, g.height, g.frames.len()), (4, 3, 2));
        assert_eq!(g.frames[0].delay_ms, 70);
        assert_eq!(g.frames[1].rgba[0], 255);
    }

    #[test]
    fn corrupt_bytes_decode_as_broken() {
        assert!(decode(b"not an image").is_err());
        let b = broken();
        assert!(b.broken && b.width > 0 && b.frames[0].rgba.len() == (b.width * b.height * 4) as usize);
        assert!(decode(&png(MAX_SIDE + 1, 1)).is_err(), "too wide");
    }

    #[test]
    fn sizes_follow_iterm2() {
        let (cell, grid) = ((10.0, 20.0), (80, 24));
        let img = (100, 40);
        assert_eq!(size_in_cells(img, Dim::Auto, Dim::Auto, true, cell, grid, 0), (10, 2));
        assert_eq!(size_in_cells(img, Dim::Cells(5), Dim::Auto, true, cell, grid, 0), (5, 1));
        assert_eq!(size_in_cells(img, Dim::Pixels(55), Dim::Auto, true, cell, grid, 0).0, 6);
        assert_eq!(size_in_cells(img, Dim::Percent(50), Dim::Auto, true, cell, grid, 0), (40, 8));
        assert_eq!(size_in_cells(img, Dim::Cells(5), Dim::Auto, false, cell, grid, 0), (5, 2));
        assert_eq!(size_in_cells(img, Dim::Auto, Dim::Cells(4), true, cell, grid, 0), (20, 4));
        assert_eq!(size_in_cells(img, Dim::Cells(7), Dim::Cells(3), true, cell, grid, 0), (7, 3));
        assert_eq!(size_in_cells((0, 0), Dim::Auto, Dim::Auto, true, cell, grid, 0), (1, 1));
    }

    #[test]
    fn wide_images_shrink_to_the_grid() {
        assert_eq!(size_in_cells((2000, 100), Dim::Auto, Dim::Auto, true, (10.0, 20.0), (80, 24), 30), (80, 2));
    }

    #[test]
    fn height_caps_at_255() {
        assert_eq!(size_in_cells((10, 100_000), Dim::Auto, Dim::Auto, true, (10.0, 20.0), (80, 24), 0).1, MAX_ROWS);
    }

    #[test]
    fn images_fit_a_gpu_texture() {
        assert!(decode(&png(8193, 1)).is_err(), "wider than 8192 px");
        assert!(decode(&png(8192, 1)).is_ok());
    }

    #[test]
    fn gif_frames_stop_at_the_budget() {
        // Each 4x3 frame is 48 bytes.
        let d = decode_within(&gif(), 60).unwrap();
        assert_eq!(d.frames.len(), 1);
        assert_eq!(decode_within(&gif(), 1000).unwrap().frames.len(), 2);
    }

    #[test]
    fn folded_images_are_kept() {
        let mut s = Screen::with_scrollback(20, 6, 100);
        s.feed(b"a\r\n");
        s.place_image(picture(2, 1), true);
        s.feed(b"\r\nb\r\n$ ");
        let f = s.fold(0, 1).unwrap();
        s.prune_images();
        assert!(s.unfold(f));
        assert_eq!(s.images.by_id.len(), 1);
    }

    #[test]
    fn the_image_store_has_a_budget() {
        let mut s = Screen::new(20, 6);
        s.limits.images = 1000;
        s.set_rules(crate::emu::Rules { clear_history: Some(false), ..Default::default() });
        let big = |n: usize| Image { frames: vec![Frame { rgba: Arc::new(vec![0; n]), delay_ms: 0 }], ..picture(1, 1) };
        assert_ne!(s.place_image(big(600), true), 0);
        assert_eq!(s.place_image(big(600), true), 0, "over the budget while the first still shows");
        s.feed(b"\x1b[2J\x1b[3J");
        assert_ne!(s.place_image(big(600), true), 0, "the first was dropped");
    }
}
