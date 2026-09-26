//! Reads the process tree from Windows for `proctree`: a Toolhelp snapshot, creation
//! times, the GUI flag from each program's PE header, and command lines. A watcher thread
//! checks the panes' shells and wakes the window when a job changes.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::{GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};

use crate::proctree::{self, Job, Proc};

/// How often the watcher looks while Tandem is in front, and while it is not (iTerm2's
/// process cache rates).
pub const FAST: Duration = Duration::from_millis(500);
pub const SLOW: Duration = Duration::from_secs(5);

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open(pid: u32) -> Option<Handle> {
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    (!h.is_null()).then_some(Handle(h))
}

/// Every process's pid, parent, and image name.
fn toolhelp() -> Result<Vec<(u32, u32, String)>, String> {
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
            out.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..len])));
            ok = Process32NextW(snap.0, &mut e);
        }
        Ok(out)
    }
}

fn created(h: &Handle) -> u64 {
    let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let (mut c, mut x, mut k, mut u) = (z, z, z, z);
    if unsafe { GetProcessTimes(h.0, &mut c, &mut x, &mut k, &mut u) } == 0 {
        return 0;
    }
    ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64
}

fn image_path(h: &Handle) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    (unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) } != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// Whether a program is a Windows-subsystem (GUI) program, from its PE header.
pub fn is_gui_exe(path: &str) -> bool {
    use std::io::Read;
    let mut head = [0u8; 1024];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)).unwrap_or(0);
    subsystem(&head[..n]) == Some(2)
}

/// The PE optional header's Subsystem field (2 GUI, 3 console).
pub fn subsystem(head: &[u8]) -> Option<u16> {
    let at = |o: usize, n: usize| head.get(o..o + n);
    if at(0, 2)? != b"MZ" {
        return None;
    }
    let pe = u32::from_le_bytes(at(0x3C, 4)?.try_into().ok()?) as usize;
    if at(pe, 4)? != b"PE\0\0" {
        return None;
    }
    // The optional header starts 24 bytes in; Subsystem is at 68 for PE32 and PE32+.
    Some(u16::from_le_bytes(at(pe + 24 + 68, 2)?.try_into().ok()?))
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

/// Program facts kept between looks: the GUI flag by image path, and command lines by
/// (pid, creation time).
#[derive(Default)]
pub struct Cache {
    gui: HashMap<String, bool>,
    lines: HashMap<(u32, u64), Option<String>>,
}

/// The processes under the given shells, with creation times, GUI flags, and the command
/// lines of each shell's job. Only processes under a shell are opened.
pub fn snapshot(roots: &[u32], cache: &mut Cache) -> Result<Vec<Proc>, String> {
    let all = toolhelp()?;
    let mut kids: HashMap<u32, Vec<usize>> = HashMap::new();
    for (i, (pid, ppid, _)) in all.iter().enumerate() {
        if pid != ppid {
            kids.entry(*ppid).or_default().push(i);
        }
    }
    let mut want: Vec<usize> = all.iter().enumerate().filter(|(_, (pid, _, _))| roots.contains(pid)).map(|(i, _)| i).collect();
    let mut seen: HashSet<usize> = want.iter().copied().collect();
    let mut i = 0;
    while i < want.len() && want.len() < 4096 {
        for &k in kids.get(&all[want[i]].0).map(Vec::as_slice).unwrap_or(&[]) {
            if seen.insert(k) {
                want.push(k);
            }
        }
        i += 1;
    }
    let mut procs = Vec::with_capacity(want.len());
    for i in want {
        let (pid, ppid, ref name) = all[i];
        let h = open(pid);
        let made = h.as_ref().map_or(0, created);
        let gui = h.as_ref().and_then(image_path).is_some_and(|p| *cache.gui.entry(p.clone()).or_insert_with(|| is_gui_exe(&p)));
        procs.push(Proc { pid, ppid, name: name.clone(), created: made, gui, cmdline: None });
    }
    let mut live = HashSet::new();
    for &root in roots {
        let Some(job) = proctree::display_job(&procs, root) else { continue };
        let Some(p) = procs.iter_mut().find(|p| p.pid == job.pid) else { continue };
        let key = (p.pid, p.created);
        live.insert(key);
        if !cache.lines.contains_key(&key) {
            let line = open(p.pid).and_then(|h| command_line(&h));
            cache.lines.insert(key, line);
        }
        p.cmdline = cache.lines[&key].clone();
    }
    cache.lines.retain(|k, _| live.contains(k));
    Ok(procs)
}

/// What the window knows about one shell's job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellJob {
    pub job: Option<Job>,
    pub ancestors: Vec<String>,
    /// The shell's own name (`proctree::root_name`); None once it is gone.
    pub root: Option<String>,
}

