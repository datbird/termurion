//! Crash capture: an unhandled SEH exception writes a minidump and one line in host.log, then
//! lets Windows Error Reporting see the crash as before. Both programs that must survive a
//! post-mortem (termurion.exe and termurion-host.exe) install it first thing in `main`.
//!
//! Dumps go to `%LOCALAPPDATA%\Termurion\crashes\<exe>-<pid>-<unixsecs>.dmp`; the newest
//! `KEEP` stay. dbghelp.dll is loaded from System32 only, and only when a crash happens.

use std::path::{Path, PathBuf};

/// How many dumps the crashes folder keeps.
pub const KEEP: usize = 5;

/// `%LOCALAPPDATA%\Termurion\crashes` (the temp folder when the variable is missing).
pub fn crashes_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Termurion").join("crashes")
}

/// A dump's file name.
pub fn dump_name(exe: &str, pid: u32, secs: u64) -> String {
    format!("{exe}-{pid}-{secs}.dmp")
}

/// Deletes all but the newest `keep` `.dmp` files in `dir` (by modified time, then name).
pub fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("dmp")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    let drop_n = files.len().saturating_sub(keep);
    for (_, p) in files.into_iter().take(drop_n) {
        let _ = std::fs::remove_file(p);
    }
}

/// The log line for a crash.
pub fn crash_line(exe: &str, code: u32, module: &str, offset: usize, dump: &str) -> String {
    format!("crash: {exe} code 0x{code:08x} at {module}+0x{offset:x}, dump {dump}")
}

#[cfg(windows)]
pub use win::{describe, install, write_dump};

