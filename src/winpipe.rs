//! Named pipes for the session host: the per-user pipe name, a DACL that lets only this
//! user in, overlapped I/O (so one thread can read while another writes on the same pipe),
//! and the check that the pipe's server runs as this user.

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    HLOCAL, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW, GetSecurityInfo, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::{GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile,
    SECURITY_IDENTIFICATION, SECURITY_SQOS_PRESENT, WriteFile,
};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION, WaitForSingleObject,
};

/// A connected pipe end. Reads and writes may run on different threads at once.
pub struct Pipe {
    h: usize,
    closed: AtomicBool,
    /// I/O holds it shared; closing the handle holds it alone, so no I/O ever uses a closed
    /// (and maybe reused) handle value.
    gate: std::sync::RwLock<bool>,
}

/// The listening side: one pipe instance always waits, so the name is never free.
pub struct Listener {
    name: String,
    waiting: usize,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn last_error(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

/// A handle closed when dropped.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn token_sid(token: HANDLE) -> Result<String, String> {
    unsafe {
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut len);
        // u64 cells keep TOKEN_USER aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        if GetTokenInformation(token, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) == 0 {
            return Err(last_error("GetTokenInformation"));
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        sid_text(user.User.Sid)
    }
}

/// A SID as text ("S-1-5-21-...").
unsafe fn sid_text(sid: PSID) -> Result<String, String> {
    unsafe {
        let mut text: *mut u16 = null_mut();
        if ConvertSidToStringSidW(sid, &mut text) == 0 {
            return Err(last_error("ConvertSidToStringSidW"));
        }
        let mut n = 0;
        while *text.add(n) != 0 {
            n += 1;
        }
        let sid = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text as HLOCAL);
        Ok(sid)
    }
}

/// The owner SID of an open file or other kernel object, as text. The handle needs
/// `READ_CONTROL`.
pub fn file_owner_sid(handle: HANDLE) -> Result<String, String> {
    unsafe {
        let mut owner: PSID = null_mut();
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        let err = GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        );
        if err != 0 {
            return Err(format!("GetSecurityInfo failed: {}", std::io::Error::from_raw_os_error(err as i32)));
        }
        let text = sid_text(owner);
        LocalFree(sd as HLOCAL);
        text
    }
}

/// Makes this user the owner of an open file. An elevated process's new files are owned by
/// BUILTIN\Administrators; the user's own SID is always a valid owner for its token. The handle
/// needs `WRITE_OWNER`.
pub fn set_file_owner_to_user(handle: HANDLE) -> Result<(), String> {
    let text = wide(&user_sid()?);
    unsafe {
        let mut sid: PSID = null_mut();
        if ConvertStringSidToSidW(text.as_ptr(), &mut sid) == 0 {
            return Err(last_error("ConvertStringSidToSidW"));
        }
        let err = SetSecurityInfo(handle, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, sid, null_mut(), null(), null());
        LocalFree(sid as HLOCAL);
        if err != 0 {
            return Err(format!("SetSecurityInfo failed: {}", std::io::Error::from_raw_os_error(err as i32)));
        }
    }
    Ok(())
}

/// This user's SID as text ("S-1-5-21-...").
pub fn user_sid() -> Result<String, String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        let token = Owned(token);
        token_sid(token.0)
    }
}

/// Whether this process runs elevated (as administrator).
pub fn elevated() -> Result<bool, String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        let token = Owned(token);
        token_elevated(token.0)
    }
}

fn token_elevated(token: HANDLE) -> Result<bool, String> {
    let mut e = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    if unsafe { GetTokenInformation(token, TokenElevation, (&mut e as *mut TOKEN_ELEVATION).cast(), size_of::<TOKEN_ELEVATION>() as u32, &mut len) } == 0 {
        return Err(last_error("GetTokenInformation(TokenElevation)"));
    }
    Ok(e.TokenIsElevated != 0)
}

/// The token of process `pid`, for queries.
fn process_token(pid: u32) -> Result<Owned, String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Err(last_error("OpenProcess"));
        }
        let process = Owned(process);
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(process.0, TOKEN_QUERY, &mut token) == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        Ok(Owned(token))
    }
}

