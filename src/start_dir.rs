//! The initial working directory (iTerm2's `iTermInitialDirectory`, `ID:19-110`): Home, Custom
//! or Recycle, chosen per object (window, tab or split pane) when the profile is Advanced. The
//! custom text is an interpolated string. Pure: the app passes what it knows.

use crate::profile::Profile;
use std::path::{Path, PathBuf};

/// The profile keys holding a path (the custom directory and the three Advanced ones, then the
/// log and archive folders, the log filename template (an absolute name replaces the folder), the
/// custom icon, the two background image keys and a Browser profile's start URL).
pub const PATH_KEYS: [&str; 11] = [
    "Working Directory",
    "AWDS Window Directory",
    "AWDS Tab Directory",
    "AWDS Pane Directory",
    "Log Directory",
    "Archive Directory",
    "Custom Icon Path",
    "Background Image Location",
    "Background Image Folder Location",
    "Log Filename Format",
    "Initial URL",
];

/// Every key `scrub_value` acts on: the path keys and the SSH dictionary (its program and files).
pub const SCRUBBED_KEYS: [&str; 12] = [
    "Working Directory",
    "AWDS Window Directory",
    "AWDS Tab Directory",
    "AWDS Pane Directory",
    "Log Directory",
    "Archive Directory",
    "Custom Icon Path",
    "Background Image Location",
    "Background Image Folder Location",
    "Log Filename Format",
    "Initial URL",
    crate::ssh_config::KEY,
];

/// Whether a stored path (a start folder or any other path key) names a network or device path
/// after the trimming `resolve` does.
pub fn network_folder(v: &str) -> bool {
    // Whitespace, control characters and quotes around the text are not part of the path the
    // person meant: a leading space must not hide `\\host` from this filter while `resolve` trims it.
    crate::prompted::network_path(v.trim_matches(|c: char| c.is_whitespace() || c.is_control() || c == '"' || c == '\''))
}

/// Whether `v` stored under `key` is a network or device path, or for a start URL a `file:` URL
/// whose host is another machine (`file://host/share`, or `file:////host/share`; `file:///C:/x`, `file:///share/x` and `localhost` are local).
pub fn network_value(key: &str, v: &str) -> bool {
    if network_folder(v) {
        return true;
    }
    key == "Initial URL" && {
        let t = v.trim_matches(|c: char| c.is_whitespace() || c.is_control());
        let rest = match t.get(..5) {
            Some(scheme) if scheme.eq_ignore_ascii_case("file:") => &t[5..],
            _ => return false,
        };
        let host: String = rest.chars().skip_while(|c| *c == '/' || *c == '\\').take_while(|c| *c != '/' && *c != '\\').collect();
        let slashes = rest.chars().take_while(|c| *c == '/' || *c == '\\').count();
        // Exactly two slashes lead to a host; three are a local path (`file:///share/x`), four
        // or more a UNC path (`file:////host/share`).
        slashes >= 4 || (slashes == 2 && !host.is_empty() && !host.eq_ignore_ascii_case("localhost") && !(host.len() == 2 && host.ends_with(':')))
    }
}

/// Empties `v` (stored under `key`) when it is a path key naming a network or device path; a
/// network program or file in the SSH dictionary is dropped from it. True when it changed `v`.
/// The one filter every ingress other than Settings uses: Windows connects to the host (and
/// offers the person's credentials) when it opens such a path.
pub fn scrub_value(key: &str, v: &mut toml::Value) -> bool {
    if PATH_KEYS.contains(&key) && v.as_str().is_some_and(|t| network_value(key, t)) {
        *v = toml::Value::String(String::new());
        return true;
    }
    match v.as_table_mut() {
        Some(t) if key == crate::ssh_config::KEY => scrub_ssh(t),
        _ => false,
    }
}

/// The SSH dictionary's `pathToSSH` (a program) and the source of each `filesToCopy` entry.
fn scrub_ssh(t: &mut toml::Table) -> bool {
    let mut changed = false;
    if t.get("pathToSSH").and_then(toml::Value::as_str).is_some_and(network_folder) {
        t.remove("pathToSSH");
        changed = true;
    }
    if let Some(files) = t.get_mut("filesToCopy").and_then(toml::Value::as_array_mut) {
        let before = files.len();
        files.retain(|e| !e.as_array().and_then(|a| a.first()).and_then(toml::Value::as_str).is_some_and(network_folder));
        changed |= files.len() != before;
    }
    changed
}

