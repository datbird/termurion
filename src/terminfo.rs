//! iTerm2's compiled terminfo entries (`Resources/terminfo`, 46 files), embedded so a WSL
//! session can find them through `TERMINFO_DIRS` (`PS:3360`). Linux ncurses reads the
//! first-letter layout (`x/xterm-256color`); iTerm2's copy uses Darwin's hex directories.
//! The files are written once per run under `%APPDATA%\Tandem\terminfo`.

/// Each entry's path relative to the terminfo folder, and its compiled bytes.
pub const FILES: &[(&str, &[u8])] = &[
    ("a/ansi+enq", include_bytes!("../resources/terminfo/a/ansi+enq")),
    ("a/ansi+rep", include_bytes!("../resources/terminfo/a/ansi+rep")),
    ("d/dec+sl", include_bytes!("../resources/terminfo/d/dec+sl")),
    ("n/nxterm", include_bytes!("../resources/terminfo/n/nxterm")),
    ("t/tmux-256color", include_bytes!("../resources/terminfo/t/tmux-256color")),
    ("v/vs100", include_bytes!("../resources/terminfo/v/vs100")),
    ("v/vt220+keypad", include_bytes!("../resources/terminfo/v/vt220+keypad")),
    ("x/xterm", include_bytes!("../resources/terminfo/x/xterm")),
    ("x/xterm-16color", include_bytes!("../resources/terminfo/x/xterm-16color")),
    ("x/xterm-24", include_bytes!("../resources/terminfo/x/xterm-24")),
    ("x/xterm+256color", include_bytes!("../resources/terminfo/x/xterm+256color")),
    ("x/xterm-256color", include_bytes!("../resources/terminfo/x/xterm-256color")),
    ("x/xterm-88color", include_bytes!("../resources/terminfo/x/xterm-88color")),
    ("x/xterm-8bit", include_bytes!("../resources/terminfo/x/xterm-8bit")),
    ("x/xterm+app", include_bytes!("../resources/terminfo/x/xterm+app")),
    ("x/xterm-basic", include_bytes!("../resources/terminfo/x/xterm-basic")),
    ("x/xterm-bold", include_bytes!("../resources/terminfo/x/xterm-bold")),
    ("x/xterm-color", include_bytes!("../resources/terminfo/x/xterm-color")),
    ("x/xterm+edit", include_bytes!("../resources/terminfo/x/xterm+edit")),
    ("x/xterm-hp", include_bytes!("../resources/terminfo/x/xterm-hp")),
    ("x/xterm+kbs", include_bytes!("../resources/terminfo/x/xterm+kbs")),
    ("x/xterm-new", include_bytes!("../resources/terminfo/x/xterm-new")),
    ("x/xterm-noapp", include_bytes!("../resources/terminfo/x/xterm-noapp")),
    ("x/xterm-old", include_bytes!("../resources/terminfo/x/xterm-old")),
    ("x/xterm+pcc0", include_bytes!("../resources/terminfo/x/xterm+pcc0")),
    ("x/xterm+pcc1", include_bytes!("../resources/terminfo/x/xterm+pcc1")),
    ("x/xterm+pcc2", include_bytes!("../resources/terminfo/x/xterm+pcc2")),
    ("x/xterm+pcc3", include_bytes!("../resources/terminfo/x/xterm+pcc3")),
    ("x/xterm+pce2", include_bytes!("../resources/terminfo/x/xterm+pce2")),
    ("x/xterm+pc+edit", include_bytes!("../resources/terminfo/x/xterm+pc+edit")),
    ("x/xterm+pcf0", include_bytes!("../resources/terminfo/x/xterm+pcf0")),
    ("x/xterm+pcf2", include_bytes!("../resources/terminfo/x/xterm+pcf2")),
    ("x/xterm+pcfkeys", include_bytes!("../resources/terminfo/x/xterm+pcfkeys")),
    ("x/xterm-r5", include_bytes!("../resources/terminfo/x/xterm-r5")),
    ("x/xterm-r6", include_bytes!("../resources/terminfo/x/xterm-r6")),
    ("x/xterms", include_bytes!("../resources/terminfo/x/xterms")),
    ("x/xterm-sco", include_bytes!("../resources/terminfo/x/xterm-sco")),
    ("x/xterm+sm+1006", include_bytes!("../resources/terminfo/x/xterm+sm+1006")),
    ("x/xterm-sun", include_bytes!("../resources/terminfo/x/xterm-sun")),
    ("x/xterm+tmux", include_bytes!("../resources/terminfo/x/xterm+tmux")),
    ("x/xterm-vt220", include_bytes!("../resources/terminfo/x/xterm-vt220")),
    ("x/xterm-vt52", include_bytes!("../resources/terminfo/x/xterm-vt52")),
    ("x/xterm+vt+edit", include_bytes!("../resources/terminfo/x/xterm+vt+edit")),
    ("x/xterm+x11mouse", include_bytes!("../resources/terminfo/x/xterm+x11mouse")),
    ("x/xterm-xf86-v44", include_bytes!("../resources/terminfo/x/xterm-xf86-v44")),
    ("x/xterm-xfree86", include_bytes!("../resources/terminfo/x/xterm-xfree86")),
];

