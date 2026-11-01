//! Named pipes for the session host: the per-user pipe name, a DACL that lets only this
//! user in, overlapped I/O (so one thread can read while another writes on the same pipe),
//! and the check that the pipe's server runs as this user.

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_FILE_NOT_FOUND, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_OPERATION_ABORTED,
    ERROR_PIPE_BUSY, ERROR_PIPE_CONNECTED, ERROR_PIPE_NOT_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE,
    HLOCAL, INVALID_HANDLE_VALUE, LocalFree, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW, GetSecurityInfo, SDDL_REVISION_1,
    SE_FILE_OBJECT, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, IsValidSid, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_ELEVATION, TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TOKEN_USER, TokenElevation,
    TokenIntegrityLevel, TokenIsAppContainer, TokenIsRestricted, TokenSessionId, TokenUser,
};
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
pub(crate) unsafe fn sid_text(sid: PSID) -> Result<String, String> {
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
pub fn file_owner_sid(object: impl std::os::windows::io::AsHandle) -> Result<String, String> {
    use std::os::windows::io::AsRawHandle;
    let handle: HANDLE = object.as_handle().as_raw_handle();
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
pub fn set_file_owner_to_user(object: impl std::os::windows::io::AsHandle) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    let handle: HANDLE = object.as_handle().as_raw_handle();
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

/// The peer facts (token and Windows session) of the identity this thread runs as: after
/// `CoImpersonateClient`, the caller's. Read from one thread token opened as this process.
pub fn thread_peer() -> Result<crate::api_peer::Peer, String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if windows_sys::Win32::System::Threading::OpenThreadToken(windows_sys::Win32::System::Threading::GetCurrentThread(), TOKEN_QUERY, 1, &mut token) == 0 {
            return Err(last_error("OpenThreadToken"));
        }
        let token = Owned(token);
        Ok(crate::api_peer::Peer { token: token_facts(token.0)?, session: Some(token_dword(token.0, TokenSessionId, "TokenSessionId")?) })
    }
}

