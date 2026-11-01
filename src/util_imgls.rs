//! imgls (milestone 11e): iTerm2's perl `ls` with thumbnails (`OtherResources/Utilities/imgls`),
//! byte for byte. Pure over `Fs`; `WinFs` is the real file system.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
#[cfg(windows)]
use std::path::PathBuf;

use crate::prompted::network_path;
use crate::stamps::Parts;
use crate::util_cli::Io;
use crate::util_osc::b64;

/// The script's single black pixel PNG, already base64 (`get_black_pixel_image`).
pub const BLACK_PIXEL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIW2NgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==";

/// The one way Termurion measures images (its own decoder), the name `--method` matches.
pub const METHOD: &str = "image";

/// The options as `get_options` leaves them. `dimensions` and `unknown` are the script's
/// counters: each of `--[no]dimensions` and `--[no]unknown` turns the other on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opts {
    pub width: String,
    pub height: String,
    pub preserve: String,
    pub dimensions: Option<bool>,
    pub unknown: Option<bool>,
    pub method: Option<String>,
    pub d_format: Option<String>,
    /// The single-letter ls options set (`-U` is `c`).
    pub flags: BTreeSet<char>,
}

impl Default for Opts {
    /// The script's defaults: width 3, height 1, preserveAspectRatio true.
    fn default() -> Opts {
        Opts {
            width: "3".into(),
            height: "1".into(),
            preserve: "true".into(),
            dimensions: None,
            unknown: None,
            method: None,
            d_format: None,
            flags: BTreeSet::new(),
        }
    }
}

impl Opts {
    fn has(&self, c: char) -> bool {
        self.flags.contains(&c)
    }
}

/// Perl's truth for a string: not empty and not "0".
fn truthy(s: &str) -> bool {
    !s.is_empty() && s != "0"
}

fn truthy_opt(s: &Option<String>) -> bool {
    s.as_deref().is_some_and(truthy)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Neg,
    Str,
    Flag,
}

/// Every option `GetOptions` is given (`imgls:62-78`): `--` matches single letters too,
/// as Getopt::Long does in bundling mode.
const LONG: [(&str, Kind); 28] = [
    ("dimensions", Kind::Neg),
    ("height", Kind::Str),
    ("method", Kind::Str),
    ("preserve_ratio", Kind::Neg),
    ("unknown", Kind::Neg),
    ("width", Kind::Str),
    ("D", Kind::Str),
    ("t", Kind::Flag),
    ("S", Kind::Flag),
    ("A", Kind::Flag),
    ("F", Kind::Flag),
    ("R", Kind::Flag),
    ("T", Kind::Flag),
    ("a", Kind::Flag),
    ("d", Kind::Flag),
    ("h", Kind::Flag),
    ("i", Kind::Flag),
    ("k", Kind::Flag),
    ("l", Kind::Flag),
    ("n", Kind::Flag),
    ("o", Kind::Flag),
    ("p", Kind::Flag),
    ("r", Kind::Flag),
    ("s", Kind::Flag),
    ("u", Kind::Flag),
    ("y", Kind::Flag),
    ("c", Kind::Flag),
    ("U", Kind::Flag),
];

/// A single-letter option: `-t` and `-S` exclude each other, `-U` is `-c`.
fn set_flag(o: &mut Opts, c: char) {
    match c {
        't' => {
            o.flags.remove(&'S');
        }
        'S' => {
            o.flags.remove(&'t');
        }
        _ => {}
    }
    o.flags.insert(if c == 'U' { 'c' } else { c });
}

/// Getopt::Long's names: each option, and `no<name>` / `no-<name>` for a negatable one.
/// Returns (name, option, negated), sorted by name.
fn long_names() -> Vec<(String, &'static str, Kind, bool)> {
    let mut v = Vec::new();
    for (name, kind) in LONG {
        v.push((name.to_string(), name, kind, false));
        if kind == Kind::Neg {
            v.push((format!("no{name}"), name, kind, true));
            v.push((format!("no-{name}"), name, kind, true));
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn set_long(o: &mut Opts, name: &str, negated: bool, value: Option<String>) {
    match name {
        "dimensions" => {
            o.unknown = Some(true);
            o.dimensions = Some(!negated);
        }
        "unknown" => {
            o.dimensions = Some(true);
            o.unknown = Some(!negated);
        }
        "preserve_ratio" => o.preserve = if negated { "false" } else { "true" }.into(),
        "height" => o.height = value.unwrap_or_default(),
        "width" => o.width = value.unwrap_or_default(),
        _ => o.method = value,
    }
}

/// `get_options`: Getopt::Long with bundling, case-sensitive, stopping at the first
/// non-option (`require_order`), unique abbreviations of the long names. Err is Getopt's
/// warning text (the first one; the script exits on it).
pub fn parse(args: &[String]) -> Result<(Opts, Vec<String>), String> {
    let mut o = Opts::default();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            i += 1;
            break;
        }
        if let Some(body) = a.strip_prefix("--") {
            i += 1;
            let (typed, optarg) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            let names = long_names();
            let hit = match names.iter().find(|n| n.0 == typed) {
                Some(n) => n.clone(),
                None => {
                    let hits: Vec<_> = if typed.is_empty() { Vec::new() } else { names.iter().filter(|n| n.0.starts_with(typed)).collect() };
                    let distinct: HashSet<&str> = hits.iter().map(|n| n.1).collect();
                    match distinct.len() {
                        0 => return Err(format!("Unknown option: {typed}\n")),
                        1 => hits[0].clone(),
                        _ => {
                            let list: Vec<&str> = hits.iter().map(|n| n.0.as_str()).collect();
                            return Err(format!("Option {typed} is ambiguous ({})\n", list.join(", ")));
                        }
                    }
                }
            };
            let (full, name, kind, negated) = hit;
            match kind {
                Kind::Flag => {
                    if optarg.is_some() {
                        return Err(format!("Option {full} does not take an argument\n"));
                    }
                    set_flag(&mut o, name.chars().next().unwrap_or('?'));
                }
                Kind::Neg => {
                    if optarg.is_some() {
                        return Err(format!("Option {full} does not take an argument\n"));
                    }
                    set_long(&mut o, name, negated, None);
                }
                Kind::Str => {
                    let value = match optarg {
                        Some(v) if v.is_empty() => return Err(format!("Option {full} requires an argument\n")),
                        Some(v) => v,
                        None if i < args.len() => {
                            i += 1;
                            args[i - 1].clone()
                        }
                        None => return Err(format!("Option {full} requires an argument\n")),
                    };
                    if name == "D" {
                        o.d_format = Some(value);
                    } else {
                        set_long(&mut o, name, false, Some(value));
                    }
                }
            }
            continue;
        }
        let Some(bundle) = a.strip_prefix('-').filter(|b| !b.is_empty()) else {
            break;
        };
        i += 1;
        for (at, c) in bundle.char_indices() {
            match c {
                'D' => {
                    let rest = &bundle[at + 1..];
                    let value = if !rest.is_empty() {
                        rest.to_string()
                    } else if i < args.len() {
                        i += 1;
                        args[i - 1].clone()
                    } else {
                        return Err("Option D requires an argument\n".into());
                    };
                    o.d_format = Some(value);
                    break;
                }
                't' | 'S' | 'U' | 'c' | 'A' | 'F' | 'R' | 'T' | 'a' | 'd' | 'h' | 'i' | 'k' | 'l' | 'n' | 'o' | 'p' | 'r' | 's' | 'u' | 'y' => {
                    set_flag(&mut o, c);
                }
                _ => return Err(format!("Unknown option: {c}\n")),
            }
        }
    }
    if o.has('d') {
        o.flags.remove(&'R');
    }
    if truthy_opt(&o.d_format) {
        o.flags.remove(&'T');
    }
    if o.has('n') || o.has('o') {
        o.flags.insert('l');
    }
    Ok((o, args[i..].to_vec()))
}

/// One `lstat` as the script reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub is_dir: bool,
    pub is_file: bool,
    pub is_link: bool,
    pub is_exec: bool,
    pub size: u64,
    pub mtime: i64,
    pub atime: i64,
    pub ctime: i64,
    pub mode: u32,
    pub nlink: u64,
    pub ino: u64,
    pub blocks: u64,
}

/// A file's owner and group: account names and SID strings.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Owners {
    pub owner: String,
    pub group: String,
    pub owner_id: String,
    pub group_id: String,
}

/// What imgls asks of the file system. `list` gives a folder's names without "." and "..".
/// `exists` is Perl's `-e` (links followed). `owners` is asked only for the long listing;
/// `names` false leaves the account names unlooked-up (`-n` shows SIDs).
pub trait Fs {
    fn lstat(&self, path: &str) -> Option<Entry>;
    fn exists(&self, path: &str) -> bool;
    fn owners(&self, path: &str, names: bool) -> Owners;
    fn list(&self, dir: &str) -> Result<Vec<String>, String>;
    fn open(&self, path: &str) -> Option<Box<dyn std::io::Read + '_>>;
    fn dims(&self, path: &str) -> Option<(u32, u32)>;
}

