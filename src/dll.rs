//! Loading DLLs without the application folder or the current folder in the search path.
//! A bare `LoadLibraryW("name.dll")` looks beside the exe and in the current folder before
//! System32 unless the DLL is a KnownDLL, so a planted file runs in the process. Every
//! runtime load goes through here instead.

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Foundation::HMODULE;
#[cfg(windows)]
use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};

/// Restricts the whole process's DLL search to the application folder, System32 and the folders
/// added by `AddDllDirectory` (`LOAD_LIBRARY_SEARCH_DEFAULT_DIRS`), and removes the current folder
/// (`SetDllDirectoryW("")`). Every program calls it first in `main`, before any window or
/// graphics initialization, so a bare load anywhere (a dependency's `libloading`, wgpu-hal's
/// vulkan-1.dll and d3d12.dll) never finds a DLL planted in the current folder or on PATH.
/// Both calls always run (one failing never skips the other) and each failure is written to
/// stderr; false when Windows refused either.
#[cfg(windows)]
pub fn harden_process() -> bool {
    use windows_sys::Win32::System::LibraryLoader::{LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, SetDefaultDllDirectories, SetDllDirectoryW};
    let empty = [0u16];
    // SAFETY: both calls take plain values or a NUL-terminated string that outlives the call.
    let dirs = unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) != 0 };
    let cwd = unsafe { SetDllDirectoryW(empty.as_ptr()) != 0 };
    let mut ok = true;
    for (name, done) in [("SetDefaultDllDirectories", dirs), ("SetDllDirectoryW", cwd)] {
        if let Some(line) = failure_line(name, done) {
            eprintln!("{line}");
            ok = false;
        }
    }
    ok
}

/// The stderr line for a hardening call that failed; None when it succeeded.
pub fn failure_line(call: &str, ok: bool) -> Option<String> {
    (!ok).then(|| format!("tandem: {call} failed; the DLL search is not fully restricted"))
}

/// A system DLL by bare name (`icu.dll`): only System32 is searched, never the exe's folder or
/// the current folder. None when it is not there.
#[cfg(windows)]
pub fn load_system(name: &str) -> Option<HMODULE> {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
    (!module.is_null()).then_some(module)
}

/// A DLL Tandem ships, by full path: its own folder and System32 (for its imports) are
/// searched. A relative path is refused, since the flag needs an absolute one and a relative
/// one would depend on the current folder.
#[cfg(windows)]
pub fn load_bundled(path: &std::path::Path) -> Result<HMODULE, String> {
    if !path.is_absolute() {
        return Err(format!("{} is not an absolute path", path.display()));
    }
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let module = unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32) };
    if module.is_null() {
        return Err(format!("LoadLibraryExW({}) failed: {}", path.display(), std::io::Error::last_os_error()));
    }
    Ok(module)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                sources(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// Every runtime load of a DLL passes a search-path flag: no `LoadLibraryW`, `LoadLibraryA`,
    /// `LoadLibraryExA` or `LoadPackagedLibrary` token (an alias import counts) and no `libloading`
    /// remains outside comments; `LoadLibraryExW` appears only in this file and the vendored
    /// conpty, and each call there names `LOAD_LIBRARY_SEARCH_SYSTEM32` within its own text
    /// (a call may span lines).
    #[test]
    fn no_dll_loads_from_the_exe_or_current_folder() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        for dir in ["src", "crates", "tests", "examples"] {
            if root.join(dir).is_dir() {
                sources(&root.join(dir), &mut files);
            }
        }
        for f in ["build.rs"] {
            if root.join(f).is_file() {
                files.push(root.join(f));
            }
        }
        let bare: Vec<String> = ["LoadLibraryW", "LoadLibraryA", "LoadLibraryExA", "LoadPackagedLibrary", "libloading"].iter().map(|t| t.to_string()).collect();
        let ex = "LoadLibraryEx".to_string() + "W";
        let flag = "LOAD_LIBRARY_SEARCH_".to_string() + "SYSTEM32";
        let allowed = [root.join("src").join("dll.rs"), root.join("crates").join("alacritty_terminal").join("src").join("tty").join("windows").join("conpty.rs")];
        let this = root.join("src").join("dll.rs");
        let mut bad = Vec::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap();
            // This file's own tests hold the tokens as data and its docs name them.
            let code: String = text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
            let code = if f == this { code.split("#[cfg(test)]").next().unwrap_or("").to_string() } else { code };
            for b in &bare {
                if code.contains(b.as_str()) {
                    bad.push(format!("{}: names {b}", f.display()));
                }
            }
            for (at, _) in code.match_indices(ex.as_str()) {
                if !allowed.contains(&f) {
                    bad.push(format!("{}: LoadLibraryExW outside the two allowed files", f.display()));
                    break;
                }
                let call = &code[at..(at + 200).min(code.len())];
                // An import names it without calling it; an error message may quote it.
                let in_text = code[..at].ends_with('"');
                if !in_text && call[ex.len()..].starts_with('(') && !call.contains(&flag) {
                    bad.push(format!("{}: a LoadLibraryExW call without {flag}", f.display()));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }

    /// Review finding 1 (class fix): every program starts by restricting the process's DLL search
    /// (`dll::harden_process`), before any window, graphics or other load, so no DLL anywhere in
    /// the process (wgpu-hal and ash load vulkan-1.dll and d3d12.dll through libloading's bare
    /// `LoadLibraryExW`) comes from the current folder or PATH.
    #[test]
    fn every_program_hardens_the_dll_search_first() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("bin");
        let mut seen = 0;
        for e in std::fs::read_dir(&dir).unwrap().flatten() {
            let text = std::fs::read_to_string(e.path()).unwrap();
            let name = e.path().file_name().unwrap().to_string_lossy().to_string();
            let main_at = text.find("fn main()").unwrap_or_else(|| panic!("{name}: no main"));
            let body = &text[main_at..];
            let harden = body.find("tandem::dll::harden_process()").unwrap_or_else(|| panic!("{name}: main never calls harden_process"));
            let first_use = body.find("tandem::").unwrap();
            assert_eq!(harden, first_use, "{name}: harden_process is not the first call into Tandem");
            // Nothing before it in main but the opening: no other statement ends with a semicolon.
            assert!(!body[..harden].contains(';'), "{name}: a statement runs before harden_process");
            seen += 1;
        }
        assert!(seen >= 7, "{seen} programs");
    }

    /// Review minor: each failed call is reported, a success is silent.
    #[test]
    fn a_failed_hardening_call_is_reported() {
        assert_eq!(super::failure_line("X", true), None);
        assert!(super::failure_line("X", false).unwrap().contains("X failed"));
    }

    #[cfg(windows)]
    #[test]
    fn harden_process_restricts_the_search() {
        assert!(super::harden_process());
        // A system DLL still loads, by the flags `load_system` passes.
        assert!(super::load_system("kernel32.dll").is_some());
    }

    #[cfg(windows)]
    #[test]
    fn loads_follow_the_search_flags() {
        assert!(super::load_system("kernel32.dll").is_some());
        assert!(super::load_system("tandem-no-such-dll.dll").is_none());
        let err = super::load_bundled(Path::new("conpty.dll")).unwrap_err();
        assert!(err.contains("not an absolute path"), "{err}");
        // The shipped conpty.dll loads by full path.
        assert!(super::load_bundled(&crate::conpty::dev_vendor_dir().join("conpty.dll")).is_ok());
    }
}
