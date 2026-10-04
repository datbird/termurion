//! A file on the remote host as framer.py describes it (`F:881-898`, `SE:65-198`, milestone
//! 14d): the JSON, the 2001 epoch, fetched data, and the texts of the panel's columns.
//! Pure. Everything here reads hostile input: a malformed reply is `ssh_files::GARBAGE`.

use crate::ssh_files::GARBAGE;
use crate::stamps::Parts;

/// Seconds from 1970 to 2001-01-01: framer's times count from Swift's reference date.
pub const REFERENCE_EPOCH: f64 = 978_307_200.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub r: bool,
    pub w: bool,
    pub x: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    File { size: Option<u64> },
    Folder,
    Host,
    Symlink(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFile {
    pub kind: Kind,
    pub absolute_path: String,
    pub permissions: Option<Permissions>,
    pub parent_permissions: Option<Permissions>,
    /// Seconds since 2001-01-01, as sent.
    pub ctime: Option<f64>,
    pub mtime: Option<f64>,
}

fn garbage<T>() -> Result<T, String> {
    Err(GARBAGE.to_string())
}

fn permissions(v: Option<&serde_json::Value>) -> Result<Option<Permissions>, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else { return Ok(None) };
    let Some(o) = v.as_object() else { return garbage() };
    let flag = |k: &str| match o.get(k) {
        None => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(_) => garbage(),
    };
    Ok(Some(Permissions { r: flag("r")?, w: flag("w")?, x: flag("x")? }))
}

fn time(v: Option<&serde_json::Value>) -> Result<Option<f64>, String> {
    match v {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v.as_f64().map(Some).ok_or_else(|| GARBAGE.to_string()),
    }
}

fn kind(v: Option<&serde_json::Value>) -> Result<Kind, String> {
    let Some(o) = v.and_then(|v| v.as_object()) else { return garbage() };
    if o.len() != 1 {
        return garbage();
    }
    let (key, body) = o.iter().next().unwrap();
    match key.as_str() {
        "folder" if body.is_object() => Ok(Kind::Folder),
        "host" if body.is_object() => Ok(Kind::Host),
        "file" => {
            let Some(inner) = body.get("_0").and_then(|v| v.as_object()) else { return garbage() };
            match inner.get("size") {
                None | Some(serde_json::Value::Null) => Ok(Kind::File { size: None }),
                Some(s) => s.as_u64().map(|n| Kind::File { size: Some(n) }).ok_or_else(|| GARBAGE.to_string()),
            }
        }
        "symlink" => match body.get("_0").and_then(|v| v.as_str()) {
            Some(t) => Ok(Kind::Symlink(t.to_string())),
            None => garbage(),
        },
        _ => garbage(),
    }
}

impl RemoteFile {
    fn from_value(v: &serde_json::Value) -> Result<RemoteFile, String> {
        let Some(o) = v.as_object() else { return garbage() };
        let Some(path) = o.get("absolutePath").and_then(|v| v.as_str()) else { return garbage() };
        Ok(RemoteFile {
            kind: kind(o.get("kind"))?,
            absolute_path: path.to_string(),
            permissions: permissions(o.get("permissions"))?,
            parent_permissions: permissions(o.get("parentPermissions"))?,
            ctime: time(o.get("ctime"))?,
            mtime: time(o.get("mtime"))?,
        })
    }

    /// One entry; a malformed or undecodable one (a lone surrogate escape) is `GARBAGE`.
    pub fn parse(json: &str) -> Result<RemoteFile, String> {
        match serde_json::from_str::<serde_json::Value>(json) {
            Ok(v) => RemoteFile::from_value(&v),
            Err(_) => garbage(),
        }
    }

    /// `lastPathComponent`.
    pub fn name(&self) -> String {
        let p = self.absolute_path.trim_end_matches('/');
        if p.is_empty() {
            return if self.absolute_path.starts_with('/') { "/".into() } else { String::new() };
        }
        p.rsplit('/').next().unwrap_or(p).to_string()
    }

