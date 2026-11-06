//! The desktop entry and icon an AppImage run installs for itself. GNOME gives a window its dock
//! icon only from an installed desktop entry (Mutter offers no window icon protocol), and an
//! AppImage installs nothing. So the first run from an AppImage writes the entry and its icon
//! under the user's data folder, and a later run from a moved AppImage points the entry there.
//! An entry a package installed, or one the user wrote, is never touched.

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
}

/// The packaged entry, with Exec pointing at `appimage` and the mark added.
fn entry_text(appimage: &Path) -> String {
    let packaged = include_str!("../../../packaging/linux/dev.termurion.Termurion.desktop");
    let mut out = String::new();
    for line in packaged.lines() {
        if line.starts_with("Exec=") {
            out.push_str(&format!("Exec={} %U\n", exec_arg(&appimage.to_string_lossy())));
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
    let name = format!("{APP_ID}.desktop");
    if system_dirs.iter().any(|d| d.join("applications").join(&name).is_file()) {
        return Ok(Installed::PackageHasOne);
    }
    let entry = data_home.join("applications").join(&name);
    let want = entry_text(appimage);
    let outcome = match std::fs::read_to_string(&entry) {
        Ok(have) if !have.lines().any(|l| l == MARK) => return Ok(Installed::NotOurs),
        Ok(have) if have == want => Installed::Unchanged,
        Ok(_) => Installed::Updated,
        Err(e) if e.kind() == io::ErrorKind::NotFound => Installed::Wrote,
        Err(e) => return Err(e),
    };
    if let Some(icon) = icon {
        let to = data_home.join("icons/hicolor/256x256/apps").join(format!("{APP_ID}.png"));
        let art = std::fs::read(icon)?;
        if std::fs::read(&to).ok().as_deref() != Some(art.as_slice()) {
            write_whole(&to, &art)?;
        }
    }
    if outcome != Installed::Unchanged {
        write_whole(&entry, want.as_bytes())?;
    }
    Ok(outcome)
}

/// At startup, before any thread: takes the AppImage's path out of the environment and, when
/// there is one, installs the entry for it.
pub fn install_at_startup() {
    let Some(appimage) = std::env::var_os(APPIMAGE_VAR) else { return };
    // SAFETY: called first thing in `run`, before any thread exists.
    unsafe { std::env::remove_var(APPIMAGE_VAR) };
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    let Some(data_home) = var("XDG_DATA_HOME").map(PathBuf::from).or_else(|| var("HOME").map(|h| PathBuf::from(h).join(".local/share"))) else { return };
    let system: Vec<PathBuf> = std::env::split_paths(&var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into())).collect();
    // The AppDir's icon, beside the running binary (usr/bin/termurion).
    let icon = std::env::current_exe().ok().and_then(|e| e.parent()?.parent().map(|usr| usr.join("share/icons/hicolor/256x256/apps").join(format!("{APP_ID}.png")))).filter(|p| p.is_file());
    match install_in(&data_home, &system, Path::new(&appimage), icon.as_deref()) {
        Ok(Installed::Wrote) => crate::rlog!("installed the desktop entry for this AppImage"),
        Ok(Installed::Updated) => crate::rlog!("the desktop entry now points at this AppImage"),
        Ok(_) => {}
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
        assert!(text.lines().any(|l| l == format!("Icon={APP_ID}")) && text.lines().any(|l| l == format!("StartupWMClass={APP_ID}")), "{text}");
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
        assert!(!home.join("applications").exists(), "nothing written beside a package's entry");
        let other = s.0.join("none");
        std::fs::create_dir_all(home.join("applications")).unwrap();
        std::fs::write(home.join(format!("applications/{APP_ID}.desktop")), "[Desktop Entry]\nExec=mine\n").unwrap();
        assert_eq!(install_in(&home, std::slice::from_ref(&other), app, None).unwrap(), Installed::NotOurs);
        assert_eq!(entry(&home), "[Desktop Entry]\nExec=mine\n");
    }

    #[test]
    fn an_exec_path_is_quoted_as_the_spec_says() {
        assert_eq!(exec_arg("/a b/T.AppImage"), "\"/a b/T.AppImage\"");
        assert_eq!(exec_arg("/x$y\"z`w\\v%u"), "\"/x\\\\$y\\\\\"z\\\\`w\\\\\\\\v%%u\"");
    }
}
