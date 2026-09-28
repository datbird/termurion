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
    /// `type`: a MIME type, `.ext`, or a language name.
    pub kind: Option<String>,
    /// `mode=wide`: a text document shows in Wide mode.
    pub wide: bool,
    /// A `name` was given.
    pub named: bool,
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
    /// An inline file whose display was allowed when it began.
    pub confirmed: bool,
}

/// An inline file waiting for the person to allow its display (`terminalWillReceiveInlineFileNamed:`,
/// `VT100Terminal.m:4031-4056`): its arguments, its data (None for a MultipartFile, whose parts
/// follow the answer), and whether the app has asked.
#[derive(Clone, Debug)]
pub struct InlineHold {
    args: FileArgs,
    data: Option<String>,
    asked: bool,
}

/// iTerm2's guess at whether an inline file is a picture: no type means a picture unless the
/// name has a known text ending; `.ext` with known languages, a language's name, and every
/// other type that is not `image/` or `video/` mean text.
pub fn smells_like_image(kind: Option<&str>, filename: Option<&str>) -> bool {
    use crate::porthole::detect;
    match kind {
        None => filename.is_none_or(|f| detect::for_path(f).is_empty()),
        Some(k) => k.starts_with("image/") || k.starts_with("video/"),
    }
}

pub fn parse_args(s: &str) -> FileArgs {
    use base64::Engine;
    let mut a = FileArgs { name: "Unnamed file".into(), size: None, width: Dim::Auto, height: Dim::Auto, keep_aspect: true, inline: false, kind: None, wide: false, named: false };
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
                    a.named = true;
                }
            }
            "size" => a.size = v.parse().ok(),
            "width" => a.width = dim(v),
            "height" => a.height = dim(v),
            "preserveAspectRatio" => a.keep_aspect = v != "0",
            "inline" => a.inline = v == "1",
            "type" => a.kind = Some(v.to_string()),
            "mode" => a.wide = v == "wide",
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
    // Format characters (a right-to-left override, say) could disguise the file's type.
    let format_char = |c: char| matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}' | '\u{061c}' | '\u{180e}' | '\u{00ad}');
    let clean: String = base
        .chars()
        .filter(|c| !format_char(*c))
        .map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c })
        .collect();
    // Windows drops trailing dots and spaces, and keeps some names for devices.
    let clean = clean.trim_end_matches(['.', ' ']).to_string();
    let stem = clean.split('.').next().unwrap_or_default().to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    let clean = if reserved { format!("_{clean}") } else { clean };
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

/// Saves a download in `dir` under a free name, marked as coming from the internet (the
/// Mark of the Web), so Windows warns before running it.
pub fn save(dir: &std::path::Path, name: &str, bytes: &[u8]) -> std::io::Result<std::path::PathBuf> {
    let path = unique_path(dir, name);
    std::fs::write(&path, bytes)?;
    // The mark lives in an alternate data stream; a drive without them (FAT) cannot keep it,
    // and the file is still saved.
    let _ = std::fs::write(format!("{}:Zone.Identifier", path.display()), "[ZoneTransfer]\r\nZoneId=3\r\n");
    Ok(path)
}

impl Screen {
    /// OSC 1337 `File`, `MultipartFile`, `FilePart`, and `FileEnd`.
    pub fn file_osc(&mut self, key: &str, value: &str) {
        match key {
            "File" => {
                let (args, data) = value.split_once(':').unwrap_or((value, ""));
                self.receive_file(parse_args(args), data);
            }
            "MultipartFile" => {
                let args = parse_args(value);
                self.multipart = None;
                if args.inline {
                    // Asked before its parts arrive, as iTerm2 asks at the header.
                    self.inline_hold = Some(InlineHold { args, data: None, asked: false });
                } else {
                    self.multipart = Some(Multipart { args: Some(args), data: String::new(), confirmed: false });
                }
            }
            "FilePart" => {
                if let Some(m) = &mut self.multipart {
                    m.data.push_str(value);
                    if m.data.len() > self.limits.transfer {
                        self.multipart = None;
                    }
                }
            }
            "FileEnd" => {
                if let Some(m) = self.multipart.take() {
                    let args = m.args.unwrap_or_else(|| parse_args(""));
                    if args.inline && m.confirmed {
                        self.show_inline(&args, &m.data);
                    } else {
                        self.receive_file(args, &m.data);
                    }
                }
            }
            _ => {}
        }
    }

    /// An inline file waits for the person to allow its display (the output after it held);
    /// any other file waits in `downloads` for the app to ask.
    fn receive_file(&mut self, args: FileArgs, data: &str) {
        if args.inline {
            self.inline_hold = Some(InlineHold { args, data: Some(data.to_string()), asked: false });
            return;
        }
        // One download waits at a time, so output cannot bury the window in questions.
        if let (Some(bytes), true) = (decode_base64(data), self.downloads.is_empty()) {
            self.downloads.push(Download { name: args.name, bytes });
        }
    }

