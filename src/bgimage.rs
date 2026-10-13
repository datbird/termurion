//! Profile background images: the four scaling modes (Stretch, Tile, Scale to Fill, Scale
//! to Fit), laid out over a container (the pane, or the tab's content area when panes
//! share one image), loading with a size cap, and folder rotation in a shuffled order.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::layout::Rect;

/// Images are downscaled to this many pixels on their long side when loaded.
pub const MAX_SIDE: u32 = 5120;
/// Folder rotation never changes images faster than this.
pub const MIN_INTERVAL_SECS: u64 = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Stretch,
    Tile,
    Fill,
    Fit,
}

impl Mode {
    /// The profile's `Background Image Mode` (0 to 3).
    pub fn from_int(n: i64) -> Mode {
        match n {
            1 => Mode::Tile,
            2 => Mode::Fill,
            3 => Mode::Fit,
            _ => Mode::Stretch,
        }
    }
}

/// One textured rectangle: where on screen, which part of the image (u0, v0, u1, v1),
/// and whether the texture repeats (Tile).
pub type Piece = (Rect, [f32; 4], bool);

/// The pieces that draw an `image` (w, h in pixels) laid out over `container`, for the
/// part inside `pane`.
pub fn quads(mode: Mode, image: (u32, u32), container: Rect, pane: Rect) -> Vec<Piece> {
    let (iw, ih) = (image.0 as f32, image.1 as f32);
    let c = container;
    if iw <= 0.0 || ih <= 0.0 || c.w <= 0.0 || c.h <= 0.0 {
        return Vec::new();
    }
    let full = [0.0, 0.0, 1.0, 1.0];
    match mode {
        Mode::Stretch => slice(pane, c, full),
        Mode::Fill => {
            let (cont, img) = (c.w / c.h, iw / ih);
            let src = if img > cont {
                let f = cont / img;
                [(1.0 - f) / 2.0, 0.0, (1.0 + f) / 2.0, 1.0]
            } else {
                let f = img / cont;
                [0.0, (1.0 - f) / 2.0, 1.0, (1.0 + f) / 2.0]
            };
            slice(pane, c, src)
        }
        Mode::Fit => {
            let s = (c.w / iw).min(c.h / ih);
            let (w, h) = (iw * s, ih * s);
            slice(pane, Rect { x: c.x + (c.w - w) / 2.0, y: c.y + (c.h - h) / 2.0, w, h }, full)
        }
        Mode::Tile => {
            let uv = [(pane.x - c.x) / iw, (pane.y - c.y) / ih, (pane.x + pane.w - c.x) / iw, (pane.y + pane.h - c.y) / ih];
            vec![(pane, uv, true)]
        }
    }
}

/// The part of `shown` (the image laid out there, showing `src` of it) inside `pane`.
fn slice(pane: Rect, shown: Rect, src: [f32; 4]) -> Vec<Piece> {
    let x0 = pane.x.max(shown.x);
    let y0 = pane.y.max(shown.y);
    let x1 = (pane.x + pane.w).min(shown.x + shown.w);
    let y1 = (pane.y + pane.h).min(shown.y + shown.h);
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let u = |x: f32| src[0] + (x - shown.x) / shown.w * (src[2] - src[0]);
    let v = |y: f32| src[1] + (y - shown.y) / shown.h * (src[3] - src[1]);
    vec![(Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }, [u(x0), v(y0), u(x1), v(y1)], false)]
}

/// A decoded background image.
#[derive(Debug)]
pub struct Loaded {
    pub path: PathBuf,
    /// Tells textures apart: the path and its modified time.
    pub key: u64,
    pub size: (u32, u32),
    pub rgba: Arc<Vec<u8>>,
}

/// Decodes an image, downscaled to `MAX_SIDE`. None when it is missing or does not decode.
pub fn load(path: &Path) -> Option<Loaded> {
    use std::hash::{Hash, Hasher};
    // The location is a profile setting (saved, importable): judged before anything touches it, as
    // opening a share path sends the person's credentials to its server.
    let local = crate::localfs::LocalPath::judge(path)?;
    let modified = crate::localfs::stat(&local).ok()?.modified?;
    let file = std::io::BufReader::new(crate::localfs::open(&local).ok()?);
    let mut img = image::ImageReader::new(file).with_guessed_format().ok()?.decode().ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    if w.max(h) > MAX_SIDE {
        let s = MAX_SIDE as f64 / w.max(h) as f64;
        let (nw, nh) = (((w as f64 * s).round() as u32).max(1), ((h as f64 * s).round() as u32).max(1));
        img = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Triangle);
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    modified.hash(&mut hasher);
    let size = img.dimensions();
    // The top bit keeps background textures apart from inline images' ids.
    Some(Loaded { path: path.to_path_buf(), key: hasher.finish() | (1 << 63), size, rgba: Arc::new(img.into_raw()) })
}

/// Folder rotation's order: every image once per cycle, reshuffled each cycle, and never
/// the same image twice in a row across cycles.
#[derive(Clone, Debug, Default)]
pub struct Deck {
    order: Vec<PathBuf>,
    last: Option<PathBuf>,
    seed: u64,
}

impl Deck {
    pub fn new(seed: u64) -> Deck {
        Deck { order: Vec::new(), last: None, seed: seed | 1 }
    }

