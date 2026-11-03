//! The one Unix judge in front of `open` and `xdg-open`: which files would run if opened (they
//! are shown in the file manager instead), which link schemes are never handed over, and how a
//! `file:` link names a local path. The Windows lists stay Windows' own; they are kept here too,
//! as a floor, so a type Windows refuses is never opened on Unix either.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Types `open` or `xdg-open` runs, installs, mounts or follows, lower case, no dot: macOS
/// Terminal and Automator files, bundles and packages, link files that can name an app,
/// Linux launchers and packages, and shell and script types.
const RUNNABLE_TYPES: &[&str] = &[
    "command", "terminal", "tool", "workflow", "action", "app", "pkg", "mpkg", "prefpane", "saver", "qlgenerator", "kext", "plugin", "osax",
    "scpt", "scptd", "applescript", "mobileconfig", "webloc", "inetloc", "fileloc", "afploc", "dmg", "desktop", "appimage", "run", "deb",
    "rpm", "flatpakref", "snap", "sh", "bash", "zsh", "tcsh", "fish", "pl", "rb", "php", "lua",
];

/// Schemes no choice opens on Unix, with the reason: the opener mounts a share, starts a remote
/// session or runs help content for them.
const BLOCKED_SCHEMES: &[(&str, &str)] = &[
    ("afp", "Finder mounts a network share"),
    ("nfs", "Finder mounts a network share"),
    ("cifs", "Finder mounts a network share"),
    ("smb", "the file manager mounts a network share"),
    ("sftp", "the file manager mounts a remote folder"),
    ("ftps", "the file manager mounts a remote folder"),
    ("fish", "the file manager opens a remote shell"),
    ("vnc", "starts a remote desktop session"),
    ("rdp", "starts a remote desktop session"),
    ("spice", "starts a remote desktop session"),
    ("help", "macOS Help Viewer runs help content"),
    ("applescript", "opens a script ready to run"),
];

/// Whether opening `path` could run something: a type Windows or Unix runs, a file with any
/// execute bit, a macOS bundle folder, or anything that is not a plain file or folder. A file
/// whose kind cannot be read counts as runnable (shown, never opened); one that is not there
/// does not (it is not opened either).
pub fn runnable(path: &str) -> bool {
    crate::inert_files::high_risk(path) || runnable_type(path) || runs_by_kind(Path::new(path))
}

fn runnable_type(path: &str) -> bool {
    let name = Path::new(path).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    name.rsplit_once('.').is_some_and(|(_, ext)| RUNNABLE_TYPES.contains(&ext))
}

fn runs_by_kind(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
        Ok(m) if m.is_dir() => is_bundle(path),
        Ok(m) if m.is_file() => m.permissions().mode() & 0o111 != 0,
        Ok(_) => true,
    }
}

