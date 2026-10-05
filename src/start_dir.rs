//! The initial working directory (iTerm2's `iTermInitialDirectory`, `ID:19-110`): Home, Custom
//! or Recycle, chosen per object (window, tab or split pane) when the profile is Advanced. The
//! custom text is an interpolated string. Pure: the app passes what it knows.

use crate::profile::Profile;
use std::path::{Path, PathBuf};

/// The profile keys holding a start folder (the custom directory and the three Advanced ones).
pub const FOLDER_KEYS: [&str; 4] = ["Working Directory", "AWDS Window Directory", "AWDS Tab Directory", "AWDS Pane Directory"];

/// Whether a stored start folder names a network or device path after the trimming `resolve`
/// does.
pub fn network_folder(v: &str) -> bool {
    // Whitespace, control characters and quotes around the text are not part of the path the
    // person meant: a leading space must not hide `\\host` from this filter while `resolve` trims it.
    crate::prompted::network_path(v.trim_matches(|c: char| c.is_whitespace() || c.is_control() || c == '"' || c == '\''))
}

/// Empties `v` (stored under `key`) when it is a start folder naming a network path. True when it did.
pub fn scrub_value(key: &str, v: &mut toml::Value) -> bool {
    if FOLDER_KEYS.contains(&key) && v.as_str().is_some_and(network_folder) {
        *v = toml::Value::String(String::new());
        return true;
    }
    false
}