#[derive(Default)]
struct Shared {
    roots: HashSet<u32>,
    jobs: HashMap<u32, ShellJob>,
    fast: bool,
    error: Option<String>,
}

/// The watcher: a thread that looks at the registered shells' trees and calls `notify`
/// when a job changes.
#[derive(Clone)]
pub struct JobWatch {
    shared: Arc<Mutex<Shared>>,
}

impl JobWatch {
    pub fn start(notify: Arc<dyn Fn() + Send + Sync>) -> JobWatch {
        let shared = Arc::new(Mutex::new(Shared { fast: true, ..Default::default() }));
        let weak = Arc::downgrade(&shared);
        std::thread::spawn(move || {
            let mut cache = Cache::default();
            loop {
                let Some(shared) = weak.upgrade() else { return };
                let (roots, fast) = {
                    let s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    (s.roots.iter().copied().collect::<Vec<u32>>(), s.fast)
                };
                drop(shared);
                if !roots.is_empty() {
                    let result = snapshot(&roots, &mut cache);
                    let Some(shared) = weak.upgrade() else { return };
                    let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                    let mut changed = false;
                    match result {
                        Ok(procs) => {
                            s.error = None;
                            for r in roots {
                                let now = ShellJob { job: proctree::display_job(&procs, r), ancestors: proctree::ancestors(&procs, r), root: proctree::root_name(&procs, r) };
                                if s.roots.contains(&r) && s.jobs.get(&r) != Some(&now) {
                                    s.jobs.insert(r, now);
                                    changed = true;
                                }
                            }
                        }
                        Err(e) => {
                            changed = s.error.as_ref() != Some(&e);
                            s.error = Some(e);
                        }
                    }
                    drop(s);
                    if changed {
                        notify();
                    }
                }
                std::thread::sleep(if fast { FAST } else { SLOW });
            }
        });
        JobWatch { shared }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn watch(&self, shell: u32) {
        self.lock().roots.insert(shell);
    }

    pub fn forget(&self, shell: u32) {
        let mut s = self.lock();
        s.roots.remove(&shell);
        s.jobs.remove(&shell);
    }

    /// Tandem in front: look every 0.5 s; else every 5 s.
    pub fn set_fast(&self, fast: bool) {
        self.lock().fast = fast;
    }

    pub fn job(&self, shell: u32) -> ShellJob {
        self.lock().jobs.get(&shell).cloned().unwrap_or_default()
    }

    /// The last snapshot error, shown once by the window.
    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }
}

/// Every process under a shell now (for the close prompt), with its name.
pub fn descendants_now(shell: u32) -> Result<Vec<Proc>, String> {
    let procs = snapshot(&[shell], &mut Cache::default())?;
    Ok(proctree::descendants(&procs, shell).into_iter().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pe_subsystem_reads_the_header() {
        let mut h = vec![0u8; 400];
        h[0..2].copy_from_slice(b"MZ");
        h[0x3C..0x40].copy_from_slice(&128u32.to_le_bytes());
        h[128..132].copy_from_slice(b"PE\0\0");
        h[128 + 24 + 68..128 + 24 + 70].copy_from_slice(&2u16.to_le_bytes());
        assert_eq!(subsystem(&h), Some(2));
        assert_eq!(subsystem(b"not a program"), None);
        assert!(is_gui_exe(r"C:\Windows\notepad.exe"));
        assert!(!is_gui_exe(r"C:\Windows\System32\cmd.exe"));
    }

    #[test]
    fn a_real_tree_has_a_job() {
        let mut child = std::process::Command::new("cmd.exe").args(["/d", "/c", "ping -n 4 127.0.0.1 >NUL"]).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let mut cache = Cache::default();
        let procs = snapshot(&[child.id()], &mut cache).unwrap();
        let job = proctree::display_job(&procs, child.id());
        let _ = child.kill();
        let _ = child.wait();
        let job = job.expect("the shell is there");
        assert_eq!(job.name.to_lowercase(), "ping", "{procs:?}");
        assert!(job.cmdline.to_lowercase().contains("127.0.0.1"), "{}", job.cmdline);
        assert_eq!(proctree::ancestors(&procs, child.id()), vec!["ping"]);
    }
}
