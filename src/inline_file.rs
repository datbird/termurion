//! OSC 1337 File (and MultipartFile, FilePart, FileEnd): an inline image, or a file to
//! download, as iTerm2 sends them (`imgcat`, `it2dl`).

use crate::images::{self, Dim, Image};
use crate::screen::Screen;

/// A file's arguments: iTerm2's `name` (base64), `size`, `width`, `height`,
/// `preserveAspectRatio`, and `inline`.
#[derive(Clone, Debug, PartialEq)]
pub struct FileArgs {
    pub name: String,
    pub size: Option<u64>,
    pub width: Dim,
    pub height: Dim,
    pub keep_aspect: bool,
    pub inline: bool,
}

/// A file the terminal was sent to save, waiting for the person to allow it.
#[derive(Clone, Debug, PartialEq)]
pub struct Download {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// A file arriving in parts.
#[derive(Clone, Debug, Default)]
pub struct Multipart {
    pub args: Option<FileArgs>,
    pub data: String,
}

pub fn parse_args(s: &str) -> FileArgs {
    use base64::Engine;
    let mut a = FileArgs { name: "Unnamed file".into(), size: None, width: Dim::Auto, height: Dim::Auto, keep_aspect: true, inline: false };
    let dim = |v: &str| {
        let n = |t: &str| t.parse::<u32>().ok();
        if let Some(p) = v.strip_suffix("px") {
            n(p).map_or(Dim::Auto, Dim::Pixels)
        } else if let Some(p) = v.strip_suffix('%') {
            n(p).map_or(Dim::Auto, Dim::Percent)
        } else {
            n(v).map_or(Dim::Auto, Dim::Cells)
        }
    };
    for part in s.split(';') {
        let Some((k, v)) = part.split_once('=') else { continue };
        match k {
            "name" => {
                if let Ok(b) = base64::engine::general_purpose::STANDARD.decode(v.trim()) {
                    a.name = String::from_utf8_lossy(&b).into_owned();
                }
            }
            "size" => a.size = v.parse().ok(),
            "width" => a.width = dim(v),
            "height" => a.height = dim(v),
            "preserveAspectRatio" => a.keep_aspect = v != "0",
            "inline" => a.inline = v == "1",
            _ => {}
        }
    }
    a
}

/// iTerm2's base64 is lenient: whitespace is skipped and padding may be missing.
fn decode_base64(data: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let compact: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let e = &base64::engine::general_purpose::STANDARD;
    e.decode(&compact).ok().or_else(|| base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.trim_end_matches('=')).ok())
}

/// A free name for `name` in `dir`: "a.txt", then "a (1).txt", and so on. Any folder part
/// of the name is dropped.
pub fn unique_path(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let base = std::path::Path::new(name).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let clean: String = base.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    let clean = if clean.trim().is_empty() || clean == "." || clean == ".." { "Unnamed file".to_string() } else { clean };
    let first = dir.join(&clean);
    if !first.exists() {
        return first;
    }
    let p = std::path::Path::new(&clean);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|c| !c.exists()).unwrap_or(first)
}

impl Screen {
    /// OSC 1337 `File`, `MultipartFile`, `FilePart`, and `FileEnd`.
    pub fn file_osc(&mut self, key: &str, value: &str) {
        match key {
            "File" => {
                let (args, data) = value.split_once(':').unwrap_or((value, ""));
                self.receive_file(parse_args(args), data);
            }
            "MultipartFile" => self.multipart = Some(Multipart { args: Some(parse_args(value)), data: String::new() }),
            "FilePart" => {
                if let Some(m) = &mut self.multipart {
                    m.data.push_str(value);
                }
            }
            "FileEnd" => {
                if let Some(m) = self.multipart.take() {
                    let args = m.args.unwrap_or_else(|| parse_args(""));
                    self.receive_file(args, &m.data);
                }
            }
            _ => {}
        }
    }