/// `scrub_value` over every key it acts on in a profile's values. Answers the keys changed.
pub fn scrub_map(values: &mut std::collections::BTreeMap<String, toml::Value>) -> Vec<String> {
    SCRUBBED_KEYS.iter().filter(|k| values.get_mut(**k).is_some_and(|v| scrub_value(k, v))).map(|k| k.to_string()).collect()
}

/// What the new session is for: a window's first tab, a tab, or a split (`iTermObjectType`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Object {
    Window,
    #[default]
    Tab,
    Pane,
}

/// Where the session starts, before the folder is checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Home,
    Custom(String),
    Recycle,
}

/// The mode a profile gives `object` (`ID:19-62`). "Advanced" reads that object's AWDS option,
/// which the sheet offers as Home, Custom or Recycle only (`AWD:60-86`): an option of
/// "Advanced" (iTerm2 would recurse without end) or any unknown one is Home.
pub fn mode(p: &Profile, object: Object) -> Mode {
    match p.string("Custom Directory").as_str() {
        "Yes" => Mode::Custom(p.string("Working Directory")),
        "Recycle" => Mode::Recycle,
        "Advanced" => {
            let which = match object {
                Object::Window => "Window",
                Object::Tab => "Tab",
                Object::Pane => "Pane",
            };
            match p.string(&format!("AWDS {which} Option")).as_str() {
                "Yes" => Mode::Custom(p.string(&format!("AWDS {which} Directory"))),
                "Recycle" => Mode::Recycle,
                _ => Mode::Home,
            }
        }
        _ => Mode::Home,
    }
}

/// A folder worth probing: never a network or device path (a probe would connect to it), and
/// judged by the one rule (`scripts::stays_local`: mapped and SUBST drives, links to shares) BEFORE
/// `is_dir` is asked.
pub fn probe_ok(path: &Path, is_dir: &dyn Fn(&Path) -> bool) -> bool {
    probe_ok_with(path, is_dir, &crate::scripts::stays_local)
}

/// `probe_ok` with the judge given (tests record what is asked).
pub fn probe_ok_with(path: &Path, is_dir: &dyn Fn(&Path) -> bool, judge: &dyn Fn(&Path) -> bool) -> bool {
    !crate::prompted::network_path(&path.to_string_lossy()) && judge(path) && is_dir(path)
}

/// `~` or `~\rest` as a path under `home`; None for `~user` and for text with no tilde.
fn under_home(text: &str, home: &Path) -> Option<PathBuf> {
    let rest = text.strip_prefix('~')?;
    if !rest.is_empty() && !rest.starts_with(['/', '\\']) {
        return None;
    }
    let rest = rest.trim_start_matches(['/', '\\']);
    Some(if rest.is_empty() { home.to_path_buf() } else { home.join(rest.replace('/', "\\")) })
}

/// The folder a session starts in and a warning when the chosen one is unusable. `previous`
/// is the folder of the session a Recycle comes from (a drive path: it came through
/// `profile::local_path`). `expand` evaluates the custom text as an interpolated string
/// (`ID:102-109`); it returns the text unchanged when it cannot or must not (a remote pane).
///
/// Text a program could have steered (the custom text after a `\(` expanded it) is held to a
/// local drive path or `~`, with no control character, never `\\host`, `//host` or a device
/// path, and an existing folder; anything else starts in home with the banner. A UNC path the
/// person typed into the profile is theirs: used as written, never probed.
pub fn resolve(mode: &Mode, previous: Option<&Path>, home: &Path, expand: &dyn Fn(&str) -> String, is_dir: &dyn Fn(&Path) -> bool) -> (PathBuf, Option<String>) {
    resolve_with(mode, previous, home, expand, is_dir, &crate::scripts::stays_local)
}