/// A macOS bundle is a folder with `Contents` inside; `open` launches or installs it.
fn is_bundle(dir: &Path) -> bool {
    match std::fs::symlink_metadata(dir.join("Contents")) {
        Ok(_) => true,
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

/// Why `scheme` (lower case) is never handed to the opener on Unix; None when it may be asked.
pub fn scheme_refusal(scheme: &str) -> Option<&'static str> {
    BLOCKED_SCHEMES.iter().find(|(s, _)| *s == scheme).map(|(_, why)| *why)
}

/// Whether `host` in a `file://host/...` link is this machine (`ls --hyperlink` and gcc write
/// the host name): the full name or its first label, any case.
pub fn is_this_host(host: &str) -> bool {
    let mut buf = [0u8; 256];
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return false;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let Ok(me) = std::str::from_utf8(&buf[..len]) else { return false };
    !host.is_empty() && !me.is_empty() && (host.eq_ignore_ascii_case(me) || me.split('.').next().is_some_and(|short| host.eq_ignore_ascii_case(short)))
}

/// A decoded `file:` link path as a Unix path: it must be absolute and hold no NUL; slashes stay.
pub fn file_url_os_path(path: String) -> Option<String> {
    (path.starts_with('/') && !path.contains('\0')).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::Run;

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("termurion-open-judge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, mode: u32) -> String {
        std::fs::write(p, "x").unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn opened(path: &str) -> Run {
        crate::semantic::open_local_result(path, false, &crate::netpath::Probe::default(), &|p| Path::new(p).exists())
    }

    /// A click shows a program and never runs it: Terminal files, bundles, launchers and any file
    /// with an execute bit are revealed; plain documents open.
    #[test]
    fn programs_are_revealed_and_documents_open() {
        let d = dir("kinds");
        for name in ["x.command", "x.terminal", "x.tool", "x.workflow", "x.pkg", "x.mpkg", "x.desktop", "x.sh", "x.AppImage", "X.COMMAND", "x.webloc", "x.fileloc"] {
            let p = write(&d.join(name), 0o644);
            assert_eq!(opened(&p), Run::Reveal(p.clone()), "{name}");
        }
        let tool = write(&d.join("build"), 0o755);
        assert_eq!(opened(&tool), Run::Reveal(tool.clone()), "an execute bit runs");
        let user_x = write(&d.join("notes"), 0o744);
        assert_eq!(opened(&user_x), Run::Reveal(user_x.clone()));
        let bundle = d.join("Thing.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        let b = bundle.to_string_lossy().into_owned();
        assert_eq!(opened(&b), Run::Reveal(b.clone()), "a bundle folder launches");
        let bare = d.join("plain-bundle");
        std::fs::create_dir_all(bare.join("Contents")).unwrap();
        assert!(runnable(&bare.to_string_lossy()), "a folder with Contents is a bundle whatever its name");
        // The Windows floor still holds.
        let exe = write(&d.join("x.exe"), 0o644);
        assert_eq!(opened(&exe), Run::Reveal(exe.clone()));
        for fine in ["notes.md", "a.txt", "photo.png"] {
            let p = write(&d.join(fine), 0o644);
            assert_eq!(opened(&p), Run::Open(p.clone()), "{fine}");
        }
        let folder = d.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let f = folder.to_string_lossy().into_owned();
        assert_eq!(opened(&f), Run::Open(f.clone()), "a plain folder opens");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Fail closed: a file whose kind cannot be read, or that is a pipe or device, is never opened.
    #[test]
    fn an_unreadable_kind_is_runnable() {
        assert!(runnable("/dev/null"), "a device");
        assert!(!runnable("/no/such/file.txt"));
        let d = dir("locked");
        let inner = d.join("inner");
        std::fs::create_dir_all(&inner).unwrap();
        let f = write(&inner.join("doc.txt"), 0o644);
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root reads through a mode of 0; only a normal user sees the refusal.
        if std::fs::metadata(&f).is_err() {
            assert!(runnable(&f));
        }
        std::fs::set_permissions(&inner, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Mount and remote-desktop schemes never reach the opener, whatever was chosen.
    #[test]
    fn mount_and_remote_desktop_schemes_are_blocked() {
        for url in ["afp://host/share", "nfs://host/x", "cifs://host/x", "smb://host/x", "vnc://host", "sftp://host/x", "rdp://host", "ftps://host/x", "help:openbook=x", "fish://host/x", "applescript://com.apple.scripteditor?action=new"] {
            match crate::semantic::guard_open(url, false) {
                Run::Blocked(why) => assert!(why.contains("never opens"), "{url}: {why}"),
                other => panic!("{url}: {other:?}"),
            }
        }
        assert_eq!(crate::semantic::guard_open("vscode://file/x", false), Run::OpenScheme("vscode://file/x".into()));
    }

    /// `file:` links name Unix paths: slashes stay, and this machine's own name (`ls --hyperlink`)
    /// is local; another host is not.
    #[test]
    fn file_links_are_unix_paths_and_this_host_is_local() {
        let mut buf = [0u8; 256];
        unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
        let me = std::str::from_utf8(&buf[..buf.iter().position(|&b| b == 0).unwrap()]).unwrap().to_string();
        assert_eq!(crate::semantic::file_url_path("file:///home/u/a%20b.txt").as_deref(), Some("/home/u/a b.txt"));
        assert_eq!(crate::semantic::file_url_path("file://localhost/etc/hosts").as_deref(), Some("/etc/hosts"));
        assert_eq!(crate::semantic::file_url_path(&format!("file://{me}/etc/hosts")).as_deref(), Some("/etc/hosts"));
        assert_eq!(crate::semantic::file_url_path(&format!("file://{}/etc/hosts", me.to_uppercase())).as_deref(), Some("/etc/hosts"));
        assert_eq!(crate::semantic::file_url_path("file://some-other-host.example/etc/hosts"), None);
        assert_eq!(crate::semantic::file_url_path("file:relative/x"), None);
        assert_eq!(crate::semantic::file_url_path("file:///a%00b"), None);
        assert!(crate::browser_js::gate_refusal(&format!("file://{me}/etc/hosts")).is_none());
        assert!(crate::browser_js::gate_refusal("file://some-other-host.example/etc/hosts").is_some());
        assert_eq!(crate::semantic::guard_open(&format!("file://{me}/etc/hosts"), false), Run::OpenLocal { path: "/etc/hosts".into(), has_code: false });
        assert!(!is_this_host("") && !is_this_host("some-other-host.example"));
    }
}