    /// The next image from `files` (the folder's images now).
    pub fn next(&mut self, files: &[PathBuf]) -> Option<PathBuf> {
        if files.is_empty() {
            return None;
        }
        // Files that left the folder go now; new ones join the next cycle.
        self.order.retain(|f| files.contains(f));
        if self.order.is_empty() {
            let mut order = files.to_vec();
            for i in (1..order.len()).rev() {
                let j = (self.random() % (i as u64 + 1)) as usize;
                order.swap(i, j);
            }
            if order.len() > 1 && self.last.as_ref() == Some(&order[order.len() - 1]) {
                let n = order.len();
                order.swap(n - 1, 0);
            }
            self.order = order;
        }
        let next = self.order.pop()?;
        self.last = Some(next.clone());
        Some(next)
    }

    /// xorshift64.
    fn random(&mut self) -> u64 {
        let mut x = self.seed;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.seed = x;
        x
    }
}

/// The image files directly in a folder, by name (hidden files skipped).
pub fn folder_images(folder: &Path) -> Vec<PathBuf> {
    const EXT: [&str; 9] = ["png", "jpg", "jpeg", "gif", "bmp", "webp", "tif", "tiff", "ico"];
    let Some(Ok(dir)) = crate::localfs::LocalPath::judge(folder).map(|f| crate::localfs::entries(&f)) else { return Vec::new() };
    let mut out: Vec<PathBuf> = dir
        .into_iter()
        .filter(|e| e.stat.is_file)
        .map(|e| e.path().to_path_buf())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            !name.starts_with('.') && EXT.contains(&ext.as_str())
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: fn(f32, f32, f32, f32) -> Rect = |x, y, w, h| Rect { x, y, w, h };

    #[test]
    fn stretch_fill_fit_tile_geometry() {
        let pane = R(0.0, 0.0, 200.0, 100.0);
        assert_eq!(quads(Mode::Stretch, (100, 100), pane, pane), vec![(pane, [0.0, 0.0, 1.0, 1.0], false)]);
        assert_eq!(quads(Mode::Fill, (100, 100), pane, pane), vec![(pane, [0.0, 0.25, 1.0, 0.75], false)], "crops top and bottom");
        assert_eq!(quads(Mode::Fill, (400, 100), pane, pane), vec![(pane, [0.25, 0.0, 0.75, 1.0], false)], "crops the sides");
        assert_eq!(quads(Mode::Fit, (100, 100), pane, pane), vec![(R(50.0, 0.0, 100.0, 100.0), [0.0, 0.0, 1.0, 1.0], false)], "centered, bars left and right");
        assert_eq!(quads(Mode::Tile, (100, 50), pane, pane), vec![(pane, [0.0, 0.0, 2.0, 2.0], true)], "one image pixel per screen pixel");
        assert!(quads(Mode::Stretch, (0, 10), pane, pane).is_empty());
    }

    #[test]
    fn shared_mode_slices_the_container() {
        let container = R(0.0, 0.0, 200.0, 100.0);
        let right = R(100.0, 0.0, 100.0, 100.0);
        assert_eq!(quads(Mode::Stretch, (100, 100), container, right), vec![(right, [0.5, 0.0, 1.0, 1.0], false)]);
        assert_eq!(quads(Mode::Tile, (50, 50), container, right), vec![(right, [2.0, 0.0, 4.0, 2.0], true)], "tiles continue across panes");
        // Fit: the image is centered in the container; the right pane shows its right half.
        assert_eq!(quads(Mode::Fit, (100, 100), container, right), vec![(R(100.0, 0.0, 50.0, 100.0), [0.5, 0.0, 1.0, 1.0], false)]);
    }

    #[test]
    fn rotation_shows_each_once_per_cycle() {
        let files: Vec<PathBuf> = ["a.png", "b.png", "c.png", "d.png"].iter().map(PathBuf::from).collect();
        let mut deck = Deck::new(42);
        let mut last = None;
        for _ in 0..5 {
            let mut cycle: Vec<PathBuf> = (0..4).map(|_| deck.next(&files).unwrap()).collect();
            assert_ne!(Some(&cycle[0]), last.as_ref(), "no repeat across cycles");
            last = cycle.last().cloned();
            cycle.sort();
            assert_eq!(cycle, files, "each image once per cycle");
        }
        assert_eq!(deck.next(&[]), None);
        let one = vec![PathBuf::from("only.png")];
        assert_eq!(deck.next(&one), Some(PathBuf::from("only.png")));
        assert_eq!(deck.next(&one), Some(PathBuf::from("only.png")), "a single image repeats");
    }

    #[test]
    fn bad_images_fall_back() {
        let dir = std::env::temp_dir().join(format!("termurion-bg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load(&dir.join("missing.png")).is_none());
        std::fs::write(dir.join("corrupt.png"), b"\x89PNG not really").unwrap();
        assert!(load(&dir.join("corrupt.png")).is_none());
        let wide = image::RgbaImage::from_pixel(20000, 4, image::Rgba([10, 20, 30, 255]));
        wide.save(dir.join("wide.png")).unwrap();
        let l = load(&dir.join("wide.png")).unwrap();
        assert_eq!(l.size.0, MAX_SIDE);
        assert_eq!(l.rgba.len(), (l.size.0 * l.size.1 * 4) as usize);
        std::fs::write(dir.join(".hidden.png"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();
        let names: Vec<String> = folder_images(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec!["corrupt.png", "wide.png"], "images by name, hidden and other files skipped");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
