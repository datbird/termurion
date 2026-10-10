//! Save Page As, the pure part (iTerm2's `PageSaver`): file names, CSS `url()` rewriting and the
//! volume bounds. Every name and every byte comes from a web page, so the one sanitizer is built on
//! the existing owners: `inert_files::is_hidden` (hidden and bidi characters),
//! `inert_files::is_device_name` (device names), `settings_export::aliased` (the Windows alias
//! rule) and `semantic::runnable` (executable extensions). No second list is kept here.

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

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

fn trim_end_dots(s: &str) -> &str {
    s.trim_end_matches(['.', ' '])
}

/// Whether a file name ends in an extension Windows would run, open or follow, other than "js" (a
/// saved page needs its scripts; the Mark of the Web covers them). The one list is
/// `semantic::runnable`.
pub fn refused_extension(name: &str) -> bool {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    !ext.eq_ignore_ascii_case("js") && crate::semantic::runnable(name)
}

/// `COM0`..`COM9` and `LPT0`..`LPT9` too; `is_device_name` is the owner of the rest.
fn reserved_stem(name: &str) -> bool {
    if is_device_name(name) {
        return true;
    }
    let stem = name.split('.').next().unwrap_or("").trim_end_matches(' ').to_ascii_uppercase();
    stem == "COM0" || stem == "LPT0"
}

/// Cuts a name to `MAX_NAME_UNITS`, keeping a short extension, never splitting a surrogate pair.
fn cap(name: &str) -> String {
    if units(name) <= MAX_NAME_UNITS {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if units(e) <= MAX_EXT_UNITS => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    let room = MAX_NAME_UNITS.saturating_sub(units(&ext));
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
/// `resource`; a device name gets a `_` prefix; an executable extension (`semantic::runnable`,
/// other than js) is neutralized with a trailing `_`; an 8.3 `~digit` alias becomes `_digit`;
/// then the length is capped.
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
    if reserved_stem(&s) {
        s.insert(0, '_');
    }
    if neutralize && refused_extension(&s) {
        s.push('_');
    }
    if crate::settings_export::aliased(&s) {
        s = s.replace('~', "_");
    }
    let s = cap(&s);
    if s.is_empty() { "resource".to_string() } else { s }
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
        if crate::semantic::runnable(&format!("x.{ext}")) && !ext.eq_ignore_ascii_case("js") {
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
        if !taken(&candidate) {
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
    css_regex()
        .replace_all(css, |caps: &Captures| {
            seen += 1;
            let whole = caps[0].to_string();
            if seen > MAX_CSS_URLS {
                return whole;
            }
            let url = &caps[1];
            if script_url(url) {
                return "url('about:blank')".to_string();
            }
            match resolve_url(base, url).and_then(|abs| local(&abs)) {
                Some(l) => format!("url('{}')", css_safe_path(&l)),
                None => whole,
            }
        })
        .into_owned()
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
        assert_eq!(base_filename("https://a/f%20g.js", None), "f g.js");
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
        for n in ["COM10.js", "console.js", "auxiliary.png"] {
            assert_eq!(sanitize(n), n);
        }
        assert_eq!(base_filename("https://a/CON", Some("text/css")), "_CON.css");
        assert_eq!(default_name("https://con/"), "_con");
        let long = sanitize(&("a".repeat(300) + ".png"));
        assert_eq!(u(&long), 240);
        assert!(long.ends_with(".png"));
        let emoji = sanitize(&("\u{1F600}".repeat(150) + ".png"));
        assert!(u(&emoji) <= 240 && emoji.ends_with(".png"));
        assert!(emoji.strip_suffix(".png").unwrap().chars().all(|c| c == '\u{1F600}'));
        assert_eq!(u(&sanitize(&"b".repeat(300))), 240);
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
        assert_eq!(sanitize("app.js"), "app.js");
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
}
