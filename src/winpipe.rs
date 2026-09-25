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
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
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
        let mut text: *mut u16 = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
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

/// `\\.\pipe\tandem-host-<user SID>`.
pub fn host_pipe_name() -> Result<String, String> {
    Ok(format!(r"\\.\pipe\tandem-host-{}", user_sid()?))
}

/// A pipe instance whose DACL lets only this user connect; no remote clients.
fn create_instance(name: &str, first: bool) -> Result<HANDLE, String> {
    unsafe {
        let sddl = wide(&format!("D:P(A;;GA;;;{})", user_sid()?));
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
        Ok(Some(Pipe { h: h as usize, closed: AtomicBool::new(false) }))
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
            return Ok(Some(Pipe { h: h as usize, closed: AtomicBool::new(false) }));
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
    fn handle(&self) -> HANDLE {
        self.h as HANDLE
    }

    /// Reads what is there (blocking). 0 means the other end closed.
    pub fn read(&self, buf: &mut [u8]) -> Result<usize, String> {
        if self.closed.load(Ordering::Acquire) {
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
        let mut done = 0;
        while done < bytes.len() {
            if self.closed.load(Ordering::Acquire) {
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

    /// Ends every read and write in progress; later ones fail.
    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        unsafe { CancelIoEx(self.handle(), null()) };
    }

    /// Ok when the process serving this pipe runs as this user.
    pub fn server_is_this_user(&self) -> Result<(), String> {
        unsafe {
            let mut pid = 0u32;
            if GetNamedPipeServerProcessId(self.handle(), &mut pid) == 0 {
                return Err(last_error("GetNamedPipeServerProcessId"));
            }
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return Err(last_error("OpenProcess"));
            }
            let process = Owned(process);
            let mut token: HANDLE = null_mut();
            if OpenProcessToken(process.0, TOKEN_QUERY, &mut token) == 0 {
                return Err(last_error("OpenProcessToken"));
            }
            let token = Owned(token);
            let theirs = token_sid(token.0)?;
            let mine = user_sid()?;
            if theirs != mine {
                return Err(format!("the session host pipe is served by another user ({theirs})"));
            }
            Ok(())
        }
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
        unsafe { CloseHandle(self.handle()) };
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