    /// The inline file waiting to be asked about, once: its name and declared size (iTerm2
    /// asks with the `size` argument, before the data is read).
    pub fn inline_waiting(&mut self) -> Option<(String, usize)> {
        let hold = self.inline_hold.as_mut().filter(|h| !h.asked)?;
        hold.asked = true;
        Some((hold.args.name.clone(), hold.args.size.unwrap_or(0) as usize))
    }

    /// An inline file is waiting for the person.
    pub fn holding_inline(&self) -> bool {
        self.inline_hold.is_some()
    }

    /// The person answered the waiting inline file: allowed, it shows (a MultipartFile takes
    /// its parts); denied, it is dropped. Then the output held after it is read. Gives the
    /// bytes the terminal must send back, as `feed` does.
    pub fn inline_answered(&mut self, allow: bool) -> Vec<u8> {
        let Some(hold) = self.inline_hold.take() else { return Vec::new() };
        if allow {
            match hold.data {
                Some(data) => self.show_inline(&hold.args, &data),
                None => self.multipart = Some(Multipart { args: Some(hold.args), data: String::new(), confirmed: true }),
            }
        }
        let held = std::mem::take(&mut self.held_output);
        self.feed(&held)
    }

    /// Allows every waiting inline file, as a remembered Yes does (tests that feed files).
    #[cfg(test)]
    pub fn allow_inline(&mut self) -> Vec<u8> {
        let mut reply = Vec::new();
        while self.holding_inline() {
            reply.extend(self.inline_answered(true));
        }
        reply
    }

    /// Shows an inline file (a file that does not decode shows the broken image).
    fn show_inline(&mut self, args: &FileArgs, data: &str) {
        use alacritty_terminal::grid::Dimensions;
        let bytes = decode_base64(data);
        // Text (a type or name that says so, or a picture that does not decode but reads as
        // UTF-8) shows rendered in place.
        let text = bytes.as_deref().and_then(|b| std::str::from_utf8(b).ok()).map(str::to_string);
        let name = args.named.then(|| args.name.clone());
        if let (false, Some(t)) = (smells_like_image(args.kind.as_deref(), name.as_deref()), text.as_deref()) {
            return self.write_text_document(t, args);
        }
        let d = match bytes.as_deref().map(images::decode) {
            Some(Ok(d)) => d,
            _ => match text.as_deref() {
                Some(t) => return self.write_text_document(t, args),
                None => images::broken(),
            },
        };
        let (cw, ch) = self.cell_px();
        let grid = (self.term().columns(), self.term().screen_lines());
        let col = self.term().grid().cursor.point.column.0;
        let (cols, rows) = images::size_in_cells((d.width, d.height), args.width, args.height, args.keep_aspect, (cw as f32, ch as f32), grid, col);
        let image = Image { width_px: d.width, height_px: d.height, cols, rows, frames: d.frames, broken: d.broken, keep_aspect: args.keep_aspect, uid: 0 };
        self.place_image(image, true);
    }

    /// iTerm2's text document: the lines are printed from the start of a line, then those
    /// rows become a porthole.
    fn write_text_document(&mut self, text: &str, args: &FileArgs) {
        use alacritty_terminal::vte::ansi::Handler;
        let feed = |s: &mut Self, start: &mut i32| {
            let before = s.term.grid().cursor.point.line.0;
            s.term.linefeed();
            s.term.carriage_return();
            if s.term.grid().cursor.point.line.0 == before {
                *start -= 1;
            }
        };
        let mut start = self.term.grid().cursor.point.line.0;
        if self.term.grid().cursor.point.column.0 > 0 {
            feed(self, &mut start);
            start = self.term.grid().cursor.point.line.0;
        }
        for line in text.split('\n') {
            for ch in line.trim_end_matches('\r').chars() {
                match ch {
                    '\t' => self.term.put_tab(1),
                    c if (c as u32) < 0x20 || c == '\u{7f}' => {}
                    c => self.term.input(c),
                }
            }
            feed(self, &mut start);
        }
        let last = self.term.grid().cursor.point.line.0 - 1;
        let source = crate::porthole::store::Source { hint: args.kind.clone(), filename: args.named.then(|| args.name.clone()), wide: args.wide || self.porthole_wide_default };
        self.render_range_estimated(start, last, source);
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
            FileArgs { name: "hi.txt".into(), size: Some(3), width: Dim::Pixels(10), height: Dim::Percent(50), keep_aspect: false, inline: true, kind: None, wide: false, named: true }
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
        s.inline_answered(true);
        assert!(s.term().grid()[Line(0)][Column(0)].image().is_some());
        assert!(s.term().grid()[Line(1)][Column(1)].image().is_some());
        assert_eq!(s.images.by_id.len(), 1);
        s.feed(b"\x1b]1337;File=inline=1:!!!notbase64\x07");
        s.inline_answered(true);
        assert!(s.images.by_id.values().any(|i| i.broken));
    }