/// The SID of the user process `pid` runs as, as text.
pub fn process_sid(pid: u32) -> Result<String, String> {
    token_sid(process_token(pid)?.0)
}

/// Whether process `pid` runs elevated.
pub fn process_elevated(pid: u32) -> Result<bool, String> {
    token_elevated(process_token(pid)?.0)
}

/// `\\.\pipe\tandem-host-<user SID>`.
pub fn host_pipe_name() -> Result<String, String> {
    // An elevated Tandem has its own host: normal and elevated shells never mix.
    Ok(format!(r"\\.\pipe\tandem-host-{}{}", user_sid()?, if elevated()? { "-admin" } else { "" }))
}

/// A pipe instance whose DACL lets only this user connect; no remote clients.
fn create_instance(name: &str, first: bool) -> Result<HANDLE, String> {
    unsafe {
        // Elevated: a high mandatory label, so a normal-integrity process cannot open it.
        let label = if elevated()? { "S:(ML;;NWNRNX;;;HI)" } else { "" };
        let sddl = wide(&format!("D:P(A;;GA;;;{}){label}", user_sid()?));
        let mut sd: PSECURITY_DESCRIPTOR = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, null_mut()) == 0 {
            return Err(last_error("ConvertStringSecurityDescriptorToSecurityDescriptorW"));
        }
        let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd, bInheritHandle: 0 };
        let open = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let mode = PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS;
        let h = CreateNamedPipeW(wide(name).as_ptr(), open, mode, PIPE_UNLIMITED_INSTANCES, 64 * 1024, 64 * 1024, 0, &sa);
        let err = std::io::Error::last_os_error();
        LocalFree(sd as HLOCAL);
        if h == INVALID_HANDLE_VALUE {
            return Err(format!("could not create the pipe {name}: {err}"));
        }
        Ok(h)
    }
}

/// One overlapped operation with its own event, waited for to the end.
struct Op {
    ov: OVERLAPPED,
    event: Owned,
}

impl Op {
    fn new() -> Result<Op, String> {
        let ev = unsafe { CreateEventW(null(), 1, 0, null()) };
        if ev.is_null() {
            return Err(last_error("CreateEventW"));
        }
        let mut ov: OVERLAPPED = unsafe { std::mem::zeroed() };
        ov.hEvent = ev;
        Ok(Op { ov, event: Owned(ev) })
    }
}

impl Listener {
    /// Creates the first instance of `name`. Fails when another process owns the name.
    pub fn bind(name: &str) -> Result<Listener, String> {
        Ok(Listener { name: name.to_string(), waiting: create_instance(name, true)? as usize })
    }

    /// Replaces the waiting instance (after an error on it).
    pub fn renew(&mut self) -> Result<(), String> {
        let fresh = create_instance(&self.name, false)?;
        unsafe { CloseHandle(self.waiting as HANDLE) };
        self.waiting = fresh as usize;
        Ok(())
    }

    /// Waits up to `wait` for a client. None when none came.
    pub fn accept(&mut self, wait: Duration) -> Result<Option<Pipe>, String> {
        let h = self.waiting as HANDLE;
        let mut op = Op::new()?;
        unsafe {
            let connected = if ConnectNamedPipe(h, &mut op.ov) != 0 {
                true
            } else {
                match GetLastError() {
                    ERROR_PIPE_CONNECTED => true,
                    ERROR_IO_PENDING => {
                        if WaitForSingleObject(op.event.0, wait.as_millis().min(u32::MAX as u128 - 1) as u32) == WAIT_OBJECT_0 {
                            let mut n = 0u32;
                            if GetOverlappedResult(h, &op.ov, &mut n, 0) == 0 {
                                return Err(last_error("ConnectNamedPipe"));
                            }
                            true
                        } else {
                            // The wait must end before `op` goes: cancel it and let it finish.
                            CancelIoEx(h, &op.ov);
                            let mut n = 0u32;
                            GetOverlappedResult(h, &op.ov, &mut n, 1);
                            false
                        }
                    }
                    // A client came and went before the connect: take the next one.
                    ERROR_NO_DATA => {
                        windows_sys::Win32::System::Pipes::DisconnectNamedPipe(h);
                        false
                    }
                    _ => return Err(last_error("ConnectNamedPipe")),
                }
            };
            if !connected {
                return Ok(None);
            }
        }
        // A new instance waits before this one is handed out, so the name stays taken.
        self.waiting = create_instance(&self.name, false)? as usize;
        Ok(Some(Pipe::new(h)))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.waiting as HANDLE) };
    }
}

