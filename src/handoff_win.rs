//! The `ITerminalHandoff3` COM server (m13v1 Task 5): Windows hands a console program's console to
//! the default terminal through this class. Any local program can call it, so every call is
//! untrusted: the caller is checked first (`default_term::vet`: same user, session and integrity), handles are duplicated and typed
//! before use, the title is bounded and cleaned, the rate and the live count are capped, and a
//! handoff while Termurion closes is refused. Nothing here touches `App`: a good handoff becomes a
//! `Handoff` sent to the UI thread, which owns its handles from then on.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Instant;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoImpersonateClient, CoInitializeEx, CoRegisterClassObject, CoRevertToSelf, CoRevokeClassObject, CoUninitialize, IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE,
};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, Ref, implement, interface};
use windows_sys::Win32::Foundation::{CloseHandle, DuplicateHandle as DupHandle, DUPLICATE_SAME_ACCESS, GetLastError, SysStringLen, WAIT_FAILED};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_UNKNOWN, GetFileType};
use windows_sys::Win32::System::Console::HPCON;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetProcessId, WaitForSingleObject};

use crate::conpty::{CloseFn, PackFn};
use crate::default_term::{Args, Gate, Refusal, vet};

/// `CLSID_TERMURION_TERMINAL` as a GUID.
const CLASS: GUID = GUID::from_u128(0x6A1E8D3C_2B7F_4C9E_9D41_5F0B7A2C8E13);

#[interface("6F23DA90-15C5-4203-9DB0-64E73F1B1B00")]
pub unsafe trait ITerminalHandoff3: IUnknown {
    fn EstablishPtyHandoff(&self, input: *mut HANDLE, output: *mut HANDLE, signal: HANDLE, reference: HANDLE, server: HANDLE, client: HANDLE, startup: *const TerminalStartupInfo) -> HRESULT;
}

/// The IDL's `TERMINAL_STARTUP_INFO`. The two BSTRs belong to the caller: they are raw pointers,
/// read through `SysStringLen` and copied, never freed.
#[repr(C)]
pub struct TerminalStartupInfo {
    pub title: *const u16,
    pub icon_path: *const u16,
    pub icon_index: i32,
    pub x: u32,
    pub y: u32,
    pub x_size: u32,
    pub y_size: u32,
    pub x_count_chars: u32,
    pub y_count_chars: u32,
    pub fill_attribute: u32,
    pub flags: u32,
    pub show_window: u16,
}

/// One slot of the live-pane count, given back when the pane (or an unused handoff) is dropped.
#[derive(Debug)]
pub struct Live(Arc<AtomicUsize>);

impl Drop for Live {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A console handed to Termurion: raw handles Termurion now owns. Dropping an unused one closes them.
#[derive(Debug)]
pub struct Handoff {
    pub hpc: isize,
    pub input: isize,
    pub output: isize,
    pub client: isize,
    /// Cleaned and bounded; empty when the caller's was missing or unsafe (use the profile's name).
    pub title: String,
    pub show_window: u16,
    close: CloseFn,
    live: Option<Live>,
}

impl Handoff {
    /// The handles and the live-count slot, for the pane that takes them; this value then closes nothing.
    pub fn take(&mut self) -> (isize, isize, isize, isize, Option<Live>) {
        let r = (self.hpc, self.input, self.output, self.client, self.live.take());
        (self.hpc, self.input, self.output, self.client) = (0, 0, 0, 0);
        r
    }
}

impl Drop for Handoff {
    fn drop(&mut self) {
        unsafe {
            if self.hpc != 0 {
                (self.close)(self.hpc as HPCON);
            }
            for h in [self.input, self.output, self.client] {
                if h != 0 {
                    CloseHandle(h as *mut c_void);
                }
            }
        }
    }
}

/// A handle this call owns until it is released (to the pseudoconsole or the caller).
struct Own(isize);

impl Own {
    fn dup(h: HANDLE) -> Result<Own, i32> {
        let mut copy: *mut c_void = null_mut();
        let ok = unsafe { DupHandle(GetCurrentProcess(), h.0, GetCurrentProcess(), &mut copy, 0, 0, DUPLICATE_SAME_ACCESS) };
        if ok == 0 { Err(Refusal::Handle.hresult()) } else { Ok(Own(copy as isize)) }
    }

    fn raw(&self) -> *mut c_void {
        self.0 as *mut c_void
    }

    fn release(&mut self) -> isize {
        std::mem::take(&mut self.0)
    }
}

impl Drop for Own {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe { CloseHandle(self.0 as *mut c_void) };
        }
    }
}