    #[test]
    fn multipart_files_assemble() {
        let mut s = Screen::new(20, 6);
        s.set_cell_px(4, 4);
        let b = png_b64();
        let (x, y) = b.split_at(b.len() / 2);
        s.feed(b"\x1b]1337;MultipartFile=inline=1\x07");
        s.inline_answered(true);
        s.feed(format!("\x1b]1337;FilePart={x}\x07\x1b]1337;FilePart={y}\x07").as_bytes());
        assert!(s.images.by_id.is_empty());
        s.feed(b"\x1b]1337;FileEnd\x07");
        assert_eq!(s.images.by_id.len(), 1);
        assert!(s.images.by_id.values().all(|i| !i.broken));
    }

    /// iTerm2 asks before every inline file (`File` or `MultipartFile` with inline=1,
    /// `VT100Terminal.m:4031-4056`), pausing the output after it until the answer; a download
    /// (no inline=1) is not held.
    #[test]
    fn inline_files_wait_for_the_person() {
        let mut s = Screen::new(20, 6);
        s.set_cell_px(4, 4);
        let file = format!("\x1b]1337;File=name=YS5wbmc=;size=68;inline=1:{}\x07", png_b64());
        s.feed(format!("{file}after").as_bytes());
        assert!(s.holding_inline());
        assert!(s.images.by_id.is_empty(), "not shown before the answer");
        s.feed(b" more");
        assert_eq!(s.term().grid()[Line(0)][Column(0)].c, ' ', "the output after it waits");
        // Asked once, with the declared size.
        assert_eq!(s.inline_waiting(), Some(("a.png".to_string(), 68)));
        assert_eq!(s.inline_waiting(), None);
        // Denied: not shown, and the held output is read.
        s.inline_answered(false);
        assert!(!s.holding_inline());
        assert!(s.images.by_id.is_empty());
        let row: String = (0..10).map(|c| s.term().grid()[Line(0)][Column(c)].c).collect();
        assert_eq!(row, "after more");
        // Allowed: shown where it came.
        let mut s = Screen::new(20, 6);
        s.set_cell_px(4, 4);
        s.feed(format!("{file}{file}").as_bytes());
        s.inline_answered(true);
        assert_eq!(s.images.by_id.len(), 1);
        assert!(s.holding_inline(), "the second waits in turn");
        s.inline_answered(true);
        assert_eq!(s.images.by_id.len(), 2);
        // A MultipartFile asks at its header; denied, its parts are dropped.
        s.feed(format!("\x1b]1337;MultipartFile=inline=1;size=9\x07\x1b]1337;FilePart={}\x07\x1b]1337;FileEnd\x07", png_b64()).as_bytes());
        assert_eq!(s.inline_waiting(), Some(("Unnamed file".to_string(), 9)));
        s.inline_answered(false);
        assert_eq!(s.images.by_id.len(), 2);
        // A download does not wait.
        s.feed(b"\x1b]1337;File=name=eA==:eA==\x07");
        assert!(!s.holding_inline());
        assert_eq!(s.downloads.len(), 1);
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

    #[test]
    fn download_names_cannot_hide_their_type() {
        let d = std::env::temp_dir().join(format!("tandem-dl2-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        assert_eq!(unique_path(&d, "invoice\u{202e}fdp.exe"), d.join("invoicefdp.exe"));
        assert_eq!(unique_path(&d, "a\u{200b}b.txt. . "), d.join("ab.txt"));
        assert_eq!(unique_path(&d, "CON"), d.join("_CON"));
        assert_eq!(unique_path(&d, "nul.txt"), d.join("_nul.txt"));
    }

    #[test]
    fn one_download_waits_at_a_time() {
        let mut s = Screen::new(20, 6);
        for _ in 0..50 {
            s.feed(b"\x1b]1337;File=name=eA==:eA==\x07");
        }
        assert_eq!(s.downloads.len(), 1);
    }

    #[test]
    fn saved_downloads_are_marked_as_from_the_internet() {
        let d = std::env::temp_dir().join(format!("tandem-dl3-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = save(&d, "x.bin", b"hi").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hi");
        let zone = std::fs::read_to_string(format!("{}:Zone.Identifier", p.display())).unwrap();
        assert!(zone.contains("ZoneId=3"), "{zone}");
    }
}
