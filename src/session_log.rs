//! Session logging as iTerm2 does it (`iTermLoggingHelper.m`, `HTMLEncoding.swift`): four
//! formats, Windows-safe file names, and a file writer that lives on its own thread.
//!
//! A log is a security and integrity surface: the file name is sanitized so it never leaves
//! its folder or names a device, the file system is touched only on the writer thread (a
//! network folder never stalls the UI), the queue between them is bounded, and a logger
//! that was stopped or failed writes nothing more.

use crate::palette::Palette;
use crate::stamps::Parts;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use alacritty_terminal::vte::{Params, Parser, Perform};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// iTerm2's `iTermLoggingStyle` (`LH`): the numbers are the stored values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Raw,
    PlainText,
    Html,
    Asciicast,
}

impl Style {
    /// An unknown number is Raw, as `iTermLoggingStyleRaw = 0` is the fall-through.
    pub fn from_int(v: i64) -> Style {
        match v {
            1 => Style::PlainText,
            2 => Style::Html,
            3 => Style::Asciicast,
            _ => Style::Raw,
        }
    }

    /// Takes the bytes the program wrote (`LH:221-243`).
    pub fn raw(self) -> bool {
        matches!(self, Style::Raw | Style::Asciicast)
    }

    /// Takes the cooked text.
    pub fn cooked(self) -> bool {
        matches!(self, Style::PlainText | Style::Html)
    }
}

pub const STYLE_LABELS: [&str; 4] = ["Raw data", "Plain text", "HTML", "ASCIInema"];
pub const LOGGING_STYLE_KEY: &str = "NoSyncLoggingStyle";
pub const FOLDER_HEADING: &str = "Logging Folder Not Found";
pub const CANNOT_WRITE: &str = "Couldn\u{2019}t write to session log";
pub const LOGGING_PROBLEM_HEADING: &str = "Session Logging Problem";
pub const EXISTS_BODY: &str = "A file or folder with the same name already exists. Replacing it will overwrite its current contents.";
pub const REPLACE: &str = "Replace";
pub const APPEND: &str = "Append";

pub fn create_folder_text(dir: &str) -> String {
    format!("Would you like to create the directory at {dir}?")
}

pub fn cannot_start_text(profile: &str, error: &str) -> String {
    format!("Cannot start logging to session with profile \u{201c}{profile}\u{201d}: {error}")
}

pub fn exists_heading(file: &str) -> String {
    format!("\u{201c}{file}\u{201d} already exists. Do you want to replace it or append to it?")
}

// ---------------------------------------------------------------- file names

/// The longest file name taken; Windows allows 255 UTF-16 units and the folder needs room.
const MAX_NAME: usize = 200;

/// What stands in for a name with nothing left in it.
const FALLBACK_NAME: &str = "session.log";