/// Connects to `name`. None when nothing listens there.
pub fn connect(name: &str, wait: Duration) -> Result<Option<Pipe>, String> {
    let w = wide(name);
    let end = std::time::Instant::now() + wait;
    loop {
        let h = unsafe {
            CreateFileW(w.as_ptr(), GENERIC_READ | GENERIC_WRITE, 0, null(), OPEN_EXISTING, FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION, null_mut())
        };
        if h != INVALID_HANDLE_VALUE {
            return Ok(Some(Pipe::new(h)));
        }
        match unsafe { GetLastError() } {
            ERROR_FILE_NOT_FOUND => return Ok(None),
            ERROR_PIPE_BUSY => {
                let left = end.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    return Err(format!("the pipe {name} stayed busy"));
                }
                unsafe { WaitNamedPipeW(w.as_ptr(), left.as_millis().clamp(1, u32::MAX as u128 - 1) as u32) };
            }
            _ => return Err(last_error(&format!("connecting to {name}"))),
        }
    }
}

impl Pipe {
    fn new(h: HANDLE) -> Pipe {
        Pipe { h: h as usize, closed: AtomicBool::new(false), gate: std::sync::RwLock::new(true) }
    }

    fn handle(&self) -> HANDLE {
        self.h as HANDLE
    }

