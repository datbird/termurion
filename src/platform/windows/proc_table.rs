//! The Win32 process table: Toolhelp, process handles, start times, image paths, command lines and
//! the multi-process wait. Moved out of `jobwatch`, which is OS-free.

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW, WaitForMultipleObjects};

use super::super::proc_table::Row;
use std::time::Duration;

const SYNCHRONIZE: u32 = 0x0010_0000;

pub(super) fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

pub(super) struct Handle(pub(super) HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// A process opened for queries.
pub struct Opened(pub(super) Handle);

impl Opened {
    /// Creation time (FILETIME ticks); 0 when it cannot be read.
    pub fn created(&self) -> u64 {
        created(&self.0)
    }

    pub fn image(&self) -> Option<String> {
        image_path(&self.0)
    }

    pub fn cmdline(&self) -> Option<String> {
        command_line(&self.0)
    }
}

/// Opens `pid` with the given access; None when it is gone or off limits.
pub(super) fn open_with(pid: u32, access: u32) -> Option<Handle> {
    let h = unsafe { OpenProcess(access, 0, pid) };
    (!h.is_null()).then_some(Handle(h))
}

pub fn open(pid: u32) -> Option<Opened> {
    open_with(pid, PROCESS_QUERY_LIMITED_INFORMATION).map(Opened)
}

/// Every process's pid, parent, and image name.
pub fn rows() -> Result<Vec<Row>, String> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(os_err("CreateToolhelp32Snapshot"));
        }
        let snap = Handle(snap);
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut out = Vec::new();
        let mut ok = Process32FirstW(snap.0, &mut e);
        while ok != 0 {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            out.push(Row { pid: e.th32ProcessID, ppid: e.th32ParentProcessID, name: String::from_utf16_lossy(&e.szExeFile[..len]), background: false });
            ok = Process32NextW(snap.0, &mut e);
        }
        Ok(out)
    }
}

pub(super) fn created(h: &Handle) -> u64 {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut x, mut k, mut u) = (z, z, z, z);
    if unsafe { GetProcessTimes(h.0, &mut c, &mut x, &mut k, &mut u) } == 0 {
        return 0;
    }
    ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64
}

pub(super) fn image_path(h: &Handle) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    (unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// A process's command line (NtQueryInformationProcess, ProcessCommandLineInformation).
fn command_line(h: &Handle) -> Option<String> {
    type Query = unsafe extern "system" fn(HANDLE, u32, *mut core::ffi::c_void, u32, *mut u32) -> i32;
    let ntdll: Vec<u16> = "ntdll.dll".encode_utf16().chain(Some(0)).collect();
    let f = unsafe { GetProcAddress(GetModuleHandleW(ntdll.as_ptr()), c"NtQueryInformationProcess".as_ptr().cast())? };
    let query: Query = unsafe { std::mem::transmute(f) };
    let mut size = 1024u32;
    for _ in 0..4 {
        // A UNICODE_STRING (length, max length, pointer) followed by the text it points to.
        let mut buf = vec![0u64; size as usize / 8 + 1];
        let mut need = 0u32;
        let status = unsafe { query(h.0, 60, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut need) };
        if status == 0 {
            let len = (buf[0] & 0xFFFF) as usize / 2;
            let ptr = buf[1] as *const u16;
            if ptr.is_null() {
                return None;
            }
            // SAFETY: the pointer is into `buf`, which the call filled.
            let text = unsafe { std::slice::from_raw_parts(ptr, len) };
            return Some(String::from_utf16_lossy(text));
        }
        // STATUS_INFO_LENGTH_MISMATCH and friends: grow and retry.
        size = need.max(size * 2);
    }
    None
}

/// Waits until any of `pids` exits or `gap` passes. Handles are opened fresh and closed on
/// return, so no pid is held past its process's end; a pid that cannot be opened (gone, or
/// not ours) is skipped, and with no handle this is a plain sleep.
pub fn wait_any(pids: &[u32], gap: Duration) {
    let handles: Vec<HANDLE> = pids.iter().take(crate::jobwatch::MAX_WAIT).filter_map(|&pid| {
        let h = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
        (!h.is_null()).then_some(h)
    }).collect();
    if handles.is_empty() {
        std::thread::sleep(gap);
        return;
    }
    let ms = gap.as_millis().min(u128::from(u32::MAX - 1)) as u32;
    unsafe {
        WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, ms);
        for h in handles {
            CloseHandle(h);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobwatch::console_targets;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn watcher_wakes_on_job_exit() {
        use std::time::Instant;
        let mut child = std::process::Command::new("powershell.exe").args(["-NoProfile", "-Command", "Start-Sleep -Milliseconds 300"]).spawn().expect("spawn");
        let t = Instant::now();
        wait_any(&[child.id()], Duration::from_secs(5));
        assert!(t.elapsed() < Duration::from_secs(2), "woke on exit, not on the gap: {:?}", t.elapsed());
        let _ = child.wait();
        let t = Instant::now();
        wait_any(&[4_000_000_000], Duration::from_millis(100));
        let e = t.elapsed();
        assert!(e >= Duration::from_millis(90) && e < Duration::from_secs(1), "a gone pid waits the gap: {e:?}");
        let t = Instant::now();
        wait_any(&[], Duration::from_millis(50));
        let e = t.elapsed();
        assert!(e >= Duration::from_millis(40) && e < Duration::from_secs(1), "{e:?}");
    }

    #[test]
    fn an_unreadable_console_is_not_a_prompt() {
        // No such process: the mode is unknown, which the watcher reads as no prompt.
        let got = crate::platform::pty::console_mode(0xFFFF_FFF0);
        assert!(got.is_err(), "{got:?}");
        assert!(!got.is_ok_and(crate::pwm_prompt::at_prompt));
        let w = crate::jobwatch::JobWatch::start(std::sync::Arc::new(|| {}));
        assert!(!w.password_input(0xFFFF_FFF0), "an unwatched shell is not at a prompt");
    }

    /// A paused local console (Undo Close, or output held for an inline file question) is
    /// never read: it is left out of each look, and one paused after the look is refused under
    /// `CONSOLE` by `console_mode_unless`.
    #[test]
    fn a_paused_console_is_not_read() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (running, paused) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(true)));
        let consoles: HashMap<u32, Arc<AtomicBool>> = [(1, running.clone()), (2, paused)].into_iter().collect();
        let ids = |c: &HashMap<u32, Arc<AtomicBool>>| console_targets(c).into_iter().map(|t| t.0).collect::<Vec<_>>();
        assert_eq!(ids(&consoles), vec![1]);
        let targets = console_targets(&consoles);
        running.store(true, Ordering::Release);
        assert_eq!(crate::platform::pty::console_mode_unless(targets[0].0, &targets[0].1), Err("the console is paused".to_string()));
    }

    #[test]
    fn real_programs_are_told_gui_from_console() {
        assert!(crate::jobwatch::is_gui_exe(r"C:\Windows\notepad.exe"));
        assert!(!crate::jobwatch::is_gui_exe(r"C:\Windows\System32\cmd.exe"));
    }
}