/// `resolve` with the judge given: every folder is judged before `is_dir` is asked of it. A typed
/// path the judge refuses (a mapped drive, a link to a share) is the person's own: used as written,
/// never probed, as a typed UNC path is.
pub fn resolve_with(mode: &Mode, previous: Option<&Path>, home: &Path, expand: &dyn Fn(&str) -> String, is_dir: &dyn Fn(&Path) -> bool, judge: &dyn Fn(&Path) -> bool) -> (PathBuf, Option<String>) {
    let probe_ok = |d: &Path, is_dir: &dyn Fn(&Path) -> bool| probe_ok_with(d, is_dir, judge);
    let home_with = |warning: Option<String>| (home.to_path_buf(), warning);
    match mode {
        Mode::Home => home_with(None),
        Mode::Recycle => match previous.filter(|d| probe_ok(d, is_dir)) {
            Some(d) => (d.to_path_buf(), None),
            None => home_with(None),
        },
        Mode::Custom(text) => {
            let text = text.trim();
            if text.is_empty() {
                return home_with(None);
            }
            let result = if text.contains("\\(") { expand(text) } else { text.to_string() };
            let result = result.trim();
            let expanded = result != text;
            if result.is_empty() {
                return home_with(None);
            }
            if expanded {
                let shown = if result.chars().any(char::is_control) { text } else { result };
                let local = if crate::prompted::network_path(result) || result.chars().any(char::is_control) {
                    None
                } else {
                    under_home(result, home).or_else(|| crate::profile::local_path(result))
                };
                return match local.filter(|d| probe_ok(d, is_dir)) {
                    Some(d) => (d, None),
                    None => home_with(Some(unavailable_text(shown))),
                };
            }
            if crate::prompted::network_path(result) {
                return (PathBuf::from(result), None);
            }
            let dir = under_home(result, home).unwrap_or_else(|| PathBuf::from(result));
            if !judge(&dir) {
                return (dir, None);
            }
            if is_dir(&dir) {
                (dir, None)
            } else {
                home_with(Some(unavailable_text(result)))
            }
        }
    }
}

/// The folder a Recycle reuses: a new window takes the previously current window's focused
/// session's folder, a tab or a split the one it comes from (R3: the window's own tab list
/// is empty when the new tab starts).
pub fn previous_for(object: Object, focused_here: Option<PathBuf>, previous_window: Option<PathBuf>) -> Option<PathBuf> {
    match object {
        Object::Window => previous_window,
        Object::Tab | Object::Pane => focused_here,
    }
}