/// `Stat::lsMode::format_mode`.
pub fn format_mode(mode: u32) -> String {
    const PERM: [&str; 8] = ["---", "--x", "-w-", "-wx", "r--", "r-x", "rw-", "rwx"];
    const FTYPE: [&str; 16] = ["", "p", "c", "?", "d", "?", "b", "?", "-", "?", "l", "?", "s", "?", "?", "?"];
    let setids = (mode & 0o7000) >> 9;
    let mut perms: Vec<String> =
        [(mode & 0o700) >> 6, (mode & 0o070) >> 3, mode & 0o007].iter().map(|&p| PERM[p as usize].to_string()).collect();
    let ftype = FTYPE[((mode & 0o170000) >> 12) as usize];
    let mark = |s: &mut String, on: char, off: char| {
        let last = s.pop().unwrap_or('-');
        s.push(if last == 'x' { on } else { off });
    };
    if setids & 0o1 != 0 {
        mark(&mut perms[2], 't', 'T');
    }
    if setids & 0o4 != 0 {
        mark(&mut perms[0], 's', 'S');
    }
    if setids & 0o2 != 0 {
        mark(&mut perms[1], 's', 'S');
    }
    format!("{ftype}{}", perms.concat())
}

/// `format_human`.
pub fn format_human(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "K", "M", "G", "T", "P"];
    let len = bytes.to_string().len();
    let scale = (len - 1) / 3;
    let float = bytes as f64 / 1024f64.powi(scale as i32);
    let int = float.trunc();
    let frac = float - int;
    let unit = UNITS.get(scale).copied().unwrap_or("");
    if len < 3 || format!("{int}").len() >= 2 {
        let v = if frac < 0.5 { float } else { float + 1.0 };
        format!("{}{unit}", v.trunc() as u64)
    } else {
        format!("{float:.1}{unit}")
    }
}

/// Day of the year, 1 to 366.
fn yday(p: &Parts) -> u32 {
    const BEFORE: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let y = u32::from(p.year);
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let m = usize::from(p.month.clamp(1, 12)) - 1;
    BEFORE[m] + u32::from(p.day) + u32::from(leap && m >= 2)
}

/// POSIX `strftime` in the C locale for the conversions ls formats use; any other `%x` is
/// written as is.
pub fn strftime(fmt: &str, p: &Parts) -> String {
    const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    let day = DAYS.get(usize::from(p.weekday)).copied().unwrap_or("?");
    let month = MONTHS.get(usize::from(p.month).wrapping_sub(1)).copied().unwrap_or("?");
    let h12 = if p.hour.is_multiple_of(12) { 12 } else { p.hour % 12 };
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        let Some(k) = chars.next() else {
            out.push('%');
            break;
        };
        let s = match k {
            'a' => day[..3].to_string(),
            'A' => day.to_string(),
            'b' => month[..3].to_string(),
            'B' => month.to_string(),
            'd' => format!("{:02}", p.day),
            'e' => format!("{:2}", p.day),
            'H' => format!("{:02}", p.hour),
            'I' => format!("{h12:02}"),
            'M' => format!("{:02}", p.minute),
            'S' => format!("{:02}", p.second),
            'p' => if p.hour < 12 { "AM" } else { "PM" }.to_string(),
            'y' => format!("{:02}", p.year % 100),
            'Y' => p.year.to_string(),
            'm' => format!("{:02}", p.month),
            'j' => format!("{:03}", yday(p)),
            'R' => format!("{:02}:{:02}", p.hour, p.minute),
            'T' => format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second),
            'F' => format!("{}-{:02}-{:02}", p.year, p.month, p.day),
            'D' => format!("{:02}/{:02}/{:02}", p.month, p.day, p.year % 100),
            'n' => "\n".to_string(),
            't' => "\t".to_string(),
            '%' => "%".to_string(),
            other => format!("%{other}"),
        };
        out.push_str(&s);
    }
    out
}

/// `format_time`: `-D`'s format, `-T`'s full time, else ls's recent (within six months either
/// way) or old form.
pub fn format_time(t: i64, now: i64, o: &Opts, local: &dyn Fn(i64) -> Parts) -> String {
    const SIX_MONTHS: i64 = 365 * 86_400 / 2;
    let fmt = match &o.d_format {
        Some(f) if truthy(f) => f.as_str(),
        _ if o.has('T') => "%b %e %T %Y",
        _ if t + SIX_MONTHS > now && t < now + SIX_MONTHS => "%b %e %R",
        _ => "%b %e  %Y",
    };
    strftime(fmt, &local(t))
}

/// `usage`, to stderr.
pub fn usage() -> String {
    let leader = "usage: imgls";
    format!(
        "{leader} [--width #] [--height #] [--[no]preserve_ratio] [--[no]dimensions]\n{} [--[no]unknown] [ls options] [file ...]\n",
        " ".repeat(leader.len())
    )
}

