//! Session restoration's saved state: every window as an arrangement with contents, the
//! host a session ran in, when to save, and the "Session Contents Restored" banner.

use std::path::Path;
use std::time::{Duration, Instant};

use alacritty_terminal::vte::ansi::Rgb;
use toml::Value;

/// The state file's format. A file with another version is not read.
pub const VERSION: i64 = 1;

/// Per session: the host session it ran in (`{Id, Pid, Offset}`), iTerm2's `Server Dict`; the
/// host instance is the state's `Host Instance`.
pub const HOST_KEY: &str = "Host Session";

/// Quiet time after the last change before a save, and the least time between saves.
pub const QUIET: Duration = Duration::from_secs(5);
pub const FLOOR: Duration = Duration::from_secs(30);
/// The longest a change waits when output never goes quiet.
pub const MAX_WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// The host process instance the sessions ran in, when they ran in one.
    pub host: Option<String>,
    pub windows: Vec<Value>,
}

pub fn encode(s: &State) -> Result<Vec<u8>, String> {
    let mut d = plist::Dictionary::new();
    d.insert("Version".into(), plist::Value::Integer(VERSION.into()));
    if let Some(h) = &s.host {
        d.insert("Host Instance".into(), plist::Value::String(h.clone()));
    }
    d.insert("Windows".into(), plist::Value::Array(s.windows.iter().map(crate::arrangement::to_plist_value).collect()));
    let mut out = Vec::new();
    plist::Value::Dictionary(d).to_writer_binary(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<State, String> {
    let root = plist::Value::from_reader(std::io::Cursor::new(bytes)).map_err(|e| format!("the saved state cannot be read: {e}"))?;
    let d = root.as_dictionary().ok_or("the saved state is not a state file")?;
    let version = d.get("Version").and_then(|v| v.as_signed_integer()).ok_or("the saved state has no version")?;
    if version != VERSION {
        return Err(format!("the saved state has version {version}; this Termurion reads version {VERSION}"));
    }
    let windows = d.get("Windows").and_then(|w| w.as_array()).ok_or("the saved state has no list of windows")?;
    Ok(State {
        host: d.get("Host Instance").and_then(|h| h.as_string()).map(str::to_string),
        windows: windows.iter().filter_map(crate::import::from_plist).collect(),
    })
}

/// Writes `bytes` to a temporary file beside `path`, then moves it over `path`, so a crash
/// mid-write never leaves half a state file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        // On Windows a rename replaces the target (MoveFileExW with REPLACE_EXISTING).
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not write {}: {e}", path.display())
    })
}

/// When to save: `QUIET` after the last change (or `MAX_WAIT` after the first one, when
/// output never stops), and never twice within `FLOOR`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Debounce {
    dirty_at: Option<Instant>,
    /// The first change since the last save.
    first_dirty: Option<Instant>,
    last_write: Option<Instant>,
}

impl Debounce {
    pub fn mark(&mut self, now: Instant) {
        self.dirty_at = Some(now);
        self.first_dirty.get_or_insert(now);
    }

    pub fn dirty(&self) -> bool {
        self.dirty_at.is_some()
    }

    pub fn due(&self, now: Instant) -> bool {
        let (Some(at), Some(first)) = (self.dirty_at, self.first_dirty) else { return false };
        let waited = now.saturating_duration_since(at) >= QUIET || now.saturating_duration_since(first) >= MAX_WAIT;
        waited && self.last_write.is_none_or(|w| now.saturating_duration_since(w) >= FLOOR)
    }

    pub fn written(&mut self, now: Instant) {
        self.dirty_at = None;
        self.first_dirty = None;
        self.last_write = Some(now);
    }
}