/// A formatted log name made safe for Windows: iTerm2's "/" becomes "__" (`PSA:160-166`),
/// then every character Windows forbids becomes "_", so no separator survives and the name
/// cannot leave its folder. Beyond iTerm2: control characters become "_", trailing dots
/// and spaces (which Windows drops) go, a name of only dots is replaced, a reserved device
/// name (`CON`, `NUL`, `COM1`, with or without an extension) gets a leading "_", and the
/// length is bounded.
pub fn safe_file_name(formatted: &str) -> String {
    let mut name: String = formatted
        .replace('/', "__")
        .chars()
        .map(|c| if matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    if name.chars().count() > MAX_NAME {
        name = name.chars().take(MAX_NAME).collect();
    }
    let trimmed = name.trim_end_matches(['.', ' ']);
    if trimmed.trim_start_matches(' ').is_empty() {
        return FALLBACK_NAME.to_string();
    }
    let name = trimmed.to_string();
    let stem = name.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let numbered = |prefix: &str| stem.strip_prefix(prefix).is_some_and(|n| matches!(n.as_bytes(), [b'1'..=b'9']));
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered("COM") || numbered("LPT") {
        return format!("_{name}");
    }
    name
}

/// `PSA:160-166`: the folder (empty is the home folder, a leading "~" is the home folder)
/// joined with the safe file name.
pub fn join_log_path(folder: &str, formatted_name: &str, home: &Path) -> PathBuf {
    let folder = folder.trim();
    let dir = if folder.is_empty() || folder == "~" {
        home.to_path_buf()
    } else if let Some(rest) = folder.strip_prefix("~/").or_else(|| folder.strip_prefix("~\\")) {
        home.join(rest.trim_start_matches(['/', '\\']))
    } else {
        PathBuf::from(folder)
    };
    dir.join(safe_file_name(formatted_name))
}

// ---------------------------------------------------------------- cooked text

/// The SGR state a run of text was printed with. `None` colors are the defaults.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attr {
    pub fg: Option<Color>,
    pub bg: Option<Color>,
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    /// 0 none, 1 single, 2 double, 3 curly, 4 dotted, 5 dashed.
    pub underline: u8,
    pub strike: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cooked {
    Text(String, Attr),
    Newline,
}

/// Printable text and line feeds out of the program's output, with the colors and styles it
/// was printed in. Every other control and sequence is dropped.
pub struct Cooker {
    parser: Parser,
    state: CookState,
}

#[derive(Default)]
struct CookState {
    attr: Attr,
    run: String,
    out: Vec<Cooked>,
}

impl Cooker {
    pub fn new() -> Cooker {
        Cooker { parser: Parser::new(), state: CookState::default() }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Cooked> {
        self.parser.advance(&mut self.state, bytes);
        self.state.flush();
        std::mem::take(&mut self.state.out)
    }
}

impl Default for Cooker {
    fn default() -> Cooker {
        Cooker::new()
    }
}

impl CookState {
    fn flush(&mut self) {
        if !self.run.is_empty() {
            self.out.push(Cooked::Text(std::mem::take(&mut self.run), self.attr));
        }
    }

    fn sgr(&mut self, params: &Params) {
        let list: Vec<&[u16]> = params.iter().collect();
        let mut at = self.attr;
        if list.is_empty() {
            at = Attr::default();
        }
        let mut i = 0;
        while i < list.len() {
            let p = list[i];
            i += 1;
            match p {
                [0] => at = Attr::default(),
                [1] => at.bold = true,
                [2] => at.faint = true,
                [3] => at.italic = true,
                [4] => at.underline = 1,
                [4, sub, ..] => at.underline = match sub {
                    0 => 0,
                    2 => 2,
                    3 => 3,
                    4 => 4,
                    5 => 5,
                    _ => 1,
                },
                [9] => at.strike = true,
                [21] => at.underline = 2,
                [22] => {
                    at.bold = false;
                    at.faint = false;
                }
                [23] => at.italic = false,
                [24] => at.underline = 0,
                [29] => at.strike = false,
                [n @ 30..=37] => at.fg = Some(Color::Named(ansi_name(*n as usize - 30))),
                [39] => at.fg = None,
                [n @ 40..=47] => at.bg = Some(Color::Named(ansi_name(*n as usize - 40))),
                [49] => at.bg = None,
                [n @ 90..=97] => at.fg = Some(Color::Named(ansi_name(*n as usize - 90 + 8))),
                [n @ 100..=107] => at.bg = Some(Color::Named(ansi_name(*n as usize - 100 + 8))),
                [38] | [48] => {
                    // The semicolon form: the kind and its numbers are the next parameters.
                    let rest: Vec<u16> = list[i..].iter().map(|s| s[0]).collect();
                    let (color, used) = extended(&rest, false);
                    i += used;
                    if let Some(c) = color {
                        if p[0] == 38 {
                            at.fg = Some(c);
                        } else {
                            at.bg = Some(c);
                        }
                    }
                }
                [38, rest @ ..] | [48, rest @ ..] => {
                    if let (Some(c), _) = extended(rest, true) {
                        if p[0] == 38 {
                            at.fg = Some(c);
                        } else {
                            at.bg = Some(c);
                        }
                    }
                }
                _ => {}
            }
        }
        if at != self.attr {
            self.flush();
            self.attr = at;
        }
    }
}

fn ansi_name(i: usize) -> NamedColor {
    const NAMES: [NamedColor; 16] = [
        NamedColor::Black,
        NamedColor::Red,
        NamedColor::Green,
        NamedColor::Yellow,
        NamedColor::Blue,
        NamedColor::Magenta,
        NamedColor::Cyan,
        NamedColor::White,
        NamedColor::BrightBlack,
        NamedColor::BrightRed,
        NamedColor::BrightGreen,
        NamedColor::BrightYellow,
        NamedColor::BrightBlue,
        NamedColor::BrightMagenta,
        NamedColor::BrightCyan,
        NamedColor::BrightWhite,
    ];
    NAMES[i.min(15)]
}

/// The numbers after 38 or 48: `5;n` or `2;r;g;b` in the semicolon form, `5:n` or
/// `2:[colorspace:]r:g:b` in the colon form. Returns the color and how many parameters the
/// semicolon form consumed.
fn extended(rest: &[u16], colon: bool) -> (Option<Color>, usize) {
    let byte = |v: u16| u8::try_from(v).ok();
    let rgb = |r: u16, g: u16, b: u16| match (byte(r), byte(g), byte(b)) {
        (Some(r), Some(g), Some(b)) => Some(Color::Spec(Rgb { r, g, b })),
        _ => None,
    };
    match rest {
        [5, n, ..] => (byte(*n).map(Color::Indexed), 2),
        [2, _, r, g, b, ..] if colon => (rgb(*r, *g, *b), 5),
        [2, r, g, b, ..] => (rgb(*r, *g, *b), 4),
        _ => (None, rest.len()),
    }
}

impl Perform for CookState {
    fn print(&mut self, c: char) {
        self.run.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x0a => {
                self.flush();
                self.out.push(Cooked::Newline);
            }
            // A tab is text in iTerm2's appended string; keep the columns readable.
            0x09 => self.run.push('\t'),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], ignore: bool, action: char) {
        if action == 'm' && intermediates.is_empty() && !ignore {
            self.sgr(params);
        }
    }
}

// ---------------------------------------------------------------- timestamps

/// A TR35 subset (`yyyy yy MM M dd d HH H hh h mm m ss s SSS a`, quoted text, `''`); every
/// other character, and a letter run of an unsupported length, is kept as written.
pub fn format_date(pattern: &str, p: &Parts, millis: u16) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            if chars.get(i + 1) == Some(&'\'') {
                out.push('\'');
                i += 2;
                continue;
            }
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if !c.is_ascii_alphabetic() {
            out.push(c);
            i += 1;
            continue;
        }
        let mut n = 1;
        while chars.get(i + n) == Some(&c) {
            n += 1;
        }
        let hour12 = if p.hour % 12 == 0 { 12 } else { p.hour % 12 };
        let piece = match (c, n) {
            ('y', 4) => Some(format!("{:04}", p.year)),
            ('y', 2) => Some(format!("{:02}", p.year % 100)),
            ('M', 2) => Some(format!("{:02}", p.month)),
            ('M', 1) => Some(p.month.to_string()),
            ('d', 2) => Some(format!("{:02}", p.day)),
            ('d', 1) => Some(p.day.to_string()),
            ('H', 2) => Some(format!("{:02}", p.hour)),
            ('H', 1) => Some(p.hour.to_string()),
            ('h', 2) => Some(format!("{hour12:02}")),
            ('h', 1) => Some(hour12.to_string()),
            ('m', 2) => Some(format!("{:02}", p.minute)),
            ('m', 1) => Some(p.minute.to_string()),
            ('s', 2) => Some(format!("{:02}", p.second)),
            ('s', 1) => Some(p.second.to_string()),
            ('S', 3) => Some(format!("{:03}", millis.min(999))),
            ('a', 1) => Some(if p.hour >= 12 { "PM" } else { "AM" }.to_string()),
            _ => None,
        };
        match piece {
            Some(s) => out.push_str(&s),
            None => out.extend(std::iter::repeat(c).take(n)),
        }
        i += n;
    }
    out
}

