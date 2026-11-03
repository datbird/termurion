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
    "rpm", "flatpakref", "flatpak", "flatpakrepo", "snap", "sparsebundle", "sparseimage", "cdr", "vncloc", "ftploc", "sh", "bash", "zsh", "tcsh", "fish", "pl", "rb", "php", "lua",
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

/// Whether opening `path` could run something: a type Windows or Unix runs (by its own name and
/// by the name of what it links to, as LaunchServices resolves a link first), a file with any
/// execute bit, a macOS bundle folder, or anything that is not a plain file or folder. A file
/// whose kind or target cannot be read counts as runnable (shown, never opened); one that is not
/// there does not (it is not opened either).
pub fn runnable(path: &str) -> bool {
    matches!(judge(Path::new(path)), Verdict::Show)
}

/// How many aliases deep a chain is followed before it counts as runnable.
const ALIAS_DEPTH: u32 = 8;

/// What one look at a path decided.
enum Verdict {
    /// Show it in the file manager.
    Show,
    /// Not there at all: nothing is shown or opened by the judge.
    Missing,
    /// Safe to open: this canonical file, the end of the link and alias chain that was judged.
    Open(std::path::PathBuf),
}

/// The one resolution of a path: the clicked name and every hop of its link and alias chain are
/// judged (names, kind, owner), and what is handed back is the very file that was judged last.
fn judge(path: &Path) -> Verdict {
    if named_runnable(path) || runs_by_kind(path) {
        return Verdict::Show;
    }
    let mut cur = match std::fs::canonicalize(path) {
        Ok(real) => real,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !is_link(path) => return Verdict::Missing,
        Err(_) => return Verdict::Show,
    };
    for _ in 0..=ALIAS_DEPTH {
        if named_runnable(&cur) || runs_by_kind(&cur) || shown_by_owner(&cur) {
            return Verdict::Show;
        }
        // A Finder alias is judged by what it names, like a link; one that does not resolve is shown.
        match super::files::finder_alias(&cur) {
            None => return Verdict::Open(cur),
            Some(Ok(next)) => cur = next,
            Some(Err(_)) => return Verdict::Show,
        }
    }
    Verdict::Show
}

/// Another person's file in a folder anyone can write to (a shared folder, /tmp): someone else
/// chose its content and its name, so it is shown and not opened. Root's files are the system's.
/// The narrow rule; `shown_for_owner` is the one the judge applies.
#[allow(dead_code)]
pub fn foreign_in_shared(owner: u32, euid: u32, dir_mode: u32) -> bool {
    owner != euid && owner != 0 && dir_mode & 0o022 != 0
}

/// Whether who owns a regular file, and who can change its folder, make it unsafe to open: the
/// owner can add an execute bit between the judge and `open` when they are not you or root; a
/// folder that others can write to (without the sticky bit) lets them swap the name; and so does
/// a folder that a group can write to, unless that group is your own primary one (your private
/// group has no other members).
pub fn shown_for_owner(owner: u32, euid: u32, parent_mode: u32, parent_gid: u32, egid: u32) -> bool {
    let sticky = parent_mode & 0o1000 != 0;
    (owner != euid && owner != 0) || (!sticky && (parent_mode & 0o002 != 0 || (parent_mode & 0o020 != 0 && parent_gid != egid)))
}

fn shown_by_owner(real: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::metadata(real) else { return true };
    if !meta.is_file() {
        return false;
    }
    let Some(parent) = real.parent() else { return false };
    match std::fs::metadata(parent) {
        Ok(dir) => shown_for_owner(meta.uid(), unsafe { libc::geteuid() }, dir.mode(), dir.gid(), unsafe { libc::getegid() }),
        // The folder cannot be read: fail safe.
        Err(_) => true,
    }
}

/// The name judged as text (the Windows floor) and as the real bytes (the Unix types).
fn named_runnable(path: &Path) -> bool {
    crate::inert_files::high_risk(&path.to_string_lossy()) || runnable_type(path)
}

/// Whether `path` is a link (its target then cannot be found: fail closed rather than guess); a
/// name that cannot be read at all counts as one.
fn is_link(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(m) => m.file_type().is_symlink(),
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    }
}