/// Saved contents as they may come back: text, CR, LF, TAB, and SGR sequences
/// (`ESC [ digits ; : m`). Every other control character and sequence goes.
pub fn sgr_safe(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        if c != '\x1b' {
            if !c.is_control() || matches!(c, '\r' | '\n' | '\t') {
                out.push(c);
            }
            continue;
        }
        match it.next() {
            Some('[') => {
                let mut seq = String::from("\x1b[");
                let mut sgr = true;
                let mut done = false;
                for d in it.by_ref() {
                    if ('\x40'..='\x7e').contains(&d) {
                        sgr &= d == 'm';
                        seq.push(d);
                        done = true;
                        break;
                    }
                    sgr &= d.is_ascii_digit() || d == ';' || d == ':';
                    seq.push(d);
                }
                if done && sgr {
                    out.push_str(&seq);
                }
            }
            // OSC ends at BEL or ST; DCS, SOS, PM, and APC end at ST.
            Some(k @ (']' | 'P' | 'X' | '^' | '_')) => {
                while let Some(d) = it.next() {
                    if d == '\x07' && k == ']' {
                        break;
                    }
                    if d == '\x1b' && it.peek() == Some(&'\\') {
                        it.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The last `n` lines of `text`.
pub fn cap_lines(text: &str, n: usize) -> &str {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut end = body.len();
    for _ in 0..n {
        match body[..end].rfind('\n') {
            Some(i) => end = i,
            None => return text,
        }
    }
    &text[end + 1..]
}

pub fn banner_text(date: &str) -> String {
    format!("Session Contents Restored on {date}")
}

/// The banner's (background, text) grays from the profile's background: iTerm2's system
/// message colors, lighter on a dark background and darker on a light one.
pub fn banner_grays(bg: Rgb) -> (u8, u8) {
    let b = (0.30 * bg.r as f64 + 0.59 * bg.g as f64 + 0.11 * bg.b as f64) / 255.0;
    let s = if b > 0.5 { -1.0 } else { 1.0 };
    let gray = |k: f64| ((b + k * s).clamp(0.0, 1.0) * 255.0).floor() as u8;
    (gray(0.15), gray(0.4))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(name: &str) -> Value {
        let mut t = toml::Table::new();
        t.insert("Tabs".into(), Value::Array(vec![]));
        t.insert("Title Override".into(), Value::String(name.into()));
        Value::Table(t)
    }

    #[test]
    fn state_round_trip() {
        let s = State { host: Some("abc".into()), windows: vec![window("one"), window("two")] };
        assert_eq!(decode(&encode(&s).unwrap()).unwrap(), s);
        let none = State { host: None, windows: vec![] };
        assert_eq!(decode(&encode(&none).unwrap()).unwrap(), none, "no windows is a state too");
    }

    #[test]
    fn bad_state_is_refused() {
        let good = encode(&State { host: None, windows: vec![window("w")] }).unwrap();
        assert!(decode(&good[..good.len() / 2]).is_err(), "truncated");
        assert!(decode(b"not a plist").is_err());
        let mut d = plist::Dictionary::new();
        d.insert("Version".into(), plist::Value::Integer(99.into()));
        d.insert("Windows".into(), plist::Value::Array(vec![]));
        let mut other = Vec::new();
        plist::Value::Dictionary(d).to_writer_binary(&mut other).unwrap();
        let e = decode(&other).unwrap_err();
        assert!(e.contains("version"), "{e}");
        let mut arr = Vec::new();
        plist::Value::Array(vec![]).to_writer_binary(&mut arr).unwrap();
        assert!(decode(&arr).is_err(), "an arrangement file is not a state");
    }

    #[test]
    fn debounce_waits_and_floors() {
        let t0 = Instant::now();
        let mut d = Debounce::default();
        assert!(!d.due(t0), "nothing changed");
        d.mark(t0);
        assert!(!d.due(t0 + Duration::from_secs(4)));
        assert!(d.due(t0 + Duration::from_secs(5)));
        d.written(t0 + Duration::from_secs(5));
        assert!(!d.dirty());
        d.mark(t0 + Duration::from_secs(6));
        assert!(!d.due(t0 + Duration::from_secs(20)), "quiet, but within the floor");
        assert!(d.due(t0 + Duration::from_secs(35)));
        d.mark(t0 + Duration::from_secs(34));
        assert!(!d.due(t0 + Duration::from_secs(36)), "a new change restarts the quiet time");
        // Output that never stops: a save still comes once the first change waited MAX_WAIT.
        let mut busy = Debounce::default();
        for i in 0..=70 {
            busy.mark(t0 + Duration::from_secs(i));
            let due = busy.due(t0 + Duration::from_secs(i));
            assert_eq!(due, i >= 60, "second {i}");
        }
    }

    #[test]
    fn only_sgr_survives() {
        let saved = "\x1b[0;31mred\x1b[0m \x1b]52;c;SGVsbG8=\x07\x1b]0;title\x07\x1b[?1049h\x1b[2J\x1b[38:2:1:2:3mx\x1bPq#\x1b\\ok\r\n\ttab\x05\x7f";
        assert_eq!(sgr_safe(saved), "\x1b[0;31mred\x1b[0m \x1b[38:2:1:2:3mxok\r\n\ttab");
        assert_eq!(sgr_safe("\x1b[31"), "", "an unfinished sequence goes");
        assert_eq!(sgr_safe("a\x1b"), "a");
        assert_eq!(sgr_safe("\u{9b}31m"), "31m", "C1 CSI is a control, not a sequence start");
    }

    #[test]
    fn contents_are_capped() {
        assert_eq!(cap_lines("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(cap_lines("a\nb", 5), "a\nb");
        assert_eq!(cap_lines("a\nb\n", 1), "b\n", "a final newline ends the last line");
        assert_eq!(cap_lines("", 3), "");
    }

    #[test]
    fn banner_grays_follow_the_background() {
        assert_eq!(banner_text("Sep 25, 2026 at 3:04 PM"), "Session Contents Restored on Sep 25, 2026 at 3:04 PM");
        // Black: brightness 0, lighter grays.
        assert_eq!(banner_grays(Rgb { r: 0, g: 0, b: 0 }), (38, 102));
        // White: brightness 1, darker grays.
        assert_eq!(banner_grays(Rgb { r: 255, g: 255, b: 255 }), (216, 153));
    }

    #[test]
    fn atomic_write_replaces() {
        let dir = std::env::temp_dir().join(format!("tandem-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "no temporary file stays: {left:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