/// `(split /\//, $file)[-1]`: the last '/' field, trailing empty fields dropped.
fn last_field(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// One listed entry (`%h` in `do_ls`).
struct Row {
    file: String,
    st: Entry,
    ino: Option<String>,
    bytes: u64,
    bytesh: Option<String>,
    dims: Option<(u32, u32)>,
    nlink: Option<String>,
    blocks: Option<String>,
    lsmodes: Option<String>,
    owner: Option<String>,
    group: Option<String>,
    time: Option<String>,
}

struct Ls<'a, 'b> {
    o: Opts,
    fs: &'a dyn Fs,
    io: &'a mut Io<'b>,
    now: i64,
    local: &'a dyn Fn(i64) -> Parts,
    samesort: bool,
    cache: RefCell<HashMap<String, Option<Entry>>>,
    failed_types: HashSet<String>,
    do_header: u32,
    do_newline: u32,
}

impl Ls<'_, '_> {
    fn put(&mut self, s: &str) {
        let _ = self.io.out.write_all(s.as_bytes());
    }

    fn say_err(&mut self, s: &str) {
        let _ = self.io.err.write_all(format!("{s}\n").as_bytes());
    }

    /// `f` of `_lstat`, cached; a network path is never probed.
    fn with_stat<R>(&self, path: &str, f: impl FnOnce(Option<&Entry>) -> R) -> R {
        if network_path(crate::util_cli::local_form(path)) {
            return f(None);
        }
        let mut cache = self.cache.borrow_mut();
        f(cache.entry(path.to_string()).or_insert_with(|| self.fs.lstat(path)).as_ref())
    }

    fn lstat(&self, path: &str) -> Option<Entry> {
        self.with_stat(path, |e| e.cloned())
    }

    fn st(&self, path: &str) -> Entry {
        self.lstat(path).unwrap_or_default()
    }

    /// `ls_sort`, each entry's key read once.
    fn sort(&self, mut v: Vec<String>) -> Vec<String> {
        let o = &self.o;
        let keyed = |v: Vec<String>, key: fn(&Entry) -> i128| -> Vec<(i128, String)> {
            v.into_iter().map(|p| (self.with_stat(&p, |e| e.map_or(0, key)), p)).collect()
        };
        if o.has('t') {
            let same = o.has('y') || self.samesort;
            let mut k = keyed(v, |e| i128::from(e.mtime));
            k.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| if same { b.1.cmp(&a.1) } else { a.1.cmp(&b.1) }));
            v = k.into_iter().map(|(_, p)| p).collect();
        } else if o.has('c') {
            v.sort();
        } else if o.has('u') {
            let mut k = keyed(v, |e| i128::from(e.atime));
            k.sort_by_key(|a| a.0);
            v = k.into_iter().map(|(_, p)| p).collect();
        } else if o.has('S') {
            let mut k = keyed(v, |e| i128::from(e.size));
            k.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            v = k.into_iter().map(|(_, p)| p).collect();
        } else {
            v.sort();
        }
        if o.has('r') {
            v.reverse();
        }
        v
    }

    /// `get_dimensions`: an extension that failed once is not tried again.
    fn dimensions(&mut self, file: &str) -> Option<(u32, u32)> {
        let ext = file.rfind('.').map(|at| &file[at + 1..]).filter(|e| !e.is_empty()).map(str::to_string);
        if ext.as_ref().is_some_and(|e| self.failed_types.contains(e)) {
            return None;
        }
        let got = self.fs.dims(file);
        if got.is_none()
            && let Some(e) = ext
        {
            self.failed_types.insert(e);
        }
        got
    }

    /// `write_image`: the file's own bytes, or the black pixel for `None` or a file that
    /// cannot be read or reads empty. Parameters in a fixed order (the script's is a Perl
    /// hash's). The file is streamed: `encode_base64`'s 76-column lines (57 bytes each, every
    /// line ending "\n") written a block at a time.
    fn write_image(&mut self, file: Option<(&str, u64)>) {
        const BLOCK: usize = 57 * 1024;
        let o = &self.o;
        let head = |name: &str, size: u64| {
            format!("\x1b]1337;File=inline=1;height={};width={};preserveAspectRatio={};name={name};size={size}:", o.height, o.width, o.preserve)
        };
        // Reads until `buf` is full or the file ends; Err when a read failed.
        let fill = |r: &mut dyn std::io::Read, buf: &mut [u8]| -> Result<usize, ()> {
            match crate::util_osc::fill(r, buf) {
                (n, false) => Ok(n),
                (_, true) => Err(()),
            }
        };
        let encode = |bytes: &[u8]| -> String { bytes.chunks(57).map(|c| format!("{}\n", b64(c))).collect() };
        if let Some((f, size)) = file
            && let Some(mut r) = self.fs.open(f)
        {
            let mut buf = vec![0u8; BLOCK];
            // A read failing in the first block is a failed read (the black pixel, as the
            // script's); once bytes are out, a later failure ends the sequence there.
            if let Ok(mut n) = fill(&mut *r, &mut buf)
                && n > 0
            {
                let mut s = head(&b64(f.as_bytes()), size);
                loop {
                    s.push_str(&encode(&buf[..n]));
                    let _ = self.io.out.write_all(s.as_bytes());
                    s.clear();
                    if n < BLOCK {
                        break;
                    }
                    match fill(&mut *r, &mut buf) {
                        Ok(k) if k > 0 => n = k,
                        _ => break,
                    }
                }
                let _ = self.io.out.write_all(b"\x07");
                return;
            }
        }
        let s = format!("{}{BLACK_PIXEL}\x07", head(&b64(b"one_pixel_black"), BLACK_PIXEL.len() as u64));
        self.put(&s);
    }

    /// `do_ls`.
    fn do_ls(&mut self, path: Option<&str>, entries: Vec<String>) {
        let o = self.o.clone();
        let dims_on = o.dimensions == Some(true);
        let mut blocks_total = 0u64;
        let mut rows = Vec::new();
        let (mut dim_w, mut dim_h) = (0usize, 0usize);
        let mut widths: HashMap<&str, usize> = HashMap::new();
        for file in self.sort(entries) {
            let st = self.st(&file);
            let filename = if path.is_some() { last_field(&file).to_string() } else { file.clone() };
            let bytes = st.size;
            let mut r = Row {
                file: file.clone(),
                ino: o.has('i').then(|| st.ino.to_string()),
                bytes,
                bytesh: o.has('h').then(|| format_human(bytes)),
                dims: None,
                nlink: (o.has('s') || o.has('l')).then(|| st.nlink.to_string()),
                blocks: None,
                lsmodes: None,
                owner: None,
                group: None,
                time: None,
                st: st.clone(),
            };
            if dims_on && st.is_file && bytes > 0 {
                r.dims = self.dimensions(&file);
            }
            if o.has('s') || o.has('l') {
                r.blocks = Some(st.blocks.to_string());
                let dotted = filename.starts_with('.') && filename[1..].starts_with(|c: char| c != '.');
                if !st.is_dir && (o.has('a') || !dotted) {
                    blocks_total += st.blocks;
                }
            }
            if o.has('l') {
                r.lsmodes = Some(format_mode(st.mode));
                // Owners are looked up only here, names only when shown.
                let who = self.fs.owners(&file, !o.has('n'));
                r.owner = Some(if o.has('n') { who.owner_id.clone() } else { who.owner.clone() });
                if !o.has('o') {
                    r.group = Some(if o.has('n') { who.group_id } else { who.group });
                }
                r.time = Some(format_time(if o.has('c') { st.ctime } else { st.mtime }, self.now, &o, self.local));
            }
            dim_w = dim_w.max(r.dims.map_or(0, |d| d.0.to_string().len()));
            dim_h = dim_h.max(r.dims.map_or(0, |d| d.1.to_string().len()));
            for (key, v) in [
                ("blocks", &r.blocks),
                ("ino", &r.ino),
                ("bytes", &Some(r.bytes.to_string())),
                ("owner", &r.owner),
                ("group", &r.group),
                ("nlink", &r.nlink),
            ] {
                if let Some(v) = v {
                    let w = widths.entry(key).or_insert(0);
                    *w = (*w).max(v.chars().count());
                }
            }
            rows.push(r);
        }

        let path_true = path.is_some_and(truthy);
        if path_true && self.do_newline > 0 {
            self.put("\n");
        }
        if path_true {
            let show = self.do_header > 0;
            self.do_header += 1;
            if show {
                self.put(&format!("{}:\n", path.unwrap_or("")));
            }
        }
        if path_true && !o.has('d') && (o.has('s') || o.has('l')) {
            let total = if o.has('k') {
                if blocks_total.is_multiple_of(2) { (blocks_total / 2).to_string() } else { format!("{}.5", blocks_total / 2) }
            } else {
                blocks_total.to_string()
            };
            self.put(&format!("total {total}\n"));
        }

        let w = |k: &str| widths.get(k).copied().unwrap_or(0);
        for r in rows {
            if !r.st.is_file || r.bytes == 0 || (dims_on && r.dims.is_none() && o.unknown != Some(true)) {
                self.write_image(None);
            } else {
                self.write_image(Some((&r.file, r.bytes)));
            }
            let mut s = String::new();
            if dims_on && (dim_w > 0 || dim_h > 0) {
                match r.dims {
                    Some((dw, dh)) if dw != 0 || dh != 0 => s.push_str(&format!(" [{dw:>dim_w$} x {dh:>dim_h$}] ")),
                    _ => s.push_str(&format!(" {:>dim_w$}   {:>dim_h$}   ", " ", " ")),
                }
            }
            if let Some(v) = &r.ino {
                s.push_str(&format!(" {v:>0$}", w("ino")));
            }
            if o.has('s')
                && let Some(v) = &r.blocks
            {
                s.push_str(&format!(" {v:>0$}", w("blocks")));
            }
            if let Some(v) = &r.lsmodes {
                s.push_str(&format!(" {v}"));
            }
            if let Some(v) = &r.nlink {
                s.push_str(&format!(" {v:>0$}", w("nlink")));
            }
            if let Some(v) = &r.owner {
                s.push_str(&format!(" {v:>0$}", w("owner")));
            }
            if let Some(v) = &r.group {
                s.push_str(&format!("  {v:>0$}", w("group")));
            }
            if o.has('l') {
                match &r.bytesh {
                    Some(h) => s.push_str(&format!("  {h:>4}")),
                    None => s.push_str(&format!("  {:>1$}", r.bytes, w("bytes"))),
                }
            }
            if let Some(v) = &r.time {
                s.push_str(&format!(" {v}"));
            }
            s.push(' ');
            s.push_str(if path.is_some() { last_field(&r.file) } else { &r.file });
            if o.has('F') || o.has('p') {
                s.push_str(self.f_type(&r.st));
            }
            s.push('\n');
            self.put(&s);
        }
    }

    /// `get_F_type` (Windows has no pipes or sockets in a listing).
    fn f_type(&self, st: &Entry) -> &'static str {
        if st.is_dir && (self.o.has('p') || self.o.has('F')) {
            return "/";
        }
        if !self.o.has('F') {
            return "";
        }
        if st.is_link {
            "@"
        } else if st.is_exec {
            "*"
        } else {
            ""
        }
    }
}

/// The real file system: paths relative to `cwd`, `lstat` as Perl's Windows `stat` fills it
/// (folders 0o040777; files 0o100666, 0o100444 read-only, plus 0o111 for .exe, .bat, .cmd and
/// .com; symbolic links and junctions 0o120777), link counts and file index from
/// `GetFileInformationByHandle`, blocks of 512 bytes from the allocation size, owner and group
/// as account names ("?" when a lookup fails) and SIDs. A network path is never probed, and
/// neither is a path reached through a link to one.
#[cfg(windows)]
pub struct WinFs {
    cwd: PathBuf,
    /// Folders the judge passed, by path.
    checked: RefCell<HashMap<PathBuf, crate::localfs::LocalPath>>,
    /// Account names by SID string, for this run.
    names: RefCell<HashMap<String, String>>,
}

#[cfg(windows)]
impl WinFs {
    pub fn new(cwd: PathBuf) -> WinFs {
        WinFs { cwd, checked: RefCell::default(), names: RefCell::default() }
    }

    /// `path` against `cwd`, when the judge passes it: its folder judged once (kept for the run),
    /// then the entry itself (its own link read, a link judged whole).
    fn full(&self, path: &str) -> Option<crate::localfs::LocalPath> {
        match self.folder_of(path)? {
            Ok((folder, name)) => folder.child(name),
            Err(whole) => crate::localfs::LocalPath::judge(whole),
        }
    }