/// A file-like handle (the signal pipe, the console reference) must be valid and not a disk file.
fn check_file(h: &Own) -> Result<(), i32> {
    unsafe {
        windows_sys::Win32::Foundation::SetLastError(0);
        let t = GetFileType(h.raw());
        if t == FILE_TYPE_DISK || (t == FILE_TYPE_UNKNOWN && GetLastError() != 0) {
            return Err(Refusal::Handle.hresult());
        }
    }
    Ok(())
}

/// A process handle: it names a process, not this one; `need_wait` also demands the access
/// `Pty` waits with.
fn check_process(h: &Own, need_wait: bool) -> Result<(), i32> {
    unsafe {
        let pid = GetProcessId(h.raw());
        if pid == 0 || pid == GetCurrentProcessId() || (need_wait && WaitForSingleObject(h.raw(), 0) == WAIT_FAILED) {
            return Err(Refusal::Handle.hresult());
        }
    }
    Ok(())
}

struct Shared {
    send: Box<dyn Fn(Handoff) + Send + Sync>,
    pack: PackFn,
    close: CloseFn,
    gate: Mutex<Gate>,
    live: Arc<AtomicUsize>,
    /// This process's token and session: the caller must match them (`api_peer::judge_peer`).
    mine: crate::api_peer::Peer,
    /// Calls inside `EstablishPtyHandoff` right now: the server's shutdown waits for them (round 5
    /// race A; `CoRevokeClassObject` does not wait for calls in flight).
    inflight: Arc<AtomicUsize>,
}

/// The longest a server shutdown waits for calls in flight.
const INFLIGHT_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

/// One call in flight, counted from its first step until it returns (any way).
struct InFlight(Arc<AtomicUsize>);

