//! The kitty graphics protocol: `ESC _ G <keys> ; <base64 payload> ESC \`. iTerm2 carries
//! transmit, display, delete, and query (animation is answered but not run); so does Termurion.

use std::collections::HashMap;

use crate::screen::Screen;

/// A parsed command: its keys (one letter each) and its payload (still base64).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Command {
    pub keys: HashMap<char, String>,
    pub payload: Vec<u8>,
}

impl Command {
    pub fn char(&self, k: char, default: char) -> char {
        self.keys.get(&k).and_then(|v| v.chars().next()).unwrap_or(default)
    }

    pub fn num(&self, k: char) -> Option<u32> {
        self.keys.get(&k).and_then(|v| v.parse().ok())
    }
}

/// Kitty image data kept at most (iTerm2's cache budget); the oldest goes first.
pub const BUDGET: usize = 320 << 20;

/// Stored kitty images and a transfer in progress.
#[derive(Clone, Debug, Default)]
pub struct Kitty {
    /// By kitty image id: the decoded picture (width, height, RGBA) and when it came.
    pub images: HashMap<u32, (u32, u32, std::sync::Arc<Vec<u8>>, u64)>,
    /// Image number (I) to the id Termurion gave it.
    pub numbers: HashMap<u32, u32>,
    /// Placements: kitty image id, placement id, and the image store id showing it.
    pub placements: Vec<(u32, u32, u32)>,
    /// The first chunk's command and the payload so far, while `m=1` chunks arrive.
    pub pending: Option<Command>,
    next_id: u32,
    arrivals: u64,
}

pub fn parse(body: &[u8]) -> Result<Command, String> {
    let text = String::from_utf8_lossy(body);
    let (keys, payload) = text.split_once(';').unwrap_or((&text, ""));
    let mut cmd = Command { keys: HashMap::new(), payload: payload.as_bytes().to_vec() };
    for part in keys.split(',').filter(|p| !p.is_empty()) {
        let (k, v) = part.split_once('=').ok_or_else(|| format!("Bad key {part:?}"))?;
        let mut chars = k.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) => {
                cmd.keys.insert(c, v.to_string());
            }
            _ => return Err(format!("Bad key {k:?}")),
        }
    }
    Ok(cmd)
}

/// Pixels a kitty image may have on a side.
const MAX_SIDE: u32 = crate::images::MAX_SIDE;
/// A file or inflated payload may be at most this large.
const MAX_BYTES: u64 = 400 << 20;