    /// `parentAbsolutePath`: none for "/" (and for a path with no directory part).
    pub fn parent(&self) -> Option<String> {
        let p = self.absolute_path.trim_end_matches('/');
        if p.is_empty() || !p.contains('/') {
            return None;
        }
        Some(posix_parent(p))
    }

    pub fn is_hidden(&self) -> bool {
        self.name().starts_with('.')
    }
    pub fn is_folder(&self) -> bool {
        self.kind == Kind::Folder
    }
    pub fn is_regular(&self) -> bool {
        matches!(self.kind, Kind::File { .. })
    }
    pub fn size(&self) -> Option<u64> {
        match self.kind {
            Kind::File { size } => size,
            _ => None,
        }
    }
    pub fn unix_mtime(&self) -> Option<f64> {
        self.mtime.map(|t| t + REFERENCE_EPOCH)
    }
    pub fn unix_ctime(&self) -> Option<f64> {
        self.ctime.map(|t| t + REFERENCE_EPOCH)
    }
}

/// The lines of an `ls` reply joined with nothing: a JSON array of entries.
pub fn parse_list(content: &str) -> Result<Vec<RemoteFile>, String> {
    let Ok(serde_json::Value::Array(items)) = serde_json::from_str::<serde_json::Value>(content) else { return garbage() };
    items.iter().map(RemoteFile::from_value).collect()
}

/// Fetched data: the lines joined with nothing are standard base64 with padding.
pub fn decode_fetch(content: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(content).map_err(|_| GARBAGE.to_string())
}

/// `ByteCountFormatter`, file style, decimal units.
pub fn size_text(size: Option<u64>) -> String {
    let Some(n) = size else { return "--".into() };
    match n {
        0 => return "Zero bytes".into(),
        1 => return "1 byte".into(),
        _ if n < 1000 => return format!("{n} bytes"),
        _ => {}
    }
    // iTerm2 allows bytes to TB only (`allowedUnits`).
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut v = n as f64 / 1000.0;
    let mut i = 0;
    while v >= 999.5 && i + 1 < UNITS.len() {
        v /= 1000.0;
        i += 1;
    }
    let places = match i {
        0 => 0,
        1 => 1,
        _ => 2,
    };
    let scale = 10f64.powi(places);
    let r = (v * scale + 0.5).floor() / scale;
    let mut t = format!("{r:.*}", places as usize);
    if t.contains('.') {
        t = t.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    format!("{t} {}", UNITS[i])
}

/// The Kind column (`FL:567-604`).
pub fn kind_text(f: &RemoteFile) -> String {
    match &f.kind {
        Kind::Folder => return "Folder".into(),
        Kind::Host => return "Host".into(),
        Kind::Symlink(_) => return "Alias".into(),
        Kind::File { .. } => {}
    }
    // NSString.pathExtension: empty for a dotfile and for a trailing dot.
    let name = f.name();
    let ext = match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].to_ascii_lowercase(),
        _ => String::new(),
    };
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tiff" => format!("{} image", ext.to_ascii_uppercase()),
        "txt" | "md" | "rtf" => "Text document".into(),
        "pdf" => "PDF document".into(),
        "mp4" | "mov" | "avi" | "mkv" => "Video".into(),
        "mp3" | "wav" | "aac" | "flac" => "Audio".into(),
        "zip" | "tar" | "gz" | "bz2" => "Archive".into(),
        "lua" => "Lua script".into(),
        "nvim" => "Neovim config".into(),
        "sh" => "Shell script".into(),
        "m" => "Objective-C source".into(),
        _ => "Document".into(),
    }
}

/// "M/d/yy h:mm a" of a local time.
pub fn date_parts_text(p: &Parts) -> String {
    let h = match p.hour % 12 {
        0 => 12,
        h => h,
    };
    let ampm = if p.hour < 12 { "AM" } else { "PM" };
    format!("{}/{}/{:02} {}:{:02} {}", p.month, p.day, p.year % 100, h, p.minute, ampm)
}

/// 9999-12-31 23:59:59 UTC in Unix seconds.
#[cfg(windows)]
const MAX_DATE: f64 = 253_402_300_799.0;