    /// `full` for an open that never follows the entry itself (`lstat`, the owners): only its
    /// folder is judged, so a link to a share is shown as the link it is.
    fn unfollowed(&self, path: &str) -> Option<crate::localfs::Unfollowed> {
        match self.folder_of(path)? {
            Ok((folder, name)) => folder.unfollowed(name),
            Err(whole) => crate::localfs::LocalPath::judge(whole).map(crate::localfs::Unfollowed::from),
        }
    }

    /// The judged folder of `path` against `cwd` (kept for the run) and the entry's name; Err with
    /// the whole path when it ends in no plain name (a drive root, `..`), for the judge to take whole.
    #[allow(clippy::type_complexity)]
    fn folder_of(&self, path: &str) -> Option<Result<(crate::localfs::LocalPath, std::ffi::OsString), PathBuf>> {
        use crate::localfs::LocalPath;
        if network_path(crate::util_cli::local_form(path)) {
            return None;
        }
        let p = crate::util_cli::join_local(&self.cwd, path);
        if network_path(&p.to_string_lossy()) {
            return None;
        }
        let ends_plain = matches!(p.components().next_back(), Some(std::path::Component::Normal(_)));
        let (Some(parent), Some(name), true) = (p.parent().filter(|d| !d.as_os_str().is_empty()), p.file_name(), ends_plain) else { return Some(Err(p)) };
        let name = name.to_os_string();
        let known = self.checked.borrow().get(parent).cloned();
        let folder = match known {
            Some(f) => f,
            None => {
                let f = LocalPath::judge(parent)?;
                self.checked.borrow_mut().insert(parent.to_path_buf(), f.clone());
                f
            }
        };
        Some(Ok((folder, name)))
    }
}

#[cfg(windows)]
mod win {
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{FILETIME, HANDLE, HLOCAL, LocalFree};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        GROUP_SECURITY_INFORMATION, LookupAccountSidW, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SID_NAME_USE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_ATTRIBUTE_TAG_INFO, FILE_STANDARD_INFO, FileAttributeTagInfo, FileStandardInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx,
    };

    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::{Entry, Owners};

    const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
    const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;

    /// A FILETIME as Unix seconds.
    fn unix(ft: FILETIME) -> i64 {
        let ticks = (u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime);
        (ticks / 10_000_000) as i64 - 11_644_473_600
    }

    /// The account name of a SID (without its domain), "?" when it has none.
    unsafe fn account(sid: PSID) -> String {
        unsafe {
            let mut name = [0u16; 256];
            let mut domain = [0u16; 256];
            let (mut n, mut d) = (name.len() as u32, domain.len() as u32);
            let mut kind: SID_NAME_USE = 0;
            if sid.is_null()
                || LookupAccountSidW(std::ptr::null(), sid, name.as_mut_ptr(), &mut n, domain.as_mut_ptr(), &mut d, &mut kind) == 0
            {
                return "?".into();
            }
            String::from_utf16_lossy(&name[..n as usize])
        }
    }

    /// Owner and group SID strings of a file and, with `names`, their account names (looked
    /// up once per SID in `cache`); "?" each when unreadable. The file is opened without
    /// following a link.
    pub fn owners(path: &crate::localfs::Unfollowed, names: bool, cache: &RefCell<HashMap<String, String>>) -> Owners {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL,
        };
        let unknown = || Owners { owner: "?".into(), group: "?".into(), owner_id: "?".into(), group_id: "?".into() };
        let Ok(file) = crate::localfs::open_unfollowed(path, |o| {
            o.access_mode(READ_CONTROL).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        }) else {
            return unknown();
        };
        let h = file.as_raw_handle() as HANDLE;
        unsafe {
            let (mut owner, mut group): (PSID, PSID) = (null_mut(), null_mut());
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            let err = GetSecurityInfo(
                h,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION,
                &mut owner,
                &mut group,
                null_mut(),
                null_mut(),
                &mut sd,
            );
            if err != 0 {
                return unknown();
            }
            let sid_of = |s: PSID| if s.is_null() { "?".to_string() } else { crate::platform::windows::ipc::sid_text(s).unwrap_or_else(|_| "?".into()) };
            let name_of = |s: PSID, id: &str| {
                if !names {
                    return String::new();
                }
                if let Some(n) = cache.borrow().get(id) {
                    return n.clone();
                }
                let n = account(s);
                cache.borrow_mut().insert(id.to_string(), n.clone());
                n
            };
            let (owner_id, group_id) = (sid_of(owner), sid_of(group));
            let got = Owners { owner: name_of(owner, &owner_id), group: name_of(group, &group_id), owner_id, group_id };
            LocalFree(sd as HLOCAL);
            got
        }
    }

    /// `lstat` of a file opened with backup semantics and without following a reparse point.
    pub fn lstat(local: &crate::localfs::Unfollowed) -> Option<Entry> {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let path = local.path();
        let file = crate::localfs::open_unfollowed(local, |o| {
            o.access_mode(FILE_READ_ATTRIBUTES).share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        })
        .ok()?;
        let h = file.as_raw_handle() as HANDLE;
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(h, &mut info) } == 0 {
            return None;
        }
        let mut standard: FILE_STANDARD_INFO = unsafe { std::mem::zeroed() };
        let size_of = std::mem::size_of::<FILE_STANDARD_INFO>() as u32;
        let alloc = if unsafe { GetFileInformationByHandleEx(h, FileStandardInfo, &mut standard as *mut _ as *mut _, size_of) } != 0 {
            standard.AllocationSize.max(0) as u64
        } else {
            0
        };
        let attrs = info.dwFileAttributes;
        let mut link = false;
        if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            let mut tag: FILE_ATTRIBUTE_TAG_INFO = unsafe { std::mem::zeroed() };
            let n = std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32;
            if unsafe { GetFileInformationByHandleEx(h, FileAttributeTagInfo, &mut tag as *mut _ as *mut _, n) } != 0 {
                link = tag.ReparseTag == IO_REPARSE_TAG_SYMLINK || tag.ReparseTag == IO_REPARSE_TAG_MOUNT_POINT;
            }
        }
        let dir = !link && attrs & FILE_ATTRIBUTE_DIRECTORY != 0;
        let exec = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .is_some_and(|e| matches!(e.as_str(), "exe" | "bat" | "cmd" | "com"));
        let mode = if link {
            0o120777
        } else if dir {
            0o040777
        } else {
            let base = if attrs & FILE_ATTRIBUTE_READONLY != 0 { 0o100444 } else { 0o100666 };
            if exec { base | 0o111 } else { base }
        };
        Some(Entry {
            is_dir: dir,
            is_file: !dir && !link,
            is_link: link,
            is_exec: mode & 0o111 != 0,
            size: if dir { 0 } else { (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow) },
            mtime: unix(info.ftLastWriteTime),
            atime: unix(info.ftLastAccessTime),
            ctime: unix(info.ftCreationTime),
            mode,
            nlink: u64::from(info.nNumberOfLinks),
            ino: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            blocks: alloc / 512,
        })
    }
}

/// Perl's `$!` text for a failed `opendir`.
#[cfg(windows)]
fn errno_text(e: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound => "No such file or directory".into(),
        ErrorKind::NotADirectory => "Not a directory".into(),
        ErrorKind::PermissionDenied => "Permission denied".into(),
        _ => e.to_string(),
    }
}

#[cfg(windows)]
impl Fs for WinFs {
    fn lstat(&self, path: &str) -> Option<Entry> {
        win::lstat(&self.unfollowed(path)?)
    }
    fn exists(&self, path: &str) -> bool {
        self.full(path).is_some_and(|p| crate::localfs::stat(&p).is_ok())
    }
    fn owners(&self, path: &str, names: bool) -> Owners {
        match self.unfollowed(path) {
            Some(p) => win::owners(&p, names, &self.names),
            None => Owners { owner: "?".into(), group: "?".into(), owner_id: "?".into(), group_id: "?".into() },
        }
    }
    fn list(&self, dir: &str) -> Result<Vec<String>, String> {
        let full = self.full(dir).ok_or_else(|| "No such file or directory".to_string())?;
        let entries = crate::localfs::entries(&full).map_err(|e| errno_text(&e))?;
        Ok(entries.into_iter().map(|e| e.name.to_string_lossy().into_owned()).collect())
    }
    fn open(&self, path: &str) -> Option<Box<dyn std::io::Read + '_>> {
        let f = crate::localfs::open(&self.full(path)?).ok()?;
        Some(Box::new(f))
    }
    fn dims(&self, path: &str) -> Option<(u32, u32)> {
        let f = std::io::BufReader::new(crate::localfs::open(&self.full(path)?).ok()?);
        image::ImageReader::new(f).with_guessed_format().ok()?.into_dimensions().ok()
    }
}