fn base64_lenient(data: &[u8]) -> Option<Vec<u8>> {
    use base64::Engine;
    let compact: Vec<u8> = data.iter().copied().filter(|b| !b.is_ascii_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(&compact)
        .ok()
        .or_else(|| base64::engine::general_purpose::STANDARD_NO_PAD.decode(compact.strip_suffix(b"=").unwrap_or(&compact)).ok())
}

/// Every refusal of a file gets this one answer, so answers never tell whether a file
/// exists.
fn bad_file() -> String {
    "EBADF:Cannot use the file".into()
}

/// A file a program asked Termurion to read: only a regular local file (no share, device, or
/// folder); a temporary file (`t=t`) only in the temp folder with kitty's marker in its
/// name, deleted after reading.
fn read_file(raw: &[u8], temporary: bool) -> Option<Vec<u8>> {
    let path = std::str::from_utf8(raw).ok()?;
    // The one judge (network and device paths, mapped and SUBST drives, links to shares).
    let path = crate::localfs::LocalPath::judge(path)?;
    let meta = crate::localfs::stat(&path).ok()?;
    if !meta.is_file || meta.len > MAX_BYTES {
        return None;
    }
    if temporary {
        let tmp = crate::localfs::canonicalize(&crate::localfs::LocalPath::judge(std::env::temp_dir())?)?;
        let real = crate::localfs::canonicalize(&path)?;
        let named = real.path().file_name()?.to_string_lossy().contains("tty-graphics-protocol");
        if !real.path().starts_with(tmp.path()) || !named {
            return None;
        }
        let bytes = crate::localfs::read(&real).ok();
        let _ = crate::localfs::remove_file(&real);
        return bytes;
    }
    crate::localfs::read(&path).ok()
}

/// The picture a transmit or query carries: width, height, RGBA.
fn decode_data(cmd: &Command) -> Result<(u32, u32, Vec<u8>), String> {
    // A file that is read but is not a picture gets the same answer as one that cannot be read.
    let from_file = matches!(cmd.char('t', 'd'), 'f' | 't');
    decode_payload(cmd).map_err(|e| if from_file { bad_file() } else { e })
}

fn decode_payload(cmd: &Command) -> Result<(u32, u32, Vec<u8>), String> {
    let raw = base64_lenient(&cmd.payload).ok_or("EINVAL:The payload is not base64")?;
    let bytes = match cmd.char('t', 'd') {
        'd' => raw,
        m @ ('f' | 't') => read_file(&raw, m == 't').ok_or_else(bad_file)?,
        's' => return Err("EINVAL:Shared memory is not supported".into()),
        _ => return Err("EINVAL:Unknown transmission medium".into()),
    };
    let bytes = if cmd.char('o', ' ') == 'z' {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(bytes.as_slice()).take(MAX_BYTES).read_to_end(&mut out).map_err(|_| "EINVAL:The zlib data is bad")?;
        out
    } else {
        bytes
    };
    match cmd.num('f').unwrap_or(32) {
        100 => {
            let d = crate::images::decode(&bytes).map_err(|e| format!("EBADPNG:{e}"))?;
            let f = d.frames.into_iter().next().ok_or("EBADPNG:No frames")?;
            Ok((d.width, d.height, std::sync::Arc::try_unwrap(f.rgba).unwrap_or_else(|a| (*a).clone())))
        }
        bpp @ (24 | 32) => {
            let (w, h) = (cmd.num('s').unwrap_or(0), cmd.num('v').unwrap_or(0));
            if w == 0 || h == 0 || w > MAX_SIDE || h > MAX_SIDE {
                return Err("EINVAL:Bad image size".into());
            }
            let px = (bpp / 8) as usize;
            if bytes.len() != w as usize * h as usize * px {
                return Err("EINVAL:The data does not match the image size".into());
            }
            let rgba = if px == 4 { bytes } else { bytes.as_chunks::<3>().0.iter().flat_map(|c| [c[0], c[1], c[2], 255]).collect() };
            Ok((w, h, rgba))
        }
        _ => Err("EINVAL:Unknown format".into()),
    }
}

impl Screen {
    /// Runs one kitty graphics command (the APC body after `G`); any reply goes to the
    /// program through `feed`'s reply.
    pub fn kitty(&mut self, body: &[u8]) {
        let Ok(cmd) = parse(body) else { return };
        // Chunks: the first carries the keys; later ones add payload until m=0.
        let more = cmd.num('m') == Some(1);
        let cmd = match self.kitty.pending.take() {
            Some(mut first) => {
                first.payload.extend_from_slice(&cmd.payload);
                if first.payload.len() > self.limits.transfer {
                    return;
                }
                if more {
                    self.kitty.pending = Some(first);
                    return;
                }
                first
            }
            None if more => {
                if cmd.payload.len() <= self.limits.transfer {
                    self.kitty.pending = Some(cmd);
                }
                return;
            }
            None => cmd,
        };
        let result = match cmd.char('a', 't') {
            'q' => decode_data(&cmd).map(|_| ()),
            a @ ('t' | 'T') => match decode_data(&cmd) {
                Err(e) => Err(e),
                Ok(pic) => {
                    let id = self.kitty_store(&cmd, pic);
                    if a == 'T' { self.kitty_display(id, &cmd) } else { Ok(()) }
                }
            },
            'p' => match self.kitty_id(&cmd) {
                Some(id) if self.kitty.images.contains_key(&id) => self.kitty_display(id, &cmd),
                _ => Err("ENOENT:No such image".into()),
            },
            'd' => {
                self.kitty_delete(&cmd);
                return;
            }
            'a' | 'f' | 'c' => Ok(()),
            _ => Err("EINVAL:Unknown action".into()),
        };
        self.kitty_reply(&cmd, result);
    }

    fn kitty_id(&self, cmd: &Command) -> Option<u32> {
        cmd.num('i').or_else(|| cmd.num('I').and_then(|n| self.kitty.numbers.get(&n).copied()))
    }

    /// Keeps a transmitted picture under its id (a new one for an image number), dropping
    /// the oldest pictures past the budget.
    fn kitty_store(&mut self, cmd: &Command, (w, h, px): (u32, u32, Vec<u8>)) -> u32 {
        let k = &mut self.kitty;
        let id = match (cmd.num('i'), cmd.num('I')) {
            (Some(i), _) => i,
            (None, Some(n)) => {
                k.next_id = k.next_id.wrapping_add(1).max(1 << 31);
                k.numbers.insert(n, k.next_id);
                k.next_id
            }
            (None, None) => {
                k.next_id = k.next_id.wrapping_add(1).max(1 << 31);
                k.next_id
            }
        };
        k.arrivals += 1;
        k.images.insert(id, (w, h, std::sync::Arc::new(px), k.arrivals));
        while k.images.values().map(|i| i.2.len()).sum::<usize>() > BUDGET {
            let Some(oldest) = k.images.iter().min_by_key(|(_, i)| i.3).map(|(id, _)| *id) else { break };
            k.images.remove(&oldest);
        }
        id
    }

    /// Shows a stored picture at the cursor: its source rectangle (x, y, w, h), in `c`
    /// columns and `r` rows when given; `C=1` leaves the cursor where it is.
    fn kitty_display(&mut self, id: u32, cmd: &Command) -> Result<(), String> {
        use alacritty_terminal::grid::Dimensions;
        use crate::images::{Dim, Frame, Image};
        let (w, h, px, _) = self.kitty.images.get(&id).cloned().ok_or("ENOENT:No such image")?;
        let x = cmd.num('x').unwrap_or(0).min(w.saturating_sub(1));
        let y = cmd.num('y').unwrap_or(0).min(h.saturating_sub(1));
        let sw = cmd.num('w').filter(|v| *v > 0).unwrap_or(w - x).min(w - x);
        let sh = cmd.num('h').filter(|v| *v > 0).unwrap_or(h - y).min(h - y);
        let (pixels, pw, ph) = if (x, y, sw, sh) == (0, 0, w, h) {
            (px, w, h)
        } else {
            let mut out = Vec::with_capacity((sw * sh * 4) as usize);
            for row in y..y + sh {
                let start = ((row * w + x) * 4) as usize;
                out.extend_from_slice(&px[start..start + (sw * 4) as usize]);
            }
            (std::sync::Arc::new(out), sw, sh)
        };
        let (cols, rows) = (cmd.num('c'), cmd.num('r'));
        let (cw, ch) = self.cell_px();
        let grid = (self.term().columns(), self.term().screen_lines());
        let col = self.term().grid().cursor.point.column.0;
        let dim = |v: Option<u32>| v.filter(|n| *n > 0).map_or(Dim::Auto, Dim::Cells);
        let keep = cols.is_none() || rows.is_none();
        let (c, r) = crate::images::size_in_cells((pw, ph), dim(cols), dim(rows), keep, (cw as f32, ch as f32), grid, col);
        let image = Image { width_px: pw, height_px: ph, cols: c, rows: r, frames: vec![Frame { rgba: pixels, delay_ms: 0 }], broken: false, keep_aspect: keep, uid: 0, filename: None, paused: None };
        let store_id = self.place_image(image, cmd.num('C') != Some(1));
        self.kitty.placements.push((id, cmd.num('p').unwrap_or(0), store_id));
        Ok(())
    }

    /// Removes placements from the screen, chosen by `d` (upper and lower case alike, as in
    /// iTerm2).
    fn kitty_delete(&mut self, cmd: &Command) {
        use alacritty_terminal::grid::Dimensions;
        use alacritty_terminal::index::{Column, Line};
        let which = cmd.char('d', 'a').to_ascii_lowercase();
        let id = self.kitty_id(cmd);
        let placements = self.kitty.placements.clone();
        let by = |f: &dyn Fn(&(u32, u32, u32)) -> bool| placements.iter().filter(|p| f(p)).map(|p| p.2).collect::<Vec<u32>>();
        // The store ids showing at a cell (1-based column and row on screen).
        let at = |s: &Screen, x: u32, y: u32| -> Vec<u32> {
            let g = s.term().grid();
                if x == 0 || y == 0 || y as usize > g.screen_lines() || x as usize > g.columns() {
                return Vec::new();
            }
            let (l, c) = (y as i32 - 1, x as usize);
            g[Line(l)][Column(c - 1)].image().map(|t| t.image).into_iter().collect()
        };
        let cursor = self.term().grid().cursor.point;
        let gone: Vec<u32> = match which {
            'a' => by(&|_| true),
            'i' | 'n' => {
                let p = cmd.num('p');
                by(&|pl| Some(pl.0) == id && p.is_none_or(|p| p == pl.1))
            }
            'r' => {
                let (lo, hi) = (cmd.num('x').unwrap_or(0), cmd.num('y').unwrap_or(0));
                by(&|pl| (lo..=hi).contains(&pl.0))
            }
            'c' => at(self, cursor.column.0 as u32 + 1, cursor.line.0.max(0) as u32 + 1),
            'p' | 'q' => at(self, cmd.num('x').unwrap_or(0), cmd.num('y').unwrap_or(0)),
            'x' | 'y' => {
                let n = if which == 'x' { cmd.num('x') } else { cmd.num('y') }.unwrap_or(0);
                let g = self.term().grid();
                let mut ids = Vec::new();
                let bound = if which == 'x' { g.columns() } else { g.screen_lines() };
                if n > 0 && n as usize <= bound {
                    let cells: Vec<(i32, usize)> = if which == 'x' {
                        (0..g.screen_lines() as i32).map(|l| (l, n as usize - 1)).collect()
                    } else {
                        (0..g.columns()).map(|c| (n as i32 - 1, c)).collect()
                    };
                    for (l, c) in cells {
                        if l < g.screen_lines() as i32 && c < g.columns() {
                            ids.extend(g[Line(l)][Column(c)].image().map(|t| t.image));
                        }
                    }
                }
                ids
            }
            _ => Vec::new(),
        };
        if gone.is_empty() {
            return;
        }
        let grid = self.term_mut().grid_mut();
        let top = -(grid.history_size() as i32);
        for l in top..grid.screen_lines() as i32 {
            for c in 0..grid.columns() {
                let cell = &mut grid[Line(l)][Column(c)];
                if cell.image().is_some_and(|t| gone.contains(&t.image)) {
                    cell.set_image(None);
                }
            }
        }
        self.kitty.placements.retain(|p| !gone.contains(&p.2));
    }

    fn kitty_reply(&mut self, cmd: &Command, result: Result<(), String>) {
        let quiet = cmd.num('q').unwrap_or(0);
        let (i, n) = (cmd.num('i'), cmd.num('I'));
        if i.is_none() && n.is_none() {
            return;
        }
        let text = match result {
            Ok(()) if quiet >= 1 => return,
            Err(_) if quiet >= 2 => return,
            Ok(()) => "OK".to_string(),
            Err(e) => e,
        };
        let mut head = Vec::new();
        if let Some(i) = i.or_else(|| n.and_then(|n| self.kitty.numbers.get(&n).copied())) {
            head.push(format!("i={i}"));
        }
        if let Some(n) = n {
            head.push(format!("I={n}"));
        }
        if let Some(p) = cmd.num('p') {
            head.push(format!("p={p}"));
        }
        self.replies.extend_from_slice(format!("\x1b_G{};{text}\x1b\\", head.join(",")).as_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::index::{Column, Line};
    use base64::Engine;

    fn b64(b: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    fn send(s: &mut Screen, keys: &str, data: &[u8]) -> String {
        String::from_utf8(s.feed(format!("\x1b_G{keys};{}\x1b\\", b64(data)).as_bytes())).unwrap()
    }

    #[test]
    fn kitty_keys_parse() {
        let c = parse(b"a=T,f=100,i=5,q=2;AAAA").unwrap();
        assert_eq!((c.char('a', 't'), c.num('f'), c.num('i'), c.num('q')), ('T', Some(100), Some(5), Some(2)));
        assert_eq!(c.payload, b"AAAA");
        assert_eq!(parse(b"i=1").unwrap().payload, b"");
        assert!(parse(b"a").is_err());
    }

    #[test]
    fn transmit_and_display_places_tiles() {
        let mut s = Screen::new(20, 6);
        s.set_cell_px(1, 1);
        let reply = send(&mut s, "a=T,f=24,s=2,v=1,i=3", &[255, 0, 0, 0, 0, 255]);
        assert_eq!(reply, "\x1b_Gi=3;OK\x1b\\");
        assert!(s.term().grid()[Line(0)][Column(0)].image().is_some());
        assert!(s.term().grid()[Line(0)][Column(1)].image().is_some());
        // Transmit only, then show it with a=p.
        let mut t = Screen::new(20, 6);
        t.set_cell_px(1, 1);
        assert_eq!(send(&mut t, "a=t,f=32,s=1,v=1,i=9", &[1, 2, 3, 4]), "\x1b_Gi=9;OK\x1b\\");
        assert!(t.term().grid()[Line(0)][Column(0)].image().is_none());
        assert_eq!(send(&mut t, "a=p,i=9,c=3,r=2", b""), "\x1b_Gi=9;OK\x1b\\");
        assert!(t.term().grid()[Line(1)][Column(2)].image().is_some());
    }

    #[test]
    fn chunks_and_zlib_assemble() {
        use std::io::Write;
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&[9, 9, 9, 255]).unwrap();
        let text = b64(&z.finish().unwrap());
        let (a, b) = text.split_at(text.len() / 2);
        let mut s = Screen::new(20, 6);
        let r1 = s.feed(format!("\x1b_Ga=t,f=32,s=1,v=1,o=z,i=4,m=1;{a}\x1b\\").as_bytes());
        assert!(r1.is_empty(), "no answer while chunks come");
        let r2 = String::from_utf8(s.feed(format!("\x1b_Gm=0;{b}\x1b\\").as_bytes())).unwrap();
        assert_eq!(r2, "\x1b_Gi=4;OK\x1b\\");
        assert_eq!(s.kitty.images.get(&4).map(|i| i.2[0]), Some(9));
    }

    #[test]
    fn deletes_by_id() {
        let mut s = Screen::new(20, 6);
        s.set_cell_px(1, 1);
        send(&mut s, "a=T,f=32,s=1,v=1,i=3,q=2", &[1, 2, 3, 4]);
        assert!(s.term().grid()[Line(0)][Column(0)].image().is_some());
        send(&mut s, "a=d,d=i,i=3,q=2", b"");
        assert!(s.term().grid()[Line(0)][Column(0)].image().is_none());
        send(&mut s, "a=T,f=32,s=1,v=1,i=5,q=2", &[1, 2, 3, 4]);
        send(&mut s, "a=d,q=2", b"");
        assert!((0..6).all(|l| (0..20).all(|c| s.term().grid()[Line(l)][Column(c)].image().is_none())));
    }

    #[test]
    fn bad_kitty_commands_reply_errors() {
        let mut s = Screen::new(20, 6);
        assert!(send(&mut s, "a=t,f=24,s=2,v=2,i=4", &[1, 2, 3]).contains("EINVAL"));
        assert!(send(&mut s, "a=t,t=s,i=4", b"/shm").contains("EINVAL"));
        assert!(send(&mut s, "a=t,f=32,s=100000,v=100000,i=4", &[0; 16]).contains("EINVAL"));
        assert!(send(&mut s, "a=p,i=77", b"").contains("ENOENT"));
        assert!(s.feed(b"\x1b_Ga=t,f=32,i=4;!!!\x1b\\").starts_with(b"\x1b_Gi=4;E"));
        assert_eq!(send(&mut s, "a=t,f=24,s=2,v=2", &[1]), "", "no id, no answer");
        let _ = s.feed(b"\x1b_G\x1b\\\x1b_Gzz,,==;\x1b\\");
    }

    #[test]
    fn quiet_levels() {
        let mut s = Screen::new(20, 6);
        assert_eq!(send(&mut s, "a=t,f=32,s=1,v=1,i=1,q=1", &[1, 2, 3, 4]), "");
        assert!(send(&mut s, "a=t,f=24,s=9,v=9,i=1,q=1", &[1]).contains("EINVAL"));
        assert_eq!(send(&mut s, "a=t,f=24,s=9,v=9,i=1,q=2", &[1]), "");
        assert_eq!(send(&mut s, "a=q,f=32,s=1,v=1,i=31", &[1, 2, 3, 4]), "\x1b_Gi=31;OK\x1b\\");
        assert!(s.kitty.images.get(&31).is_none(), "a query stores nothing");
    }

    #[test]
    fn huge_delete_coordinates_are_harmless() {
        let mut s = Screen::new(20, 6);
        for keys in ["a=d,d=y,y=3000000000", "a=d,d=p,x=1,y=2147483649", "a=d,d=x,x=4294967295", "a=d,d=q,x=99,y=99", "a=d,d=c"] {
            send(&mut s, keys, b"");
        }
    }

    #[test]
    fn file_media_refuse_what_they_should() {
        let tmp = std::env::temp_dir();
        let d = tmp.join(format!("termurion-kitty-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let mut s = Screen::new(20, 6);
        let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
        // A folder, a share, and a missing file all get the same answer.
        let folder = send(&mut s, "a=q,t=f,i=1", path(&d).as_bytes());
        let share = send(&mut s, "a=q,t=f,i=1", b"\\\\server\\share\\x.png");
        let missing = send(&mut s, "a=q,t=f,i=1", path(&d.join("none.png")).as_bytes());
        let text = d.join("words.txt");
        std::fs::write(&text, "not an image").unwrap();
        let not_image = send(&mut s, "a=q,t=f,i=1", path(&text).as_bytes());
        assert!(folder.contains("EBADF"), "{folder}");
        assert_eq!(folder, share);
        assert_eq!(folder, missing);
        assert_eq!(folder, not_image, "no answer tells whether a file exists");
        // t=t deletes only temporary files in the temp folder.
        let outside = std::env::current_dir().unwrap().join("tty-graphics-protocol-keep.bin");
        std::fs::write(&outside, [1, 2, 3, 4]).unwrap();
        assert!(send(&mut s, "a=t,t=t,f=32,s=1,v=1,i=2", path(&outside).as_bytes()).contains("EBADF"));
        assert!(outside.exists(), "a file outside the temp folder is never deleted");
        std::fs::remove_file(&outside).unwrap();
        let inside = tmp.join(format!("tty-graphics-protocol-{}.bin", std::process::id()));
        std::fs::write(&inside, [1, 2, 3, 4]).unwrap();
        assert_eq!(send(&mut s, "a=t,t=t,f=32,s=1,v=1,i=2", path(&inside).as_bytes()), "\x1b_Gi=2;OK\x1b\\");
        assert!(!inside.exists());
    }

    #[test]
    fn transfers_stop_at_their_cap() {
        let mut s = Screen::new(20, 6);
        s.limits.transfer = 64;
        let chunk = "A".repeat(40);
        s.feed(format!("\x1b_Ga=t,f=32,s=1,v=1,i=4,m=1;{chunk}\x1b\\").as_bytes());
        s.feed(format!("\x1b_Gm=1;{chunk}\x1b\\").as_bytes());
        assert!(s.kitty.pending.is_none(), "dropped past the cap");
        s.feed(b"\x1b]1337;MultipartFile=inline=1\x07");
        s.inline_answered(true);
        for _ in 0..3 {
            s.feed(format!("\x1b]1337;FilePart={chunk}\x07").as_bytes());
        }
        assert!(s.multipart.is_none());
    }

    #[test]
    fn program_chosen_share_paths_are_never_read() {
        // Every spelling of a share, none probed (a dead address takes about 21 s to answer).
        let t = std::time::Instant::now();
        for p in [r"\\10.255.255.1\x\y", "//10.255.255.1/x", r"\/10.255.255.1/x", r"/\10.255.255.1\x", r"\\?\UNC\10.255.255.1\x"] {
            assert_eq!(read_file(p.as_bytes(), false), None, "{p}");
        }
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "{:?}", t.elapsed());
    }
}