/// A Unix time as the panel's date; none, or a time before 1970, is "--".
#[cfg(windows)]
pub fn date_text(unix: Option<f64>) -> String {
    match unix {
        // Past year 9999 is "--": stamps::local multiplies into u64 ticks, which a hostile time overflows.
        Some(t) if (0.0..=MAX_DATE).contains(&t) => date_parts_text(&crate::stamps::local(t.floor() as u64)),
        _ => "--".into(),
    }
}
#[cfg(not(windows))]
pub fn date_text(_unix: Option<f64>) -> String {
    "--".into()
}

/// `deletingLastPathComponent` for a POSIX string.
pub fn posix_parent(path: &str) -> String {
    let p = path.trim_end_matches('/');
    match p.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => match p[..i].trim_end_matches('/') {
            "" => "/".into(),
            q => q.to_string(),
        },
        None => if path.starts_with('/') { "/".into() } else { String::new() },
    }
}

/// `appendingPathComponent`: one slash between.
pub fn posix_join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    format!("{}/{}", dir.trim_end_matches('/'), name.trim_start_matches('/'))
}

/// `standardizingPath`: no ".", no "..", no empty or trailing components. "/.." is "/".
pub fn posix_standardize(path: &str) -> String {
    let abs = path.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if matches!(out.last(), Some(l) if *l != "..") {
                    out.pop();
                } else if !abs {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    let joined = out.join("/");
    if abs { format!("/{joined}") } else { joined }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, kind: Kind) -> RemoteFile {
        RemoteFile { kind, absolute_path: path.into(), permissions: None, parent_permissions: None, ctime: None, mtime: None }
    }

    #[test]
    fn remote_file_json_follows_iterm2() {
        let f = RemoteFile::parse(r#"{"absolutePath": "/h/a.txt", "kind": {"file": {"_0": {"size": 5}}}, "permissions": {"r": true, "w": true, "x": false}, "parentPermissions": {"r": true, "w": true, "x": true}, "ctime": 700000000, "mtime": 700000000.5}"#).unwrap();
        assert_eq!(f.kind, Kind::File { size: Some(5) });
        assert_eq!(f.absolute_path, "/h/a.txt");
        assert_eq!(f.permissions, Some(Permissions { r: true, w: true, x: false }));
        assert_eq!(f.parent_permissions, Some(Permissions { r: true, w: true, x: true }));
        assert_eq!(f.mtime, Some(700000000.5));
        assert_eq!(f.unix_mtime(), Some(1678307200.5));
        assert_eq!(f.unix_ctime(), Some(1678307200.0));
        assert_eq!(f.size(), Some(5));
        let d = RemoteFile::parse(r#"{"absolutePath":"/h/d","kind":{"folder":{}}}"#).unwrap();
        assert_eq!((d.kind.clone(), d.permissions, d.mtime, d.ctime), (Kind::Folder, None, None, None));
        assert_eq!(RemoteFile::parse(r#"{"absolutePath":"/h/l","kind":{"symlink":{"_0":"/etc"}}}"#).unwrap().kind, Kind::Symlink("/etc".into()));
        assert_eq!(RemoteFile::parse(r#"{"absolutePath":"x","kind":{"host":{}}}"#).unwrap().kind, Kind::Host);
        assert_eq!(RemoteFile::parse(r#"{"absolutePath":"/a","kind":{"file":{"_0":{}}}}"#).unwrap().kind, Kind::File { size: None });
        assert!(RemoteFile::parse(r#"{"kind":{"folder":{}}}"#).is_err());
        assert!(RemoteFile::parse(r#"{"absolutePath":"/a","kind":{"pipe":{}}}"#).is_err());
        assert!(RemoteFile::parse(r#"{"absolutePath":"/a"}"#).is_err());
        assert!(RemoteFile::parse(r#"{"absolutePath":"/a","kind":{"folder":{},"host":{}}}"#).is_err());
        assert!(RemoteFile::parse(r#"{"absolutePath":"/a","kind":{"file":{"_0":{"size":-1}}}}"#).is_err());
        assert!(RemoteFile::parse(r#"{"absolutePath":"/a","kind":{"symlink":{"_0":5}}}"#).is_err());
        assert!(RemoteFile::parse(r#"{"absolutePath":"/a","kind":{"folder":{}},"mtime":"x"}"#).is_err());
        assert!(RemoteFile::parse("not json").is_err());
        assert!(RemoteFile::parse("[]").is_err());
        let extra = RemoteFile::parse(r#"{"absolutePath":"/a","kind":{"folder":{}},"owner":"root","extra":[1,2]}"#).unwrap();
        assert_eq!(extra.absolute_path, "/a");

        let a = r#"{"absolutePath":"/h/a","kind":{"folder":{}}}"#;
        let b = r#"{"absolutePath":"/h/b","kind":{"file":{"_0":{"size":1}}}}"#;
        let both = parse_list(&format!("[{a},{b}]")).unwrap();
        assert_eq!(both.iter().map(|f| f.absolute_path.as_str()).collect::<Vec<_>>(), ["/h/a", "/h/b"]);
        assert_eq!(parse_list("[]").unwrap(), vec![]);
        assert!(parse_list("[").is_err());
        assert!(parse_list(&format!("[{a},")).is_err());
        assert!(parse_list("{}").is_err());
        assert!(parse_list(&format!("[{a},5]")).is_err());

        // framer decodes UTF-8 names as latin1; the characters arrive as they are
        let l1 = RemoteFile::parse(r#"{"absolutePath":"/h/Ã©","kind":{"folder":{}}}"#).unwrap();
        assert_eq!(l1.absolute_path, "/h/\u{c3}\u{a9}");
        // a name framer could not decode is a lone surrogate escape
        let bad = r#"{"absolutePath":"/h/\udce9","kind":{"folder":{}}}"#;
        assert_eq!(RemoteFile::parse(bad), Err(GARBAGE.to_string()));
        assert_eq!(parse_list(&format!("[{a},{bad}]")), Err(GARBAGE.to_string()));
    }

    #[test]
    fn paths_follow_iterm2() {
        let f = |p: &str| file(p, Kind::Folder);
        assert_eq!(f("/h/a.txt").name(), "a.txt");
        assert_eq!(f("/").name(), "/");
        assert_eq!(f("/h/a.txt").parent(), Some("/h".to_string()));
        assert_eq!(f("/h").parent(), Some("/".to_string()));
        assert_eq!(f("/").parent(), None);
        assert!(f("/h/.x").is_hidden());
        assert!(!f("/h/x").is_hidden());
        assert!(f("/h").is_folder() && !f("/h").is_regular());
        assert!(file("/h", Kind::File { size: None }).is_regular());
        assert_eq!(posix_parent("/a/b"), "/a");
        assert_eq!(posix_parent("/a"), "/");
        assert_eq!(posix_parent("/"), "/");
        assert_eq!(posix_parent("a"), "");
        assert_eq!(posix_join("/a", "b"), "/a/b");
        assert_eq!(posix_join("/a/", "b"), "/a/b");
        assert_eq!(posix_join("/", "b"), "/b");
        assert_eq!(posix_standardize("/a/./b/../c//d/"), "/a/c/d");
        assert_eq!(posix_standardize("/.."), "/");
        assert_eq!(posix_standardize("/"), "/");
    }

    #[test]
    fn fetch_decode_follows_iterm2() {
        assert_eq!(decode_fetch("AAEC"), Ok(vec![0, 1, 2]));
        assert_eq!(decode_fetch(""), Ok(vec![]));
        assert_eq!(decode_fetch("@@"), Err(GARBAGE.to_string()));
        assert!(decode_fetch("AAE").is_err());
        assert!(decode_fetch("AA EC").is_err());
        assert_eq!(decode_fetch("AAE="), Ok(vec![0, 1]));
    }

    #[test]
    fn listing_texts_follow_iterm2() {
        assert_eq!(size_text(None), "--");
        assert_eq!(size_text(Some(0)), "Zero bytes");
        assert_eq!(size_text(Some(1)), "1 byte");
        assert_eq!(size_text(Some(999)), "999 bytes");
        assert_eq!(size_text(Some(1000)), "1 KB");
        assert_eq!(size_text(Some(1500)), "2 KB");
        assert_eq!(size_text(Some(1_500_000)), "1.5 MB");
        assert_eq!(size_text(Some(1_234_567_890)), "1.23 GB");
        let k = |n: &str| kind_text(&file(n, Kind::File { size: None }));
        assert_eq!(k("a.PNG"), "PNG image");
        assert_eq!(k("a.md"), "Text document");
        assert_eq!(k("a.pdf"), "PDF document");
        assert_eq!(k("a.mkv"), "Video");
        assert_eq!(k("a.flac"), "Audio");
        assert_eq!(k("a.gz"), "Archive");
        assert_eq!(k("a.lua"), "Lua script");
        assert_eq!(k("a.nvim"), "Neovim config");
        assert_eq!(k("a.sh"), "Shell script");
        assert_eq!(k("a.m"), "Objective-C source");
        assert_eq!(k("Makefile"), "Document");
        assert_eq!(k("a.rs"), "Document");
        assert_eq!(kind_text(&file("/h", Kind::Folder)), "Folder");
        assert_eq!(kind_text(&file("x", Kind::Host)), "Host");
        assert_eq!(kind_text(&file("/l", Kind::Symlink("/e".into()))), "Alias");
        assert_eq!(date_text(None), "--");
        let p = Parts { year: 2025, month: 10, day: 30, weekday: 4, hour: 22, minute: 44, second: 9 };
        assert_eq!(date_parts_text(&p), "10/30/25 10:44 PM");
        assert_eq!(date_parts_text(&Parts { hour: 0, minute: 5, month: 1, day: 2, year: 2009, ..p }), "1/2/09 12:05 AM");
        assert_eq!(date_parts_text(&Parts { hour: 12, ..p }), "10/30/25 12:44 PM");
    }

    #[cfg(windows)]
    #[test]
    fn date_text_uses_local_time() {
        let t = 1_761_878_640.0; // some moment in late 2025
        let p = crate::stamps::local(t as u64);
        assert_eq!(date_text(Some(t)), date_parts_text(&p));
        assert_eq!(date_text(Some(t + 0.9)), date_text(Some(t)));
        assert_eq!(date_text(Some(-5.0)), "--");
    }

    #[cfg(windows)]
    #[test]
    fn hostile_times_are_dashes() {
        // A time the remote chose: stamps::local multiplies it into u64 ticks, which overflows
        // far below f64::MAX. Past year 9999 is "--".
        for t in [253_402_300_800.0, 1.0e12, 1.0e15, 1.0e300, f64::MAX, f64::INFINITY, f64::NAN] {
            assert_eq!(date_text(Some(t)), "--", "{t}");
        }
        assert_ne!(date_text(Some(253_402_300_799.0)), "--");
    }

    #[test]
    fn source_kinds_and_sizes_are_exact() {
        let k = |n: &str| kind_text(&file(n, Kind::File { size: None }));
        // Extensions iTerm2's kindDescription does not list are Document.
        for n in ["a.tif", "a.heic", "a.webp", "a.svg", "a.ico", "a.text", "a.webm", "a.m4v", "a.m4a", "a.ogg", "a.tgz", "a.xz", "a.7z", "a.bash", "a.zsh"] {
            assert_eq!(k(n), "Document", "{n}");
        }
        assert_eq!(k("a.jpg"), "JPG image");
        assert_eq!(k("a.jpeg"), "JPEG image");
        assert_eq!(k("a.tiff"), "TIFF image");
        // pathExtension is empty for a dotfile and a trailing dot.
        assert_eq!(k(".png"), "Document");
        assert_eq!(k("a."), "Document");
        assert_eq!(k("x.tar.gz"), "Archive");
        // The sizes stop at TB.
        assert_eq!(size_text(Some(1_000_000_000_000_000)), "1000 TB");
        assert_eq!(size_text(Some(u64::MAX)), "18446744.07 TB");
        assert_eq!(posix_parent("/a//b"), "/a");
        assert_eq!(posix_parent("//a"), "/");
        assert_eq!(f_parent("/a//b"), Some("/a".to_string()));
    }

    fn f_parent(p: &str) -> Option<String> {
        file(p, Kind::Folder).parent()
    }
}