    /// Reads what is there (blocking). 0 means the other end closed.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, String> {
        let open = self.gate.read().unwrap();
        if !*open || self.closed.load(Ordering::Acquire) {
            return Ok(0);
        }
        let mut op = Op::new()?;
        let mut n = 0u32;
        unsafe {
            if ReadFile(self.handle(), buf.as_mut_ptr(), buf.len() as u32, null_mut(), &mut op.ov) == 0 && GetLastError() != ERROR_IO_PENDING {
                return closed_or(GetLastError(), "ReadFile");
            }
            if GetOverlappedResult(self.handle(), &op.ov, &mut n, 1) == 0 {
                return closed_or(GetLastError(), "ReadFile");
            }
        }
        Ok(n as usize)
    }

    pub fn write_all(&self, bytes: &[u8]) -> Result<(), String> {
        let open = self.gate.read().unwrap();
        let mut done = 0;
        while done < bytes.len() {
            if !*open || self.closed.load(Ordering::Acquire) {
                return Err("the pipe is closed".into());
            }
            let mut op = Op::new()?;
            let mut n = 0u32;
            let rest = &bytes[done..];
            unsafe {
                if WriteFile(self.handle(), rest.as_ptr(), rest.len() as u32, null_mut(), &mut op.ov) == 0 && GetLastError() != ERROR_IO_PENDING {
                    return Err(last_error("WriteFile"));
                }
                if GetOverlappedResult(self.handle(), &op.ov, &mut n, 1) == 0 {
                    return Err(last_error("WriteFile"));
                }
            }
            done += n as usize;
        }
        Ok(())
    }

    /// Ends every read and write in progress and closes the handle, so the other end sees
    /// the pipe close; later reads give 0 and writes fail.
    pub fn shutdown(&self) {
        // Only the first call closes; a later one must not touch a handle value that may
        // belong to something else by now.
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        loop {
            unsafe { CancelIoEx(self.handle(), null()) };
            if let Ok(mut open) = self.gate.try_write() {
                if *open {
                    unsafe { CloseHandle(self.handle()) };
                    *open = false;
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Ok when the process serving this pipe runs as this user.
    pub fn server_is_this_user(&self) -> Result<(), String> {
        let open = self.gate.read().unwrap();
        if !*open {
            return Err("the pipe is closed".into());
        }
        let mut pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(self.handle(), &mut pid) } == 0 {
            return Err(last_error("GetNamedPipeServerProcessId"));
        }
        let theirs = process_sid(pid)?;
        let mine = user_sid()?;
        if theirs != mine {
            return Err(format!("the session host pipe is served by another user ({theirs})"));
        }
        if process_elevated(pid)? != elevated()? {
            return Err("the session host runs at another elevation than this Tandem".into());
        }
        Ok(())
    }
}

fn closed_or(code: u32, what: &str) -> Result<usize, String> {
    match code {
        ERROR_BROKEN_PIPE | ERROR_PIPE_NOT_CONNECTED | ERROR_OPERATION_ABORTED | ERROR_NO_DATA => Ok(0),
        _ => Err(format!("{what} failed: {}", std::io::Error::from_raw_os_error(code as i32))),
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        if *self.gate.get_mut().unwrap() {
            unsafe { CloseHandle(self.handle()) };
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) fn test_name(what: &str) -> String {
        static N: AtomicUsize = AtomicUsize::new(0);
        format!(r"\\.\pipe\tandem-test-{what}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
    }

    fn pair() -> (Pipe, Pipe) {
        let name = test_name("pair");
        let mut l = Listener::bind(&name).unwrap();
        let n2 = name.clone();
        let t = std::thread::spawn(move || connect(&n2, Duration::from_secs(5)).unwrap().unwrap());
        let server = l.accept(Duration::from_secs(5)).unwrap().expect("a client");
        (server, t.join().unwrap())
    }

    #[test]
    fn pipes_carry_bytes_both_ways() {
        let (server, client) = pair();
        let server = Arc::new(server);
        // A read waits on the server while the server writes: overlapped I/O lets both run.
        let s2 = server.clone();
        let reader = std::thread::spawn(move || {
            let mut buf = [0u8; 64];
            let n = s2.read(&mut buf).unwrap();
            buf[..n].to_vec()
        });
        std::thread::sleep(Duration::from_millis(200));
        server.write_all(b"from server").unwrap();
        let mut buf = [0u8; 64];
        let n = client.read(&mut buf).unwrap();
        assert_eq!(&buf[..n], b"from server");
        client.write_all(b"from client").unwrap();
        assert_eq!(reader.join().unwrap(), b"from client");
        drop(client);
        assert_eq!(server.read(&mut buf).unwrap(), 0, "the other end closed");
    }

    #[test]
    fn shutdown_ends_a_waiting_read() {
        let (server, _client) = pair();
        let server = Arc::new(server);
        let s2 = server.clone();
        let reader = std::thread::spawn(move || s2.read(&mut [0u8; 8]));
        std::thread::sleep(Duration::from_millis(200));
        server.shutdown();
        assert!(reader.join().unwrap().map_or(true, |n| n == 0));
    }

    #[test]
    fn pipe_names_follow_elevation() {
        let admin = elevated().unwrap();
        assert_eq!(host_pipe_name().unwrap().ends_with("-admin"), admin, "an elevated Tandem never shares a host with a normal one");
    }

    #[test]
    fn the_host_runs_as_us() {
        assert!(user_sid().unwrap().starts_with("S-1-5-"));
        assert!(host_pipe_name().unwrap().starts_with(r"\\.\pipe\tandem-host-S-1-5-"));
        let (_server, client) = pair();
        client.server_is_this_user().unwrap();
    }

    #[test]
    fn a_second_listener_is_refused() {
        let name = test_name("twice");
        let _first = Listener::bind(&name).unwrap();
        assert!(Listener::bind(&name).is_err());
    }

    #[test]
    fn no_listener_is_none() {
        assert!(connect(&test_name("absent"), Duration::from_millis(100)).unwrap().is_none());
    }

    #[test]
    fn accept_times_out_quietly() {
        let mut l = Listener::bind(&test_name("quiet")).unwrap();
        assert!(l.accept(Duration::from_millis(100)).unwrap().is_none());
    }
}