/// `PS:23840-23860`: "[" the date "] ", with the no-break and narrow spaces newer systems
/// put in a time made plain, so the log is plain ASCII spacing.
pub fn plain_timestamp(pattern: &str, p: &Parts, millis: u16) -> String {
    let text = format!("[{}] ", format_date(pattern, p, millis));
    text.chars().map(|c| if !c.is_ascii() && c.is_whitespace() { ' ' } else { c }).collect()
}

// ---------------------------------------------------------------- html

pub const HTML_NEWLINE: &str = "<br/>\n";
pub const HTML_TITLE_PATTERN: &str = "yyyy-MM-dd hh.mm.ss.SSS";

/// `HE:181-198`: "#RRGGBBAA", uppercase.
pub fn html_color(c: Rgb, alpha: f32) -> String {
    let a = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02X}{:02X}{:02X}{a:02X}", c.r, c.g, c.b)
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// `HE:61-110`: a span with the title and the style, then `b i u strike`, the text escaped
/// (a Decision: iTerm2 writes it raw, which lets program output inject markup).
pub fn html_run(text: &str, attr: &Attr, palette: &Palette, title: &str) -> String {
    let mut css: Vec<String> = Vec::new();
    if let Some(fg) = attr.fg {
        css.push(format!("color:{}", html_color(palette.color(fg), if attr.faint { 0.5 } else { 1.0 })));
    }
    if let Some(bg) = attr.bg {
        css.push(format!("background-color:{}", html_color(palette.color(bg), 1.0)));
    }
    let style = match attr.underline {
        3 => Some("wavy"),
        2 => Some("double"),
        4 => Some("dotted"),
        5 => Some("dashed"),
        _ => None,
    };
    if let Some(s) = style {
        css.push(format!("text-decoration-style:{s}"));
    }
    let mut out = format!("<span title='{}'", escape(title));
    if !css.is_empty() {
        out.push_str(&format!(" style='{}'", css.join(";")));
    }
    out.push('>');
    let tags: Vec<&str> = [(attr.bold, "b"), (attr.italic, "i"), (attr.underline != 0, "u"), (attr.strike, "strike")]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, t)| t)
        .collect();
    for t in &tags {
        out.push_str(&format!("<{t}>"));
    }
    out.push_str(&escape(text));
    for t in tags.iter().rev() {
        out.push_str(&format!("</{t}>"));
    }
    out.push_str("</span>");
    out
}

/// `HE:26-40`, the properties in a fixed order. A font name cannot close the element: the
/// characters that would are dropped.
pub fn html_stylesheet(family: &str, points: f32, bg: Rgb, fg: Rgb) -> String {
    let family: String = family.chars().filter(|c| !matches!(c, '<' | '>' | ';' | '{' | '}' | '\n' | '\r')).collect();
    format!(
        "<style>body {{ font-family:{family};font-size:{points:.1}pt;background-color:{};color:{}}}</style>",
        html_color(bg, 1.0),
        html_color(fg, 1.0)
    )
}

// ---------------------------------------------------------------- asciicast

pub struct AsciicastHeader {
    pub width: u16,
    pub height: u16,
    pub timestamp: u64,
    pub command: String,
    pub title: String,
    pub term: String,
    pub shell: String,
    pub fg: Rgb,
    pub bg: Rgb,
    pub ansi: [Rgb; 16],
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

impl AsciicastHeader {
    /// `LH:161-185`: asciicast v2's header on one line.
    pub fn json_line(&self) -> String {
        let palette: Vec<String> = self.ansi.iter().map(|c| hex(*c)).collect();
        let v = serde_json::json!({
            "version": 2,
            "width": self.width,
            "height": self.height,
            "timestamp": self.timestamp,
            "idle_time_limit": 1,
            "command": self.command,
            "title": self.title,
            "env": { "SHELL": self.shell, "TERM": self.term },
            "theme": { "fg": hex(self.fg), "bg": hex(self.bg), "palette": palette.join(":") },
        });
        format!("{v}\n")
    }
}

/// `LH:318-340`: `[elapsed, "o", text]`; invalid UTF-8 is U+FFFD.
pub fn asciicast_output(elapsed: f64, bytes: &[u8]) -> String {
    format!("{}\n", serde_json::json!([elapsed, "o", String::from_utf8_lossy(bytes)]))
}

pub fn asciicast_resize(elapsed: f64, cols: u16, rows: u16) -> String {
    format!("{}\n", serde_json::json!([elapsed, "r", format!("{cols}x{rows}")]))
}

// ---------------------------------------------------------------- sinks

pub trait Sink: Send {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String>;

    /// The file this sink writes, when it is one.
    fn path(&self) -> Option<&Path> {
        None
    }

    /// The first error the sink met, when it learns of one after a write returned.
    fn failed(&self) -> Option<String> {
        None
    }
}

/// The bytes waiting for the writer thread, bounded. An oversized chunk is taken when the
/// queue is empty so one big write cannot jam a log for good.
pub struct Budget {
    cap: usize,
    used: AtomicUsize,
}

impl Budget {
    pub fn new(cap: usize) -> Budget {
        Budget { cap, used: AtomicUsize::new(0) }
    }

    pub fn try_add(&self, n: usize) -> bool {
        self.used.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |u| if u == 0 || u + n <= self.cap { Some(u + n) } else { None }).is_ok()
    }

    pub fn sub(&self, n: usize) {
        let _ = self.used.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |u| Some(u.saturating_sub(n)));
    }

    pub fn queued(&self) -> usize {
        self.used.load(Ordering::SeqCst)
    }
}

/// The most the queue holds: 8 MiB or 1024 chunks, whichever is first.
const QUEUE_BYTES: usize = 8 * 1024 * 1024;
const QUEUE_CHUNKS: usize = 1024;
/// The writer flushes its buffer when it has been idle this long.
const FLUSH_IDLE: Duration = Duration::from_millis(250);