/// `PS:3623`.
pub fn unavailable_text(dir: &str) -> String {
    format!("The directory \u{201c}{dir}\u{201d} is unavailable. Started in home directory instead.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> toml::Value {
        toml::Value::String(v.into())
    }

    fn profile(pairs: &[(&str, &str)]) -> Profile {
        let mut p = Profile::default();
        for (k, v) in pairs {
            p.set(k, s(v));
        }
        p
    }

    const ALL: [Object; 3] = [Object::Window, Object::Tab, Object::Pane];

    #[test]
    fn initial_directory_follows_iterm2() {
        for o in ALL {
            assert_eq!(mode(&profile(&[("Custom Directory", "No")]), o), Mode::Home);
            assert_eq!(mode(&Profile::default(), o), Mode::Home);
            assert_eq!(mode(&profile(&[("Custom Directory", "Yes"), ("Working Directory", r"D:\w")]), o), Mode::Custom(r"D:\w".into()));
            assert_eq!(mode(&profile(&[("Custom Directory", "Recycle")]), o), Mode::Recycle);
        }
        let adv = profile(&[
            ("Custom Directory", "Advanced"),
            ("AWDS Window Option", "Yes"),
            ("AWDS Window Directory", r"D:\win"),
            ("AWDS Tab Option", "Recycle"),
            ("AWDS Pane Option", "No"),
        ]);
        assert_eq!(mode(&adv, Object::Window), Mode::Custom(r"D:\win".into()));
        assert_eq!(mode(&adv, Object::Tab), Mode::Recycle);
        assert_eq!(mode(&adv, Object::Pane), Mode::Home);
        // An AWDS option of Advanced (iTerm2 would recurse) or an unknown one is Home.
        for bad in ["Advanced", "bogus"] {
            let p = profile(&[("Custom Directory", "Advanced"), ("AWDS Tab Option", bad), ("AWDS Tab Directory", r"D:\t")]);
            assert_eq!(mode(&p, Object::Tab), Mode::Home, "{bad}");
        }
        // An unset directory with option Yes is Custom("").
        let p = profile(&[("Custom Directory", "Advanced"), ("AWDS Pane Option", "Yes")]);
        assert_eq!(mode(&p, Object::Pane), Mode::Custom(String::new()));
    }

    fn home() -> PathBuf {
        PathBuf::from(r"C:\Users\me")
    }

    /// `resolve` with the judge answering yes for every path (the tests' paths are not on this
    /// machine's disks; the judge itself is tested in `netpath` and `scripts`).
    fn resolve_t(mode: &Mode, previous: Option<&Path>, home: &Path, expand: &dyn Fn(&str) -> String, is_dir: &dyn Fn(&Path) -> bool) -> (PathBuf, Option<String>) {
        resolve_with(mode, previous, home, expand, is_dir, &|_| true)
    }

    /// Task 3 fix round 2: a folder from outside (an arrangement's Forced folder, Previous, a restart
    /// folder, a reported cwd, a typed path) is judged BEFORE `is_dir` is asked: a path the judge
    /// refuses (a mapped drive, a junction to a share, a SUBST onto one) is never stat'ed.
    #[test]
    fn start_dirs_are_judged_before_is_dir() {
        let h = home();
        let asked = std::cell::RefCell::new(Vec::new());
        let is_dir = |p: &Path| {
            asked.borrow_mut().push(p.to_path_buf());
            true
        };
        let refuse = |_: &Path| false;
        assert!(!probe_ok_with(Path::new(r"Z:\x"), &is_dir, &refuse));
        assert!(probe_ok_with(Path::new(r"D:\w"), &is_dir, &|_| true));
        assert_eq!(asked.borrow().len(), 1, "only the judged path was asked");
        asked.borrow_mut().clear();
        // Recycle, an expanded Custom value and a typed Custom path.
        assert_eq!(resolve_with(&Mode::Recycle, Some(Path::new(r"Z:\prev")), &h, &id, &is_dir, &refuse), (h.clone(), None));
        let expand = |_: &str| r"Z:\x".to_string();
        assert_eq!(resolve_with(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &is_dir, &refuse).0, h);
        assert_eq!(resolve_with(&Mode::Custom(r"Z:\typed".into()), None, &h, &id, &is_dir, &refuse), (PathBuf::from(r"Z:\typed"), None), "typed by the person: used as written, never probed");
        assert!(asked.borrow().is_empty(), "a refused folder is never asked about: {:?}", asked.borrow());
    }

    fn id(t: &str) -> String {
        t.to_string()
    }

    fn exists(p: &Path) -> bool {
        matches!(p.to_str(), Some(r"D:\w") | Some(r"D:\x") | Some(r"C:\Users\me\src") | Some(r"D:\prev"))
    }

    #[test]
    fn resolve_follows_iterm2() {
        let h = home();
        assert_eq!(resolve_t(&Mode::Custom(r"D:\w".into()), None, &h, &id, &exists), (PathBuf::from(r"D:\w"), None));
        assert_eq!(resolve_t(&Mode::Custom(r"D:\gone".into()), None, &h, &id, &exists), (h.clone(), Some(unavailable_text(r"D:\gone"))));
        assert_eq!(resolve_t(&Mode::Custom(String::new()), None, &h, &id, &exists), (h.clone(), None));
        assert_eq!(resolve_t(&Mode::Custom(r"~\src".into()), None, &h, &id, &exists), (PathBuf::from(r"C:\Users\me\src"), None));
        assert_eq!(resolve_t(&Mode::Home, None, &h, &id, &exists), (h.clone(), None));
        assert_eq!(resolve_t(&Mode::Recycle, Some(Path::new(r"D:\prev")), &h, &id, &exists), (PathBuf::from(r"D:\prev"), None));
        assert_eq!(resolve_t(&Mode::Recycle, None, &h, &id, &exists), (h.clone(), None));
        assert_eq!(resolve_t(&Mode::Recycle, Some(Path::new(r"D:\gone")), &h, &id, &exists), (h.clone(), None));
        assert_eq!(unavailable_text(r"D:\w"), "The directory \u{201c}D:\\w\u{201d} is unavailable. Started in home directory instead.");
        // The custom text is an interpolated string.
        let expand = |t: &str| t.replace(r"\(session.name)", "x");
        assert_eq!(resolve_t(&Mode::Custom(r"D:\\(session.name)".into()), None, &h, &expand, &exists), (PathBuf::from(r"D:\x"), None));
    }

    /// The real interpolation parser: `\\` is an escape that eats the second backslash, so
    /// the working Windows forms are `\\\(` (after a path separator) or `/` before `\(`.
    #[test]
    fn windows_paths_interpolate_through_the_real_parser() {
        let h = home();
        let lookup = |name: &str| (name == "session.name").then(|| "proj".to_string());
        let real = |t: &str| crate::interp::evaluate_lax(t, &lookup, &[], true).unwrap_or_else(|_| t.to_string());
        let exists = |_: &Path| true;
        assert_eq!(resolve_t(&Mode::Custom(r"D:/w/\(session.name)".into()), None, &h, &real, &exists), (PathBuf::from("D:/w/proj"), None));
        assert_eq!(resolve_t(&Mode::Custom(r"D:\w\\\(session.name)".into()), None, &h, &real, &exists), (PathBuf::from(r"D:\w\proj"), None));
        // `\\(` is an escaped backslash then literal text: not expanded, so a literal that
        // does not exist.
        let missing = |_: &Path| false;
        let (dir, warning) = resolve_t(&Mode::Custom(r"D:\w\\(session.name)".into()), None, &h, &real, &missing);
        assert_eq!(dir, h);
        assert!(warning.is_some());
    }

    #[test]
    fn a_unc_path_is_never_probed() {
        let h = home();
        let never = |p: &Path| -> bool { panic!("probed {p:?}") };
        // A UNC path the person typed is theirs: used as written.
        assert_eq!(resolve_t(&Mode::Custom(r"\\srv\share".into()), None, &h, &id, &never), (PathBuf::from(r"\\srv\share"), None));
        // A previous UNC folder never reaches a probe and starts in home.
        assert_eq!(resolve_t(&Mode::Recycle, Some(Path::new(r"\\srv\s\d")), &h, &id, &never), (h.clone(), None));
    }

    #[test]
    fn literal_unc_directory_is_kept() {
        let h = home();
        let never = |p: &Path| -> bool { panic!("probed {p:?}") };
        for t in [r"\\srv\share", "//srv/share"] {
            let (d, w) = resolve_t(&Mode::Custom(t.into()), None, &h, &id, &never);
            assert_eq!((d, w), (PathBuf::from(t), None));
        }
    }

    #[test]
    fn expanded_directory_refuses_unc() {
        let h = home();
        let always = |_: &Path| true;
        // The variable a remote program set names a host, a device, or a control character.
        for hostile in [r"\\evil\share", "//evil/share", r"\\?\C:\x", r"\\.\pipe\x", r"/\evil\s", "D:\\w\u{1b}[31m", "D:\\w\nx", r"\??\C:\x"] {
            let expand = |_: &str| hostile.to_string();
            let (d, w) = resolve_t(&Mode::Custom(r"\(session.path)".into()), None, &h, &expand, &always);
            assert_eq!(d, h, "{hostile:?}");
            assert!(w.is_some(), "{hostile:?} shows the banner");
            assert!(!w.unwrap().chars().any(char::is_control), "the banner carries no control character");
        }
        // Not a drive path or ~: relative and POSIX results are refused too.
        for odd in ["src", "/usr/bin", "~root"] {
            let expand = |_: &str| odd.to_string();
            assert_eq!(resolve_t(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &always).0, h, "{odd}");
        }
        // A good local result still works, and ~ expands.
        let expand = |_: &str| r"D:\x".to_string();
        assert_eq!(resolve_t(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &exists), (PathBuf::from(r"D:\x"), None));
        let expand = |_: &str| r"~\src".to_string();
        assert_eq!(resolve_t(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &exists), (PathBuf::from(r"C:\Users\me\src"), None));
        // An expanded folder that is missing falls to home with the banner.
        let expand = |_: &str| r"D:\gone".to_string();
        assert_eq!(resolve_t(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &exists), (h.clone(), Some(unavailable_text(r"D:\gone"))));
    }

    #[test]
    fn expanded_directory_from_a_remote_pane_is_not_expanded() {
        // The app passes the text itself when the focused pane is remote or absent: a `\(`
        // left in it is no folder, so it starts in home.
        let h = home();
        let (d, w) = resolve_t(&Mode::Custom(r"D:\\(session.name)".into()), None, &h, &id, &exists);
        assert_eq!(d, h);
        assert!(w.is_some());
        let (d, _) = resolve_t(&Mode::Custom(r"\\(session.name)".into()), None, &h, &id, &|_| panic!("a UNC-looking text is never probed"));
        assert_eq!(d, PathBuf::from(r"\\(session.name)"));
    }

    #[test]
    fn window_recycle_uses_the_previous_window() {
        let here = Some(PathBuf::from(r"D:\here"));
        let before = Some(PathBuf::from(r"D:\before"));
        assert_eq!(previous_for(Object::Window, here.clone(), before.clone()), before);
        assert_eq!(previous_for(Object::Tab, here.clone(), before.clone()), here);
        assert_eq!(previous_for(Object::Pane, here.clone(), before), here);
        assert_eq!(previous_for(Object::Window, here, None), None);
    }

    /// The filter reads a folder as `resolve` does: whitespace and quotes around it do not hide a network path.
    #[test]
    fn network_folder_follows_resolve_normalization() {
        for p in [" \\\\evil\\share", "\t\\\\evil\\share", "\n//evil/share", "\"\\\\evil\\share\"", "'//evil/share'", "  \\\\?\\UNC\\evil\\s", " \\\\.\\pipe\\x", "\\??\\UNC\\evil\\s", "/\\evil\\s", "\\\\evil\\share\t "] {
            assert!(network_folder(p), "{p:?}");
        }
        for p in ["D:\\ok", " D:\\ok ", "~\\src", "", "  ", "\\rooted", "relative\\x", "\"D:\\x y\"", "D:/a//b"] {
            assert!(!network_folder(p), "{p:?}");
        }
        // The two agree: every form the filter refuses resolves, were it not filtered, as a UNC the person typed.
        let mut v = toml::Value::String(" \\\\evil\\share".into());
        assert!(scrub_value("Working Directory", &mut v));
        assert_eq!(v.as_str(), Some(""));
        let mut other = toml::Value::String(" \\\\evil\\share".into());
        assert!(!scrub_value("Name", &mut other), "only folder keys");
        assert_eq!(other.as_str(), Some(" \\\\evil\\share"));
        let mut map: std::collections::BTreeMap<String, toml::Value> = PATH_KEYS.iter().map(|k| (k.to_string(), toml::Value::String("\t//evil/s".into()))).collect();
        map.insert("Name".into(), toml::Value::String("//keep".into()));
        assert_eq!(scrub_map(&mut map).len(), PATH_KEYS.len());
        assert!(PATH_KEYS.iter().all(|k| map[*k].as_str() == Some("")) && map["Name"].as_str() == Some("//keep"));
    }
    /// Task 4 ruling: the one network-path filter covers EVERY path-valued profile key, not only
    /// the start folders. Any string key named like a path must be listed (a new one fails here).
    #[test]
    fn path_keys_cover_every_path_valued_profile_key() {
        for k in ["Working Directory", "AWDS Window Directory", "AWDS Tab Directory", "AWDS Pane Directory", "Log Directory", "Archive Directory", "Custom Icon Path", "Background Image Location", "Background Image Folder Location"] {
            assert!(PATH_KEYS.contains(&k), "{k}");
        }
        for s in crate::schema::profile() {
            let named = ["directory", "path", "location", "folder"].iter().any(|w| s.key.to_lowercase().contains(w));
            if named && s.kind == crate::schema::Kind::String {
                assert!(PATH_KEYS.contains(&s.key.as_str()), "{} looks like a path and is not filtered", s.key);
            }
        }
    }

    /// Review of Task 6 (r1, item 4): the log filename template and a Browser profile's start URL
    /// go through the same filter, so a later reader cannot reintroduce the leak.
    #[test]
    fn log_filename_and_initial_url_are_filtered() {
        assert!(PATH_KEYS.contains(&"Log Filename Format") && PATH_KEYS.contains(&"Initial URL"));
        assert!(SCRUBBED_KEYS.contains(&"Log Filename Format") && SCRUBBED_KEYS.contains(&"Initial URL"));
        for (key, bad) in [("Log Filename Format", "\\\\evil\\share\\x.log"), ("Initial URL", "file://evil/share/x.html"), ("Initial URL", "FILE:\\\\evil\\s"), ("Initial URL", "\\\\evil\\s\\x.html")] {
            let mut v = toml::Value::String(bad.into());
            assert!(scrub_value(key, &mut v), "{key} {bad}");
            assert_eq!(v.as_str(), Some(""));
        }
        assert!(network_value("Initial URL", "file:////evil/share/x.html"), "four slashes is a UNC path");
        assert!(!network_value("Initial URL", "file:///share/x.html"), "three slashes is a local path, not a host");
        for (key, ok) in [("Log Filename Format", "\\(creationTimeString).log"), ("Initial URL", "https://example.com"), ("Initial URL", "file:///C:/x.html"), ("Initial URL", "file://localhost/C:/x.html"), ("Initial URL", "iterm2-about:welcome")] {
            let mut v = toml::Value::String(ok.into());
            assert!(!scrub_value(key, &mut v), "{key} {ok}");
        }
    }

    /// Each class of key is emptied by the filter in every network spelling, and keeps a local path.
    #[test]
    fn scrub_value_blanks_every_path_class() {
        let evil = [" \\\\evil\\share\\x", "//evil/s/x.png", "\"\\\\?\\UNC\\evil\\s\"", "\t\\??\\UNC\\evil"];
        for key in PATH_KEYS {
            for v in evil {
                let mut value = toml::Value::String(v.into());
                assert!(scrub_value(key, &mut value), "{key} {v:?}");
                assert_eq!(value.as_str(), Some(""), "{key}");
            }
            for ok in ["D:\\logs", "~\\logs", "", "relative\\x"] {
                let mut value = toml::Value::String(ok.into());
                assert!(!scrub_value(key, &mut value), "{key} {ok:?}");
                assert_eq!(value.as_str(), Some(ok));
            }
        }
        let mut map: std::collections::BTreeMap<String, toml::Value> = PATH_KEYS.iter().map(|k| (k.to_string(), toml::Value::String("\t//evil/s".into()))).collect();
        map.insert("Name".into(), toml::Value::String("//keep".into()));
        assert_eq!(scrub_map(&mut map).len(), PATH_KEYS.len());
        assert!(PATH_KEYS.iter().all(|k| map[*k].as_str() == Some("")) && map["Name"].as_str() == Some("//keep"));
    }

    /// The SSH dictionary holds two more paths: the ssh program and the files copied to the
    /// remote host. A network one is dropped, the rest of the dictionary stays.
    #[test]
    fn scrub_value_cleans_the_ssh_dict() {
        let table = |ssh: &str, files: &[(&str, &str)]| {
            let mut t = toml::Table::new();
            t.insert("pathToSSH".into(), toml::Value::String(ssh.into()));
            t.insert("sshIntegration".into(), toml::Value::Boolean(true));
            t.insert("filesToCopy".into(), toml::Value::Array(files.iter().map(|(a, b)| toml::Value::Array(vec![toml::Value::String(a.to_string()), toml::Value::String(b.to_string())])).collect()));
            toml::Value::Table(t)
        };
        let mut v = table(" \\\\evil\\s\\ssh.exe", &[("\\\\evil\\s\\key", "~"), ("D:\\a", "~"), ("//evil/x", "~/x")]);
        assert!(scrub_value(crate::ssh_config::KEY, &mut v));
        let t = v.as_table().unwrap();
        assert!(t.get("pathToSSH").is_none(), "a network ssh program is dropped");
        assert_eq!(t["sshIntegration"].as_bool(), Some(true));
        let files: Vec<&str> = t["filesToCopy"].as_array().unwrap().iter().map(|e| e.as_array().unwrap()[0].as_str().unwrap()).collect();
        assert_eq!(files, ["D:\\a"], "only the local file stays");
        let mut ok = table("C:\\Windows\\System32\\OpenSSH\\ssh.exe", &[("D:\\a", "~")]);
        assert!(!scrub_value(crate::ssh_config::KEY, &mut ok));
        assert_eq!(ok.as_table().unwrap()["pathToSSH"].as_str(), Some("C:\\Windows\\System32\\OpenSSH\\ssh.exe"));
        assert!(SCRUBBED_KEYS.contains(&crate::ssh_config::KEY));
    }
}