impl InFlight {
    fn enter(calls: &Arc<AtomicUsize>) -> InFlight {
        calls.fetch_add(1, Ordering::SeqCst);
        InFlight(calls.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Waits until no call is in flight, at most `limit`; false when one still is.
fn wait_idle(calls: &AtomicUsize, limit: std::time::Duration) -> bool {
    let end = Instant::now() + limit;
    while calls.load(Ordering::SeqCst) != 0 {
        if Instant::now() >= end {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    true
}

/// What a COM entry point returns when it panicked.
const E_UNEXPECTED: i32 = 0x8000_FFFFu32 as i32;

/// Runs a COM entry point's body; a panic becomes `E_UNEXPECTED` for the caller instead of unwinding
/// into COM, which would abort Termurion with every session (final review item 13, 13u's rule for
/// callbacks on foreign threads).
fn unwind_safe<T>(body: impl FnOnce() -> Result<T, i32>) -> Result<T, i32> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(body)).unwrap_or(Err(E_UNEXPECTED))
}

/// Set once Termurion is ending: a handoff after that is refused with `E_ABORT`.
static CLOSING: AtomicBool = AtomicBool::new(false);

pub fn begin_closing() {
    CLOSING.store(true, Ordering::SeqCst);
}

/// The token facts and session of whoever is calling, read from the impersonated thread token;
/// None when COM cannot impersonate (no call in progress) or the token is unreadable.
fn caller_peer() -> Option<crate::api_peer::Peer> {
    unsafe {
        CoImpersonateClient().ok()?;
        let peer = crate::platform::ipc::thread_peer().ok();
        let _ = CoRevertToSelf();
        peer
    }
}

/// The title's UTF-16 units, at most `MAX_TITLE_UNITS` of them, from a caller-owned BSTR.
unsafe fn bstr_units(p: *const u16) -> Vec<u16> {
    if p.is_null() {
        return Vec::new();
    }
    unsafe {
        let n = (SysStringLen(p as *mut u16) as usize).min(crate::default_term::MAX_TITLE_UNITS + 1);
        std::slice::from_raw_parts(p, n).to_vec()
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn establish(sh: &Shared, input: *mut HANDLE, output: *mut HANDLE, signal: HANDLE, reference: HANDLE, server: HANDLE, client: HANDLE, startup: *const TerminalStartupInfo) -> Result<(), i32> {
    unsafe {
        if !input.is_null() {
            *input = HANDLE(null_mut());
        }
        if !output.is_null() {
            *output = HANDLE(null_mut());
        }
        let caller = caller_peer();
        let args = Args { signal: signal.0 as isize, reference: reference.0 as isize, server: server.0 as isize, client: client.0 as isize, outputs: !input.is_null() && !output.is_null(), startup: !startup.is_null() };
        vet(caller.as_ref(), &sh.mine, &args, &mut sh.gate.lock().unwrap_or_else(|e| e.into_inner()), Instant::now(), sh.live.load(Ordering::SeqCst), CLOSING.load(Ordering::SeqCst)).map_err(Refusal::hresult)?;
        // Our own copies: the originals go back to COM when the call returns.
        let (mut sig, mut rf, mut srv, mut cli) = (Own::dup(signal)?, Own::dup(reference)?, Own::dup(server)?, Own::dup(client)?);
        check_file(&sig)?;
        check_file(&rf)?;
        check_process(&srv, false)?;
        check_process(&cli, true)?;
        let title = crate::default_term::clean_title(&bstr_units((*startup).title)).unwrap_or_default();
        let show_window = (*startup).show_window;
        let (mut in_read, mut in_write, mut out_read, mut out_write) = (null_mut(), null_mut(), null_mut(), null_mut());
        if CreatePipe(&mut in_read, &mut in_write, std::ptr::null(), 0) == 0 {
            return Err(Refusal::Handle.hresult());
        }
        let (in_read, in_write) = (Own(in_read as isize), Own(in_write as isize));
        if CreatePipe(&mut out_read, &mut out_write, std::ptr::null(), 0) == 0 {
            return Err(Refusal::Handle.hresult());
        }
        let (out_read, out_write) = (Own(out_read as isize), Own(out_write as isize));
        let mut hpc: HPCON = 0;
        let hr = (sh.pack)(srv.raw(), rf.raw(), sig.raw(), &mut hpc);
        if hr < 0 || hpc == 0 {
            return Err(if hr < 0 { hr } else { Refusal::Handle.hresult() });
        }
        // The pseudoconsole owns the three handles now.
        srv.release();
        rf.release();
        sig.release();
        let (mut in_read, mut in_write, mut out_read, mut out_write) = (in_read, in_write, out_read, out_write);
        let handoff = Handoff { hpc, input: in_write.release(), output: out_read.release(), client: cli.release(), title, show_window, close: sh.close, live: None };
        let mut handoff = handoff;
        sh.live.fetch_add(1, Ordering::SeqCst);
        handoff.live = Some(Live(sh.live.clone()));
        // The console's ends go back to COM, which marshals and closes them.
        *input = HANDLE(in_read.release() as *mut c_void);
        *output = HANDLE(out_write.release() as *mut c_void);
        (sh.send)(handoff);
        Ok(())
    }
}

#[implement(ITerminalHandoff3)]
struct Handler {
    shared: Arc<Shared>,
}

impl ITerminalHandoff3_Impl for Handler_Impl {
    unsafe fn EstablishPtyHandoff(&self, input: *mut HANDLE, output: *mut HANDLE, signal: HANDLE, reference: HANDLE, server: HANDLE, client: HANDLE, startup: *const TerminalStartupInfo) -> HRESULT {
        let _call = InFlight::enter(&self.shared.inflight);
        match unwind_safe(|| unsafe { establish(&self.shared, input, output, signal, reference, server, client, startup) }) {
            Ok(()) => HRESULT(0),
            Err(hr) => HRESULT(hr),
        }
    }
}

#[implement(IClassFactory)]
struct Factory {
    shared: Arc<Shared>,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, outer: Ref<IUnknown>, iid: *const GUID, object: *mut *mut c_void) -> windows::core::Result<()> {
        let made = unwind_safe(|| {
            if !outer.is_null() {
                return Err(windows::Win32::Foundation::CLASS_E_NOAGGREGATION.0);
            }
            let unknown: IUnknown = Handler { shared: self.shared.clone() }.into();
            unsafe { unknown.query(iid, object) }.ok().map_err(|e| e.code().0)
        });
        made.map_err(|hr| HRESULT(hr).into())
    }

    fn LockServer(&self, _lock: BOOL) -> windows::core::Result<()> {
        unwind_safe(|| Ok(())).map_err(|hr: i32| HRESULT(hr).into())
    }
}

/// The registered class. Dropping it revokes the class and ends its thread.
pub struct Server {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
    shared: Arc<Shared>,
}

impl Server {
    /// Registers `CLSID_TERMURION_TERMINAL` from a dedicated multithreaded-apartment thread, so
    /// calls arrive on COM's RPC threads and never through winit's message loop. `send` gets
    /// every good handoff; `close` is the api's `ConptyClosePseudoConsole`.
    pub fn register(send: Box<dyn Fn(Handoff) + Send + Sync>, pack: PackFn, close: CloseFn) -> Result<Server, String> {
        let mine = crate::platform::ipc::own_peer()?;
        let shared = Arc::new(Shared { send, pack, close, gate: Mutex::new(Gate::default()), live: Arc::new(AtomicUsize::new(0)), mine, inflight: Arc::new(AtomicUsize::new(0)) });
        let (stop, wait) = mpsc::channel::<()>();
        let (done, result) = mpsc::channel::<Result<(), String>>();
        let for_thread = shared.clone();
        let thread = std::thread::Builder::new()
            .name("handoff-com".into())
            .spawn(move || unsafe {
                if let Err(e) = CoInitializeEx(None, COINIT_MULTITHREADED).ok() {
                    return drop(done.send(Err(format!("CoInitializeEx: {e}"))));
                }
                let factory: IClassFactory = Factory { shared: for_thread }.into();
                match CoRegisterClassObject(&CLASS, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE) {
                    Ok(cookie) => {
                        let _ = done.send(Ok(()));
                        let _ = wait.recv();
                        let _ = CoRevokeClassObject(cookie);
                    }
                    Err(e) => {
                        let _ = done.send(Err(format!("CoRegisterClassObject: {e}")));
                    }
                }
                drop(factory);
                CoUninitialize();
            })
            .map_err(|e| e.to_string())?;
        match result.recv() {
            Ok(Ok(())) => Ok(Server { stop: Some(stop), thread: Some(thread), shared }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("the handoff thread ended before it registered".into()),
        }
    }

    /// Refused calls since the last time, for one banner.
    pub fn take_ignored(&self) -> usize {
        self.shared.gate.lock().unwrap_or_else(|e| e.into_inner()).take_ignored()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // The class is revoked; a call already inside has sent its handoff when this returns.
        if !wait_idle(&self.shared.inflight, INFLIGHT_LIMIT) {
            crate::host_base::log("handoff: a call was still in flight when the class closed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    /// The production text of this file.
    fn production() -> &'static str {
        include_str!("handoff_win.rs").split("\n#[cfg(test)]").next().unwrap()
    }

    /// Round 5 race A: a call already inside EstablishPtyHandoff when the class is revoked is waited
    /// for (bounded), so a stopping `-Embedding` process has queued the handoff before it ends.
    #[test]
    fn a_call_in_flight_holds_the_server_shutdown() {
        let src = production();
        let est = &src[src.find("unsafe fn EstablishPtyHandoff(").unwrap()..];
        let est = &est[..est.find("\n    }\n").unwrap()];
        let enter = est.find("InFlight::enter(&self.shared.inflight)").expect("the call counts itself first");
        assert!(enter < est.find("establish(").unwrap());
        let drop = &src[src.find("impl Drop for Server").unwrap()..];
        let drop = &drop[..drop.find("\n}\n").unwrap()];
        assert!(drop.find("t.join()").unwrap() < drop.find("wait_idle(&self.shared.inflight, INFLIGHT_LIMIT)").expect("waits for the calls in flight"), "after the revoke");
    }

    /// Final review item 13 (13u: a callback on a foreign thread never panics through it): a panic in a
    /// COM entry point is E_UNEXPECTED to the caller, never an abort of Termurion with every session.
    #[test]
    fn a_panic_in_a_com_entry_point_is_an_error() {
        assert_eq!(unwind_safe(|| -> Result<(), i32> { panic!("boom") }), Err(E_UNEXPECTED));
        assert_eq!(unwind_safe(|| -> Result<u8, i32> { Ok(7) }), Ok(7));
        let src = production();
        for f in ["unsafe fn EstablishPtyHandoff(", "fn CreateInstance(", "fn LockServer("] {
            let b = &src[src.find(f).unwrap_or_else(|| panic!("{f}"))..];
            let b = &b[..b.find("\n    }\n").unwrap()];
            assert!(b.contains("unwind_safe("), "{f} catches a panic");
        }
    }

    /// The wait itself: it ends when the last call in flight leaves, and is bounded.
    #[test]
    fn the_shutdown_waits_for_calls_in_flight_bounded() {
        let calls = Arc::new(AtomicUsize::new(0));
        let held = InFlight::enter(&calls);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let t = Instant::now();
        assert!(!wait_idle(&calls, std::time::Duration::from_millis(100)), "a call that never leaves is reported");
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "and the wait is bounded");
        let leaving = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(150));
            drop(held);
        });
        let t = Instant::now();
        assert!(wait_idle(&calls, std::time::Duration::from_secs(2)));
        assert!(t.elapsed() >= std::time::Duration::from_millis(100), "it waited for the call");
        leaving.join().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn startup_info_matches_the_idl() {
        assert_eq!(size_of::<TerminalStartupInfo>(), 56);
        let offsets = [
            offset_of!(TerminalStartupInfo, title),
            offset_of!(TerminalStartupInfo, icon_path),
            offset_of!(TerminalStartupInfo, icon_index),
            offset_of!(TerminalStartupInfo, x),
            offset_of!(TerminalStartupInfo, y),
            offset_of!(TerminalStartupInfo, x_size),
            offset_of!(TerminalStartupInfo, y_size),
            offset_of!(TerminalStartupInfo, x_count_chars),
            offset_of!(TerminalStartupInfo, y_count_chars),
            offset_of!(TerminalStartupInfo, fill_attribute),
            offset_of!(TerminalStartupInfo, flags),
            offset_of!(TerminalStartupInfo, show_window),
        ];
        assert_eq!(offsets, [0, 8, 16, 20, 24, 28, 32, 36, 40, 44, 48, 52]);
    }

    #[test]
    fn the_interface_and_class_ids_are_windows_terminals() {
        assert_eq!(ITerminalHandoff3::IID, GUID::from_u128(0x6F23DA90_15C5_4203_9DB0_64E73F1B1B00));
        assert_eq!(CLASS, GUID::from_u128(0x6A1E8D3C_2B7F_4C9E_9D41_5F0B7A2C8E13));
        assert_eq!(format!("{{{CLASS:?}}}"), crate::default_term::CLSID_TERMURION_TERMINAL);
    }

    fn pipe() -> (Own, Own) {
        let (mut r, mut w) = (null_mut(), null_mut());
        assert_ne!(unsafe { CreatePipe(&mut r, &mut w, std::ptr::null(), 0) }, 0);
        (Own(r as isize), Own(w as isize))
    }

    /// B9: a handle is typed before use. A disk file and a bad handle are refused as the
    /// signal or reference, a pipe passes, and only a process passes as the server or client.
    #[test]
    fn handles_are_typed_before_use() {
        let (r, _w) = pipe();
        assert!(check_file(&r).is_ok());
        let path = std::env::temp_dir().join(format!("termurion-handoff-{}", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        let f = std::fs::File::open(&path).unwrap();
        use std::os::windows::io::AsRawHandle;
        let disk = Own::dup(HANDLE(f.as_raw_handle())).unwrap();
        assert!(check_file(&disk).is_err(), "a disk file is not a console handle");
        assert!(check_process(&disk, false).is_err(), "a file is not a process");
        assert!(check_process(&r, false).is_err(), "a pipe is not a process");
        let _ = std::fs::remove_file(&path);
        let me = Own::dup(HANDLE(unsafe { GetCurrentProcess() })).unwrap();
        assert!(check_process(&me, true).is_err(), "Termurion itself is never the client");
        assert!(Own::dup(HANDLE(0x7777_7770 as *mut c_void)).is_err(), "a handle that is not there");
    }

    /// An unused handoff closes its handles when it is dropped, and gives its live slot back.
    #[test]
    fn an_unused_handoff_closes_what_it_owns() {
        unsafe extern "system" fn close(_: HPCON) {
            CLOSED.fetch_add(1, Ordering::SeqCst);
        }
        static CLOSED: AtomicUsize = AtomicUsize::new(0);
        let live = Arc::new(AtomicUsize::new(1));
        let (r, w) = pipe();
        let (rr, ww) = (r.0, w.0);
        let mut h = Handoff { hpc: 7, input: rr, output: ww, client: 0, title: String::new(), show_window: 0, close, live: Some(Live(live.clone())) };
        std::mem::forget((r, w));
        let _ = &mut h;
        drop(h);
        assert_eq!(CLOSED.load(Ordering::SeqCst), 1);
        assert_eq!(live.load(Ordering::SeqCst), 0);
        // The handles are closed: a second close fails.
        assert_eq!(unsafe { CloseHandle(rr as *mut c_void) }, 0);
    }

    /// The pane that takes a handoff's handles leaves it nothing to close.
    #[test]
    fn taking_a_handoff_leaves_it_empty() {
        unsafe extern "system" fn close(_: HPCON) {
            panic!("closed a taken pseudoconsole");
        }
        let live = Arc::new(AtomicUsize::new(1));
        let mut h = Handoff { hpc: 7, input: 0, output: 0, client: 0, title: String::new(), show_window: 0, close, live: Some(Live(live.clone())) };
        let (hpc, _, _, _, slot) = h.take();
        assert_eq!(hpc, 7);
        drop(h);
        assert_eq!(live.load(Ordering::SeqCst), 1, "the slot went with the pane");
        drop(slot);
        assert_eq!(live.load(Ordering::SeqCst), 0);
    }
}