enum Msg {
    Data(Vec<u8>),
    Sync(mpsc::Sender<()>),
}

type Failure = Arc<Mutex<Option<String>>>;

fn fail(slot: &Failure, error: String) {
    if let Ok(mut s) = slot.lock() {
        s.get_or_insert(error);
    }
}

/// A log file written on its own thread. `open` returns at once; the thread opens or
/// creates the file, seeks to the end or truncates, and writes what it is sent.
/// Nothing touches the file system on the calling thread, so a network folder that hangs
/// stalls only the writer. Dropping the sink lets the thread drain its queue, flush and
/// end.
pub struct FileSink {
    path: PathBuf,
    tx: SyncSender<Msg>,
    budget: Arc<Budget>,
    failure: Failure,
    ready: Mutex<mpsc::Receiver<Result<(), String>>>,
    ready_result: Mutex<Option<Result<(), String>>>,
}

impl FileSink {
    pub fn open(path: &Path, append: bool) -> FileSink {
        let (tx, rx) = mpsc::sync_channel::<Msg>(QUEUE_CHUNKS);
        let (ready_tx, ready_rx) = mpsc::channel();
        let budget = Arc::new(Budget::new(QUEUE_BYTES));
        let failure: Failure = Arc::new(Mutex::new(None));
        let (thread_path, thread_budget, thread_failure) = (path.to_path_buf(), budget.clone(), failure.clone());
        let spawned = std::thread::Builder::new().name("session-log".into()).spawn(move || {
            let opened = std::fs::OpenOptions::new().create(true).write(true).open(&thread_path).and_then(|mut f| {
                if append {
                    f.seek(SeekFrom::End(0))?;
                } else {
                    f.set_len(0)?;
                }
                Ok(f)
            });
            let file = match opened {
                Ok(f) => {
                    let _ = ready_tx.send(Ok(()));
                    f
                }
                Err(e) => {
                    let text = e.to_string();
                    fail(&thread_failure, text.clone());
                    let _ = ready_tx.send(Err(text));
                    return;
                }
            };
            let mut out = std::io::BufWriter::with_capacity(64 * 1024, file);
            loop {
                match rx.recv_timeout(FLUSH_IDLE) {
                    Ok(Msg::Data(bytes)) => {
                        thread_budget.sub(bytes.len());
                        if let Err(e) = out.write_all(&bytes) {
                            fail(&thread_failure, e.to_string());
                            return;
                        }
                    }
                    Ok(Msg::Sync(done)) => {
                        if let Err(e) = out.flush() {
                            fail(&thread_failure, e.to_string());
                            return;
                        }
                        let _ = done.send(());
                    }
                    Err(RecvTimeoutError::Timeout) => {
                        if let Err(e) = out.flush() {
                            fail(&thread_failure, e.to_string());
                            return;
                        }
                    }
                    Err(RecvTimeoutError::Disconnected) => {
                        if let Err(e) = out.flush() {
                            fail(&thread_failure, e.to_string());
                        }
                        return;
                    }
                }
            }
        });
        if let Err(e) = spawned {
            fail(&failure, e.to_string());
        }
        FileSink { path: path.to_path_buf(), tx, budget, failure, ready: Mutex::new(ready_rx), ready_result: Mutex::new(None) }
    }

    /// The first error the open or a write met. Never blocks.
    pub fn failed(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|s| s.clone())
    }

    /// Waits for the open's answer (the tests and a caller that must know the file opened).
    pub fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let mut cached = self.ready_result.lock().unwrap();
        if cached.is_none() {
            *cached = Some(match self.ready.lock().unwrap().recv_timeout(timeout) {
                Ok(answer) => answer,
                Err(_) => return Err("the writer did not answer".into()),
            });
        }
        cached.clone().unwrap()
    }

    /// Waits until everything sent so far is on disk.
    pub fn finish_for_test(&mut self) {
        let (tx, rx) = mpsc::channel();
        if self.tx.send(Msg::Sync(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(5));
        }
    }
}

impl Sink for FileSink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        if let Some(e) = self.failed() {
            return Err(e);
        }
        if bytes.is_empty() {
            return Ok(());
        }
        if !self.budget.try_add(bytes.len()) {
            let e = "the log writer cannot keep up".to_string();
            fail(&self.failure, e.clone());
            return Err(e);
        }
        match self.tx.try_send(Msg::Data(bytes.to_vec())) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.budget.sub(bytes.len());
                let e = match error {
                    TrySendError::Full(_) => "the log writer cannot keep up".to_string(),
                    TrySendError::Disconnected(_) => self.failed().unwrap_or_else(|| "the log writer stopped".into()),
                };
                fail(&self.failure, e.clone());
                Err(self.failed().unwrap_or(e))
            }
        }
    }

    fn path(&self) -> Option<&Path> {
        Some(&self.path)
    }

    fn failed(&self) -> Option<String> {
        FileSink::failed(self)
    }
}

// ---------------------------------------------------------------- logger

/// One session's log. Writes nothing once its sink failed or it was stopped.
pub struct Logger {
    style: Style,
    sink: Option<Box<dyn Sink>>,
    timestamps: Option<String>,
    cooker: Cooker,
    line_start: bool,
    error: Option<String>,
}

impl Logger {
    /// `timestamps` is the plain-text pattern when `LogTimestampsWithPlainText` is on.
    /// The asciicast header or the HTML stylesheet is written first (`LH:195-219`).
    pub fn new(style: Style, sink: Box<dyn Sink>, timestamps: Option<String>, header: Option<AsciicastHeader>, stylesheet: Option<String>) -> Logger {
        let mut l = Logger { style, sink: Some(sink), timestamps, cooker: Cooker::new(), line_start: true, error: None };
        match style {
            Style::Asciicast => {
                if let Some(h) = header {
                    l.write(h.json_line().as_bytes());
                }
            }
            Style::Html => {
                if let Some(s) = stylesheet {
                    l.write(s.as_bytes());
                }
            }
            _ => {}
        }
        l
    }