fn runnable_type(path: &Path) -> bool {
    path.extension().is_some_and(|ext| RUNNABLE_TYPES.iter().any(|t| ext.eq_ignore_ascii_case(t)))
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

/// What the opener is handed for a judged path.
#[derive(Debug, PartialEq, Eq)]
pub enum Judged {
    /// Open this path (the canonical one that was judged).
    Open(std::path::PathBuf),
    /// Show this path in the file manager instead.
    Reveal(std::path::PathBuf),
}

/// The last word before the opener: a runnable path is shown, anything else is opened as the
/// canonical path that was judged.
pub fn judged_open(path: &Path) -> Judged {
    match judge(path) {
        Verdict::Open(real) => Judged::Open(real),
        // Not there at all: the opener says so.
        Verdict::Missing => Judged::Open(path.to_path_buf()),
        Verdict::Show => Judged::Reveal(path.to_path_buf()),
    }
}

/// The folder a failed reveal opens instead: the item's parent, or the first ancestor that is
/// not a bundle (opening a bundle launches it).
pub fn fallback_folder(item: &Path) -> Option<std::path::PathBuf> {
    let mut dir = item.parent()?;
    // The outermost bundle counts: X.app/Contents/MacOS climbs to the folder holding X.app.
    let mut out = dir;
    while let Some(up) = dir.parent() {
        if runnable(&dir.to_string_lossy()) {
            out = up;
        }
        dir = up;
    }
    (!runnable(&out.to_string_lossy())).then(|| out.to_path_buf())
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
        // A folder others can write to is shown, so the tests do not follow the umask.
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
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

    /// Another person's file in a folder anyone writes to is shown; your own, root's and files in
    /// private folders are not.
    #[test]
    fn another_users_file_in_a_shared_folder_is_shown() {
        assert!(foreign_in_shared(1001, 501, 0o1777));
        assert!(!foreign_in_shared(501, 501, 0o1777));
        assert!(!foreign_in_shared(0, 501, 0o1777));
        assert!(!foreign_in_shared(1001, 501, 0o755));
    }

    /// The wider rule: another user's file is shown in any folder, a folder others can write to
    /// shows even your own file, and a group-writable one does unless the group is your own.
    #[test]
    fn a_file_is_shown_for_its_owner_or_its_folder() {
        let (me, mine, other) = (501, 20, 80);
        assert!(shown_for_owner(1001, me, 0o755, mine, mine), "another user's file in a private folder");
        assert!(!shown_for_owner(0, me, 0o755, mine, mine), "root's file");
        assert!(!shown_for_owner(me, me, 0o775, mine, mine), "your file in a folder of your own group");
        assert!(shown_for_owner(me, me, 0o775, other, mine), "your file in a folder of another group");
        assert!(shown_for_owner(me, me, 0o777, mine, mine), "world-writable without sticky");
        assert!(!shown_for_owner(me, me, 0o1777, other, mine), "sticky");
        assert!(!shown_for_owner(me, me, 0o755, other, mine));
    }

    /// One click resolves the alias chain once, and what is opened is what was judged: the code
    /// has a single alias resolution, and an opened path is never one the judge shows.
    #[test]
    fn the_judged_file_is_the_opened_file() {
        let src = include_str!("open_judge.rs");
        let code = &src[..src.find("#[cfg(test)]").unwrap()];
        assert_eq!(code.matches("finder_alias(").count(), 1, "one resolution");
        let d = dir("once");
        let doc = d.join("doc.txt");
        write(&doc, 0o644);
        let link = d.join("link.md");
        std::os::unix::fs::symlink(&doc, &link).unwrap();
        let prog = d.join("run.command");
        write(&prog, 0o644);
        for p in [&doc, &link, &prog, &d.join("missing.txt")] {
            if let Judged::Open(real) = judged_open(p) {
                assert!(!runnable(&real.to_string_lossy()), "{}", real.display());
            }
        }
        let _ = std::fs::remove_dir_all(&d);
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

    /// A link is judged by what it points at too: a harmless name on a link to a program is
    /// shown, and what is opened is the target that was judged.
    #[test]
    fn a_link_is_judged_by_its_target() {
        let d = dir("links");
        let real = std::fs::canonicalize(&d).unwrap();
        for target in ["payload.terminal", "tool.py", "inst.pkg"] {
            write(&d.join(target), 0o644);
            let link = d.join(format!("README-{target}.txt"));
            std::os::unix::fs::symlink(d.join(target), &link).unwrap();
            let l = link.to_string_lossy().into_owned();
            assert_eq!(opened(&l), Run::Reveal(l.clone()), "{target}");
            assert_eq!(judged_open(&link), Judged::Reveal(link.clone()), "{target}");
        }
        let doc = d.join("doc.txt");
        write(&doc, 0o644);
        let link = d.join("notes.md");
        std::os::unix::fs::symlink(&doc, &link).unwrap();
        assert_eq!(judged_open(&link), Judged::Open(real.join("doc.txt")), "the judged target is what opens");
        // A loop cannot be resolved: fail closed.
        let (a, b) = (d.join("loop-a.txt"), d.join("loop-b.txt"));
        std::os::unix::fs::symlink(&b, &a).unwrap();
        std::os::unix::fs::symlink(&a, &b).unwrap();
        assert!(runnable(&a.to_string_lossy()));
        assert_eq!(judged_open(&a), Judged::Reveal(a.clone()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A failed reveal never opens a bundle around the item.
    #[test]
    fn the_reveal_fallback_never_opens_a_bundle() {
        let d = dir("fallback");
        let inner = d.join("X.app/Contents/MacOS");
        std::fs::create_dir_all(&inner).unwrap();
        let item = inner.join("X");
        write(&item, 0o755);
        assert_eq!(fallback_folder(&item), Some(d.clone()));
        let plain = d.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(fallback_folder(&plain.join("f.txt")), Some(plain.clone()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Disk images, Flatpak bundles and the remaining Finder location files are shown too.
    #[test]
    fn images_bundles_and_location_files_are_runnable() {
        for name in ["x.sparsebundle", "x.sparseimage", "x.cdr", "x.flatpak", "x.flatpakrepo", "x.vncloc", "x.ftploc"] {
            assert!(runnable(&format!("/no/such/{name}")), "{name}");
        }
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