/// This process's peer facts (token and Windows session).
pub fn own_peer() -> Result<crate::api_peer::Peer, String> {
    Ok(crate::api_peer::Peer { token: own_token_facts()?, session: session_of(std::process::id()) })
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

/// A DWORD-sized token fact.
fn token_dword(token: HANDLE, class: TOKEN_INFORMATION_CLASS, what: &str) -> Result<u32, String> {
    let mut v = 0u32;
    let mut len = 0u32;
    if unsafe { GetTokenInformation(token, class, (&mut v as *mut u32).cast(), size_of::<u32>() as u32, &mut len) } == 0 {
        return Err(last_error(&format!("GetTokenInformation({what})")));
    }
    Ok(v)
}

/// The token's integrity level: the last subauthority of its mandatory label's SID.
fn token_integrity(token: HANDLE) -> Result<u32, String> {
    unsafe {
        let mut len = 0u32;
        GetTokenInformation(token, TokenIntegrityLevel, null_mut(), 0, &mut len);
        // u64 cells keep TOKEN_MANDATORY_LABEL aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
        if GetTokenInformation(token, TokenIntegrityLevel, buf.as_mut_ptr().cast(), len, &mut len) == 0 {
            return Err(last_error("GetTokenInformation(TokenIntegrityLevel)"));
        }
        let sid = (*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
        if sid.is_null() || IsValidSid(sid) == 0 {
            return Err("GetTokenInformation(TokenIntegrityLevel) failed: no valid label".into());
        }
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err("GetTokenInformation(TokenIntegrityLevel) failed: the label has no level".into());
        }
        Ok(*GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

/// Everything the peer check reads, from one token handle.
fn token_facts(token: HANDLE) -> Result<crate::api_peer::Token, String> {
    Ok(crate::api_peer::Token {
        sid: token_sid(token)?,
        elevated: token_elevated(token)?,
        integrity: token_integrity(token)?,
        app_container: token_dword(token, TokenIsAppContainer, "TokenIsAppContainer")? != 0,
        restricted: token_dword(token, TokenIsRestricted, "TokenIsRestricted")? != 0,
    })
}

/// The peer check's facts about process `pid`'s token, opened once.
pub fn process_token_facts(pid: u32) -> Result<crate::api_peer::Token, String> {
    token_facts(process_token(pid)?.0)
}

/// The full path of process `pid`'s program, when it can be read.
pub fn process_image(pid: u32) -> Option<String> {
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let h = Owned(h);
        let mut buf = vec![0u16; 4096];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) != 0;
        ok.then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// The Windows session id of process `pid` (`ProcessIdToSessionId`), when it can be read.
pub fn session_of(pid: u32) -> Option<u32> {
    use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    let mut session = 0u32;
    // SAFETY: `session` is a valid out pointer for the call.
    (unsafe { ProcessIdToSessionId(pid, &mut session) } != 0).then_some(session)
}

/// The peer check's facts about this process's token.
pub fn own_token_facts() -> Result<crate::api_peer::Token, String> {
    unsafe {
        let mut token: HANDLE = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(last_error("OpenProcessToken"));
        }
        let token = Owned(token);
        token_facts(token.0)
    }
}

/// `\\.\pipe\termurion-host-<user SID>`.
pub fn host_pipe_name() -> Result<String, String> {
    // An elevated Termurion has its own host: normal and elevated shells never mix.
    Ok(format!(r"\\.\pipe\termurion-host-{}{}", user_sid()?, if elevated()? { "-admin" } else { "" }))
}

/// `\\.\pipe\termurion-ssh-secrets-<user SID>`, plus `-admin` when elevated: the origin secrets
/// it2ssh fetches.
pub fn secret_pipe_name() -> Result<String, String> {
    // As the host pipe: an elevated Termurion's secrets are its own.
    Ok(format!(r"\\.\pipe\termurion-ssh-secrets-{}{}", user_sid()?, if elevated()? { "-admin" } else { "" }))
}

/// `\\.\pipe\termurion-url-<user SID>`, plus `-admin` when elevated: the pipe a second `termurion.exe
/// --url` hands an ssh: URL to (m14e). It is also the one channel for every other request of a
/// second `termurion.exe` (`launch::Request`, m13v1): there is no second app pipe.
pub fn url_pipe_name() -> Result<String, String> {
    // As the host pipe: an elevated Termurion's URLs are its own, never a normal one's.
    Ok(format!(r"\\.\pipe\termurion-url-{}{}", user_sid()?, if elevated()? { "-admin" } else { "" }))
}

/// The security descriptor of every pipe instance: a DACL that lets only `sid` in, and a
/// mandatory label with no-read-up. Elevated: high, so a normal-integrity process cannot open
/// it. Not elevated: medium, so a low-integrity process of this user (a sandboxed browser or
/// document reader) cannot open it even to read, where an unlabeled object only refuses the
/// write (the secret pipe hands a secret to every client that connects).
fn pipe_sddl(sid: &str, elevated: bool) -> String {
    let label = if elevated { "S:(ML;;NWNRNX;;;HI)" } else { "S:(ML;;NWNR;;;ME)" };
    format!("D:P(A;;GA;;;{sid}){label}")
}

/// A pipe instance whose DACL lets only this user connect; no remote clients.
fn create_instance(name: &str, first: bool) -> Result<HANDLE, String> {
    create_instance_with(name, first, &pipe_sddl(&user_sid()?, elevated()?))
}

fn create_instance_with(name: &str, first: bool, sddl: &str) -> Result<HANDLE, String> {
    unsafe {
        let sddl = wide(sddl);
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
    connect_owned(name, wait).map_err(|e| match e {
        ConnectError::Denied(e) | ConnectError::Other(e) => e,
    })
}

/// Why `connect_owned` failed.
#[derive(Debug)]
pub enum ConnectError {
    /// The pipe's DACL refused this user: someone else serves the name.
    Denied(String),
    Other(String),
}

/// `connect`, telling a pipe whose owner refused this user from any other failure.
pub fn connect_owned(name: &str, wait: Duration) -> Result<Option<Pipe>, ConnectError> {
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
                    return Err(ConnectError::Other(format!("the pipe {name} stayed busy")));
                }
                unsafe { WaitNamedPipeW(w.as_ptr(), left.as_millis().clamp(1, u32::MAX as u128 - 1) as u32) };
            }
            ERROR_ACCESS_DENIED => return Err(ConnectError::Denied(last_error(&format!("connecting to {name}")))),
            _ => return Err(ConnectError::Other(last_error(&format!("connecting to {name}")))),
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

    /// Reads what is there, waiting at most `wait`. None when nothing came; Some(0) when the other
    /// end closed. The read is cancelled and finished before this returns, so the `OVERLAPPED`
    /// outlives its I/O (R14de-10: `read` blocks without limit and is not for the URL pipe).
    pub fn read_timeout(&self, buf: &mut [u8], wait: Duration) -> Result<Option<usize>, String> {
        let open = self.gate.read().unwrap();
        if !*open || self.closed.load(Ordering::Acquire) {
            return Ok(Some(0));
        }
        let mut op = Op::new()?;
        let mut n = 0u32;
        unsafe {
            if ReadFile(self.handle(), buf.as_mut_ptr(), buf.len() as u32, null_mut(), &mut op.ov) != 0 {
                return if GetOverlappedResult(self.handle(), &op.ov, &mut n, 1) == 0 { closed_or(GetLastError(), "ReadFile").map(Some) } else { Ok(Some(n as usize)) };
            }
            if GetLastError() != ERROR_IO_PENDING {
                return closed_or(GetLastError(), "ReadFile").map(Some);
            }
            if WaitForSingleObject(op.event.0, wait.as_millis().min(u32::MAX as u128 - 1) as u32) != WAIT_OBJECT_0 {
                // The wait must end before `op` goes: cancel it and let it finish.
                CancelIoEx(self.handle(), &op.ov);
                if GetOverlappedResult(self.handle(), &op.ov, &mut n, 1) == 0 {
                    return match GetLastError() {
                        ERROR_OPERATION_ABORTED => Ok(None),
                        other => closed_or(other, "ReadFile").map(Some),
                    };
                }
                // The data landed as the cancel went in: it is not lost.
                return Ok(Some(n as usize));
            }
            if GetOverlappedResult(self.handle(), &op.ov, &mut n, 0) == 0 {
                return closed_or(GetLastError(), "ReadFile").map(Some);
            }
        }
        Ok(Some(n as usize))
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

    /// The id of the process serving this pipe.
    pub fn server_pid(&self) -> Result<u32, String> {
        let open = self.gate.read().unwrap();
        if !*open {
            return Err("the pipe is closed".into());
        }
        let mut pid = 0u32;
        if unsafe { GetNamedPipeServerProcessId(self.handle(), &mut pid) } == 0 {
            return Err(last_error("GetNamedPipeServerProcessId"));
        }
        Ok(pid)
    }

    /// Ok when the process serving this pipe runs as this user.
    pub fn server_is_this_user(&self) -> Result<(), String> {
        let pid = self.server_pid()?;
        let token = process_token(pid)?;
        let theirs = token_sid(token.0)?;
        let mine = user_sid()?;
        if theirs != mine {
            return Err(format!("the session host pipe is served by another user ({theirs})"));
        }
        if token_elevated(token.0)? != elevated()? {
            return Err("the session host runs at another elevation than this Termurion".into());
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

/// The same-user check of a local API connection (`api_peer`).
pub fn check_peer(peer: std::net::SocketAddr, local: std::net::SocketAddr) -> Result<u32, String> {
    crate::api_peer::check(peer, local)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) fn test_name(what: &str) -> String {
        static N: AtomicUsize = AtomicUsize::new(0);
        format!(r"\\.\pipe\termurion-test-{what}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))
    }

    fn pair() -> (Pipe, Pipe) {
        let name = test_name("pair");
        let mut l = Listener::bind(&name).unwrap();
        let n2 = name.clone();
        let t = std::thread::spawn(move || connect(&n2, Duration::from_secs(5)).unwrap().unwrap());
        let server = l.accept(Duration::from_secs(5)).unwrap().expect("a client");
        (server, t.join().unwrap())
    }

    /// A copy of this process's token lowered to Low integrity (as a sandbox broker makes for
    /// its children) reads as Low, so the peer check refuses it.
    #[test]
    fn a_lowered_token_reads_as_low() {
        use windows_sys::Win32::Security::{
            DuplicateTokenEx, SID_AND_ATTRIBUTES, SecurityImpersonation, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
            TOKEN_DUPLICATE, TokenPrimary,
        };
        const SE_GROUP_INTEGRITY: u32 = 0x20;
        unsafe {
            let mut own: HANDLE = null_mut();
            assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut own), 0);
            let own = Owned(own);
            let mut dup: HANDLE = null_mut();
            assert_ne!(DuplicateTokenEx(own.0, TOKEN_QUERY | TOKEN_ADJUST_DEFAULT, null(), SecurityImpersonation, TokenPrimary, &mut dup), 0);
            let dup = Owned(dup);
            let mut low: PSID = null_mut();
            assert_ne!(ConvertStringSidToSidW(wide("S-1-16-4096").as_ptr(), &mut low), 0);
            let label = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: low, Attributes: SE_GROUP_INTEGRITY } };
            let set = SetTokenInformation(dup.0, TokenIntegrityLevel, (&label as *const TOKEN_MANDATORY_LABEL).cast(), size_of::<TOKEN_MANDATORY_LABEL>() as u32);
            LocalFree(low as HLOCAL);
            assert_ne!(set, 0, "{}", last_error("SetTokenInformation"));
            let lowered = token_facts(dup.0).unwrap();
            let mine = own_token_facts().unwrap();
            assert_eq!(lowered.integrity, crate::api_peer::LOW_RID);
            assert_eq!(lowered.sid, mine.sid);
            assert_eq!(crate::api_peer::judge(&lowered, &mine), Err(crate::api_peer::OTHER_INTEGRITY.to_string()));
        }
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
    fn read_timeout_gives_none_on_silence() {
        let (server, client) = pair();
        let t = std::time::Instant::now();
        assert_eq!(client.read_timeout(&mut [0u8; 8], Duration::from_millis(100)), Ok(None));
        assert!(t.elapsed() < Duration::from_secs(1));
        server.write_all(b"x").unwrap();
        assert_eq!(client.read_timeout(&mut [0u8; 8], Duration::from_secs(2)), Ok(Some(1)));
        drop(server);
        assert_eq!(client.read_timeout(&mut [0u8; 8], Duration::from_secs(2)), Ok(Some(0)), "the other end closed");
    }

    #[test]
    fn url_pipe_name_follows_the_host_pipe() {
        let sid = user_sid().unwrap();
        let admin = elevated().unwrap();
        assert_eq!(url_pipe_name().unwrap(), format!(r"\\.\pipe\termurion-url-{sid}{}", if admin { "-admin" } else { "" }));
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
        assert_eq!(host_pipe_name().unwrap().ends_with("-admin"), admin, "an elevated Termurion never shares a host with a normal one");
    }

    #[test]
    fn the_host_runs_as_us() {
        assert!(user_sid().unwrap().starts_with("S-1-5-"));
        assert!(host_pipe_name().unwrap().starts_with(r"\\.\pipe\termurion-host-S-1-5-"));
        let (_server, client) = pair();
        client.server_is_this_user().unwrap();
    }

    #[test]
    fn secret_pipe_name_follows_the_host_pipe() {
        let name = secret_pipe_name().unwrap();
        assert!(name.starts_with(r"\\.\pipe\termurion-ssh-secrets-S-1-"));
        let sid = user_sid().unwrap();
        let admin = elevated().unwrap();
        let bare = name.strip_suffix("-admin").filter(|_| admin).unwrap_or(&name);
        assert!(bare.ends_with(&sid), "the name ends with the user SID");
        assert_eq!(name, format!(r"\\.\pipe\termurion-ssh-secrets-{sid}{}", if admin { "-admin" } else { "" }));
    }

    /// Every pipe carries a mandatory label: medium with no-read-up when not elevated (a
    /// low-integrity process of this user must not read a secret or the host's output), high
    /// when elevated.
    #[test]
    fn every_pipe_has_a_mandatory_label() {
        assert_eq!(pipe_sddl("S-1-5-21-1", false), "D:P(A;;GA;;;S-1-5-21-1)S:(ML;;NWNR;;;ME)");
        assert_eq!(pipe_sddl("S-1-5-21-1", true), "D:P(A;;GA;;;S-1-5-21-1)S:(ML;;NWNRNX;;;HI)");
    }

    /// The instance's label reads back from the kernel object as the SDDL asked for.
    #[test]
    fn the_pipe_label_reads_back() {
        use windows_sys::Win32::Security::Authorization::{ConvertSecurityDescriptorToStringSecurityDescriptorW, SE_KERNEL_OBJECT};
        use windows_sys::Win32::Security::LABEL_SECURITY_INFORMATION;
        let h = Owned(create_instance(&test_name("label"), true).unwrap());
        unsafe {
            let mut sd: PSECURITY_DESCRIPTOR = null_mut();
            let got = GetSecurityInfo(h.0, SE_KERNEL_OBJECT, LABEL_SECURITY_INFORMATION, null_mut(), null_mut(), null_mut(), null_mut(), &mut sd);
            assert_eq!(got, 0, "GetSecurityInfo failed with {got}");
            let mut text: *mut u16 = null_mut();
            let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(sd, SDDL_REVISION_1, LABEL_SECURITY_INFORMATION, &mut text, null_mut());
            LocalFree(sd as HLOCAL);
            assert_ne!(ok, 0, "{}", last_error("ConvertSecurityDescriptorToStringSecurityDescriptorW"));
            let len = (0..).take_while(|&i| *text.add(i) != 0).count();
            let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
            LocalFree(text as HLOCAL);
            // The kernel adds its own flags (AI) after "S:"; the ACE is what was asked for.
            let want = if elevated().unwrap() { "(ML;;NWNRNX;;;HI)" } else { "(ML;;NWNR;;;ME)" };
            assert!(sddl.starts_with("S:") && sddl.ends_with(want), "{sddl}");
        }
    }

    /// Opens `name` for reading only (the open a secret thief needs) on a thread that
    /// impersonates this user's token lowered to Low integrity; the Win32 error, 0 when opened.
    fn low_integrity_read_open(name: &str) -> u32 {
        use windows_sys::Win32::Security::{
            DuplicateTokenEx, RevertToSelf, SID_AND_ATTRIBUTES, SecurityImpersonation, SetTokenInformation, TOKEN_ADJUST_DEFAULT,
            TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TokenImpersonation,
        };
        use windows_sys::Win32::System::Threading::SetThreadToken;
        const SE_GROUP_INTEGRITY: u32 = 0x20;
        let name = name.to_string();
        std::thread::spawn(move || unsafe {
            let mut own: HANDLE = null_mut();
            assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut own), 0);
            let own = Owned(own);
            let mut dup: HANDLE = null_mut();
            let access = TOKEN_QUERY | TOKEN_ADJUST_DEFAULT | TOKEN_IMPERSONATE;
            assert_ne!(DuplicateTokenEx(own.0, access, null(), SecurityImpersonation, TokenImpersonation, &mut dup), 0);
            let dup = Owned(dup);
            let mut low: PSID = null_mut();
            assert_ne!(ConvertStringSidToSidW(wide("S-1-16-4096").as_ptr(), &mut low), 0);
            let label = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: low, Attributes: SE_GROUP_INTEGRITY } };
            let set = SetTokenInformation(dup.0, TokenIntegrityLevel, (&label as *const TOKEN_MANDATORY_LABEL).cast(), size_of::<TOKEN_MANDATORY_LABEL>() as u32);
            LocalFree(low as HLOCAL);
            assert_ne!(set, 0, "{}", last_error("SetTokenInformation"));
            assert_ne!(SetThreadToken(null(), dup.0), 0, "{}", last_error("SetThreadToken"));
            let h = CreateFileW(wide(&name).as_ptr(), GENERIC_READ, 0, null(), OPEN_EXISTING, 0, null_mut());
            let err = if h == INVALID_HANDLE_VALUE { GetLastError() } else { 0 };
            RevertToSelf();
            if h != INVALID_HANDLE_VALUE {
                CloseHandle(h);
            }
            err
        })
        .join()
        .unwrap()
    }

    /// A low-integrity process of this user cannot open a pipe even to read; without the
    /// label (the control) the same open succeeds, which is what the label closes.
    #[test]
    fn a_low_integrity_reader_is_refused() {
        use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;
        let sid = user_sid().unwrap();
        let labeled = test_name("low");
        let _h = Owned(create_instance_with(&labeled, true, &pipe_sddl(&sid, false)).unwrap());
        assert_eq!(low_integrity_read_open(&labeled), ERROR_ACCESS_DENIED);
        let bare = test_name("low-bare");
        let _b = Owned(create_instance_with(&bare, true, &format!("D:P(A;;GA;;;{sid})")).unwrap());
        assert_eq!(low_integrity_read_open(&bare), 0, "the control: no label lets a low reader in");
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