    fn write(&mut self, bytes: &[u8]) {
        if self.error.is_some() || bytes.is_empty() {
            return;
        }
        let Some(sink) = self.sink.as_mut() else { return };
        if let Err(e) = sink.write(bytes) {
            self.error = Some(e);
        }
    }

    /// The bytes the program wrote: Raw logs them, asciicast records them at `elapsed`.
    pub fn raw(&mut self, bytes: &[u8], elapsed: f64) {
        match self.style {
            Style::Raw => self.write(bytes),
            Style::Asciicast => self.write(asciicast_output(elapsed, bytes).as_bytes()),
            _ => {}
        }
    }

    /// The program's output as text, for plain text and HTML.
    pub fn cooked(&mut self, bytes: &[u8], palette: &Palette, now: (&Parts, u16)) {
        if !self.style.cooked() || self.error.is_some() || self.sink.is_none() {
            return;
        }
        let items = self.cooker.feed(bytes);
        self.emit(items, palette, now);
    }

    /// Compact logging's prompt and command text. Escapes in it are dropped like the
    /// program's.
    pub fn cooked_text(&mut self, text: &str, now: (&Parts, u16)) {
        if !self.style.cooked() || self.error.is_some() || self.sink.is_none() {
            return;
        }
        let items = Cooker::new().feed(text.replace("\r\n", "\n").as_bytes());
        self.emit(items, &Palette::default(), now);
    }

    fn emit(&mut self, items: Vec<Cooked>, palette: &Palette, now: (&Parts, u16)) {
        let mut out = String::new();
        let html = self.style == Style::Html;
        for item in items {
            match item {
                Cooked::Newline => {
                    out.push_str(if html { HTML_NEWLINE } else { "\n" });
                    self.line_start = true;
                }
                Cooked::Text(text, attr) => {
                    if html {
                        out.push_str(&html_run(&text, &attr, palette, &format_date(HTML_TITLE_PATTERN, now.0, now.1)));
                    } else {
                        if self.line_start {
                            if let Some(pattern) = &self.timestamps {
                                out.push_str(&plain_timestamp(pattern, now.0, now.1));
                            }
                        }
                        out.push_str(&text);
                    }
                    self.line_start = false;
                }
            }
        }
        self.write(out.as_bytes());
    }

    /// An asciicast "r" record.
    pub fn resize(&mut self, cols: u16, rows: u16, elapsed: f64) {
        if self.style == Style::Asciicast {
            self.write(asciicast_resize(elapsed, cols, rows).as_bytes());
        }
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn path(&self) -> Option<&Path> {
        self.sink.as_ref().and_then(|s| s.path())
    }

    /// The first error the sink met, including one it learned of after a write returned.
    pub fn failed(&self) -> Option<String> {
        self.error.clone().or_else(|| self.sink.as_ref().and_then(|s| s.failed()))
    }

    /// Ends the log: nothing is written after this, and the file's writer drains and ends.
    pub fn stop(&mut self) {
        self.sink = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct VecSink(Arc<Mutex<Vec<u8>>>);
    impl Sink for VecSink {
        fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }
    }

    fn vec_sink() -> (Box<dyn Sink>, Arc<Mutex<Vec<u8>>>) {
        let shared = Arc::new(Mutex::new(Vec::new()));
        (Box::new(VecSink(shared.clone())), shared)
    }

    fn text(shared: &Arc<Mutex<Vec<u8>>>) -> String {
        String::from_utf8_lossy(&shared.lock().unwrap()).into_owned()
    }

    fn parts() -> Parts {
        Parts { year: 2026, month: 9, day: 27, weekday: 0, hour: 15, minute: 4, second: 5 }
    }

    fn red_palette() -> Palette {
        let mut p = Palette::default();
        p.ansi[1] = Rgb { r: 205, g: 0, b: 0 };
        p.ansi[4] = Rgb { r: 0, g: 0, b: 238 };
        p
    }

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tandem-session-log-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("log.txt")
    }

    #[test]
    fn styles_follow_iterm2() {
        assert_eq!(Style::from_int(0), Style::Raw);
        assert_eq!(Style::from_int(1), Style::PlainText);
        assert_eq!(Style::from_int(2), Style::Html);
        assert_eq!(Style::from_int(3), Style::Asciicast);
        assert_eq!(Style::from_int(9), Style::Raw);
        assert_eq!(Style::from_int(-1), Style::Raw);
        assert!(Style::Raw.raw() && Style::Asciicast.raw());
        assert!(!Style::PlainText.raw() && !Style::Html.raw());
        assert!(Style::PlainText.cooked() && Style::Html.cooked());
        assert!(!Style::Raw.cooked() && !Style::Asciicast.cooked());
        assert_eq!(STYLE_LABELS, ["Raw data", "Plain text", "HTML", "ASCIInema"]);
        assert_eq!(LOGGING_STYLE_KEY, "NoSyncLoggingStyle");
    }

    #[test]
    fn texts_follow_iterm2() {
        assert_eq!(FOLDER_HEADING, "Logging Folder Not Found");
        assert_eq!(create_folder_text("D:\\l"), "Would you like to create the directory at D:\\l?");
        assert_eq!(CANNOT_WRITE, "Couldn\u{2019}t write to session log");
        assert_eq!(LOGGING_PROBLEM_HEADING, "Session Logging Problem");
        assert_eq!(
            cannot_start_text("Default", "bad"),
            "Cannot start logging to session with profile \u{201c}Default\u{201d}: bad"
        );
        assert_eq!(exists_heading("a.log"), "\u{201c}a.log\u{201d} already exists. Do you want to replace it or append to it?");
        assert_eq!(EXISTS_BODY, "A file or folder with the same name already exists. Replacing it will overwrite its current contents.");
        assert_eq!((REPLACE, APPEND), ("Replace", "Append"));
    }