/// `scrub_value` over every folder key of a profile's values. Answers the keys emptied.
pub fn scrub_map(values: &mut std::collections::BTreeMap<String, toml::Value>) -> Vec<String> {
    FOLDER_KEYS.iter().filter(|k| values.get_mut(**k).is_some_and(|v| scrub_value(k, v))).map(|k| k.to_string()).collect()
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

/// A folder worth probing: never a network or device path (a probe would connect to it).
pub fn probe_ok(path: &Path, is_dir: &dyn Fn(&Path) -> bool) -> bool {
    !crate::prompted::network_path(&path.to_string_lossy()) && is_dir(path)
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

    fn id(t: &str) -> String {
        t.to_string()
    }

    fn exists(p: &Path) -> bool {
        matches!(p.to_str(), Some(r"D:\w") | Some(r"D:\x") | Some(r"C:\Users\me\src") | Some(r"D:\prev"))
    }

    #[test]
    fn resolve_follows_iterm2() {
        let h = home();
        assert_eq!(resolve(&Mode::Custom(r"D:\w".into()), None, &h, &id, &exists), (PathBuf::from(r"D:\w"), None));
        assert_eq!(resolve(&Mode::Custom(r"D:\gone".into()), None, &h, &id, &exists), (h.clone(), Some(unavailable_text(r"D:\gone"))));
        assert_eq!(resolve(&Mode::Custom(String::new()), None, &h, &id, &exists), (h.clone(), None));
        assert_eq!(resolve(&Mode::Custom(r"~\src".into()), None, &h, &id, &exists), (PathBuf::from(r"C:\Users\me\src"), None));
        assert_eq!(resolve(&Mode::Home, None, &h, &id, &exists), (h.clone(), None));
        assert_eq!(resolve(&Mode::Recycle, Some(Path::new(r"D:\prev")), &h, &id, &exists), (PathBuf::from(r"D:\prev"), None));
        assert_eq!(resolve(&Mode::Recycle, None, &h, &id, &exists), (h.clone(), None));
        assert_eq!(resolve(&Mode::Recycle, Some(Path::new(r"D:\gone")), &h, &id, &exists), (h.clone(), None));
        assert_eq!(unavailable_text(r"D:\w"), "The directory \u{201c}D:\\w\u{201d} is unavailable. Started in home directory instead.");
        // The custom text is an interpolated string.
        let expand = |t: &str| t.replace(r"\(session.name)", "x");
        assert_eq!(resolve(&Mode::Custom(r"D:\\(session.name)".into()), None, &h, &expand, &exists), (PathBuf::from(r"D:\x"), None));
    }

    /// The real interpolation parser: `\\` is an escape that eats the second backslash, so
    /// the working Windows forms are `\\\(` (after a path separator) or `/` before `\(`.
    #[test]
    fn windows_paths_interpolate_through_the_real_parser() {
        let h = home();
        let lookup = |name: &str| (name == "session.name").then(|| "proj".to_string());
        let real = |t: &str| crate::interp::evaluate_lax(t, &lookup, &[], true).unwrap_or_else(|_| t.to_string());
        let exists = |_: &Path| true;
        assert_eq!(resolve(&Mode::Custom(r"D:/w/\(session.name)".into()), None, &h, &real, &exists), (PathBuf::from("D:/w/proj"), None));
        assert_eq!(resolve(&Mode::Custom(r"D:\w\\\(session.name)".into()), None, &h, &real, &exists), (PathBuf::from(r"D:\w\proj"), None));
        // `\\(` is an escaped backslash then literal text: not expanded, so a literal that
        // does not exist.
        let missing = |_: &Path| false;
        let (dir, warning) = resolve(&Mode::Custom(r"D:\w\\(session.name)".into()), None, &h, &real, &missing);
        assert_eq!(dir, h);
        assert!(warning.is_some());
    }

    #[test]
    fn a_unc_path_is_never_probed() {
        let h = home();
        let never = |p: &Path| -> bool { panic!("probed {p:?}") };
        // A UNC path the person typed is theirs: used as written.
        assert_eq!(resolve(&Mode::Custom(r"\\srv\share".into()), None, &h, &id, &never), (PathBuf::from(r"\\srv\share"), None));
        // A previous UNC folder never reaches a probe and starts in home.
        assert_eq!(resolve(&Mode::Recycle, Some(Path::new(r"\\srv\s\d")), &h, &id, &never), (h.clone(), None));
    }

    #[test]
    fn literal_unc_directory_is_kept() {
        let h = home();
        let never = |p: &Path| -> bool { panic!("probed {p:?}") };
        for t in [r"\\srv\share", "//srv/share"] {
            let (d, w) = resolve(&Mode::Custom(t.into()), None, &h, &id, &never);
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
            let (d, w) = resolve(&Mode::Custom(r"\(session.path)".into()), None, &h, &expand, &always);
            assert_eq!(d, h, "{hostile:?}");
            assert!(w.is_some(), "{hostile:?} shows the banner");
            assert!(!w.unwrap().chars().any(char::is_control), "the banner carries no control character");
        }
        // Not a drive path or ~: relative and POSIX results are refused too.
        for odd in ["src", "/usr/bin", "~root"] {
            let expand = |_: &str| odd.to_string();
            assert_eq!(resolve(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &always).0, h, "{odd}");
        }
        // A good local result still works, and ~ expands.
        let expand = |_: &str| r"D:\x".to_string();
        assert_eq!(resolve(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &exists), (PathBuf::from(r"D:\x"), None));
        let expand = |_: &str| r"~\src".to_string();
        assert_eq!(resolve(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &exists), (PathBuf::from(r"C:\Users\me\src"), None));
        // An expanded folder that is missing falls to home with the banner.
        let expand = |_: &str| r"D:\gone".to_string();
        assert_eq!(resolve(&Mode::Custom(r"\(x)".into()), None, &h, &expand, &exists), (h.clone(), Some(unavailable_text(r"D:\gone"))));
    }

    #[test]
    fn expanded_directory_from_a_remote_pane_is_not_expanded() {
        // The app passes the text itself when the focused pane is remote or absent: a `\(`
        // left in it is no folder, so it starts in home.
        let h = home();
        let (d, w) = resolve(&Mode::Custom(r"D:\\(session.name)".into()), None, &h, &id, &exists);
        assert_eq!(d, h);
        assert!(w.is_some());
        let (d, _) = resolve(&Mode::Custom(r"\\(session.name)".into()), None, &h, &id, &|_| panic!("a UNC-looking text is never probed"));
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
        let mut map: std::collections::BTreeMap<String, toml::Value> = FOLDER_KEYS.iter().map(|k| (k.to_string(), toml::Value::String("\t//evil/s".into()))).collect();
        map.insert("Name".into(), toml::Value::String("//keep".into()));
        assert_eq!(scrub_map(&mut map).len(), 4);
        assert!(FOLDER_KEYS.iter().all(|k| map[*k].as_str() == Some("")) && map["Name"].as_str() == Some("//keep"));
    }
}
