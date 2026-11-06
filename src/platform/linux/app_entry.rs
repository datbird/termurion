//! The desktop entry and icon an AppImage run installs for itself. GNOME gives a window its dock
//! icon only from an installed desktop entry (Mutter offers no window icon protocol), and an
//! AppImage installs nothing. So the first run from an AppImage writes the entry and its icon
//! under the user's data folder, and a later run from a moved AppImage points the entry there.
//! An entry a package installed, or one the user wrote, is never touched.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use super::super::window::APP_ID;

/// The variable AppRun hands the AppImage's path in (the runtime's own APPIMAGE is cleared there,
/// so no shell gets it); Termurion takes it out of its environment at once.
pub const APPIMAGE_VAR: &str = "TERMURION_APPIMAGE";

/// The line that marks an entry as the one this code wrote.
const MARK: &str = "X-Termurion-AppImage=true";

/// What an install did.
#[derive(Debug, PartialEq, Eq)]
pub enum Installed {
    Wrote,
    /// The AppImage moved: the entry now points at the new place.
    Updated,
    Unchanged,
    /// A package installed an entry in a system data folder: that one is the launcher.
    PackageHasOne,
    /// An entry without the mark is there (the user's own): left alone.
    NotOurs,
    /// The AppImage's path cannot go into an entry, and why.
    Refused(&'static str),
}

/// The packaged entry, with Exec pointing at `appimage`, Icon at the copied PNG by its absolute
/// path when there is one (gnome-shell reads the icon theme once, at its start, so a themed name
/// installed later is never found until it restarts), and the mark added.
fn entry_text(appimage: &Path, icon: Option<&Path>) -> String {
    let packaged = include_str!("../../../packaging/linux/dev.termurion.Termurion.desktop");
    let mut out = String::new();
    for line in packaged.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec={} %U\n", exec_arg(&appimage.to_string_lossy())));
        } else if let (true, Some(icon)) = (line.starts_with("Icon="), icon.and_then(Path::to_str).filter(|p| !p.chars().any(|c| c.is_control() || c == '\\'))) {
            out.push_str(&format!("Icon={icon}\n"));
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str(MARK);
    out.push('\n');
    out
}

/// A path as one quoted Exec argument (Desktop Entry spec): `"`, backquote, `$` and `\` take a
/// backslash inside the quotes, then the value's own escapes double each backslash, and `%` is `%%`.
fn exec_arg(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            '\\' => quoted.push_str("\\\\\\\\"),
            '%' => quoted.push_str("%%"),
            _ => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Writes `text` to `path` through a file beside it, so a reader never sees half of it.
fn write_whole(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().ok_or_else(|| io::Error::other("no folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.part", path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Installs the entry for `appimage` under `data_home` (and copies `icon` beside it), unless a
/// system data folder already has one or the one there is not ours.
pub fn install_in(data_home: &Path, system_dirs: &[PathBuf], appimage: &Path, icon: Option<&Path>) -> io::Result<Installed> {
    // A line break in the path would add lines to the entry (the last Exec wins); a path that is
    // not UTF-8 cannot be written into it at all. Either way no entry is made.
    let Some(path) = appimage.to_str() else { return Ok(Installed::Refused("the AppImage's path is not UTF-8")) };
    if path.chars().any(char::is_control) {
        return Ok(Installed::Refused("the AppImage's path has a control character"));
    }
    let name = format!("{APP_ID}.desktop");
    if system_dirs.iter().any(|d| d.join("applications").join(&name).is_file()) {
        return Ok(Installed::PackageHasOne);
    }
    let entry = data_home.join("applications").join(&name);
    let icon_to = data_home.join("icons/hicolor/256x256/apps").join(format!("{APP_ID}.png"));
    let want = entry_text(appimage, icon.map(|_| icon_to.as_path()));
    let outcome = match std::fs::read_to_string(&entry) {
        Ok(have) if !have.lines().any(|l| l == MARK) => return Ok(Installed::NotOurs),
        Ok(have) if have == want => Installed::Unchanged,
        Ok(_) => Installed::Updated,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Installed::Wrote,
        Err(e) => return Err(e),
    };
    if let Some(icon) = icon {
        let art = std::fs::read(icon)?;
        if std::fs::read(&icon_to).ok().as_deref() != Some(art.as_slice()) {
            write_whole(&icon_to, &art)?;
        }
    }
    if outcome != Installed::Unchanged {
        write_whole(&entry, want.as_bytes())?;
    }
    Ok(outcome)
}

/// The user's data folder and the system ones, as the XDG Base Directory spec reads them: a
/// relative value is ignored (the default stands), and a system folder that is the user's own
/// (some profiles list it) is left out, so the entry written there is never taken for a package's.
pub fn data_folders(home: Option<OsString>, data_home: Option<OsString>, data_dirs: Option<OsString>) -> Option<(PathBuf, Vec<PathBuf>)> {
    let absolute = |v: Option<OsString>| v.map(PathBuf::from).filter(|p| p.is_absolute());
    let user = absolute(data_home).or_else(|| absolute(home).map(|h| h.join(".local/share")))?;
    let listed = data_dirs.filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let system = std::env::split_paths(&listed).filter(|p| p.is_absolute() && *p != user).collect();
    Some((user, system))
}

/// At startup, before any thread: takes the AppImage's path out of the environment and, when
/// there is one, installs the entry for it.
pub fn install_at_startup() {
    let Some(appimage) = std::env::var_os(APPIMAGE_VAR) else { return };
    // SAFETY: called first thing in `run`, before any thread exists.
    unsafe { std::env::remove_var(APPIMAGE_VAR) };
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let Some((data_home, system)) = data_folders(var("HOME"), var("XDG_DATA_HOME"), var("XDG_DATA_DIRS")) else { return };
    // The AppDir's icon, beside the running binary (usr/bin/termurion).
    let icon = std::env::current_exe().ok().and_then(|e| e.parent()?.parent().map(|usr| usr.join("share/icons/hicolor/256x256/apps").join(format!("{APP_ID}.png")))).filter(|p| p.is_file());
    match install_in(&data_home, &system, Path::new(&appimage), icon.as_deref()) {
        Ok(Installed::Wrote) => crate::rlog!("installed the desktop entry for this AppImage"),
        Ok(Installed::Updated) => crate::rlog!("the desktop entry now points at this AppImage"),
        Ok(_) => {}
        Ok(Installed::Refused(why)) => crate::rlog!("no desktop entry for this AppImage: {}", why),
        Err(_) => crate::rlog!("could not install the desktop entry"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let d = std::env::temp_dir().join(format!("termurion-app-entry-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Scratch(d)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn entry(home: &Path) -> String {
        std::fs::read_to_string(home.join(format!("applications/{APP_ID}.desktop"))).unwrap()
    }

    #[test]
    fn the_first_run_writes_the_entry_and_its_icon_and_a_move_updates_exec() {
        let s = Scratch::new("first");
        let (home, sys) = (s.0.join("home"), s.0.join("sys"));
        let icon = s.0.join("icon.png");
        std::fs::write(&icon, b"png bytes").unwrap();
        let first = Path::new("/home/me/Apps/Termurion.AppImage");
        assert_eq!(install_in(&home, std::slice::from_ref(&sys), first, Some(&icon)).unwrap(), Installed::Wrote);
        let text = entry(&home);
        assert!(text.lines().any(|l| l == "Exec=\"/home/me/Apps/Termurion.AppImage\" %U"), "{text}");
        // The icon by its absolute path: gnome-shell finds it without a restart.
        let png = home.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png"));
        assert!(text.lines().any(|l| l == format!("Icon={}", png.display())) && text.lines().any(|l| l == format!("StartupWMClass={APP_ID}")), "{text}");
        assert_eq!(std::fs::read(home.join(format!("icons/hicolor/256x256/apps/{APP_ID}.png"))).unwrap(), b"png bytes");
        assert_eq!(install_in(&home, std::slice::from_ref(&sys), first, Some(&icon)).unwrap(), Installed::Unchanged);
        let moved = Path::new("/home/me/Downloads/Termurion.AppImage");
        assert_eq!(install_in(&home, std::slice::from_ref(&sys), moved, Some(&icon)).unwrap(), Installed::Updated);
        assert!(entry(&home).contains("Exec=\"/home/me/Downloads/Termurion.AppImage\" %U"));
    }

    #[test]
    fn a_package_entry_or_the_users_own_is_left_alone() {
        let s = Scratch::new("others");
        let (home, sys) = (s.0.join("home"), s.0.join("sys"));
        std::fs::create_dir_all(sys.join("applications")).unwrap();
        std::fs::write(sys.join(format!("applications/{APP_ID}.desktop")), "[Desktop Entry]\n").unwrap();
        let app = Path::new("/a/T.AppImage");
        assert_eq!(install_in(&home, std::slice::from_ref(&sys), app, None).unwrap(), Installed::PackageHasOne);
        // With no icon to copy the entry keeps the themed name.
        let bare = s.0.join("bare");
        assert_eq!(install_in(&bare, &[], app, None).unwrap(), Installed::Wrote);
        assert!(entry(&bare).lines().any(|l| l == format!("Icon={APP_ID}")));
        assert!(!home.join("applications").exists(), "nothing written beside a package's entry");
        let other = s.0.join("none");
        std::fs::create_dir_all(home.join("applications")).unwrap();
        std::fs::write(home.join(format!("applications/{APP_ID}.desktop")), "[Desktop Entry]\nExec=mine\n").unwrap();
        assert_eq!(install_in(&home, std::slice::from_ref(&other), app, None).unwrap(), Installed::NotOurs);
        assert_eq!(entry(&home), "[Desktop Entry]\nExec=mine\n");
    }

    /// A path with a line break (or any control character) would add lines to the entry, and
    /// the last Exec wins: no entry is written for it.
    #[test]
    fn a_path_with_a_control_character_gets_no_entry() {
        let s = Scratch::new("control");
        let home = s.0.join("home");
        for bad in ["/a/x\nExec=sh -c evil\nX=/T.AppImage", "/a/tab\there.AppImage", "/a/cr\r.AppImage"] {
            assert!(matches!(install_in(&home, &[], Path::new(bad), None), Ok(Installed::Refused(_))), "{bad:?}");
        }
        assert!(!home.join("applications").exists(), "nothing written");
    }

    #[test]
    fn data_folders_follow_the_xdg_spec() {
        let os = |s: &str| Some(OsString::from(s));
        let (user, system) = data_folders(os("/home/me"), None, None).unwrap();
        assert_eq!((user, system), (PathBuf::from("/home/me/.local/share"), vec![PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]));
        // A relative XDG_DATA_HOME is ignored, and so is a relative system folder.
        let (user, system) = data_folders(os("/home/me"), os("rel/data"), os("rel:/usr/share")).unwrap();
        assert_eq!((user, system), (PathBuf::from("/home/me/.local/share"), vec![PathBuf::from("/usr/share")]));
        // The user's own folder listed among the system ones is left out of them.
        let (user, system) = data_folders(os("/home/me"), os("/home/me/data"), os("/home/me/data:/usr/share")).unwrap();
        assert_eq!((user, system), (PathBuf::from("/home/me/data"), vec![PathBuf::from("/usr/share")]));
        assert_eq!(data_folders(None, os("rel"), None), None, "no absolute folder at all: nothing");
    }

    /// The entry written last run is ours, also when the user's folder is listed as a system one:
    /// a moved AppImage still updates it.
    #[test]
    fn a_user_folder_listed_as_a_system_one_still_gets_updates() {
        let s = Scratch::new("listed");
        let home = s.0.join("home");
        let os = |p: &Path| Some(OsString::from(p));
        let (user, system) = data_folders(None, os(&home), os(&home)).unwrap();
        assert_eq!(install_in(&user, &system, Path::new("/a/T.AppImage"), None).unwrap(), Installed::Wrote);
        assert_eq!(install_in(&user, &system, Path::new("/b/T.AppImage"), None).unwrap(), Installed::Updated);
    }

    #[test]
    fn an_exec_path_is_quoted_as_the_spec_says() {
        assert_eq!(exec_arg("/a b/T.AppImage"), "\"/a b/T.AppImage\"");
        assert_eq!(exec_arg("/x$y\"z`w\\v%u"), "\"/x\\\\$y\\\\\"z\\\\`w\\\\\\\\v%%u\"");
    }
}