    #[test]
    fn file_names_follow_iterm2() {
        let name = "20260927_150405.Default.w0t0p0.3.1234.42.log";
        assert_eq!(safe_file_name(name), name);
        assert_eq!(safe_file_name("a/b:c"), "a__b_c");
        assert_eq!(safe_file_name("a\\b*c?d\"e<f>g|h"), "a_b_c_d_e_f_g_h");
        let home = Path::new(r"C:\Users\u");
        assert_eq!(join_log_path("", "x.log", home), PathBuf::from(r"C:\Users\u").join("x.log"));
        assert_eq!(join_log_path("~\\logs", "x.log", home), PathBuf::from(r"C:\Users\u").join("logs").join("x.log"));
        assert_eq!(join_log_path("~", "x.log", home), PathBuf::from(r"C:\Users\u").join("x.log"));
        assert_eq!(join_log_path(r"D:\l", "x.log", home), PathBuf::from(r"D:\l").join("x.log"));
    }

    #[test]
    fn file_names_cannot_escape_or_hit_devices() {
        let home = Path::new(r"C:\Users\u");
        // No separators survive, so a name never leaves the folder.
        assert_eq!(safe_file_name("../../evil.log"), "..__..__evil.log");
        assert_eq!(safe_file_name(r"..\..\evil.log"), ".._.._evil.log");
        // A name that is only dots is a traversal; it gets a stand-in.
        assert_eq!(safe_file_name(".."), "session.log");
        assert_eq!(safe_file_name("."), "session.log");
        assert_eq!(safe_file_name(""), "session.log");
        assert_eq!(safe_file_name("   "), "session.log");
        // Reserved device names, with or without an extension, any case.
        assert_eq!(safe_file_name("CON"), "_CON");
        assert_eq!(safe_file_name("nul.log"), "_nul.log");
        assert_eq!(safe_file_name("Com1.txt"), "_Com1.txt");
        assert_eq!(safe_file_name("LPT9"), "_LPT9");
        assert_eq!(safe_file_name("console.log"), "console.log");
        assert_eq!(safe_file_name("COM10"), "COM10");
        // Trailing dots and spaces are dropped by Windows, so they are removed here.
        assert_eq!(safe_file_name("a.log. ."), "a.log");
        // Control characters become underscores; the length is bounded.
        assert_eq!(safe_file_name("a\u{1}b\nc"), "a_b_c");
        assert!(safe_file_name(&"x".repeat(1000)).chars().count() <= 200);
        // The joined path stays inside the folder even for a hostile name.
        let p = join_log_path(r"D:\l", "../../evil.log", home);
        assert_eq!(p.parent(), Some(Path::new(r"D:\l")));
    }

    #[test]
    fn cooked_text_follows_iterm2() {
        let mut c = Cooker::new();
        let d = Attr::default();
        let red = Attr { fg: Some(Color::Named(NamedColor::Red)), ..d };
        assert_eq!(
            c.feed(b"ab\x1b[31mc\x1b[0m\r\nd"),
            vec![Cooked::Text("ab".into(), d), Cooked::Text("c".into(), red), Cooked::Newline, Cooked::Text("d".into(), d)]
        );
        let one = |bytes: &[u8]| -> Attr {
            match Cooker::new().feed(&[bytes, b"x"].concat()).pop() {
                Some(Cooked::Text(_, a)) => a,
                other => panic!("{other:?}"),
            }
        };
        let a = one(b"\x1b[1;2;3;9m");
        assert!(a.bold && a.faint && a.italic && a.strike);
        assert_eq!(one(b"\x1b[4:3m").underline, 3);
        assert_eq!(one(b"\x1b[4m").underline, 1);
        assert_eq!(one(b"\x1b[21m").underline, 2);
        assert_eq!(one(b"\x1b[4m\x1b[24m").underline, 0);
        assert_eq!(one(b"\x1b[38;2;1;2;3m").fg, Some(Color::Spec(Rgb { r: 1, g: 2, b: 3 })));
        assert_eq!(one(b"\x1b[48;5;200m").bg, Some(Color::Indexed(200)));
        assert_eq!(one(b"\x1b[38:2::1:2:3m").fg, Some(Color::Spec(Rgb { r: 1, g: 2, b: 3 })));
        assert_eq!(one(b"\x1b[91m").fg, Some(Color::Named(NamedColor::BrightRed)));
        assert_eq!(one(b"\x1b[31m\x1b[39m").fg, None);
        assert_eq!(one(b"\x1b[1m\x1b[22m"), Attr::default());
        assert_eq!(one(b"\x1b[1;31m\x1b[m"), Attr::default());
        for dropped in [&b"\x1b]0;title\x07"[..], b"\x1b[2J", b"\x08", b"\r", b"\x1b[5;5H", b"\x07"] {
            assert_eq!(Cooker::new().feed(dropped), vec![], "{dropped:?}");
        }
        let mut c = Cooker::new();
        assert_eq!(c.feed(&[0xc3]), vec![]);
        assert_eq!(c.feed(&[0xa9]), vec![Cooked::Text("\u{e9}".into(), d)]);
    }

    #[test]
    fn timestamps_follow_iterm2() {
        let p = parts();
        assert_eq!(format_date("yyyy-MM-dd hh.mm.ss.SSS", &p, 7), "2026-09-27 03.04.05.007");
        assert_eq!(format_date("HH:mm 'at' a", &p, 7), "15:04 at PM");
        assert_eq!(format_date("yyyyMMdd_HHmmss", &p, 7), "20260927_150405");
        assert_eq!(format_date("yy M d H h m s", &p, 7), "26 9 27 15 3 4 5");
        assert_eq!(format_date("''x'' 'a''b'", &p, 0), "'x' a'b");
        let am = Parts { hour: 0, ..p };
        assert_eq!(format_date("hh a", &am, 0), "12 AM");
        assert_eq!(plain_timestamp("yyyy-MM-dd hh.mm.ss.SSS", &p, 7), "[2026-09-27 03.04.05.007] ");
        assert_eq!(plain_timestamp("d\u{202f}a", &p, 7), "[27 PM] ");
    }

