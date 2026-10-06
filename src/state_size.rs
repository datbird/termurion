//! `sizeInfo` over Tandem's saved state (`NSDictionary+iTerm.m:44-68`, `:287-298`, `:359-383`): the
//! size and count of every keypath, never a value (P5). Written to `statesize.*.txt` while
//! `LogRestorableStateSize` is on.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Largest file written (P5); the rest is cut with one line saying so.
pub const MAX_FILE: usize = 1024 * 1024;
const DEPTH: usize = 64;

/// A path segment: a plain key name as it is, anything else `<key>` (a table keyed by user text).
fn segment(key: &str) -> String {
    let plain = !key.is_empty() && key.chars().count() <= 64 && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ' ' | '.' | '-'));
    if plain { key.to_string() } else { "<key>".to_string() }
}

fn walk(v: &toml::Value, path: &str, depth: usize, out: &mut BTreeMap<String, (usize, usize)>) -> usize {
    let size = match v {
        toml::Value::String(s) => 2 * s.encode_utf16().count(),
        toml::Value::Table(t) if depth < DEPTH => t.iter().map(|(k, c)| walk(c, &format!("{path}.{}", segment(k)), depth + 1, out)).sum(),
        toml::Value::Array(a) if depth < DEPTH => a.iter().map(|c| walk(c, &format!("{path}[*]"), depth + 1, out)).sum(),
        _ => 16,
    };
    let e = out.entry(path.to_string()).or_default();
    e.0 += size;
    e.1 += 1;
    size
}

/// One line "size count keypath" per keypath, the root as the empty keypath, sorted by keypath
/// (Decision: iTerm2's order is unordered). Cut at `MAX_FILE`.
pub fn size_info(v: &toml::Value) -> String {
    let mut by_path = BTreeMap::new();
    walk(v, "", 0, &mut by_path);
    let mut out = String::new();
    for (path, (size, count)) in &by_path {
        let line = format!("{size} {count} {path}\n");
        if out.len() + line.len() > MAX_FILE {
            out.push_str("... truncated\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

/// `statesize.app-{pid}.txt`.
pub fn app_file(temp: &Path, pid: u32) -> PathBuf {
    temp.join(format!("statesize.app-{pid}.txt"))
}

/// `statesize.window-{pid}-{n}.txt`.
pub fn window_file(temp: &Path, pid: u32, n: usize) -> PathBuf {
    temp.join(format!("statesize.window-{pid}-{n}.txt"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// `NSDictionary+iTerm.m:44-68`, `:287-298`, `:359-383`: a string is two bytes a UTF-16 unit,
    /// any other scalar 16; a table entry adds under "<keypath>.<key>", an array element under
    /// "<keypath>[*]", and a nested table or array adds its total under its own keypath too.
    #[test]
    fn size_info_follows_iterm2() {
        let v: toml::Value = toml::from_str("a = \"xy\"\nb = [1, 2]\n[c]\nd = \"z\"\n").unwrap();
        assert_eq!(size_info(&v), "38 1 \n4 1 .a\n32 1 .b\n32 2 .b[*]\n2 1 .c\n2 1 .c.d\n");
    }

    /// P5: sizes only, never a value; a key that is not a plain name prints as `<key>`; the
    /// file is cut at one MiB with one line saying so.
    #[test]
    fn size_info_reports_sizes_only() {
        let v: toml::Value = toml::from_str("\"secret value=hunter2\" = 1\n\"ok key.1\" = \"pw\"\n").unwrap();
        let out = size_info(&v);
        assert!(!out.contains("hunter2") && !out.contains("secret"), "{out}");
        assert!(out.contains(".<key>") && out.contains(".ok key.1"), "{out}");
        let mut big = toml::Table::new();
        for i in 0..100_000 {
            big.insert(format!("k{i:06}"), toml::Value::Table(toml::Table::new()));
        }
        let out = size_info(&toml::Value::Table(big));
        assert!(out.len() <= MAX_FILE + 32 && out.ends_with("... truncated\n"), "{}", out.len());
    }

    #[test]
    fn file_names_follow_iterm2() {
        let t = Path::new("T");
        assert_eq!(app_file(t, 12), t.join("statesize.app-12.txt"));
        assert_eq!(window_file(t, 12, 3), t.join("statesize.window-12-3.txt"));
    }
}
