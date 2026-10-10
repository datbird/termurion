//! Save Page As, the pure part (iTerm2's `PageSaver`): file names, CSS `url()` rewriting and the
//! volume bounds. Every name and every byte comes from a web page, so the one sanitizer is built on
//! the existing owners: `inert_files::is_hidden` (hidden and bidi characters),
//! `inert_files::is_device_name` (device names), `settings_export::aliased` (the Windows alias
//! rule) and `inert_files::high_risk` (the one dangerous-type list, which `semantic::runnable` reads
//! too). No second list is kept here.

use crate::inert_files::{is_device_name, is_hidden};
use crate::mime_ext::extension_for_mime;
use regex::{Captures, Regex};
use std::sync::OnceLock;

/// The longest file name, in UTF-16 units.
pub const MAX_NAME_UNITS: usize = 240;
/// The longest extension kept when a long name is cut, in UTF-16 units.
const MAX_EXT_UNITS: usize = 16;
/// Resource count, total bytes and per-resource bytes one save may take (review B5).
pub const MAX_RESOURCES: usize = 2000;
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_RESOURCE_BYTES: u64 = 8 * 1024 * 1024;
/// A stylesheet longer than this is not scanned (it is kept as is), and at most this many `url()`
/// references of one stylesheet are read.
pub const MAX_CSS_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_CSS_URLS: usize = 5000;
/// How deep stylesheets that import stylesheets are followed (Task 9 review I2).
pub const MAX_CSS_DEPTH: usize = 8;

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

fn trim_end_dots(s: &str) -> &str {
    s.trim_end_matches(['.', ' '])
}

/// Whether a file name ends in an extension Windows would run, open, follow or parse: the one
/// list, `inert_files::high_risk` (a saved script too; the door refuses it, controller ruling for
/// Task 8 I2).
pub fn refused_extension(name: &str) -> bool {
    crate::inert_files::high_risk(name)
}

/// Cuts a name to `max` UTF-16 units, keeping a short extension, never splitting a surrogate pair;
/// trailing dots and spaces left by the cut go.
fn cap(name: &str, max: usize) -> String {
    if units(name) <= max {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if units(e) <= MAX_EXT_UNITS => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let room = max.saturating_sub(units(&ext));
    let mut cut = String::new();
    let mut used = 0;
    for c in stem.chars() {
        used += c.len_utf16();
        if used > room {
            break;
        }
        cut.push(c);
    }
    let cut = trim_end_dots(&cut).to_string();
    format!("{cut}{ext}")
}

/// A file name that is safe to create under the folder the person chose. iTerm2's
/// `\/:*?"<>|` become `_`; then control characters become `_`; hidden, bidi and format characters
/// are dropped (`inert_files::is_hidden`); trailing dots and spaces go; empty, `.` and `..` become
/// `resource`; the length is cut to two units under `MAX_NAME_UNITS`; a device name gets a `_`
/// prefix; a high-risk extension (the one list, `inert_files::high_risk`) is neutralized with a
/// trailing `_`; an 8.3 `~digit` alias becomes `_digit`; the final name is rechecked with the
/// owners (`resource` when it fails).
pub fn sanitize(name: &str) -> String {
    sanitize_with(name, true)
}

/// `sanitize`; `neutralize` false leaves an executable-looking ending alone (a folder named after
/// a host such as "example.com" is never run).
fn sanitize_with(name: &str, neutralize: bool) -> String {
    let mut s: String = name
        .chars()
        .filter_map(|c| {
            if "\\/:*?\"<>|".contains(c) || (c as u32) < 0x20 {
                Some('_')
            } else if is_hidden(c) {
                None
            } else {
                Some(c)
            }
        })
        .collect();
    s = trim_end_dots(&s).to_string();
    if s.is_empty() || s == "." || s == ".." {
        s = "resource".to_string();
    }
    // Cut first (room for the two `_` the steps below may add), so no later cut can bring back a
    // type the neutralize step did not see (Task 8 review I1).
    s = cap(&s, MAX_NAME_UNITS - 2);
    if s.is_empty() {
        s = "resource".to_string();
    }
    if is_device_name(&s) {
        s.insert(0, '_');
    }
    if neutralize && refused_extension(&s) {
        s.push('_');
    }
    if crate::settings_export::aliased(&s) {
        s = s.replace('~', "_");
    }
    if final_ok(&s, neutralize) { s } else { "resource".to_string() }
}

/// The recheck of a final name with the owners: within `MAX_NAME_UNITS`, no device or alias form,
/// no listing type, and (for a file) no high-risk type.
fn final_ok(s: &str, file: bool) -> bool {
    units(s) <= MAX_NAME_UNITS
        && !s.is_empty()
        && !is_device_name(s)
        && !crate::settings_export::aliased(s)
        && (!file || (!refused_extension(s) && !crate::inert_files::refused_on_listing(s)))
}

/// The folder-name default (`PSV:347`): the host sanitized, else "page".
pub fn default_name(url: &str) -> String {
    let host = url
        .split_once("://")
        .map(|(_, r)| r.split(['/', '?', '#']).next().unwrap_or(""))
        .map(|a| a.rsplit('@').next().unwrap_or(""))
        .unwrap_or("");
    if host.is_empty() { "page".to_string() } else { sanitize_with(host, false) }
}

/// The saved name of a resource (`PSV:234-250`). The extension follows Tandem's MIME table when a
/// MIME is known and the page's own extension is missing or would run; a MIME that maps to a
/// runnable extension is saved as `bin`.
pub fn base_filename(url: &str, mime: Option<&str>) -> String {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let path = path.split_once("://").map_or(path, |(_, r)| r.find('/').map_or("", |i| &r[i..]));
    let decode = |s: &str| crate::marks::percent_decode(s);
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let last = path.rsplit('/').next().unwrap_or("");
    let mut name = if last.is_empty() { segments.next_back().map(decode).unwrap_or_default() } else { decode(last) };
    if name.is_empty() || name == "/" {
        name = "resource".to_string();
    }
    if let Some(m) = mime {
        let mut ext = extension_for_mime(m);
        if refused_extension(&format!("x.{ext}")) {
            ext = "bin".to_string();
        }
        match name.rsplit_once('.') {
            None => name = format!("{name}.{ext}"),
            Some((stem, own)) if !stem.is_empty() || !own.is_empty() => {
                // A page-supplied extension that would run never survives next to a known type.
                if refused_extension(&name) {
                    name = format!("{stem}.{ext}");
                }
            }
            _ => {}
        }
    }
    sanitize(&name)
}

/// The first of `name_1.ext`, `name_2.ext`, ... that is free (`PSV:252-262`); `base` itself when
/// free. A suffixed name that would pass `MAX_NAME_UNITS` gets a shorter stem, never a shorter
/// suffix or extension.
pub fn unique_filename(base: &str, taken: &dyn Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    let (stem, ext) = match base.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && units(e) <= MAX_EXT_UNITS => (s, format!(".{e}")),
        _ => (base, String::new()),
    };
    let mut i: u64 = 1;
    loop {
        let suffix = format!("_{i}{ext}");
        let room = MAX_NAME_UNITS.saturating_sub(units(&suffix));
        let mut cut = String::new();
        let mut used = 0;
        for c in stem.chars() {
            used += c.len_utf16();
            if used > room {
                break;
            }
            cut.push(c);
        }
        let candidate = format!("{cut}{suffix}");
        if final_ok(&candidate, true) && !taken(&candidate) {
            return candidate;
        }
        i += 1;
    }
}

/// The count and byte bounds of one save (review B5): `admit` answers whether one more resource of
/// `len` bytes may be kept, and counts it when it may.
#[derive(Default, Debug)]
pub struct Budget {
    count: usize,
    bytes: u64,
}