    #[test]
    fn html_follows_iterm2() {
        let pal = red_palette();
        let rgb = |r, g, b| Rgb { r, g, b };
        assert_eq!(html_color(rgb(205, 0, 0), 1.0), "#CD0000FF");
        assert_eq!(html_color(rgb(205, 0, 0), 0.5), "#CD000080");
        let d = Attr::default();
        assert_eq!(html_run("x<y", &d, &pal, "T"), "<span title='T'>x&lt;y</span>");
        assert_eq!(html_run("a&b>c", &d, &pal, "T"), "<span title='T'>a&amp;b&gt;c</span>");
        let red = Attr { fg: Some(Color::Named(NamedColor::Red)), bold: true, ..d };
        assert_eq!(html_run("x", &red, &pal, "T"), "<span title='T' style='color:#CD0000FF'><b>x</b></span>");
        let faint = Attr { fg: Some(Color::Named(NamedColor::Red)), faint: true, ..d };
        assert_eq!(html_run("x", &faint, &pal, "T"), "<span title='T' style='color:#CD000080'>x</span>");
        let bg = Attr { fg: Some(Color::Named(NamedColor::Red)), bg: Some(Color::Indexed(4)), ..d };
        assert_eq!(
            html_run("x", &bg, &pal, "T"),
            "<span title='T' style='color:#CD0000FF;background-color:#0000EEFF'>x</span>"
        );
        let all = Attr { bold: true, italic: true, underline: 1, strike: true, ..d };
        assert_eq!(html_run("x", &all, &pal, "T"), "<span title='T'><b><i><u><strike>x</strike></u></i></b></span>");
        for (u, style) in [(3, "wavy"), (2, "double"), (4, "dotted"), (5, "dashed")] {
            let a = Attr { underline: u, ..d };
            assert_eq!(
                html_run("x", &a, &pal, "T"),
                format!("<span title='T' style='text-decoration-style:{style}'><u>x</u></span>")
            );
        }
        let single = Attr { underline: 1, ..d };
        assert_eq!(html_run("x", &single, &pal, "T"), "<span title='T'><u>x</u></span>");
        assert_eq!(
            html_stylesheet("Cascadia Mono", 12.0, rgb(0, 0, 0), rgb(255, 255, 255)),
            "<style>body { font-family:Cascadia Mono;font-size:12.0pt;background-color:#000000FF;color:#FFFFFFFF}</style>"
        );
        assert_eq!(HTML_NEWLINE, "<br/>\n");
        assert_eq!(HTML_TITLE_PATTERN, "yyyy-MM-dd hh.mm.ss.SSS");
        // A font name cannot close the style element.
        assert!(!html_stylesheet("x</style><script>", 12.0, rgb(0, 0, 0), rgb(0, 0, 0)).contains("<script"));
    }

    #[test]
    fn asciicast_follows_iterm2() {
        let one = Rgb { r: 1, g: 2, b: 3 };
        let h = AsciicastHeader {
            width: 80,
            height: 24,
            timestamp: 1_790_000_000,
            command: "pwsh.exe".into(),
            title: "t".into(),
            term: "xterm-256color".into(),
            shell: "pwsh.exe".into(),
            fg: Rgb { r: 255, g: 255, b: 255 },
            bg: Rgb { r: 0, g: 0, b: 0 },
            ansi: [one; 16],
        };
        let palette = vec!["#010203"; 16].join(":");
        assert_eq!(
            h.json_line(),
            format!(
                "{{\"version\":2,\"width\":80,\"height\":24,\"timestamp\":1790000000,\"idle_time_limit\":1,\"command\":\"pwsh.exe\",\"title\":\"t\",\"env\":{{\"SHELL\":\"pwsh.exe\",\"TERM\":\"xterm-256color\"}},\"theme\":{{\"fg\":\"#ffffff\",\"bg\":\"#000000\",\"palette\":\"{palette}\"}}}}\n"
            )
        );
        assert_eq!(asciicast_output(1.5, b"hi\n"), "[1.5,\"o\",\"hi\\n\"]\n");
        assert_eq!(asciicast_output(1.0, &[b'a', 0xff]), "[1.0,\"o\",\"a\u{fffd}\"]\n");
        assert_eq!(asciicast_resize(2.0, 100, 30), "[2.0,\"r\",\"100x30\"]\n");
        // Quotes and control characters in a title cannot break the line.
        let mut h2 = h;
        h2.title = "a\"b\nc".into();
        let line = h2.json_line();
        assert_eq!(line.matches('\n').count(), 1);
        assert!(line.contains("\"title\":\"a\\\"b\\nc\""));
    }