/// imgls: returns its exit code. `now` is the time in Unix seconds, `local` the local time
/// of a Unix time.
pub fn imgls(args: &[String], io: &mut Io, fs: &dyn Fs, now: i64, local: &dyn Fn(i64) -> Parts) -> i32 {
    let (o, mut args) = match parse(args) {
        Ok(v) => v,
        Err(warning) => {
            let _ = io.out.write_all(format!("imgls: {warning}\n").as_bytes());
            let _ = io.err.write_all(usage().as_bytes());
            return 1;
        }
    };
    // `find_dims_methods`: `--method` keeps only the method it names.
    if let Some(m) = o.method.as_deref().filter(|m| truthy(m))
        && m.to_lowercase() != METHOD
    {
        let _ = io.err.write_all(b"imgls: no methods found to obtain image dimensions. Tried: \n");
        return 1;
    }
    let samesort = (io.env)("LS_SAMESORT").is_some_and(|v| truthy(&v));
    let mut ls = Ls {
        o,
        fs,
        io,
        now,
        local,
        samesort,
        cache: RefCell::new(HashMap::new()),
        failed_types: HashSet::new(),
        do_header: u32::from(args.len() > 1),
        do_newline: 0,
    };
    if args.is_empty() {
        args.push(".".into());
    }
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    for a in args {
        match ls.lstat(&a) {
            None => ls.say_err(&format!("imgls: {a}: No such file or directory")),
            Some(st) if st.is_file => files.push(a),
            Some(_) => dirs.push(a),
        }
    }
    let mut files = ls.sort(files);
    let mut dirs = ls.sort(dirs);
    if ls.o.has('d') {
        files.append(&mut dirs);
    }
    if !files.is_empty() {
        ls.do_ls(None, files);
    }
    let hide = |name: &str, o: &Opts| {
        if o.has('a') {
            false
        } else if o.has('A') {
            name == "." || name == ".."
        } else {
            name.starts_with('.')
        }
    };
    let mut queue: VecDeque<String> = dirs.into();
    while let Some(path) = queue.pop_front() {
        // `! -e $path`: a dangling link (or one to a network path) is not there.
        if !ls.fs.exists(&path) {
            ls.say_err(&format!("imgls: {path}: No such file or directory"));
            continue;
        }
        let names = match ls.fs.list(&path) {
            Ok(n) => n,
            Err(e) => {
                ls.say_err(&format!("Unable to open directory {path}: {e}"));
                continue;
            }
        };
        let (mut f, mut d) = (Vec::new(), Vec::new());
        for name in [".".to_string(), "..".to_string()].into_iter().chain(names) {
            if hide(&name, &ls.o) {
                continue;
            }
            let p = format!("{path}/{name}");
            if ls.with_stat(&p, |e| e.is_some_and(|e| e.is_dir)) {
                d.push(p);
            } else {
                f.push(p);
            }
        }
        let recurse: Vec<String> = if ls.o.has('R') { d.iter().filter(|p| !p.ends_with('.')).cloned().collect() } else { Vec::new() };
        f.append(&mut d);
        ls.do_ls(Some(&path), f);
        queue.extend(recurse);
        ls.do_newline += 1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stamps::Parts;
    use crate::util_cli::{Cmd, FakeTty, Io};
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// Days since 1970-01-01 of a civil date (proleptic Gregorian).
    fn days(y: i64, m: i64, d: i64) -> i64 {
        let y = if m <= 2 { y - 1 } else { y };
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let mp = (m + 9) % 12;
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    /// Unix seconds as UTC parts: the tests' `localtime`.
    fn utc(t: i64) -> Parts {
        let z = t.div_euclid(86_400) + 719_468;
        let secs = t.rem_euclid(86_400);
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        Parts {
            year: y as u16,
            month: m as u16,
            day: d as u16,
            weekday: (t.div_euclid(86_400) + 4).rem_euclid(7) as u16,
            hour: (secs / 3600) as u16,
            minute: (secs / 60 % 60) as u16,
            second: (secs % 60) as u16,
        }
    }

    const NOW: i64 = 20_723 * 86_400; // 2026-09-27 00:00 UTC
    fn t_recent() -> i64 {
        days(2026, 9, 26) * 86_400 + 14 * 3600 + 5 * 60
    }
    fn t_old() -> i64 {
        days(2020, 1, 5) * 86_400 + 12 * 3600
    }

    fn file(size: u64, ino: u64) -> Entry {
        Entry {
            is_file: true,
            size,
            mtime: t_recent(),
            atime: t_recent(),
            ctime: t_old(),
            mode: 0o100644,
            nlink: 1,
            ino,
            blocks: if size > 0 { 8 } else { 0 },
            ..Default::default()
        }
    }

    fn dir(ino: u64) -> Entry {
        Entry {
            is_dir: true,
            size: 96,
            mtime: t_recent(),
            atime: t_recent(),
            ctime: t_old(),
            mode: 0o040755,
            nlink: 3,
            ino,
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct FakeFs {
        stats: BTreeMap<String, Entry>,
        dirs: BTreeMap<String, Vec<String>>,
        bytes: BTreeMap<String, Vec<u8>>,
        dims: BTreeMap<String, (u32, u32)>,
        /// Links whose target is not there.
        dangling: Vec<String>,
        probed: RefCell<Vec<String>>,
        /// `owners` calls, with whether names were asked for.
        owner_calls: RefCell<Vec<(String, bool)>>,
        /// Files whose reads follow a script (taken by the first `open`).
        scripted: RefCell<BTreeMap<String, Scripted>>,
    }

    /// A reader answering each `read` from its list: bytes, or an error of that kind.
    #[derive(Default)]
    struct Scripted(std::collections::VecDeque<Result<Vec<u8>, std::io::ErrorKind>>);

    impl std::io::Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                None => Ok(0),
                Some(Err(kind)) => Err(kind.into()),
                Some(Ok(mut bytes)) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    if n < bytes.len() {
                        self.0.push_front(Ok(bytes.split_off(n)));
                    }
                    Ok(n)
                }
            }
        }
    }

    impl Fs for FakeFs {
        fn lstat(&self, path: &str) -> Option<Entry> {
            self.probed.borrow_mut().push(path.to_string());
            self.stats.get(path).cloned()
        }
        fn exists(&self, path: &str) -> bool {
            self.probed.borrow_mut().push(path.to_string());
            self.stats.contains_key(path) && !self.dangling.iter().any(|d| d == path)
        }
        fn owners(&self, path: &str, names: bool) -> Owners {
            self.owner_calls.borrow_mut().push((path.to_string(), names));
            let dir = self.stats.get(path).is_some_and(|e| e.is_dir);
            let group = if dir { "wheel" } else { "staff" };
            Owners {
                owner: if names { "robert".into() } else { String::new() },
                group: if names { group.into() } else { String::new() },
                owner_id: "S-1-5-21-7-1001".into(),
                group_id: "S-1-5-21-7-513".into(),
            }
        }
        fn list(&self, dir: &str) -> Result<Vec<String>, String> {
            self.probed.borrow_mut().push(dir.to_string());
            self.dirs.get(dir).cloned().ok_or_else(|| "Not a directory".to_string())
        }
        fn open(&self, path: &str) -> Option<Box<dyn std::io::Read + '_>> {
            if let Some(r) = self.scripted.borrow_mut().remove(path) {
                return Some(Box::new(r));
            }
            let b = self.bytes.get(path)?;
            Some(Box::new(&b[..]))
        }
        fn dims(&self, path: &str) -> Option<(u32, u32)> {
            self.dims.get(path).copied()
        }
    }

    /// Folder "." holds `a.png` ("PNGDATA", 2x2), an empty `b.txt`, an empty `.hidden` and
    /// folder `sub` (holding an empty `x`).
    fn fake() -> FakeFs {
        let mut fs = FakeFs::default();
        for (p, e) in [
            (".", dir(1)),
            ("./.", dir(1)),
            ("./..", dir(2)),
            ("./.hidden", file(0, 10)),
            ("./a.png", file(7, 11)),
            ("./b.txt", file(0, 12)),
            ("./sub", dir(13)),
            ("./sub/x", file(0, 14)),
            ("a.png", file(7, 11)),
            ("b.txt", file(0, 12)),
            ("sub", dir(13)),
            ("sub/x", file(0, 14)),
        ] {
            fs.stats.insert(p.into(), e);
        }
        fs.dirs.insert(".".into(), vec![".hidden".into(), "a.png".into(), "b.txt".into(), "sub".into()]);
        fs.dirs.insert("./sub".into(), vec!["x".into()]);
        fs.dirs.insert("sub".into(), vec!["x".into()]);
        for p in ["./a.png", "a.png"] {
            fs.bytes.insert(p.into(), b"PNGDATA".to_vec());
            fs.dims.insert(p.into(), (2, 2));
        }
        fs
    }

    fn run_env(fs: &FakeFs, args: &[&str], vars: &[(&str, &str)]) -> (i32, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut input: &[u8] = &[];
        let mut tty = FakeTty::default();
        let vars: BTreeMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let env = |k: &str| vars.get(k).cloned();
        let mut random = || 0u32;
        let mut run = |_: &Cmd| Err(String::new());
        let have = |_: &str| true;
        let mut io = Io {
            out: &mut out,
            err: &mut err,
            input: &mut input,
            stdin_tty: true,
            stdout_tty: true,
            tty: &mut tty,
            env: &env,
            cwd: PathBuf::from("."),
            random: &mut random,
            run: &mut run,
            have: &have,
            secret: &mut || None,
            env_all: &Vec::new,
        };
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        let code = imgls(&args, &mut io, fs, NOW, &utc);
        (code, String::from_utf8(out).unwrap(), String::from_utf8(err).unwrap())
    }

    fn run(fs: &FakeFs, args: &[&str]) -> (i32, String, String) {
        run_env(fs, args, &[])
    }

    fn img(params: &str, name: &str, size: u64, data: &str) -> String {
        format!("\x1b]1337;File={params};name={name};size={size}:{data}\x07")
    }

    const PARAMS: &str = "inline=1;height=1;width=3;preserveAspectRatio=true";

    fn a_img() -> String {
        img(PARAMS, "Li9hLnBuZw==", 7, "UE5HREFUQQ==\n")
    }

    fn black() -> String {
        img(PARAMS, "b25lX3BpeGVsX2JsYWNr", 96, BLACK_PIXEL)
    }

    fn plain() -> String {
        format!("{} a.png\n{} b.txt\n{} sub\n", a_img(), black(), black())
    }

    #[test]
    fn preview_reads_retry_and_fail_whole() {
        use std::io::ErrorKind::{Interrupted, Other};
        let listing = |reads: Vec<Result<Vec<u8>, std::io::ErrorKind>>| {
            let fs = fake();
            fs.scripted.borrow_mut().insert("./a.png".into(), Scripted(reads.into()));
            run(&fs, &[]).1
        };
        let bytes = |b: &[u8]| Ok(b.to_vec());
        assert_eq!(listing(vec![Err(Interrupted), bytes(b"PNG"), Err(Interrupted), bytes(b"DATA")]), plain(), "an interrupted read is tried again");
        let all_black = format!("{} a.png\n{} b.txt\n{} sub\n", black(), black(), black());
        assert_eq!(listing(vec![Err(Other)]), all_black, "a failed read is the black pixel");
        assert_eq!(listing(vec![bytes(b"PNG"), Err(Other)]), all_black, "a failure in the first block sends nothing of the file");
        // Past the first block, bytes are out: the sequence ends where the reads stopped.
        let block = vec![7u8; 57 * 1024];
        let encoded: String = block.chunks(57).map(|c| format!("{}\n", b64(c))).collect();
        let want = format!("{} a.png\n{} b.txt\n{} sub\n", img(PARAMS, "Li9hLnBuZw==", 7, &encoded), black(), black());
        assert_eq!(listing(vec![Ok(block), Err(Other)]), want);
    }

    #[test]
    fn imgls_follows_the_script() {
        assert_eq!(BLACK_PIXEL.len(), 96);
        let fs = fake();
        assert_eq!(run(&fs, &[]), (0, plain(), String::new()));
        assert_eq!(run(&fs, &["."]), (0, plain(), String::new()));

        // -a adds . and .. and .hidden, sorted by path; -A only .hidden.
        let hidden = format!("{} .hidden\n", black());
        let dots = format!("{} .\n{} ..\n", black(), black());
        assert_eq!(run(&fs, &["-a"]).1, format!("{dots}{hidden}{}", plain()));
        assert_eq!(run(&fs, &["-A"]).1, format!("{hidden}{}", plain()));

        // Image parameters.
        let p = "inline=1;height=2;width=5;preserveAspectRatio=false";
        let (code, out, _) = run(&fs, &["--width", "5", "--height", "2", "--nopreserve_ratio"]);
        assert_eq!(code, 0);
        assert_eq!(
            out,
            format!(
                "{} a.png\n{} b.txt\n{} sub\n",
                img(p, "Li9hLnBuZw==", 7, "UE5HREFUQQ==\n"),
                img(p, "b25lX3BpeGVsX2JsYWNr", 96, BLACK_PIXEL),
                img(p, "b25lX3BpeGVsX2JsYWNr", 96, BLACK_PIXEL)
            )
        );

        // -r reverses; -d lists "." itself.
        assert_eq!(run(&fs, &["-r"]).1, format!("{} sub\n{} b.txt\n{} a.png\n", black(), black(), a_img()));
        assert_eq!(run(&fs, &["-d"]).1, format!("{} .\n", black()));

        // File arguments name the argument itself.
        assert_eq!(
            run(&fs, &["a.png", "b.txt"]),
            (0, format!("{} a.png\n{} b.txt\n", img(PARAMS, "YS5wbmc=", 7, "UE5HREFUQQ==\n"), black()), String::new())
        );
        assert_eq!(run(&fs, &["missing"]), (0, String::new(), "imgls: missing: No such file or directory\n".into()));
        let (_, out, err) = run(&fs, &["a.png", "missing"]);
        assert_eq!(out, format!("{} a.png\n", img(PARAMS, "YS5wbmc=", 7, "UE5HREFUQQ==\n")));
        assert_eq!(err, "imgls: missing: No such file or directory\n");

        // A network path is never probed, in any spelling.
        let spellings = ["\\\\host\\share", "//host/share", "/\\host\\share", "\\/host/share", "\\\\?\\UNC\\host\\s", "\\\\.\\pipe\\x", "\\??\\UNC\\host\\s"];
        let (code, out, err) = run(&fs, &spellings);
        assert_eq!((code, out.as_str()), (0, ""));
        let want: String = spellings.iter().map(|p| format!("imgls: {p}: No such file or directory\n")).collect();
        assert_eq!(err, want);
        assert!(fs.probed.borrow().iter().all(|p| !network_path(p)), "{:?}", fs.probed.borrow());

        // A link argument whose target is not there: the script's `-e` message.
        let mut dangling = fake();
        dangling.stats.insert("L".into(), Entry { is_link: true, mode: 0o120777, ..Default::default() });
        dangling.dangling.push("L".into());
        assert_eq!(run(&dangling, &["L"]), (0, String::new(), "imgls: L: No such file or directory\n".into()));

        // Owners are asked for only by the long listing, names only when shown.
        assert!(fs.owner_calls.borrow().is_empty());
        run(&fs, &["-l"]);
        assert!(!fs.owner_calls.borrow().is_empty() && fs.owner_calls.borrow().iter().all(|(_, names)| *names));
        fs.owner_calls.borrow_mut().clear();
        run(&fs, &["-ln"]);
        assert!(!fs.owner_calls.borrow().is_empty() && fs.owner_calls.borrow().iter().all(|(_, names)| !*names));

        // --dimensions: " [%*d x %*d] " or the `%*s` blanks (" %*s   %*s   ", 9 columns here).
        let blank = " ".repeat(9);
        assert_eq!(
            run(&fs, &["--dimensions"]).1,
            format!("{} [2 x 2]  a.png\n{}{blank} b.txt\n{}{blank} sub\n", a_img(), black(), black())
        );

        // -F marks folders.
        assert_eq!(run(&fs, &["-F"]).1, format!("{} a.png\n{} b.txt\n{} sub/\n", a_img(), black(), black()));

        // -R and several arguments print headers.
        let x = format!("{} x\n", black());
        assert_eq!(run(&fs, &["-R"]).1, format!("{}\n./sub:\n{x}", plain()));
        assert_eq!(run(&fs, &[".", "sub"]).1, format!(".:\n{}\nsub:\n{x}", plain()));

        // Sorting: -t (ties by name, reversed with -y or LS_SAMESORT), -S by size.
        assert_eq!(run(&fs, &["-t"]).1, plain());
        let rev = format!("{} sub\n{} b.txt\n{} a.png\n", black(), black(), a_img());
        assert_eq!(run(&fs, &["-ty"]).1, rev);
        assert_eq!(run_env(&fs, &["-t"], &[("LS_SAMESORT", "1")]).1, rev);
        assert_eq!(run(&fs, &["-S"]).1, format!("{} sub\n{} a.png\n{} b.txt\n", black(), a_img(), black()));
        // -u: access time, oldest first; -c without -l sorts by name; -p marks folders only.
        let mut aged = fake();
        for (p, t) in [("./a.png", 30), ("./b.txt", 10), ("./sub", 20)] {
            aged.stats.get_mut(p).unwrap().atime = t;
            aged.stats.get_mut(p).unwrap().mtime = 100 - t;
        }
        assert_eq!(run(&aged, &["-u"]).1, format!("{} b.txt\n{} sub\n{} a.png\n", black(), black(), a_img()));
        assert_eq!(run(&aged, &["-c"]).1, plain());
        assert_eq!(run(&aged, &["-t"]).1, format!("{} b.txt\n{} sub\n{} a.png\n", black(), black(), a_img()));
        assert_eq!(run(&fs, &["-p"]).1, format!("{} a.png\n{} b.txt\n{} sub/\n", a_img(), black(), black()));
    }

    #[test]
    fn imgls_options_follow_the_script() {
        let fs = fake();
        assert_eq!(
            usage(),
            format!(
                "usage: imgls [--width #] [--height #] [--[no]preserve_ratio] [--[no]dimensions]\n{} [--[no]unknown] [ls options] [file ...]\n",
                " ".repeat(12)
            )
        );
        assert_eq!(run(&fs, &["-z"]), (1, "imgls: Unknown option: z\n\n".into(), usage()));
        assert_eq!(run(&fs, &["--bogus"]), (1, "imgls: Unknown option: bogus\n\n".into(), usage()));
        assert_eq!(run(&fs, &["--width"]), (1, "imgls: Option width requires an argument\n\n".into(), usage()));
        assert_eq!(
            run(&fs, &["--method", "sips"]),
            (1, String::new(), "imgls: no methods found to obtain image dimensions. Tried: \n".into())
        );
        assert_eq!(run(&fs, &["--method", "image"]), (0, plain(), String::new()));
        assert_eq!(run(&fs, &["--method=IMAGE"]), (0, plain(), String::new()));

        let parsed = |v: &[&str]| parse(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        let flags = |v: &[&str]| parsed(v).unwrap().0.flags.into_iter().collect::<String>();
        let d = Opts::default();
        assert_eq!((d.width.as_str(), d.height.as_str(), d.preserve.as_str()), ("3", "1", "true"));
        assert_eq!(flags(&["-lh"]), "hl");
        assert_eq!(flags(&["-n"]), "ln");
        assert_eq!(flags(&["-o"]), "lo");
        assert_eq!(flags(&["-tS"]), "S");
        assert_eq!(flags(&["-St"]), "t");
        assert_eq!(flags(&["-dR"]), "d");
        assert_eq!(flags(&["-U"]), "c");
        assert_eq!(flags(&["-TD%F"]), "");
        let (o, rest) = parsed(&["-lD", "%F", "x", "-a"]).unwrap();
        assert_eq!((o.d_format.as_deref(), rest), (Some("%F"), vec!["x".to_string(), "-a".to_string()]));
        assert_eq!(parsed(&["--", "-l"]).unwrap().1, vec!["-l".to_string()]);
        assert_eq!(parsed(&["-", "-l"]).unwrap().1, vec!["-".to_string(), "-l".to_string()]);
        let (o, _) = parsed(&["--wid", "4", "--height=2", "--nopreserve_ratio"]).unwrap();
        assert_eq!((o.width.as_str(), o.height.as_str(), o.preserve.as_str()), ("4", "2", "false"));
        assert_eq!(parsed(&["--no-preserve_ratio", "--preserve_ratio"]).unwrap().0.preserve, "true");
        // The counters: each of --[no]dimensions and --[no]unknown turns the other on.
        let (o, _) = parsed(&["--dimensions"]).unwrap();
        assert_eq!((o.dimensions, o.unknown), (Some(true), Some(true)));
        let (o, _) = parsed(&["--nounknown"]).unwrap();
        assert_eq!((o.dimensions, o.unknown), (Some(true), Some(false)));
        let (o, _) = parsed(&["--nodimensions"]).unwrap();
        assert_eq!((o.dimensions, o.unknown), (Some(false), Some(true)));
        let (o, _) = parsed(&["--dimensions", "--nounknown"]).unwrap();
        assert_eq!((o.dimensions, o.unknown), (Some(true), Some(false)));
        assert_eq!(parsed(&["--method", "x"]).unwrap().0.method.as_deref(), Some("x"));
        assert_eq!(parsed(&["-D"]), Err("Option D requires an argument\n".into()));
        assert_eq!(parsed(&["--width="]), Err("Option width requires an argument\n".into()));
        assert_eq!(parsed(&["--dimensions=1"]), Err("Option dimensions does not take an argument\n".into()));
        assert_eq!(parsed(&["-lz"]), Err("Unknown option: z\n".into()));
        assert_eq!(
            parsed(&["--no"]),
            Err("Option no is ambiguous (no-dimensions, no-preserve_ratio, no-unknown, nodimensions, nopreserve_ratio, nounknown)\n".into())
        );
        assert_eq!(run(&fs, &["--no"]).0, 1);
        // `--<letter>` is the single-letter option, as Getopt::Long matches it.
        assert_eq!(flags(&["--l"]), "l");
        assert_eq!(flags(&["--n"]), "ln");
        assert_eq!(flags(&["--d", "--R"]), "d");
        assert_eq!(flags(&["--u", "--h", "--F"]), "Fhu");
        assert_eq!(flags(&["--U"]), "c");
        assert_eq!(flags(&["-t", "--S"]), "S");
        let (o, rest) = parsed(&["--h", "x"]).unwrap();
        assert_eq!((o.height.as_str(), rest), ("1", vec!["x".to_string()]), "--h is -h, not --height");
        assert_eq!(parsed(&["--u"]).unwrap().0.unknown, None, "--u is -u, not --unknown");
        assert_eq!(parsed(&["--d"]).unwrap().0.dimensions, None, "--d is -d, not --dimensions");
        assert_eq!(parsed(&["--D", "%F", "--T"]).unwrap().0.d_format.as_deref(), Some("%F"));
        assert_eq!(flags(&["--D=%F", "--T"]), "");
        assert_eq!(parsed(&["--D"]), Err("Option D requires an argument\n".into()));
        assert_eq!(parsed(&["--l=1"]), Err("Option l does not take an argument\n".into()));
        assert_eq!(parsed(&["--di"]).unwrap().0.dimensions, Some(true));
        assert_eq!(run(&fs, &["--l"]).1, run(&fs, &["-l"]).1);
        assert_eq!(run(&fs, &["--d"]).1, format!("{} .\n", black()));

        // --dimensions --nounknown: a file without dimensions gets the black pixel.
        let mut fs = fake();
        fs.dims.clear();
        let blank = String::new();
        assert_eq!(
            run(&fs, &["--dimensions", "--nounknown"]).1,
            format!("{}{blank} a.png\n{} b.txt\n{} sub\n", black(), black(), black())
        );
        assert_eq!(run(&fs, &["--dimensions"]).1, plain());
    }

    #[test]
    fn long_format_follows_the_script() {
        assert_eq!(format_mode(0o100644), "-rw-r--r--");
        assert_eq!(format_mode(0o040777), "drwxrwxrwx");
        assert_eq!(format_mode(0o104755), "-rwsr-xr-x");
        assert_eq!(format_mode(0o041777), "drwxrwxrwt");
        assert_eq!(format_mode(0o102644), "-rw-r-Sr--");
        assert_eq!(format_mode(0o120777), "lrwxrwxrwx");
        for (n, s) in [(0, "0B"), (999, "999B"), (1000, "1.0K"), (1536, "1.5K"), (10240, "10K"), (123456789, "118M")] {
            assert_eq!(format_human(n), s, "{n}");
        }

        let p = utc(days(2026, 9, 6) * 86_400 + 9 * 3600 + 5 * 60 + 3);
        assert_eq!(
            strftime("%a %A %b %B %d %e %H %I %M %S %p %y %Y %m %j %R %T %F %D %% %q%n%t", &p),
            "Sun Sunday Sep September 06  6 09 09 05 03 AM 26 2026 09 249 09:05 09:05:03 2026-09-06 09/06/26 % %q\n\t"
        );
        assert_eq!(strftime("%I %p", &utc(13 * 3600)), "01 PM");
        assert_eq!(strftime("%I %p", &utc(0)), "12 AM");

        let o = Opts::default();
        assert_eq!(format_time(t_recent(), NOW, &o, &utc), "Sep 26 14:05");
        assert_eq!(format_time(t_old(), NOW, &o, &utc), "Jan  5  2020");
        let mut t = Opts::default();
        t.flags.insert('T');
        assert_eq!(format_time(t_recent(), NOW, &t, &utc), "Sep 26 14:05:00 2026");
        let f = Opts { d_format: Some("%F".into()), ..Opts::default() };
        assert_eq!(format_time(t_recent(), NOW, &f, &utc), "2026-09-26");

        let fs = fake();
        let (a, b) = (a_img(), black());
        assert_eq!(
            run(&fs, &["-l"]),
            (
                0,
                format!(
                    "total 8\n{a} -rw-r--r-- 1 robert  staff   7 Sep 26 14:05 a.png\n{b} -rw-r--r-- 1 robert  staff   0 Sep 26 14:05 b.txt\n{b} drwxr-xr-x 3 robert  wheel  96 Sep 26 14:05 sub\n"
                ),
                String::new()
            )
        );
        assert_eq!(
            run(&fs, &["-lnc"]).1,
            format!(
                "total 8\n{a} -rw-r--r-- 1 S-1-5-21-7-1001  S-1-5-21-7-513   7 Jan  5  2020 a.png\n{b} -rw-r--r-- 1 S-1-5-21-7-1001  S-1-5-21-7-513   0 Jan  5  2020 b.txt\n{b} drwxr-xr-x 3 S-1-5-21-7-1001  S-1-5-21-7-513  96 Jan  5  2020 sub\n"
            )
        );
        assert_eq!(
            run(&fs, &["-loh"]).1,
            format!(
                "total 8\n{a} -rw-r--r-- 1 robert    7B Sep 26 14:05 a.png\n{b} -rw-r--r-- 1 robert    0B Sep 26 14:05 b.txt\n{b} drwxr-xr-x 3 robert   96B Sep 26 14:05 sub\n"
            )
        );
        assert_eq!(
            run(&fs, &["-lik"]).1,
            format!(
                "total 4\n{a} 11 -rw-r--r-- 1 robert  staff   7 Sep 26 14:05 a.png\n{b} 12 -rw-r--r-- 1 robert  staff   0 Sep 26 14:05 b.txt\n{b} 13 drwxr-xr-x 3 robert  wheel  96 Sep 26 14:05 sub\n"
            )
        );
        // -s shows blocks and, as the script does, the link count.
        assert_eq!(run(&fs, &["-s"]).1, format!("total 8\n{a} 8 1 a.png\n{b} 0 1 b.txt\n{b} 0 3 sub\n"));
        // File arguments: no total.
        assert_eq!(
            run(&fs, &["-l", "b.txt"]).1,
            format!("{b} -rw-r--r-- 1 robert  staff  0 Sep 26 14:05 b.txt\n")
        );
    }

    /// The real file system on a scratch folder, and the `run` arm over it.
    #[cfg(windows)]
    #[test]
    fn win_fs_follows_perl_stat() {
        use crate::util_cli::Harness;
        use std::io::Read;
        let dir = std::env::temp_dir().join(format!("termurion-imgls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        image::RgbImage::new(2, 2).save(dir.join("a.png")).unwrap();
        std::fs::write(dir.join("b.txt"), b"").unwrap();
        std::fs::write(dir.join("run.CMD"), b"@echo off\r\n").unwrap();
        std::fs::write(dir.join("ro.txt"), b"x").unwrap();
        let mut perm = std::fs::metadata(dir.join("ro.txt")).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(dir.join("ro.txt"), perm).unwrap();

        let fs = WinFs::new(dir.clone());
        let a = fs.lstat("./a.png").unwrap();
        assert!(a.is_file && !a.is_dir && !a.is_link && !a.is_exec);
        assert_eq!(a.mode, 0o100666);
        assert_eq!(a.size, std::fs::metadata(dir.join("a.png")).unwrap().len());
        assert!(a.nlink >= 1 && a.ino != 0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        assert!((a.mtime - now).abs() < 600, "{} vs {now}", a.mtime);
        let who = fs.owners("./a.png", true);
        assert!(who.owner != "?" && !who.owner.is_empty(), "{}", who.owner);
        assert!(who.owner_id.starts_with("S-1-") && who.group_id.starts_with("S-1-"), "{} {}", who.owner_id, who.group_id);
        assert_eq!(fs.names.borrow().get(&who.owner_id), Some(&who.owner), "names are kept per SID");
        let bare = fs.owners("./a.png", false);
        assert_eq!((bare.owner.as_str(), bare.owner_id.as_str()), ("", who.owner_id.as_str()));
        assert_eq!(fs.lstat("run.CMD").unwrap().mode, 0o100777);
        assert!(fs.lstat("run.CMD").unwrap().is_exec);
        assert_eq!(fs.lstat("ro.txt").unwrap().mode, 0o100444);
        let sub = fs.lstat("sub").unwrap();
        assert!(sub.is_dir && !sub.is_file);
        assert_eq!((sub.mode, sub.size), (0o040777, 0));
        assert!(fs.lstat(".").unwrap().is_dir && fs.lstat("./..").unwrap().is_dir);
        assert_eq!(fs.lstat("missing"), None);
        for p in ["\\\\nohost\\share", "/\\nohost\\share", "\\/nohost/share", "\\\\?\\UNC\\nohost\\s"] {
            assert_eq!(fs.lstat(p), None, "{p}");
            assert!(!fs.exists(p) && fs.list(p).is_err() && fs.open(p).is_none() && fs.dims(p).is_none(), "{p}");
        }
        let mut names = fs.list(".").unwrap();
        names.sort();
        assert_eq!(names, ["a.png", "b.txt", "ro.txt", "run.CMD", "sub"]);
        assert!(fs.list("b.txt").is_err());
        assert_eq!(fs.dims("./a.png"), Some((2, 2)));
        assert_eq!(fs.dims("b.txt"), None);
        let mut x = Vec::new();
        fs.open("ro.txt").unwrap().read_to_end(&mut x).unwrap();
        assert_eq!(x, b"x");

        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        assert_eq!(h.run("imgls", &["-F", "--dimensions"]), 0);
        let out = String::from_utf8(h.out.clone()).unwrap();
        // encode_base64: 76-column lines, each ending "\n".
        let data: String = b64(&std::fs::read(dir.join("a.png")).unwrap()).as_bytes().chunks(76).map(|c| format!("{}\n", std::str::from_utf8(c).unwrap())).collect();
        assert!(data.matches('\n').count() > 1);
        assert!(out.starts_with(&format!("\x1b]1337;File={PARAMS};name=Li9hLnBuZw==;size={}:{data}\x07 [2 x 2]  a.png\n", a.size)), "{out:?}");
        assert!(out.ends_with(" run.CMD*\n\x1b]1337;File=inline=1;height=1;width=3;preserveAspectRatio=true;name=b25lX3BpeGVsX2JsYWNr;size=96:iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIW2NgYGD4DwABBAEAwS2OUAAAAABJRU5ErkJggg==\x07          sub/\n"), "{out:?}");
        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        assert_eq!(h.run("imgls", &["-l", "b.txt"]), 0);
        let out = String::from_utf8(h.out.clone()).unwrap();
        assert!(out.contains(" -rw-rw-rw- 1 "), "{out:?}");

        // A junction argument is followed (the script's `opendir`), and shows as a link.
        let made = std::process::Command::new("cmd.exe")
            .args(["/c", "mklink", "/J"])
            .arg(dir.join("jn"))
            .arg(dir.join("sub"))
            .stdout(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        assert!(made, "mklink /J needs no privilege");
        std::fs::write(dir.join("sub").join("in.txt"), b"").unwrap();
        let jn = fs.lstat("jn").unwrap();
        assert!(jn.is_link && !jn.is_dir && !jn.is_file && jn.mode == 0o120777);
        assert!(fs.exists("jn"));
        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        assert_eq!(h.run("imgls", &["jn"]), 0);
        assert!(String::from_utf8(h.out.clone()).unwrap().ends_with(" in.txt\n"));
        let mut h = Harness { cwd: dir.clone(), ..Default::default() };
        assert_eq!(h.run("imgls", &["-F", "-d", "jn"]), 0);
        assert!(String::from_utf8(h.out.clone()).unwrap().ends_with(" jn@\n"));
        // A link to a network path is never followed (making one needs the symbolic link
        // privilege or developer mode).
        match std::os::windows::fs::symlink_dir("\\\\nohost.invalid\\share", dir.join("net")) {
            Ok(()) => {
                assert!(fs.lstat("net").is_some_and(|e| e.is_link));
                assert!(!fs.exists("net") && fs.list("net").is_err() && fs.lstat("net/x").is_none() && fs.open("net").is_none());
                let mut h = Harness { cwd: dir.clone(), ..Default::default() };
                assert_eq!(h.run("imgls", &["net"]), 0);
                assert_eq!(String::from_utf8(h.err.clone()).unwrap(), "imgls: net: No such file or directory\n");
            }
            Err(e) => eprintln!("MEASURED: no symbolic link to a network path made ({e}); util_cli's may_open test covers the rule"),
        }
        let _ = std::fs::remove_dir(dir.join("jn"));
        let _ = std::fs::remove_dir(dir.join("net"));

        let mut perm = std::fs::metadata(dir.join("ro.txt")).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perm.set_readonly(false);
        std::fs::set_permissions(dir.join("ro.txt"), perm).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