impl Budget {
    pub fn admit(&mut self, len: u64) -> bool {
        if self.count >= MAX_RESOURCES || len > MAX_RESOURCE_BYTES || self.bytes.saturating_add(len) > MAX_TOTAL_BYTES {
            return false;
        }
        self.count += 1;
        self.bytes += len;
        true
    }
    /// Whether another resource may still be asked for at all (stop fetching past the cap).
    /// The bytes still allowed under `MAX_TOTAL_BYTES`.
    pub fn left(&self) -> u64 {
        MAX_TOTAL_BYTES.saturating_sub(self.bytes)
    }

    pub fn open(&self) -> bool {
        self.count < MAX_RESOURCES && self.bytes < MAX_TOTAL_BYTES
    }
}

fn css_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)url\s*\(\s*['"]?([^'")\s]+)['"]?\s*\)"#).expect("css url regex"))
}

/// The URLs of a stylesheet's `url()` references, in order (`PSV:287-289`), bounded.
pub fn css_urls(css: &str) -> Vec<String> {
    if css.len() > MAX_CSS_BYTES {
        return Vec::new();
    }
    css_regex().captures_iter(css).take(MAX_CSS_URLS).map(|c| c[1].to_string()).collect()
}

/// `rel` resolved against `base` (RFC 3986 reference resolution, enough for http and https).
pub fn resolve_url(base: &str, rel: &str) -> Option<String> {
    let rel = rel.trim();
    let scheme_end = |s: &str| {
        let i = s.find(':')?;
        let sch = &s[..i];
        (!sch.is_empty() && sch.chars().next()?.is_ascii_alphabetic() && sch.chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))).then_some(i)
    };
    if scheme_end(rel).is_some() {
        return Some(rel.to_string());
    }
    let b = scheme_end(base)?;
    let scheme = &base[..b];
    let rest = base[b + 1..].strip_prefix("//")?;
    let (auth, tail) = rest.split_at(rest.find(['/', '?', '#']).unwrap_or(rest.len()));
    let tail = tail.split('#').next().unwrap_or("");
    let (bpath, _) = tail.split_once('?').unwrap_or((tail, ""));
    let bpath = if bpath.is_empty() { "/" } else { bpath };
    if let Some(r) = rel.strip_prefix("//") {
        return Some(format!("{scheme}://{r}"));
    }
    let (rel_nofrag, frag) = rel.split_once('#').map_or((rel, ""), |(a, b)| (a, b));
    let _ = frag;
    let (rpath, rquery) = rel_nofrag.split_once('?').map_or((rel_nofrag, None), |(a, q)| (a, Some(q)));
    let merged = if rpath.is_empty() {
        bpath.to_string()
    } else if rpath.starts_with('/') {
        rpath.to_string()
    } else {
        format!("{}{}", &bpath[..bpath.rfind('/').map_or(0, |i| i + 1)], rpath)
    };
    let mut out: Vec<&str> = Vec::new();
    let trailing = merged.ends_with("/.") || merged.ends_with("/..") || merged.ends_with('/');
    for seg in merged.split('/').skip(1) {
        match seg {
            "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    let mut path = format!("/{}", out.join("/"));
    if trailing && !path.ends_with('/') {
        path.push('/');
    }
    let query = match rquery {
        Some(q) => format!("?{q}"),
        None if rpath.is_empty() => tail.split_once('?').map_or(String::new(), |(_, q)| format!("?{q}")),
        None => String::new(),
    };
    Some(format!("{scheme}://{auth}{path}{query}"))
}

/// A saved relative path as it may sit inside `url('...')`: every ASCII character that could end
/// the quote or the function, or start an escape, is percent-encoded.
fn css_safe_path(local: &str) -> String {
    let mut out = String::new();
    for c in local.chars() {
        if c.is_ascii() && !(c.is_ascii_alphanumeric() || "-._~/".contains(c)) {
            out.push_str(&format!("%{:02X}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// Whether a CSS URL is a script or document URL that must not survive in a saved stylesheet.
fn script_url(url: &str) -> bool {
    let squashed: String = url.chars().filter(|c| !c.is_whitespace() && !c.is_control()).collect::<String>().to_ascii_lowercase();
    if squashed.contains('\\') {
        return true;
    }
    squashed.starts_with("javascript:")
        || squashed.starts_with("vbscript:")
        || squashed.starts_with("livescript:")
        || ["data:text/html", "data:application/xhtml", "data:text/javascript", "data:application/javascript", "data:application/x-javascript", "data:image/svg"]
            .iter()
            .any(|p| squashed.starts_with(p))
}

/// The stylesheet with each `url()` whose resolved URL `local` answers rewritten to the saved
/// relative name (`PSV:283-313`); a URL `local` refuses is left unchanged, except that script and
/// document URLs (javascript:, vbscript:, data: of an executable type, CSS-escaped text) become
/// `url('about:blank')`. Bounded by `MAX_CSS_BYTES` and `MAX_CSS_URLS`.
pub fn rewrite_css(css: &str, base: &str, local: &dyn Fn(&str) -> Option<String>) -> String {
    if css.len() > MAX_CSS_BYTES {
        return css.to_string();
    }
    let mut seen = 0usize;
    // The output never passes MAX_CSS_BYTES: an inlined value that would push it past keeps the
    // original reference (the inlined-stylesheet quirk could otherwise multiply a small sheet
    // thousands of times per level, Task 9 review I2).
    let mut out_len = css.len();
    css_regex()
        .replace_all(css, |caps: &Captures| {
            seen += 1;
            let whole = caps[0].to_string();
            if seen > MAX_CSS_URLS {
                return whole;
            }
            let url = &caps[1];
            let new = if script_url(url) {
                "url('about:blank')".to_string()
            } else {
                match resolve_url(base, url).and_then(|abs| local(&abs)) {
                    Some(l) => format!("url('{}')", css_safe_path(&l)),
                    None => return whole,
                }
            };
            let grown = out_len - whole.len() + new.len();
            if grown > MAX_CSS_BYTES && new.len() > whole.len() {
                return whole;
            }
            out_len = grown;
            new
        })
        .into_owned()
}

// ---- the save job (Task 9, PSV:50-232) ----
use crate::browser_reader::expression_body;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

pub const SAVE_FAILED: &str = "Save Failed";
pub const FAILED_TO_GET_HTML: &str = "Failed to get page HTML content";
pub const FAILED_TO_CREATE_DIRECTORY: &str = "Failed to create save directory";
pub const FAILED_TO_SAVE_FILE: &str = "Failed to save file";
/// The reason when the page moved to another document while a save was running.
pub const NAVIGATED_AWAY: &str = "The page navigated away";
const CANCELLED: &str = "Cancelled";
/// Resources asked for at most (answered or not): the page names the list, so asking is bounded as
/// well as keeping (review B5).
pub const MAX_ATTEMPTS: usize = 4 * MAX_RESOURCES;
/// The longest URL the job asks for, and the most URLs it queues from the page's list.
const MAX_URL_LEN: usize = 8192;
const MAX_QUEUE: usize = 20_000;

/// The clone-and-apply script (`PSV:124-171`, verbatim): an IIFE expression run in the page world.
pub const CLONE_EXPR: &str = r####"(function() {
    // Clone the entire document
    const clonedDoc = document.cloneNode(true);
    
    // Apply saved src attributes in the clone
    clonedDoc.querySelectorAll('[data-saved-src]').forEach(el => {
        el.setAttribute('src', el.getAttribute('data-saved-src'));
        el.removeAttribute('data-saved-src');
    });
    
    // Replace CSS link tags with style tags in the clone
    clonedDoc.querySelectorAll('link[rel="stylesheet"][data-saved-css-content]').forEach(link => {
        const cssContent = link.getAttribute('data-saved-css-content');
        const styleEl = clonedDoc.createElement('style');
        styleEl.textContent = cssContent;
        link.parentNode.replaceChild(styleEl, link);
    });
    
    // Apply saved href attributes for non-CSS links in the clone
    clonedDoc.querySelectorAll('[data-saved-href]').forEach(el => {
        el.setAttribute('href', el.getAttribute('data-saved-href'));
        el.removeAttribute('data-saved-href');
    });
    
    // Apply saved data attributes in the clone
    clonedDoc.querySelectorAll('[data-saved-data]').forEach(el => {
        el.setAttribute('data', el.getAttribute('data-saved-data'));
        el.removeAttribute('data-saved-data');
    });
    
    // Apply saved styles in the clone
    clonedDoc.querySelectorAll('[data-saved-style]').forEach(el => {
        el.setAttribute('style', el.getAttribute('data-saved-style'));
        el.removeAttribute('data-saved-style');
    });
    
    // Apply saved content to style tags in the clone
    clonedDoc.querySelectorAll('style[data-saved-content]').forEach(styleEl => {
        styleEl.textContent = styleEl.getAttribute('data-saved-content');
        styleEl.removeAttribute('data-saved-content');
    });
    
    // Get the full document including DOCTYPE from the clone
    const doctype = document.doctype ? 
        '<!DOCTYPE ' + document.doctype.name + '>' : '';
    return doctype + clonedDoc.documentElement.outerHTML;
})();"####;

const PLACEHOLDER: &str = "new Map(URL_MAPPING_PLACEHOLDER)";

/// "Could not save the page: <e>" (`PSV:386-387`).
pub fn save_failed_text(e: &str) -> String {
    format!("Could not save the page: {e}")
}

/// The URL extraction script as a function body (`PSV:80-98`); the file begins with a `//` comment.
pub fn extract_body() -> String {
    expression_body(crate::browser_js::resource("extract-resources.js").unwrap_or_default())
}

/// The mapping as a JavaScript array literal (`PSV:104-106`): every string JSON-encoded, U+2028 and
/// U+2029 written as escapes (JSON allows them raw, JavaScript source did not before ES2019 and
/// they must never reach the page as line terminators).
pub fn mapping_json(mapping: &[(String, String)]) -> String {
    let enc = |s: &str| serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()).replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029");
    let pairs: Vec<String> = mapping.iter().map(|(o, l)| format!("[{}, {}]", enc(o), enc(l))).collect();
    format!("[{}]", pairs.join(", "))
}

/// `Some(body)` when the copied file holds `new Map(URL_MAPPING_PLACEHOLDER)` exactly once.
fn try_saved_attributes(mapping: &[(String, String)]) -> Option<String> {
    let text = crate::browser_js::resource("add-saved-attributes.js")?;
    if text.matches(PLACEHOLDER).count() != 1 {
        return None;
    }
    Some(expression_body(&text.replace(PLACEHOLDER, &format!("new Map({})", mapping_json(mapping)))))
}

/// The saved-attribute script as a function body (`PSV:100-120`). Only the placeholder inside
/// `new Map(...)` is replaced. A copied file that lost it panics in debug builds; the job itself
/// fails with `FAILED_TO_GET_HTML` in release.
pub fn saved_attributes_body(mapping: &[(String, String)]) -> String {
    let b = try_saved_attributes(mapping);
    debug_assert!(b.is_some(), "add-saved-attributes.js lost new Map(URL_MAPPING_PLACEHOLDER)");
    b.unwrap_or_default()
}

pub fn clone_body() -> String {
    expression_body(CLONE_EXPR)
}

/// One thing for the app to do, in order. The job touches neither the file system nor the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Run `extract_body()` in the page world and answer `on_urls`.
    Extract,
    /// Read this URL from the page's loaded resources (`App::devtools_read`), answer `on_resource`.
    Fetch(String),
    /// Run this body in the page world, answer `on_attributes_added`. The app runs
    /// `Job::attributes_body()` at that moment, which is current if a write failed since.
    AddAttributes(String),
    /// Run `clone_body()` in the page world, answer `on_html`.
    GetHtml,
    /// Write a NEW file through `inert_files::write_checked` (`index.html` through `write_marked`),
    /// create-new: an existing file is an error, never overwritten. On an error call
    /// `on_write_failed(path)` and carry out what it answers.
    Write { path: PathBuf, bytes: Vec<u8> },
    /// `localfs::create_dir` (no intermediates; an existing folder is an error); on an error call
    /// `on_make_dir_failed(path)` and carry out what it answers.
    MakeDir(PathBuf),
    /// Delete a file this job wrote (an abort or failure cleaning up its partial output).
    Remove(PathBuf),
    /// Delete an empty folder this job made (never recursive).
    RemoveDir(PathBuf),
    Done(PathBuf),
    Failed(String),
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Phase {
    Idle,
    Extracting,
    Fetching,
    Attributes,
    Html,
    Finished,
}

enum Kind {
    File(PathBuf, String),
    Css(String),
}

struct CssFrame {
    url: String,
    text: String,
    refs: VecDeque<String>,
}

pub struct Job {
    folder: PathBuf,
    resources: PathBuf,
    phase: Phase,
    queue: VecDeque<String>,
    seen: HashSet<String>,
    stack: Vec<CssFrame>,
    current: Option<String>,
    entries: Vec<(String, Kind)>,
    issued: HashSet<String>,
    written: Vec<PathBuf>,
    made: Vec<PathBuf>,
    budget: Budget,
    attempts: usize,
    epoch: Option<u64>,
    /// An abort or a reported failure: nothing more is accepted.
    dead: bool,
    /// The files and folders are gone (or never made).
    cleaned: bool,
    /// `Done` was sent.
    done: bool,
}

/// A name as NTFS compares it: upper case (NTFS folds through its upcase table, so `\u{3c2}` and
/// `\u{3c3}`, or the micro sign and mu, are one name; lower case keeps them apart). A full mapping
/// that folds more (`\u{df}` to `SS`) only numbers a name that was free.
fn folded(name: &str) -> String {
    name.to_uppercase()
}

fn is_web(url: &str) -> bool {
    let l = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

impl Job {
    pub fn new(folder: PathBuf) -> Job {
        let resources = folder.join("resources");
        Job {
            folder,
            resources,
            phase: Phase::Idle,
            queue: VecDeque::new(),
            seen: HashSet::new(),
            stack: Vec::new(),
            current: None,
            entries: Vec::new(),
            issued: HashSet::new(),
            written: Vec::new(),
            made: Vec::new(),
            budget: Budget::default(),
            attempts: 0,
            epoch: None,
            dead: false,
            cleaned: false,
            done: false,
        }
    }

    pub fn start(&mut self) -> Vec<Step> {
        if self.phase != Phase::Idle || self.dead {
            return Vec::new();
        }
        self.phase = Phase::Extracting;
        self.made = vec![self.folder.clone(), self.resources.clone()];
        vec![Step::MakeDir(self.folder.clone()), Step::MakeDir(self.resources.clone()), Step::Extract]
    }

    /// The app could not make this folder (`localfs::create_dir`, which fails when it exists): the
    /// save fails whole (`PSV:331-335`), and the folder is not ours, so no cleanup touches it, nor
    /// anything inside it when it is the save folder (Task 9 review I1).
    pub fn on_make_dir_failed(&mut self, path: &Path) -> Vec<Step> {
        if path == self.folder {
            self.made.clear();
        } else {
            self.made.retain(|p| p != path);
        }
        if self.dead {
            return Vec::new();
        }
        self.done = false;
        self.fail(FAILED_TO_CREATE_DIRECTORY.to_string())
    }

    pub fn on_urls(&mut self, urls: Vec<String>) -> Vec<Step> {
        if self.dead || self.phase != Phase::Extracting {
            return Vec::new();
        }
        self.queue = urls.into_iter().filter(|u| u.len() <= MAX_URL_LEN).take(MAX_QUEUE).collect();
        self.phase = Phase::Fetching;
        self.pump()
    }

    /// Whether `url` is to be asked for now; marks it asked.
    fn candidate(&mut self, url: &str) -> bool {
        if !is_web(url) || url.len() > MAX_URL_LEN || self.seen.contains(url) {
            return false;
        }
        if !self.budget.open() || self.attempts >= MAX_ATTEMPTS {
            return false;
        }
        self.seen.insert(url.to_string());
        self.attempts += 1;
        self.current = Some(url.to_string());
        true
    }

    /// The next step after an answer: the next fetch, a finished stylesheet, or the attributes.
    fn pump(&mut self) -> Vec<Step> {
        loop {
            if !self.budget.open() || self.attempts >= MAX_ATTEMPTS {
                // Past a cap: nothing more is asked for; what is open finishes (B5).
                self.queue.clear();
                for f in &mut self.stack {
                    f.refs.clear();
                }
            }
            if let Some(top) = self.stack.last_mut() {
                if let Some(r) = top.refs.pop_front() {
                    let Some(abs) = resolve_url(&top.url, &r) else { continue };
                    if self.candidate(&abs) {
                        return vec![Step::Fetch(abs)];
                    }
                    continue;
                }
                let f = self.stack.pop().expect("frame");
                self.entries.push((f.url, Kind::Css(f.text)));
                continue;
            }
            if let Some(u) = self.queue.pop_front() {
                if self.candidate(&u) {
                    return vec![Step::Fetch(u)];
                }
                continue;
            }
            self.current = None;
            return match try_saved_attributes(&self.mapping()) {
                Some(body) => {
                    self.phase = Phase::Attributes;
                    vec![Step::AddAttributes(body)]
                }
                None => self.fail(FAILED_TO_GET_HTML.to_string()),
            };
        }
    }

    /// Save Failed: nothing more is accepted, and the partial output goes (one rule with `abort`).
    fn fail(&mut self, e: String) -> Vec<Step> {
        self.dead = true;
        self.current = None;
        let mut steps = self.cleanup();
        steps.push(Step::Failed(e));
        steps
    }

    /// The mapping as it stands: files that were written, stylesheets rewritten against what was
    /// completed before them (children first). A failed write has already left it.
    fn mapping(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut known: HashMap<String, String> = HashMap::new();
        // Inlined stylesheets count against what is left of the job's total cap; a sheet over its
        // own cap or past the total keeps its live URL (Task 9 review I2).
        let mut left = self.budget.left();
        for (url, kind) in &self.entries {
            let value = match kind {
                Kind::File(_, name) => format!("resources/{name}"),
                Kind::Css(text) => {
                    let v = rewrite_css(text, url, &|abs| known.get(abs).cloned());
                    if v.len() > MAX_CSS_BYTES || v.len() as u64 > left {
                        continue;
                    }
                    left -= v.len() as u64;
                    v
                }
            };
            known.insert(url.clone(), value.clone());
            out.push((url.clone(), value));
        }
        out
    }

    /// The script body for the current mapping (what the app runs for `AddAttributes`).
    pub fn attributes_body(&self) -> String {
        try_saved_attributes(&self.mapping()).unwrap_or_default()
    }

    pub fn on_resource(&mut self, url: &str, got: Option<(Vec<u8>, Option<String>)>, taken: &dyn Fn(&str) -> bool) -> Vec<Step> {
        if self.dead || self.phase != Phase::Fetching || self.current.as_deref() != Some(url) {
            return Vec::new();
        }
        self.current = None;
        let Some((bytes, mime)) = got else { return self.pump() };
        if bytes.len() as u64 > MAX_RESOURCE_BYTES || !self.budget.admit(bytes.len() as u64) {
            return self.pump();
        }
        let is_css = mime.as_deref().map(|m| m.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/css")).unwrap_or(false);
        if is_css {
            if let Ok(text) = String::from_utf8(bytes.clone()) {
                // Nesting is bounded: a sheet at the deepest level is kept, its references are not
                // followed (Task 9 review I2).
                let refs = if self.stack.len() + 1 >= MAX_CSS_DEPTH { VecDeque::new() } else { css_urls(&text).into_iter().collect() };
                self.stack.push(CssFrame { url: url.to_string(), text, refs });
                return self.pump();
            }
        }
        let base = base_filename(url, mime.as_deref());
        let issued = &self.issued;
        let name = unique_filename(&base, &|n| issued.contains(&folded(n)) || taken(n));
        let path = self.resources.join(&name);
        // The name is the sanitizer's: one component, directly under resources, not refused.
        let direct = path.parent() == Some(self.resources.as_path()) && path.file_name().map(|n| n.to_string_lossy() == name.as_str()).unwrap_or(false);
        if !direct || name.contains(['/', '\\']) || crate::inert_files::refused_write(&path.to_string_lossy()) {
            return self.pump();
        }
        self.issued.insert(folded(&name));
        self.entries.push((url.to_string(), Kind::File(path.clone(), name)));
        self.written.push(path.clone());
        let mut steps = vec![Step::Write { path, bytes }];
        steps.extend(self.pump());
        steps
    }

    /// A write failed: a resource keeps its live URL in the page (`PSV:219-231`); `index.html`
    /// fails the save whole (`PSV:384-391`, Task 9 review I1). The failed file is not ours, so no
    /// cleanup removes it.
    pub fn on_write_failed(&mut self, path: &Path) -> Vec<Step> {
        self.entries.retain(|(_, k)| !matches!(k, Kind::File(p, _) if p == path));
        self.written.retain(|p| p != path);
        if self.dead || path != self.folder.join("index.html") {
            return Vec::new();
        }
        self.done = false;
        self.fail(FAILED_TO_SAVE_FILE.to_string())
    }

    pub fn on_attributes_added(&mut self) -> Vec<Step> {
        if self.dead || self.phase != Phase::Attributes {
            return Vec::new();
        }
        self.phase = Phase::Html;
        vec![Step::GetHtml]
    }

    pub fn on_html(&mut self, html: Option<String>) -> Vec<Step> {
        if self.dead || self.phase != Phase::Html {
            return Vec::new();
        }
        let Some(html) = html else { return self.fail(FAILED_TO_GET_HTML.to_string()) };
        self.phase = Phase::Finished;
        self.done = true;
        let path = self.folder.join("index.html");
        self.written.push(path.clone());
        vec![Step::Write { path: path.clone(), bytes: html.into_bytes() }, Step::Done(path)]
    }

    pub fn on_error(&mut self, e: String) -> Vec<Step> {
        if self.dead {
            return Vec::new();
        }
        self.done = false;
        self.fail(e)
    }

    /// The steps that remove what this job wrote and the folders it made, once.
    pub fn cleanup(&mut self) -> Vec<Step> {
        // A finished save is never removed by a stray call.
        if self.cleaned || (self.done && !self.dead) {
            return Vec::new();
        }
        self.cleaned = true;
        let mut steps: Vec<Step> = self.written.drain(..).rev().map(Step::Remove).collect();
        for d in self.made.iter().rev() {
            steps.push(Step::RemoveDir(d.clone()));
        }
        steps
    }

    fn abort(&mut self, reason: &str) -> Vec<Step> {
        if self.dead || self.done {
            return Vec::new();
        }
        self.dead = true;
        self.current = None;
        let mut steps = self.cleanup();
        steps.push(Step::Failed(reason.to_string()));
        steps
    }

    /// The person cancelled: stop, remove the partial output, say so.
    pub fn cancel(&mut self) -> Vec<Step> {
        self.abort(CANCELLED)
    }

    /// Record the pane's navigation epoch at the start; a save never spans a navigation.
    pub fn bind_epoch(&mut self, e: u64) {
        self.epoch = Some(e);
    }

    /// Called with the pane's epoch before each step is carried out: when it moved, the job
    /// aborts and cleans up, and every later answer is ignored.
    pub fn check_epoch(&mut self, now: u64) -> Vec<Step> {
        match self.epoch {
            Some(e) if e != now => self.abort(NAVIGATED_AWAY),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> usize {
        units(s)
    }

    #[test]
    fn resource_names_follow_iterm2() {
        assert_eq!(sanitize("a/b:c*?\"<>|\\d"), "a_b_c_______d");
        assert_eq!(default_name("https://www.ex.com/x"), "www.ex.com");
        assert_eq!(default_name("iterm2-about:welcome"), "page");
        assert_eq!(base_filename("https://a/img/logo.png", Some("image/png")), "logo.png");
        assert_eq!(base_filename("https://a/img/logo", Some("image/png")), "logo.png");
        assert_eq!(base_filename("https://a/", Some("text/css")), "resource.css");
        assert_eq!(base_filename("https://a/x/y/", None), "y");
        assert_eq!(base_filename("https://a/f%20g.js", None), "f g.js_");
        assert_eq!(base_filename("https://a/a:b.css", None), "a_b.css");
        assert_eq!(unique_filename("a.png", &|n| n == "a.png" || n == "a_1.png"), "a_2.png");
        assert_eq!(unique_filename("a", &|n| n == "a"), "a_1");
    }

    #[test]
    fn windows_names_are_safe() {
        for n in ["..", ".", "", ". ."] {
            assert_eq!(sanitize(n), "resource", "{n:?}");
        }
        assert_eq!(base_filename("https://a/%2e%2e", None), "resource");
        assert_eq!(base_filename("https://a/x/%2E%2E", Some("image/png")), "resource");
        assert_eq!(sanitize("a\u{1}b\u{1f}c"), "a_b_c");
        assert_eq!(sanitize("x.png. ."), "x.png");
        assert_eq!(sanitize("CON"), "_CON");
        assert_eq!(sanitize("aux.png"), "_aux.png");
        assert_eq!(sanitize("Com1.tar.gz"), "_Com1.tar.gz");
        assert_eq!(sanitize("com0.txt"), "_com0.txt");
        assert_eq!(sanitize("lpt\u{b9}.css"), "_lpt\u{b9}.css");
        assert_eq!(sanitize("nul .txt"), "_nul .txt");
        assert_eq!(sanitize("con.txt"), "_con.txt");
        for n in ["COM10.css", "console.png", "auxiliary.png"] {
            assert_eq!(sanitize(n), n);
        }
        assert_eq!(base_filename("https://a/CON", Some("text/css")), "_CON.css");
        assert_eq!(default_name("https://con/"), "_con");
        let long = sanitize(&("a".repeat(300) + ".png"));
        assert_eq!(u(&long), MAX_NAME_UNITS - 2);
        assert!(long.ends_with(".png"));
        let emoji = sanitize(&("\u{1F600}".repeat(150) + ".png"));
        assert!(u(&emoji) <= 240 && emoji.ends_with(".png"));
        assert!(emoji.strip_suffix(".png").unwrap().chars().all(|c| c == '\u{1F600}'));
        assert_eq!(u(&sanitize(&"b".repeat(300))), MAX_NAME_UNITS - 2);
        assert_eq!(sanitize(&".".repeat(250)), "resource");
        assert_eq!(sanitize(&(".".repeat(250) + "b")), ".b");
        let c = "c".repeat(240);
        let got = unique_filename(&c, &|n| n == c);
        assert_eq!(u(&got), 240);
        assert!(got.ends_with("_1"));
        let ext = "d".repeat(238) + ".h";
        let got = unique_filename(&ext, &|n| n == ext);
        assert!(u(&got) <= 240 && got.ends_with("_1.h"));
    }

    #[test]
    fn hostile_names_are_neutral() {
        // Bidi override: no hidden character reaches disk, and the name keeps no runnable tail.
        let rtl = sanitize("a\u{202e}gpj.exe");
        assert!(!rtl.chars().any(is_hidden), "{rtl:?}");
        assert!(!crate::semantic::runnable(&rtl), "{rtl:?}");
        for n in ["x.scr", "evil.hta", "a.ps1", "a.LNK", "b.url", "c.bat", "d.cmd", "e.vbs", "f.msi", "g.reg", "h.exe"] {
            let s = sanitize(n);
            assert!(!crate::semantic::runnable(&s), "{n} -> {s}");
        }
        assert_eq!(sanitize("a.html."), "a.html");
        assert_eq!(sanitize("a:b"), "a_b");
        assert_eq!(sanitize("..\\x"), ".._x");
        assert!(!sanitize("..\\x").contains(['\\', '/']));
        assert!(!crate::settings_export::aliased(&sanitize("PROGRA~1.txt")), "{}", sanitize("PROGRA~1.txt"));
        // A page-supplied runnable extension never wins over the MIME table.
        assert_eq!(base_filename("https://a/x.exe", Some("image/png")), "x.png");
        assert_eq!(base_filename("https://a/x.scr", Some("text/css")), "x.css");
        let any = base_filename("https://a/x", Some("application/x-msdownload"));
        assert!(!crate::semantic::runnable(&any), "{any}");
        let unknown = base_filename("https://a/x", Some("weird/thing"));
        assert_eq!(unknown, "x.dat");
        assert!(!crate::semantic::runnable(&base_filename("https://a/x.exe", None)));
    }

    /// A name that is a dot and an extension is that type (review I2), and a saved script is
    /// neutralized like any other high-risk type (controller ruling: no exemption at the door).
    #[test]
    fn leading_dot_names_are_neutral() {
        assert_eq!(sanitize(".scf"), ".scf_");
        assert_eq!(sanitize(".exe"), ".exe_");
        assert_eq!(sanitize(".URL"), ".URL_");
        assert_eq!(sanitize("app.js"), "app.js_");
        let scf = base_filename("https://a/.scf", Some("image/png"));
        assert_ne!(scf, ".scf");
        assert!(!crate::semantic::runnable(&scf) && !crate::inert_files::refused_on_listing(&scf), "{scf}");
        assert!(!crate::semantic::runnable(&base_filename("https://a/.exe", None)));
    }

    /// The order is cut, then neutralize, then recheck (review I1): a cut can never bring back a
    /// type the neutralize step did not see.
    #[test]
    fn a_cut_never_restores_a_dangerous_type() {
        let ok = |n: &str| {
            let s = sanitize(n);
            assert!(u(&s) <= MAX_NAME_UNITS, "{} units", u(&s));
            assert!(!crate::inert_files::high_risk(&s) && !crate::inert_files::refused_on_listing(&s), "{n:?} -> {s:?}");
            assert!(!s.ends_with(['.', ' ']) && !crate::settings_export::aliased(&s), "{s:?}");
            s
        };
        ok(&("a".repeat(236) + ".exe." + &"x".repeat(20)));
        ok(&("a".repeat(236) + ".hta" + &"b".repeat(60)));
        ok(&("a".repeat(233) + ".url." + &"y".repeat(30)));
        assert_eq!(ok("a.pdf.exe"), "a.pdf.exe_");
        assert_eq!(ok("a.exe.pdf"), "a.exe.pdf");
        let trail = ok(&("a".repeat(236) + " . " + &".".repeat(1) + &"z".repeat(20)));
        assert!(trail.chars().all(|c| c == 'a'), "{trail:?}");
        // The neutralize step still has room after the cut.
        let long = ok(&("c".repeat(300) + ".exe"));
        assert!(long.ends_with(".exe_"), "{long:?}");
        // A suffixed unique name is rechecked too.
        let base = sanitize(&("a".repeat(236) + ".exe." + &"x".repeat(20)));
        let next = unique_filename(&base, &|n| n == base);
        assert!(!crate::inert_files::high_risk(&next) && u(&next) <= MAX_NAME_UNITS, "{next:?}");
    }

    #[test]
    fn css_rewrite_follows_iterm2() {
        assert_eq!(css_urls("a{background:url( 'i.png' )} b{src:URL(\"f.woff\")} c{x:url(data:x)}"), ["i.png", "f.woff", "data:x"]);
        let local = |u: &str| (u == "https://a/css/i.png").then(|| "resources/i.png".to_string());
        assert_eq!(rewrite_css("a{b:url(i.png)} c{d:url(i.png)}", "https://a/css/s.css", &local), "a{b:url('resources/i.png')} c{d:url('resources/i.png')}");
        assert_eq!(rewrite_css("a{b:url(other.png)}", "https://a/css/s.css", &local), "a{b:url(other.png)}");
        assert_eq!(rewrite_css("a{b:url(\u{e9}.png)} \u{4e2d}{c:url(i.png)}", "https://a/css/s.css", &local), "a{b:url(\u{e9}.png)} \u{4e2d}{c:url('resources/i.png')}");
    }

    #[test]
    fn css_url_injection_is_refused() {
        let all = |_: &str| Some("resources/x');}body{background:url(javascript:alert(1)".to_string());
        let out = rewrite_css("a{b:url(i.png)}", "https://a/s.css", &all);
        assert_eq!(out, "a{b:url('resources/x%27%29%3B%7Dbody%7Bbackground%3Aurl%28javascript%3Aalert%281%29')}");
        let keep = |_: &str| None;
        for bad in ["url(javascript:alert)", "url( 'JaVaScript:x' )", "url(data:text/html,<script>)", "url(\\000006avascript:x)", "url(vbscript:x)"] {
            let out = rewrite_css(&format!("a{{b:{bad}}}"), "https://a/s.css", &keep);
            assert_eq!(out, "a{b:url('about:blank')}", "{bad}");
        }
        assert_eq!(rewrite_css("a{b:url(data:image/png;base64,AAAA)}", "https://a/s.css", &keep), "a{b:url(data:image/png;base64,AAAA)}");
        // Bounded: a hostile stylesheet of many references is read only up to the cap.
        let many = "a{b:url(i.png)}".repeat(MAX_CSS_URLS + 50);
        assert_eq!(css_urls(&many).len(), MAX_CSS_URLS);
        let local = |_: &str| Some("r/i.png".to_string());
        let out = rewrite_css(&many, "https://a/s.css", &local);
        assert_eq!(out.matches("url('r/i.png')").count(), MAX_CSS_URLS);
    }

    #[test]
    fn resolve_follows_references() {
        assert_eq!(resolve_url("https://a/css/s.css", "i.png").as_deref(), Some("https://a/css/i.png"));
        assert_eq!(resolve_url("https://a/css/s.css", "../i.png?x=1").as_deref(), Some("https://a/i.png?x=1"));
        assert_eq!(resolve_url("https://a/css/s.css", "/i.png").as_deref(), Some("https://a/i.png"));
        assert_eq!(resolve_url("https://a/css/s.css", "//b/i.png").as_deref(), Some("https://b/i.png"));
        assert_eq!(resolve_url("https://a/css/s.css", "http://b/i.png").as_deref(), Some("http://b/i.png"));
    }

    #[test]
    fn volume_is_bounded() {
        let mut b = Budget::default();
        let kept = (0..5000).filter(|_| b.admit(10)).count();
        assert_eq!(kept, MAX_RESOURCES);
        assert!(!b.open());
        let mut b = Budget::default();
        assert!(!b.admit(MAX_RESOURCE_BYTES + 1));
        let mut kept = 0;
        while b.admit(MAX_RESOURCE_BYTES) {
            kept += 1;
        }
        assert_eq!(kept as u64, MAX_TOTAL_BYTES / MAX_RESOURCE_BYTES);
    }

    // ---- Task 9: the save job ----
    use std::path::{Path, PathBuf};

    fn d() -> PathBuf {
        PathBuf::from(r"C:\save\D")
    }
    fn res(name: &str) -> PathBuf {
        d().join("resources").join(name)
    }
    fn none_taken(_: &str) -> bool {
        false
    }
    fn png() -> Vec<u8> {
        vec![0x89, b'P', b'N', b'G']
    }
    fn s(x: &str) -> String {
        x.to_string()
    }
    fn mapping_of(step: &Step) -> String {
        match step {
            Step::AddAttributes(b) => b.clone(),
            other => panic!("not AddAttributes: {other:?}"),
        }
    }

    #[test]
    fn save_job_follows_iterm2() {
        let mut j = Job::new(d());
        assert_eq!(j.start(), vec![Step::MakeDir(d()), Step::MakeDir(d().join("resources")), Step::Extract]);
        let urls = ["https://a/i.png", "https://a/s.css", "data:x", "ftp://a/f", "https://a/i.png"].map(s).to_vec();
        assert_eq!(j.on_urls(urls), vec![Step::Fetch(s("https://a/i.png"))]);
        assert_eq!(
            j.on_resource("https://a/i.png", Some((png(), Some(s("image/png")))), &none_taken),
            vec![Step::Write { path: res("i.png"), bytes: png() }, Step::Fetch(s("https://a/s.css"))]
        );
        assert_eq!(
            j.on_resource("https://a/s.css", Some((b"b{x:url(bg.gif)}".to_vec(), Some(s("text/css")))), &none_taken),
            vec![Step::Fetch(s("https://a/bg.gif"))]
        );
        let steps = j.on_resource("https://a/bg.gif", Some((b"GIF".to_vec(), Some(s("image/gif")))), &none_taken);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0], Step::Write { path: res("bg.gif"), bytes: b"GIF".to_vec() });
        let body = mapping_of(&steps[1]);
        let want = mapping_json(&[
            (s("https://a/i.png"), s("resources/i.png")),
            (s("https://a/bg.gif"), s("resources/bg.gif")),
            (s("https://a/s.css"), s("b{x:url('resources/bg.gif')}")),
        ]);
        assert!(body.contains(&format!("new Map({want})")), "{body}");
        assert_eq!(j.on_attributes_added(), vec![Step::GetHtml]);
        assert_eq!(
            j.on_html(Some(s("<html></html>"))),
            vec![Step::Write { path: d().join("index.html"), bytes: b"<html></html>".to_vec() }, Step::Done(d().join("index.html"))]
        );
        let mut j = Job::new(d());
        j.start();
        j.on_urls(vec![]);
        j.on_attributes_added();
        // A failure cleans up in the same answer (the same rule as an abort).
        assert_eq!(j.on_html(None), vec![Step::RemoveDir(d().join("resources")), Step::RemoveDir(d()), Step::Failed(s(FAILED_TO_GET_HTML))]);
    }

    fn job_with(urls: &[&str]) -> (Job, Vec<Step>) {
        let mut j = Job::new(d());
        j.start();
        let first = j.on_urls(urls.iter().map(|u| s(u)).collect());
        (j, first)
    }

    #[test]
    fn save_job_edges_follow_iterm2() {
        // None skips with no mapping and goes on.
        let (mut j, _) = job_with(&["https://a/x.png", "https://a/y.png"]);
        let st = j.on_resource("https://a/x.png", None, &none_taken);
        assert_eq!(st, vec![Step::Fetch(s("https://a/y.png"))]);
        let st = j.on_resource("https://a/y.png", Some((png(), Some(s("image/png")))), &none_taken);
        let body = mapping_of(st.last().unwrap());
        assert!(body.contains("y.png") && !body.contains("x.png\", \"resources"), "{body}");
        // A failed write drops the mapping.
        let (mut j, _) = job_with(&["https://a/i.png", "https://a/j.png"]);
        j.on_resource("https://a/i.png", Some((png(), Some(s("image/png")))), &none_taken);
        j.on_write_failed(&res("i.png"));
        let st = j.on_resource("https://a/j.png", Some((png(), Some(s("image/png")))), &none_taken);
        let body = mapping_of(st.last().unwrap());
        assert!(!body.contains("resources/i.png") && body.contains("resources/j.png"), "{body}");
        assert!(!j.attributes_body().contains("resources/i.png"));
        // A CSS that references itself.
        let (mut j, _) = job_with(&["https://a/self.css"]);
        let st = j.on_resource("https://a/self.css", Some((b"a{x:url(self.css)}".to_vec(), Some(s("text/css")))), &none_taken);
        assert_eq!(st.len(), 1);
        assert!(matches!(st[0], Step::AddAttributes(_)), "{st:?}");
        // Two CSS files that reference each other.
        let (mut j, _) = job_with(&["https://a/p.css"]);
        let st = j.on_resource("https://a/p.css", Some((b"a{x:url(q.css)}".to_vec(), Some(s("text/css")))), &none_taken);
        assert_eq!(st, vec![Step::Fetch(s("https://a/q.css"))]);
        let st = j.on_resource("https://a/q.css", Some((b"b{x:url(p.css)}".to_vec(), Some(s("text/css")))), &none_taken);
        assert_eq!(st.len(), 1);
        assert!(matches!(st[0], Step::AddAttributes(_)), "{st:?}");
        // A CSS file referencing another CSS file maps that reference to the inner CSS text.
        let (mut j, _) = job_with(&["https://a/o.css"]);
        j.on_resource("https://a/o.css", Some((b"a{x:url(in.css)}".to_vec(), Some(s("text/css")))), &none_taken);
        let st = j.on_resource("https://a/in.css", Some((b"zz".to_vec(), Some(s("text/css")))), &none_taken);
        let body = mapping_of(&st[0]);
        assert!(body.contains(&mapping_json(&[(s("https://a/in.css"), s("zz")), (s("https://a/o.css"), s("a{x:url('zz')}"))])), "{body}");
        // A CSS body that is not UTF-8 is saved as a file.
        let (mut j, _) = job_with(&["https://a/b.css"]);
        let st = j.on_resource("https://a/b.css", Some((vec![0xff, 0xfe, 0xfd], Some(s("text/css")))), &none_taken);
        assert_eq!(st[0], Step::Write { path: res("b.css"), bytes: vec![0xff, 0xfe, 0xfd] });
        // Colliding names.
        let (mut j, _) = job_with(&["https://a/1/x.png", "https://a/2/x.png"]);
        let a = j.on_resource("https://a/1/x.png", Some((png(), Some(s("image/png")))), &none_taken);
        assert_eq!(a[0], Step::Write { path: res("x.png"), bytes: png() });
        let b = j.on_resource("https://a/2/x.png", Some((png(), Some(s("image/png")))), &|n: &str| n == "x.png");
        assert_eq!(b[0], Step::Write { path: res("x_1.png"), bytes: png() });
        // The job also remembers names it issued, case-insensitively, even when the disk does not.
        let (mut j, _) = job_with(&["https://a/1/x.png", "https://a/2/X.png"]);
        j.on_resource("https://a/1/x.png", Some((png(), Some(s("image/png")))), &none_taken);
        let b = j.on_resource("https://a/2/X.png", Some((png(), Some(s("image/png")))), &none_taken);
        assert_eq!(b[0], Step::Write { path: res("X_1.png"), bytes: png() });
        // Hostile names (review I3).
        let (mut j, _) = job_with(&["https://a/%2e%2e", "https://a/NUL.png"]);
        let a = j.on_resource("https://a/%2e%2e", Some((png(), None)), &none_taken);
        assert_eq!(a[0], Step::Write { path: res("resource"), bytes: png() });
        let b = j.on_resource("https://a/NUL.png", Some((png(), Some(s("image/png")))), &none_taken);
        assert_eq!(b[0], Step::Write { path: res("_NUL.png"), bytes: png() });
        // An error fails the job.
        let (mut j, _) = job_with(&[]);
        assert_eq!(j.on_error(s("e")), vec![Step::RemoveDir(d().join("resources")), Step::RemoveDir(d()), Step::Failed(s("e"))]);
    }

    #[test]
    fn save_scripts_follow_iterm2() {
        let e = extract_body();
        assert!(e.starts_with("return (\n// Recursively extract"), "{e}");
        assert!(e.ends_with("return Array.from(urls);\n})()\n)"), "{e}");
        let b = saved_attributes_body(&[(s("https://a/\"q"), s("resources/q"))]);
        assert_eq!(b.matches("new Map([[\"https://a/\\\"q\", \"resources/q\"]])").count(), 1, "{b}");
        assert!(b.contains("// URL_MAPPING_PLACEHOLDER will be replaced with actual mapping data"));
        assert_eq!(b.matches("URL_MAPPING_PLACEHOLDER").count(), 1);
        assert!(saved_attributes_body(&[]).contains("new Map([])"));
        let c = clone_body();
        assert!(c.contains("clonedDoc.querySelectorAll('link[rel=\"stylesheet\"][data-saved-css-content]')"));
        assert!(c.contains("return doctype + clonedDoc.documentElement.outerHTML;"));
        assert_eq!(SAVE_FAILED, "Save Failed");
        assert_eq!(save_failed_text("boom"), "Could not save the page: boom");
        assert_eq!(FAILED_TO_GET_HTML, "Failed to get page HTML content");
        assert_eq!(FAILED_TO_CREATE_DIRECTORY, "Failed to create save directory");
        assert_eq!(FAILED_TO_SAVE_FILE, "Failed to save file");
    }

    #[test]
    fn saver_bodies_never_return_a_comment() {
        for b in [extract_body(), saved_attributes_body(&[]), saved_attributes_body(&[(s("a"), s("b"))]), clone_body()] {
            assert!(!b.starts_with("return //"), "{b}");
            assert!(!b.contains("return // Recursively extract"));
            assert!(b.starts_with("return (\n"), "{b}");
            for bad in ["__tandemResult", "sessionSecret", "chrome.webview"] {
                assert!(!b.contains(bad), "{bad}");
            }
        }
    }

    #[test]
    fn saver_mapping_escapes_line_separators() {
        let m = [(s("https://a/s.css"), s("a{}\u{2028}alert(1)//\u{2029}b{}"))];
        assert_eq!(mapping_json(&m), r#"[["https://a/s.css", "a{}\u2028alert(1)//\u2029b{}"]]"#);
        let b = saved_attributes_body(&m);
        assert!(!b.contains('\u{2028}') && !b.contains('\u{2029}'));
        assert_eq!(b.matches("alert(1)").count(), 1);
        assert!(b.contains("new Map([[\"https://a/s.css\", \"a{}\\u2028alert(1)"));
        let hostile = [(s("\");alert(1);(\""), s("</script>"))];
        let b = saved_attributes_body(&hostile);
        assert!(b.contains("new Map([[\"\\\");alert(1);(\\\"\", \"</script>\"]])"), "{b}");
    }

    #[test]
    fn save_job_stops_at_the_caps() {
        // 5000 URLs, every one answered: at most MAX_RESOURCES are kept and fetching stops.
        let urls: Vec<String> = (0..5000).map(|i| format!("https://a/{i}.png")).collect();
        let mut j = Job::new(d());
        j.start();
        let mut steps = j.on_urls(urls);
        let (mut fetched, mut written) = (0usize, 0usize);
        while let Some(Step::Fetch(u)) = steps.iter().find(|x| matches!(x, Step::Fetch(_))).cloned() {
            fetched += 1;
            steps = j.on_resource(&u, Some((png(), Some(s("image/png")))), &none_taken);
            written += steps.iter().filter(|x| matches!(x, Step::Write { .. })).count();
        }
        assert_eq!(written, MAX_RESOURCES);
        assert!(fetched <= MAX_RESOURCES + 1, "{fetched}");
        assert!(steps.iter().any(|x| matches!(x, Step::AddAttributes(_))), "{steps:?}");
        // One resource over the per-resource cap is skipped; the total cap stops the rest.
        let (mut j, _) = job_with(&["https://a/big.png", "https://a/ok.png"]);
        let st = j.on_resource("https://a/big.png", Some((vec![0; (MAX_RESOURCE_BYTES + 1) as usize], None)), &none_taken);
        assert_eq!(st, vec![Step::Fetch(s("https://a/ok.png"))]);
        let urls: Vec<String> = (0..64).map(|i| format!("https://a/{i}.bin")).collect();
        let mut j = Job::new(d());
        j.start();
        let mut steps = j.on_urls(urls);
        let mut total = 0u64;
        while let Some(Step::Fetch(u)) = steps.iter().find(|x| matches!(x, Step::Fetch(_))).cloned() {
            steps = j.on_resource(&u, Some((vec![1; MAX_RESOURCE_BYTES as usize], None)), &none_taken);
            total += steps.iter().filter_map(|x| if let Step::Write { bytes, .. } = x { Some(bytes.len() as u64) } else { None }).sum::<u64>();
        }
        assert!(total > 0 && total <= MAX_TOTAL_BYTES, "{total}");
    }

    #[test]
    fn save_job_does_not_span_a_navigation() {
        let (mut j, _) = job_with(&["https://a/i.png", "https://a/j.png"]);
        j.bind_epoch(7);
        assert!(j.check_epoch(7).is_empty());
        j.on_resource("https://a/i.png", Some((png(), Some(s("image/png")))), &none_taken);
        let st = j.check_epoch(8);
        assert_eq!(
            st,
            vec![Step::Remove(res("i.png")), Step::RemoveDir(d().join("resources")), Step::RemoveDir(d()), Step::Failed(s(NAVIGATED_AWAY))]
        );
        // After the abort every answer is ignored: nothing more is written.
        assert!(j.on_resource("https://a/j.png", Some((png(), None)), &none_taken).is_empty());
        assert!(j.on_html(Some(s("x"))).is_empty());
        assert!(j.on_attributes_added().is_empty());
        assert!(j.check_epoch(9).is_empty());
        // A failed write is not removed (never written); a cancel mid-job cleans what exists.
        let (mut j, _) = job_with(&["https://a/i.png"]);
        j.on_resource("https://a/i.png", Some((png(), Some(s("image/png")))), &none_taken);
        j.on_write_failed(&res("i.png"));
        let st = j.cancel();
        assert_eq!(st, vec![Step::RemoveDir(d().join("resources")), Step::RemoveDir(d()), Step::Failed(s("Cancelled"))]);
        // A finished job keeps its files.
        let (mut j, _) = job_with(&[]);
        j.bind_epoch(1);
        j.on_attributes_added();
        j.on_html(Some(s("<p>")));
        assert!(j.check_epoch(2).is_empty());
        assert!(j.cancel().is_empty());
    }

    /// NTFS compares names through its upcase table: names that differ only in case, final sigma
    /// against sigma, or the micro sign against mu are one file. Each resource still gets its own.
    /// Task 9 review I1: the folder must be new and `index.html` must land, or the save fails
    /// whole; nothing is written into, or removed from, a folder the job did not make.
    #[test]
    fn a_folder_or_page_failure_fails_the_save() {
        // The folder exists (or cannot be made): Save Failed, nothing more is written or removed.
        let mut j = Job::new(d());
        j.start();
        let st = j.on_make_dir_failed(&d());
        assert_eq!(st, vec![Step::Failed(s(FAILED_TO_CREATE_DIRECTORY))]);
        assert!(j.on_urls(vec![s("https://a/x.png")]).is_empty());
        assert!(j.on_resource("https://a/x.png", Some((png(), Some(s("image/png")))), &none_taken).is_empty());
        assert!(j.cleanup().is_empty(), "nothing of the existing folder is ever removed");
        // The folder was made but not resources: the folder is removed, resources is not touched.
        let mut j = Job::new(d());
        j.start();
        let st = j.on_make_dir_failed(&d().join("resources"));
        assert_eq!(st, vec![Step::RemoveDir(d()), Step::Failed(s(FAILED_TO_CREATE_DIRECTORY))]);
        // index.html could not be written: Save Failed, not Done, and the partial output goes.
        let (mut j, _) = job_with(&["https://a/x.png"]);
        j.on_resource("https://a/x.png", Some((png(), Some(s("image/png")))), &none_taken);
        j.on_attributes_added();
        let st = j.on_html(Some(s("<html></html>")));
        let index = d().join("index.html");
        assert!(matches!(st.last(), Some(Step::Done(_))));
        let st = j.on_write_failed(&index);
        assert_eq!(st.last(), Some(&Step::Failed(s(FAILED_TO_SAVE_FILE))), "{st:?}");
        assert!(st.contains(&Step::Remove(res("x.png"))) && !st.contains(&Step::Remove(index.clone())), "{st:?}");
        assert!(st.contains(&Step::RemoveDir(d().join("resources"))) && st.contains(&Step::RemoveDir(d())), "{st:?}");
    }

    fn css_answer(text: &str) -> Option<(Vec<u8>, Option<String>)> {
        Some((text.as_bytes().to_vec(), Some(s("text/css"))))
    }

    /// Task 9 review I2: the inlined stylesheets are bounded. A sheet that references a small
    /// sheet 5000 times would grow to 5000 copies of it; each value stays under `MAX_CSS_BYTES`.
    #[test]
    fn inlined_stylesheets_are_bounded() {
        let (mut j, _) = job_with(&["https://a/b.css"]);
        let b = "x{y:url(a.css)}".repeat(MAX_CSS_URLS);
        let st = j.on_resource("https://a/b.css", css_answer(&b), &none_taken);
        assert_eq!(st, vec![Step::Fetch(s("https://a/a.css"))]);
        let a = "a".repeat(4096);
        let st = j.on_resource("https://a/a.css", css_answer(&a), &none_taken);
        assert!(matches!(st.last(), Some(Step::AddAttributes(_))), "{}", st.len());
        let body = j.attributes_body();
        assert!(body.len() <= MAX_CSS_BYTES + 64 * 1024, "{} bytes", body.len());
        assert!(j.mapping().iter().all(|(_, v)| v.len() <= MAX_CSS_BYTES));
    }

    /// Nesting is bounded: a chain of stylesheets is followed `MAX_CSS_DEPTH` deep, no further.
    #[test]
    fn stylesheet_nesting_is_bounded() {
        let (mut j, mut st) = job_with(&["https://a/c0.css"]);
        let mut fetched = 0;
        while let Some(Step::Fetch(u)) = st.first().cloned() {
            fetched += 1;
            let n: usize = u.trim_start_matches("https://a/c").trim_end_matches(".css").parse().unwrap();
            st = j.on_resource(&u, css_answer(&format!("x{{y:url(c{}.css)}}", n + 1)), &none_taken);
        }
        assert_eq!(fetched, MAX_CSS_DEPTH, "{st:?}");
    }

    #[test]
    fn case_folded_names_never_collide() {
        for pair in [["A.png", "a.png"], ["\u{3c3}.png", "\u{3c2}.png"], ["\u{b5}.png", "\u{3bc}.png"], ["\u{3a3}.PNG", "\u{3c2}.png"]] {
            let urls: Vec<String> = pair.iter().map(|n| format!("https://a/{n}")).collect();
            let (mut j, _) = job_with(&[&urls[0], &urls[1]]);
            let mut names = Vec::new();
            for u in &urls {
                for step in j.on_resource(u, Some((png(), Some(s("image/png")))), &none_taken) {
                    if let Step::Write { path, .. } = step {
                        names.push(path.file_name().unwrap().to_string_lossy().to_uppercase());
                    }
                }
            }
            assert_eq!(names.len(), 2, "{pair:?}");
            assert_ne!(names[0], names[1], "{pair:?} collide on NTFS");
        }
        // A name on disk in another case is taken as well (the app's `taken` is the disk's).
        let on_disk = |n: &str| n.to_uppercase() == "LOGO.PNG";
        assert_eq!(unique_filename("logo.png", &on_disk), "logo_1.png");
    }

    #[test]
    fn save_job_writes_only_under_the_folder() {
        // Names come from the sanitizer: no separator, no parent, never outside resources.
        let mut wrote = 0;
        for url in ["https://a/..%2f..%2fevil.png", "https://a/a%5cb%5c..%5cc.png", "https://a/x.url", "https://a/desktop.ini"] {
            let (mut j, _) = job_with(&[url]);
            let st = j.on_resource(url, Some((png(), Some(s("image/png")))), &none_taken);
            for step in st {
                if let Step::Write { path, .. } = step {
                    wrote += 1;
                    assert_eq!(path.parent(), Some(d().join("resources").as_path()), "{path:?}");
                    let name = path.file_name().unwrap().to_string_lossy().to_string();
                    assert!(!name.contains(['/', '\\']) && name != ".." && name != ".", "{name}");
                    assert!(!crate::inert_files::refused_path(&path.to_string_lossy()), "{path:?}");
                }
            }
        }
        assert!(wrote >= 2, "{wrote}");
        let _ = Path::new("");
    }
}