/// The path of the entry `name` in ncurses' first-letter layout: its first character, a
/// slash, the name.
pub fn layout_path(name: &str) -> String {
    match name.chars().next() {
        Some(c) => format!("{c}/{name}"),
        None => String::new(),
    }
}

/// Writes every entry missing or different under `appdata\Tandem\terminfo` and returns that
/// folder; a file already identical is left alone. A UNC `appdata` is refused (never written
/// from the UI thread; `session_env::appdata_local` is the one rule for it).
#[cfg(windows)]
pub fn ensure(appdata: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    if crate::session_env::is_unc(&appdata.to_string_lossy()) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "the roaming folder is a network path"));
    }
    let dir = appdata.join("Tandem").join("terminfo");
    for (path, bytes) in FILES {
        let file = dir.join(path);
        if std::fs::read(&file).is_ok_and(|have| have == *bytes) {
            continue;
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&file, bytes)?;
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminfo_layout_follows_ncurses() {
        assert_eq!(layout_path("xterm-256color"), "x/xterm-256color");
        assert_eq!(layout_path("ansi+enq"), "a/ansi+enq");
        assert_eq!(layout_path(""), "");
        let has = |p: &str| FILES.iter().any(|(n, _)| *n == p);
        assert!(has("x/xterm") && has("x/xterm-new") && has("x/xterm-256color"));
        assert_eq!(FILES.len(), 46);
        for (path, bytes) in FILES {
            assert!(!bytes.is_empty(), "{path}");
            assert!(bytes.starts_with(&[0x1a, 0x01]) || bytes.starts_with(&[0x1e, 0x02]), "{path} is not a compiled terminfo entry");
            let (dir, name) = path.split_once('/').unwrap();
            assert_eq!(layout_path(name), *path);
            assert_eq!(dir.len(), 1);
        }
    }

    #[cfg(windows)]
    #[test]
    fn ensure_writes_and_repairs() {
        let root = std::env::temp_dir().join(format!("tandem-terminfo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = ensure(&root).unwrap();
        assert_eq!(dir, root.join("Tandem").join("terminfo"));
        for (path, bytes) in FILES {
            assert_eq!(std::fs::read(dir.join(path)).unwrap(), *bytes, "{path}");
        }
        // A changed file is put back; an identical one is not rewritten.
        let xterm = dir.join("x").join("xterm");
        std::fs::write(&xterm, b"old").unwrap();
        let other = dir.join("x").join("xterm-new");
        let before = std::fs::metadata(&other).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        ensure(&root).unwrap();
        assert_eq!(std::fs::read(&xterm).unwrap(), FILES.iter().find(|(n, _)| *n == "x/xterm").unwrap().1);
        assert_eq!(std::fs::metadata(&other).unwrap().modified().unwrap(), before);
        assert!(ensure(std::path::Path::new(r"\\host\share")).is_err(), "a network path is refused");
        let _ = std::fs::remove_dir_all(&root);
    }
}