    #[test]
    fn logger_writes_as_iterm2() {
        let p = parts();
        let pal = red_palette();
        let stamp = plain_timestamp("yyyy-MM-dd hh.mm.ss.SSS", &p, 7);
        let pattern = Some("yyyy-MM-dd hh.mm.ss.SSS".to_string());

        let (sink, out) = vec_sink();
        let mut l = Logger::new(Style::Raw, sink, None, None, None);
        l.raw(b"\x1b[31mhi", 0.0);
        l.cooked(b"ignored", &pal, (&p, 7));
        assert_eq!(text(&out), "\x1b[31mhi");

        let (sink, out) = vec_sink();
        let mut l = Logger::new(Style::PlainText, sink, pattern.clone(), None, None);
        l.raw(b"ignored", 0.0);
        l.cooked(b"a\r\nb\r\n", &pal, (&p, 7));
        assert_eq!(text(&out), format!("{stamp}a\n{stamp}b\n"));

        let (sink, out) = vec_sink();
        let mut l = Logger::new(Style::PlainText, sink, pattern.clone(), None, None);
        l.cooked(b"a\r\n\r\nb", &pal, (&p, 7));
        assert_eq!(text(&out), format!("{stamp}a\n\n{stamp}b"));
        l.cooked(b"c\r\nd", &pal, (&p, 7));
        assert_eq!(text(&out), format!("{stamp}a\n\n{stamp}bc\n{stamp}d"));

        let (sink, out) = vec_sink();
        let mut l = Logger::new(Style::PlainText, sink, None, None, None);
        l.cooked(b"a\r\nb\r\n", &pal, (&p, 7));
        assert_eq!(text(&out), "a\nb\n");
        l.cooked_text("p> ls\n", (&p, 7));
        assert_eq!(text(&out), "a\nb\np> ls\n");

        let (sink, out) = vec_sink();
        let mut l = Logger::new(Style::Html, sink, pattern, None, Some("<style>S</style>".into()));
        l.raw(b"ignored", 0.0);
        l.cooked(b"a\x1b[31mb\x1b[0m\r\nc", &pal, (&p, 7));
        let title = format_date(HTML_TITLE_PATTERN, &p, 7);
        assert_eq!(
            text(&out),
            format!(
                "<style>S</style><span title='{title}'>a</span><span title='{title}' style='color:#CD0000FF'>b</span><br/>\n<span title='{title}'>c</span>"
            )
        );

        let (sink, out) = vec_sink();
        let header = AsciicastHeader {
            width: 80,
            height: 24,
            timestamp: 5,
            command: "c".into(),
            title: "t".into(),
            term: "x".into(),
            shell: "s".into(),
            fg: Rgb { r: 1, g: 1, b: 1 },
            bg: Rgb { r: 2, g: 2, b: 2 },
            ansi: [Rgb { r: 3, g: 3, b: 3 }; 16],
        };
        let first = header.json_line();
        let mut l = Logger::new(Style::Asciicast, sink, None, Some(header), None);
        assert_eq!(text(&out), first);
        l.raw(b"hi", 0.5);
        l.resize(100, 30, 1.0);
        l.cooked(b"ignored", &pal, (&p, 7));
        assert_eq!(text(&out), format!("{first}[0.5,\"o\",\"hi\"]\n[1.0,\"r\",\"100x30\"]\n"));
        assert_eq!(l.style(), Style::Asciicast);
    }

    #[test]
    fn a_failed_sink_turns_the_logger_off() {
        let counted = Arc::new(Mutex::new(0usize));
        struct Counting(Arc<Mutex<usize>>);
        impl Sink for Counting {
            fn write(&mut self, _b: &[u8]) -> Result<(), String> {
                *self.0.lock().unwrap() += 1;
                Err("disk full".into())
            }
        }
        let mut l = Logger::new(Style::Raw, Box::new(Counting(counted.clone())), None, None, None);
        l.raw(b"a", 0.0);
        l.raw(b"b", 0.0);
        assert_eq!(*counted.lock().unwrap(), 1);
        assert_eq!(l.failed().as_deref(), Some("disk full"));
    }

    #[test]
    fn a_stopped_logger_writes_nothing() {
        let (sink, out) = vec_sink();
        let mut l = Logger::new(Style::Raw, sink, None, None, None);
        l.raw(b"a", 0.0);
        l.stop();
        l.raw(b"b", 0.0);
        assert_eq!(text(&out), "a");
    }

    #[test]
    fn file_sink_appends_or_truncates() {
        let path = temp_path("append");
        std::fs::write(&path, "old").unwrap();
        let mut s = FileSink::open(&path, true);
        s.wait_ready(Duration::from_secs(5)).unwrap();
        s.write(b"new").unwrap();
        s.finish_for_test();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "oldnew");

        let mut s = FileSink::open(&path, false);
        s.wait_ready(Duration::from_secs(5)).unwrap();
        s.write(b"new").unwrap();
        s.finish_for_test();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(s.path(), Some(path.as_path()));

        let mut s = FileSink::open(&temp_path("fresh").with_file_name("made.log"), true);
        s.wait_ready(Duration::from_secs(5)).unwrap();
        s.write(b"x").unwrap();
        s.finish_for_test();
        assert_eq!(std::fs::read_to_string(temp_path("fresh").with_file_name("made.log")).unwrap(), "x");
    }

    #[test]
    fn file_sink_failure_comes_back_without_blocking() {
        let missing = temp_path("missing").parent().unwrap().join("no").join("such").join("a.log");
        let started = std::time::Instant::now();
        let mut s = FileSink::open(&missing, true);
        assert!(started.elapsed() < Duration::from_millis(500));
        let mut error = None;
        for _ in 0..200 {
            if let Some(e) = s.failed() {
                error = Some(e);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(error.is_some(), "the open's failure never came back");
        assert!(s.wait_ready(Duration::from_secs(2)).is_err());
        assert!(s.write(b"more").is_err());
        // A logger over the failed sink writes nothing further and keeps the first error.
        let mut l = Logger::new(Style::Raw, Box::new(s), None, None, None);
        l.raw(b"a", 0.0);
        assert!(l.failed().is_some());
        assert_eq!(l.path(), Some(missing.as_path()));
        assert!(!missing.exists());
    }

    #[test]
    fn the_write_queue_is_bounded() {
        let b = Budget::new(100);
        assert!(b.try_add(60));
        assert!(!b.try_add(60));
        assert!(b.try_add(40));
        b.sub(60);
        assert!(b.try_add(60));
        assert_eq!(b.queued(), 100);
        // One oversized write on an empty queue is still taken, so a big chunk cannot jam the log.
        let e = Budget::new(10);
        assert!(e.try_add(500));
        assert!(!e.try_add(1));
    }

    /// 13p seam for Task 7: the log folder and name keys are among the keys 13p's one network
    /// filter scrubs at every non-Settings ingress (`start_dir::scrub_map`), and that filter
    /// refuses a quoted or space-led UNC; Task 7's callers use `start_dir::network_value` for any
    /// folder taken from elsewhere.
    #[test]
    fn log_keys_go_through_the_13p_network_filter() {
        for key in ["Log Directory", "Log Filename Format", "Archive Directory"] {
            assert!(crate::start_dir::PATH_KEYS.contains(&key), "{key}");
            assert!(crate::start_dir::network_value(key, " \"\\\\host\\share\""), "{key}");
            assert!(!crate::start_dir::network_value(key, "D:\\logs"), "{key}");
        }
    }
}