#[cfg(windows)]
mod win {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows_sys::Win32::Foundation::{BOOL, HANDLE};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithDataSegs, MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules,
        SetUnhandledExceptionFilter,
    };
    use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW, GetProcAddress};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

    /// Folder and program name, prepared at install time so the filter allocates little.
    static SETUP: OnceLock<(PathBuf, String)> = OnceLock::new();
    /// A crash inside the filter itself is not handled twice.
    static BUSY: AtomicBool = AtomicBool::new(false);

    type WriteFn = unsafe extern "system" fn(HANDLE, u32, HANDLE, i32, *const MINIDUMP_EXCEPTION_INFORMATION, *const core::ffi::c_void, *const core::ffi::c_void) -> BOOL;

    /// Installs the filter and creates the crashes folder. Call once, right after
    /// `dll::harden_process()`.
    pub fn install() {
        let exe = std::env::current_exe().ok().and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).unwrap_or_else(|| "termurion".into());
        let dir = crashes_dir();
        let _ = std::fs::create_dir_all(&dir);
        if SETUP.set((dir, exe)).is_ok() {
            // SAFETY: the filter is a plain `extern "system"` function that lives for the whole process.
            unsafe { SetUnhandledExceptionFilter(Some(filter)) };
        }
    }

    unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
        const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
        if BUSY.swap(true, Ordering::SeqCst) || info.is_null() {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let (thread, ptrs) = (unsafe { GetCurrentThreadId() }, info as usize);
        // A fresh thread has stack to spare, which a stack overflow's own thread does not. The
        // crashed thread waits; MiniDumpWriteDump reads its context through the pointers.
        match std::thread::Builder::new().spawn(move || report(thread, ptrs)) {
            Ok(h) => {
                let _ = h.join();
            }
            Err(_) => report(thread, ptrs),
        }
        EXCEPTION_CONTINUE_SEARCH
    }

    fn report(thread: u32, ptrs: usize) {
        let Some((dir, exe)) = SETUP.get() else { return };
        let info = ptrs as *const EXCEPTION_POINTERS;
        // SAFETY: Windows passes valid pointers to the filter, which this thread reads while the crashed thread waits.
        let (code, addr) = unsafe {
            let rec = (*info).ExceptionRecord;
            if rec.is_null() { (0, 0) } else { ((*rec).ExceptionCode as u32, (*rec).ExceptionAddress as usize) }
        };
        let dump = write_dump(dir, exe, Some((thread, ptrs)));
        let (module, offset) = describe(addr);
        let shown = match &dump {
            Ok(p) => p.display().to_string(),
            Err(e) => format!("none ({e})"),
        };
        crate::host_base::log(&crash_line(exe, code, &module, offset, &shown));
    }

    /// The module file name holding `addr` and the offset in it ("?" and the address when none).
    pub fn describe(addr: usize) -> (String, usize) {
        let mut module = std::ptr::null_mut();
        // SAFETY: FROM_ADDRESS takes the address as the "name"; UNCHANGED_REFCOUNT takes no reference.
        let ok = unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, addr as *const u16, &mut module) };
        if ok == 0 || module.is_null() {
            return ("?".into(), addr);
        }
        let mut buf = [0u16; 520];
        // SAFETY: `buf` is as long as stated.
        let n = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        let path = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
        let name = path.rsplit(['\\', '/']).next().unwrap_or("?").to_string();
        (if name.is_empty() { "?".into() } else { name }, addr.wrapping_sub(module as usize))
    }

    /// Writes a minidump of this process into `dir` (created when missing) and prunes the
    /// folder to `KEEP`. `exception` is the crashing thread's id and its `EXCEPTION_POINTERS`
    /// address; None writes a dump with no exception record.
    pub fn write_dump(dir: &Path, exe: &str, exception: Option<(u32, usize)>) -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
        let lib = crate::dll::load_system("dbghelp.dll").ok_or("dbghelp.dll did not load from System32")?;
        // SAFETY: the name is NUL-terminated; the function has this signature in dbghelp.
        let write: WriteFn = unsafe {
            let p = GetProcAddress(lib, c"MiniDumpWriteDump".as_ptr().cast()).ok_or("no MiniDumpWriteDump")?;
            std::mem::transmute::<unsafe extern "system" fn() -> isize, WriteFn>(p)
        };
        // SAFETY: plain queries.
        let pid = unsafe { GetCurrentProcessId() };
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let path = dir.join(dump_name(exe, pid, secs));
        let kind = MiniDumpWithDataSegs | MiniDumpWithThreadInfo | MiniDumpWithIndirectlyReferencedMemory | MiniDumpWithUnloadedModules;
        let exc = exception.map(|(thread, ptrs)| MINIDUMP_EXCEPTION_INFORMATION { ThreadId: thread, ExceptionPointers: ptrs as *mut EXCEPTION_POINTERS, ClientPointers: 0 });
        let exc_ptr = exc.as_ref().map_or(std::ptr::null(), |e| e as *const _);
        // Other threads keep running while the dump is read, and following pointers into memory one of them frees fails
        // with a partial copy; the second try leaves the pointer-following out and still has every stack and module.
        let mut last = String::new();
        for flags in [kind, kind & !MiniDumpWithIndirectlyReferencedMemory] {
            let file = std::fs::File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
            // SAFETY: the file handle outlives the call; `exc` holds pointers Windows gave this process.
            let ok = unsafe { write(GetCurrentProcess(), pid, file.as_raw_handle() as HANDLE, flags, exc_ptr, std::ptr::null(), std::ptr::null()) };
            if ok != 0 {
                last.clear();
                break;
            }
            last = format!("MiniDumpWriteDump: {}", std::io::Error::last_os_error());
        }
        if !last.is_empty() {
            let _ = std::fs::remove_file(&path);
            return Err(last);
        }
        prune(dir, KEEP);
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("termurion-crashdump-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names_and_lines() {
        assert_eq!(dump_name("termurion", 12, 34), "termurion-12-34.dmp");
        assert_eq!(crash_line("termurion", 0xc0000005, "icu.dll", 0x1a2b, r"C:\x\a.dmp"), r"crash: termurion code 0xc0000005 at icu.dll+0x1a2b, dump C:\x\a.dmp");
    }

    #[test]
    fn prune_keeps_the_newest() {
        let d = temp("prune");
        let base = std::time::SystemTime::now();
        for i in 0..8u64 {
            let f = std::fs::File::create(d.join(format!("t-{i}.dmp"))).unwrap();
            f.set_modified(base - std::time::Duration::from_secs(100 - i)).unwrap();
        }
        std::fs::write(d.join("note.txt"), b"x").unwrap();
        prune(&d, KEEP);
        let mut left: Vec<String> = std::fs::read_dir(&d).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, vec!["note.txt", "t-3.dmp", "t-4.dmp", "t-5.dmp", "t-6.dmp", "t-7.dmp"]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn a_dump_is_written_and_the_folder_pruned() {
        let d = temp("dump");
        for i in 0..7u64 {
            let f = std::fs::File::create(d.join(format!("old-{i}.dmp"))).unwrap();
            f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(1000 - i)).unwrap();
        }
        let p = write_dump(&d, "unit", None).expect("dump");
        let bytes = std::fs::read(&p).unwrap();
        assert!(bytes.len() > 1024 && &bytes[..4] == b"MDMP", "a minidump file");
        let n = std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "dmp")).count();
        assert_eq!(n, KEEP);
        assert!(p.exists(), "the new dump is among the newest");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[test]
    fn an_address_names_its_module() {
        let here = an_address_names_its_module as fn() as usize;
        let (module, offset) = describe(here);
        let exe = std::env::current_exe().unwrap().file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(module, exe);
        assert!(offset > 0);
    }
}