    /// An inline file is shown (a file that does not decode shows the broken image); any
    /// other file waits in `downloads` for the app to ask.
    fn receive_file(&mut self, args: FileArgs, data: &str) {
        use alacritty_terminal::grid::Dimensions;
        let bytes = decode_base64(data);
        if !args.inline {
            if let Some(bytes) = bytes {
                self.downloads.push(Download { name: args.name, bytes });
            }
            return;
        }
        let d = bytes.and_then(|b| images::decode(&b).ok()).unwrap_or_else(images::broken);
        let (cw, ch) = self.cell_px();
        let grid = (self.term().columns(), self.term().screen_lines());
        let col = self.term().grid().cursor.point.column.0;
        let (cols, rows) = images::size_in_cells((d.width, d.height), args.width, args.height, args.keep_aspect, (cw as f32, ch as f32), grid, col);
        let image = Image { width_px: d.width, height_px: d.height, cols, rows, frames: d.frames, broken: d.broken, keep_aspect: args.keep_aspect, uid: 0 };
        self.place_image(image, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line};

    fn png_b64() -> String {
        use base64::Engine;
        let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        base64::engine::general_purpose::STANDARD.encode(out.into_inner())
    }

    #[test]
    fn file_args_parse() {
        let a = parse_args("name=aGkudHh0;size=3;width=10px;height=50%;preserveAspectRatio=0;inline=1");
        assert_eq!(
            a,
            FileArgs { name: "hi.txt".into(), size: Some(3), width: Dim::Pixels(10), height: Dim::Percent(50), keep_aspect: false, inline: true }
        );
        let d = parse_args("");
        assert_eq!((d.name.as_str(), d.width, d.keep_aspect, d.inline), ("Unnamed file", Dim::Auto, true, false));
        assert_eq!(parse_args("width=7").width, Dim::Cells(7));
        assert_eq!(parse_args("width=auto;height=bad").height, Dim::Auto);
    }

    #[test]
    fn an_inline_image_is_placed() {
        let mut s = Screen::new(20, 6);
        s.set_cell_px(4, 4);
        s.feed(format!("\x1b]1337;File=inline=1:{}\x07", png_b64()).as_bytes());
        assert!(s.term().grid()[Line(0)][Column(0)].image().is_some());
        assert!(s.term().grid()[Line(1)][Column(1)].image().is_some());
        assert_eq!(s.images.by_id.len(), 1);
        s.feed(b"\x1b]1337;File=inline=1:!!!notbase64\x07");
        assert!(s.images.by_id.values().any(|i| i.broken));
    }

    #[test]
    fn multipart_files_assemble() {
        let mut s = Screen::new(20, 6);
        s.set_cell_px(4, 4);
        let b = png_b64();
        let (x, y) = b.split_at(b.len() / 2);
        s.feed(b"\x1b]1337;MultipartFile=inline=1\x07");
        s.feed(format!("\x1b]1337;FilePart={x}\x07\x1b]1337;FilePart={y}\x07").as_bytes());
        assert!(s.images.by_id.is_empty());
        s.feed(b"\x1b]1337;FileEnd\x07");
        assert_eq!(s.images.by_id.len(), 1);
        assert!(s.images.by_id.values().all(|i| !i.broken));
    }

    #[test]
    fn downloads_wait_for_the_app() {
        let mut s = Screen::new(20, 6);
        s.feed(b"\x1b]1337;File=name=cmVwb3J0LnBkZg==;size=5:aGVsbG8=\x07");
        assert_eq!(s.downloads, vec![Download { name: "report.pdf".into(), bytes: b"hello".to_vec() }]);
        assert!(s.images.by_id.is_empty());
    }

    #[test]
    fn a_large_file_streams_through_the_scanner() {
        let mut sc = crate::osc::Scanner::default();
        let t = std::time::Instant::now();
        let mut events = sc.scan(b"\x1b]1337;File=inline=1:");
        let chunk = vec![b'A'; 4096];
        for _ in 0..(20 << 20) / 4096 {
            events.extend(sc.scan(&chunk));
        }
        events.extend(sc.scan(b"\x07"));
        assert!(t.elapsed() < std::time::Duration::from_secs(3), "{:?}", t.elapsed());
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0].1, crate::osc::Osc::Kv(k, v) if k == "File" && v.len() > 20 << 20));
    }

    #[test]
    fn downloads_get_unique_names() {
        let d = std::env::temp_dir().join(format!("tandem-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(unique_path(&d, "a.txt"), d.join("a.txt"));
        std::fs::write(d.join("a.txt"), "x").unwrap();
        assert_eq!(unique_path(&d, "a.txt"), d.join("a (1).txt"));
        assert_eq!(unique_path(&d, "..\\..\\evil.txt"), d.join("evil.txt"));
        assert_eq!(unique_path(&d, "sub/x.txt"), d.join("x.txt"));
        assert_eq!(unique_path(&d, ""), d.join("Unnamed file"));
    }
}
