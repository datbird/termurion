//! `open` on macOS, `xdg-open` on Linux; the file manager through `open -R` or the FileManager1 D-Bus call.

pub use super::open_judge::{file_url_os_path, is_this_host, runnable, scheme_refusal};

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(target_os = "macos")]
const OPENER: &str = "open";
#[cfg(not(target_os = "macos"))]
const OPENER: &str = "xdg-open";

/// The opener with `target` as its one argument, never a shell string.
fn opener(target: &OsStr) -> Command {
    let mut cmd = Command::new(OPENER);
    cmd.arg(target);
    cmd
}

/// As `opener`, for a file: a bare name that starts with a dash gets "./" so it is not read as an option.
fn file_opener(path: &Path) -> Command {
    if path.as_os_str().as_encoded_bytes().first() == Some(&b'-') {
        return opener(Path::new(".").join(path).as_os_str());
    }
    opener(path.as_os_str())
}

/// Which failure line a waited-on child reports.
#[derive(Clone, Copy)]
enum Kind {
    Opener,
    FileManager,
}

/// Starts `cmd` with no stdio and waits for it on its own thread, so the UI thread never blocks (and no
/// zombie stays). Only a failed start is an error here; a non-zero exit is logged. A file manager that
/// fails with a `folder` to fall back on gets that folder opened with the opener instead.
fn spawn_and_wait(mut cmd: Command, kind: Kind, folder: Option<PathBuf>) -> Result<(), String> {
    let name = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("{name}: {e}"))?;
    std::thread::spawn(move || {
        if !child.wait().is_ok_and(|s| s.success()) {
            match kind {
                Kind::Opener => crate::dlog!("The opener reported a failure"),
                Kind::FileManager => {
                    crate::dlog!("The file manager did not show the item");
                    if let Some(folder) = folder {
                        let _ = open_file(&folder, false);
                    }
                }
            }
        }
    });
    Ok(())
}

pub fn open_url(url: &str, _background: bool) -> Result<(), String> {
    spawn_and_wait(opener(OsStr::new(url)), Kind::Opener, None)
}

/// Opens a judged file or folder with its default app.
pub fn open_file(path: &Path, _background: bool) -> Result<(), String> {
    spawn_and_wait(file_opener(path), Kind::Opener, None)
}

/// Another app's scheme, already judged and percent-encoded: the same opener.
pub fn open_scheme(encoded_url: &str, _background: bool) -> Result<(), String> {
    spawn_and_wait(opener(OsStr::new(encoded_url)), Kind::Opener, None)
}

pub fn reveal(path: &str) -> Result<(), String> {
    reveal_all(&[PathBuf::from(path)])
}

/// Shows the items selected in the file manager: `open -R` on macOS, FileManager1 on Linux. Where no
/// FileManager1 service answers (or dbus-send is missing), the first item's folder opens instead.
pub fn reveal_all(paths: &[PathBuf]) -> Result<(), String> {
    if paths.is_empty() {
        return Ok(());
    }
    if let Some(bad) = paths.iter().find(|p| !p.is_absolute()) {
        return Err(format!("the file manager needs a full path, not {}", bad.display()));
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Command::new("open");
        cmd.arg("-R").arg("--").args(paths);
        spawn_and_wait(cmd, Kind::FileManager, None)
    }
    #[cfg(not(target_os = "macos"))]
    {
        // A failed reveal opens a folder instead, never a bundle around the item.
        let folder = super::open_judge::fallback_folder(&paths[0]);
        let mut cmd = Command::new("dbus-send");
        cmd.args(dbus_reveal_args(paths)?);
        match spawn_and_wait(cmd, Kind::FileManager, folder.clone()) {
            Err(_) if folder.is_some() => open_file(&folder.unwrap_or_default(), false),
            done => done,
        }
    }
}

/// The dbus-send arguments for FileManager1.ShowItems: every path as a percent-encoded file URI.
#[cfg(any(test, not(target_os = "macos")))]
fn dbus_reveal_args(paths: &[PathBuf]) -> Result<Vec<String>, String> {
    use std::os::unix::ffi::OsStrExt;
    let mut uris = Vec::new();
    for p in paths {
        if !p.is_absolute() {
            return Err("the file manager needs a full path".to_string());
        }
        let mut uri = String::from("file://");
        for &b in p.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
                uri.push(b as char);
            } else {
                uri.push_str(&format!("%{b:02X}"));
            }
        }
        uris.push(uri);
    }
    Ok(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"]
        .iter()
        .map(|s| s.to_string())
        .chain([format!("array:string:{}", uris.join(",")), "string:".to_string()])
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(c: &Command) -> Vec<String> {
        c.get_args().map(|a| a.to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn openers_take_one_argument() {
        let url = "https://example.com/a b;rm -rf x?q=1&r=2";
        assert_eq!(args(&opener(std::ffi::OsStr::new(url))), vec![url.to_string()]);
        // A name that starts with a dash is not an option.
        assert_eq!(args(&file_opener(Path::new("-n"))), vec!["./-n".to_string()]);
        // A link is never rewritten into a file name.
        assert_eq!(args(&opener(std::ffi::OsStr::new("-n"))), vec!["-n".to_string()]);
    }

    #[test]
    fn reveal_uses_file_uris() {
        let paths = [std::path::PathBuf::from("/tmp/a b")];
        let argv = dbus_reveal_args(&paths).unwrap();
        assert!(argv.contains(&"array:string:file:///tmp/a%20b".to_string()), "{argv:?}");
        assert!(argv.contains(&"org.freedesktop.FileManager1.ShowItems".to_string()));
        assert!(dbus_reveal_args(&[std::path::PathBuf::from("rel")]).is_err());
        // The array separator and non-ASCII bytes are escaped, so one path stays one URI.
        let odd = dbus_reveal_args(&[PathBuf::from("/tmp/a,b/caf\u{e9}")]).unwrap();
        assert!(odd.contains(&"array:string:file:///tmp/a%2Cb/caf%C3%A9".to_string()), "{odd:?}");
        assert!(reveal_all(&[PathBuf::from("rel")]).is_err());
    }
}
