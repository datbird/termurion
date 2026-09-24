//! A child process running inside a ConPTY pseudoconsole.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::ptr::{null, null_mut};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, TryRecvError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Console::{COORD, HPCON};
use windows_sys::Win32::System::IO::CancelSynchronousIo;
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::conpty::Api;

pub struct Pty {
    api: Api,
    hpc: HPCON,
    /// Input goes through a writer thread, so a child that is slow to read never
    /// blocks the caller. Dropping the sender ends the thread, which closes the pipe.
    writer: Option<mpsc::Sender<Vec<u8>>>,
    write_error: Arc<std::sync::Mutex<Option<String>>>,
    process: HANDLE,
    output: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
    reader_done: Receiver<()>,
}

/// How long Drop waits for ConPTY to close the output pipe before it ends the child.
const CLOSE_WAIT: Duration = Duration::from_secs(2);

/// Called from background threads when a pty has new output or its child exits.
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// The result of a non-blocking read.
pub enum Read {
    Data(Vec<u8>),
    Empty,
    /// The output pipe closed. ConPTY closes it only after the pseudoconsole closes,
    /// so a child exit shows up in `exit_code`, not here.
    Closed,
}

fn check_size(cols: u16, rows: u16) -> Result<(), String> {
    if cols == 0 || rows == 0 || cols > i16::MAX as u16 || rows > i16::MAX as u16 {
        return Err(format!("bad pty size {cols}x{rows}"));
    }
    Ok(())
}

// SAFETY: the raw handles are process-wide kernel handles, valid on any thread. Pty is
// not Sync, so only one thread uses them at a time.
unsafe impl Send for Pty {}

fn os_err(what: &str) -> String {
    format!("{what} failed: {}", std::io::Error::last_os_error())
}

fn coord(cols: u16, rows: u16) -> COORD {
    COORD { X: cols as i16, Y: rows as i16 }
}

pub fn spawn(api: &Api, cmdline: &str, cols: u16, rows: u16) -> Result<Pty, String> {
    spawn_notify(api, cmdline, None, cols, rows, Arc::new(|| {}))
}

/// Starts `cmdline` in a new pseudoconsole, in `cwd` when given.
pub fn spawn_notify(api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, cols: u16, rows: u16, notify: Notify) -> Result<Pty, String> {
    check_size(cols, rows)?;
    let (hpc, input, output, process) = unsafe { start(api, cmdline, cwd, cols, rows)? };
    let (rx, reader_done, reader) = start_reader(output, notify.clone());
    let (writer, write_error) = start_writer(input);
    let pty = Pty { api: api.clone(), hpc, writer: Some(writer), write_error, process, output: rx, reader: Some(reader), reader_done };
    watch_exit(pty.process, notify)?;
    Ok(pty)
}

/// Calls `notify` once the child exits. It waits on its own copy of the process
/// handle, so the Pty can close its copy at any time.
fn watch_exit(process: HANDLE, notify: Notify) -> Result<(), String> {
    let mut copy: HANDLE = null_mut();
    let ok = unsafe {
        DuplicateHandle(GetCurrentProcess(), process, GetCurrentProcess(), &mut copy, 0, 0, DUPLICATE_SAME_ACCESS)
    };
    if ok == 0 {
        return Err(os_err("DuplicateHandle"));
    }
    let raw = copy as usize;
    std::thread::spawn(move || {
        let copy = raw as HANDLE;
        unsafe {
            WaitForSingleObject(copy, INFINITE);
            CloseHandle(copy);
        }
        notify();
    });
    Ok(())
}

/// Creates the pseudoconsole and the child. Returns (pseudoconsole, our input
/// write end, our output read end, child process).
unsafe fn start(api: &Api, cmdline: &str, cwd: Option<&std::path::Path>, cols: u16, rows: u16) -> Result<(HPCON, HANDLE, HANDLE, HANDLE), String> {
    unsafe {
        let (mut in_read, mut in_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        let (mut out_read, mut out_write): (HANDLE, HANDLE) = (null_mut(), null_mut());
        if CreatePipe(&mut in_read, &mut in_write, null(), 0) == 0 {
            return Err(os_err("CreatePipe (input)"));
        }
        if CreatePipe(&mut out_read, &mut out_write, null(), 0) == 0 {
            CloseHandle(in_read);
            CloseHandle(in_write);
            return Err(os_err("CreatePipe (output)"));
        }

        let mut hpc: HPCON = 0;
        let hr = (api.create)(coord(cols, rows), in_read, out_write, 0, &mut hpc);
        // ConPTY keeps its own copies of these two ends.
        CloseHandle(in_read);
        CloseHandle(out_write);
        if hr != 0 {
            CloseHandle(in_write);
            CloseHandle(out_read);
            return Err(format!("CreatePseudoConsole failed: HRESULT {hr:#010x}"));
        }
        let fail = |msg: String| {
            (api.close)(hpc);
            CloseHandle(in_write);
            CloseHandle(out_read);
            Err(msg)
        };

        let mut attr_size = 0usize;
        // This first call fails on purpose. It only reports the size to allocate.
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut attr_size);
        let mut attr_buf = vec![0u8; attr_size];
        let attrs = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
        if InitializeProcThreadAttributeList(attrs, 1, 0, &mut attr_size) == 0 {
            return fail(os_err("InitializeProcThreadAttributeList"));
        }
        // The attribute value is the HPCON itself, not a pointer to it.
        if UpdateProcThreadAttribute(
            attrs,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
            hpc as *const core::ffi::c_void,
            std::mem::size_of::<HPCON>(),
            null_mut(),
            null(),
        ) == 0
        {
            DeleteProcThreadAttributeList(attrs);
            return fail(os_err("UpdateProcThreadAttribute"));
        }

        let mut si: STARTUPINFOEXW = std::mem::zeroed();
        si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        // Null std handles with this flag, so the child cannot inherit ours.
        si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si.lpAttributeList = attrs;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let mut cmd: Vec<u16> = OsStr::new(cmdline).encode_wide().chain(Some(0)).collect();
        let dir: Option<Vec<u16>> = cwd.map(|d| d.as_os_str().encode_wide().chain(Some(0)).collect());
        let ok = CreateProcessW(
            null(),
            cmd.as_mut_ptr(),
            null(),
            null(),
            0,
            EXTENDED_STARTUPINFO_PRESENT,
            null(),
            dir.as_ref().map_or(null(), |d| d.as_ptr()),
            &si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(attrs);
        if ok == 0 {
            return fail(os_err(&format!("CreateProcessW ({cmdline})")));
        }
        CloseHandle(pi.hThread);
        Ok((hpc, in_write, out_read, pi.hProcess))
    }
}

/// Writes input on its own thread. The thread owns the input handle and closes it
/// when the sender is dropped or a write fails.
fn start_writer(input: HANDLE) -> (mpsc::Sender<Vec<u8>>, Arc<std::sync::Mutex<Option<String>>>) {
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let error = Arc::new(std::sync::Mutex::new(None));
    let seen = error.clone();
    let raw = input as usize;
    std::thread::spawn(move || {
        let input = raw as HANDLE;
        'chunks: for chunk in rx {
            let mut done = 0;
            while done < chunk.len() {
                let mut n = 0u32;
                let rest = &chunk[done..];
                let ok = unsafe { WriteFile(input, rest.as_ptr(), rest.len() as u32, &mut n, null_mut()) };
                if ok == 0 {
                    *seen.lock().unwrap() = Some(os_err("WriteFile"));
                    break 'chunks;
                }
                done += n as usize;
            }
        }
        unsafe { CloseHandle(input) };
    });
    (tx, error)
}

/// Drains ConPTY output on its own thread, so ConPTY never blocks on a full pipe.
fn start_reader(output: HANDLE, notify: Notify) -> (Receiver<Vec<u8>>, Receiver<()>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    // HANDLE is a raw pointer and not Send. The thread owns this handle from here on.
    let raw = output as usize;
    let reader = std::thread::spawn(move || {
        let output = raw as HANDLE;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let mut n = 0u32;
            let ok = unsafe { ReadFile(output, buf.as_mut_ptr(), buf.len() as u32, &mut n, null_mut()) };
            if ok == 0 || n == 0 || tx.send(buf[..n as usize].to_vec()).is_err() {
                break;
            }
            notify();
        }
        unsafe { CloseHandle(output) };
        let _ = done_tx.send(());
        notify();
    });
    (rx, done_rx, reader)
}

impl Pty {
    /// Queues `bytes` for the child. Returns at once. A failed earlier write is reported here.
    pub fn write(&self, bytes: &[u8]) -> Result<(), String> {
        if let Some(e) = self.write_error.lock().unwrap().clone() {
            return Err(e);
        }
        match &self.writer {
            Some(w) => w.send(bytes.to_vec()).map_err(|_| "the pty writer has stopped".to_string()),
            None => Err("the pty is closed".to_string()),
        }
    }

    pub fn resize(&self, cols: u16, rows: u16) -> Result<(), String> {
        check_size(cols, rows)?;
        let hr = unsafe { (self.api.resize)(self.hpc, coord(cols, rows)) };
        if hr != 0 {
            return Err(format!("ResizePseudoConsole failed: HRESULT {hr:#010x}"));
        }
        Ok(())
    }

    /// Clears ConPTY's own buffer, keeping the cursor row, so it matches a cleared screen.
    pub fn clear(&self) -> Result<(), String> {
        let Some(clear) = self.api.clear else {
            return Err("the Windows copy of ConPTY cannot clear its buffer".to_string());
        };
        let hr = unsafe { clear(self.hpc, 1) };
        if hr != 0 {
            return Err(format!("ConptyClearPseudoConsole failed: HRESULT {hr:#010x}"));
        }
        Ok(())
    }

    pub fn try_read(&self) -> Read {
        match self.output.try_recv() {
            Ok(chunk) => Read::Data(chunk),
            Err(TryRecvError::Empty) => Read::Empty,
            Err(TryRecvError::Disconnected) => Read::Closed,
        }
    }

    /// The child's exit code, or None while it runs.
    pub fn exit_code(&self) -> Option<u32> {
        self.wait_exit(Duration::ZERO)
    }

    /// Returns the next chunk of output, or None if nothing came within `wait`.
    pub fn read_timeout(&self, wait: Duration) -> Option<Vec<u8>> {
        match self.output.recv_timeout(wait) {
            Ok(chunk) => Some(chunk),
            Err(RecvTimeoutError::Timeout) => None,
            // The output pipe closed. Sleep, so callers that poll do not spin.
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(wait);
                None
            }
        }
    }

    /// Reads until `needle` appears. Ok with everything read on a hit, Err with
    /// everything read on timeout.
    pub fn read_until(&self, needle: &[u8], wait: Duration) -> Result<Vec<u8>, Vec<u8>> {
        let end = Instant::now() + wait;
        let mut all = Vec::new();
        loop {
            if all.windows(needle.len()).any(|w| w == needle) {
                return Ok(all);
            }
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(all);
            }
            if let Some(chunk) = self.read_timeout(left.min(Duration::from_millis(50))) {
                all.extend(chunk);
            }
        }
    }

    /// Waits for the child to exit and returns its exit code.
    pub fn wait_exit(&self, wait: Duration) -> Option<u32> {
        if unsafe { WaitForSingleObject(self.process, wait.as_millis() as u32) } != WAIT_OBJECT_0 {
            return None;
        }
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process, &mut code) } == 0 {
            return None;
        }
        Some(code)
    }
}

impl Pty {
    /// The Windows copy of ConPTY can ignore the close (milestone 0 results), so the
    /// output pipe never ends. End the child, then cancel the reader's blocked read until
    /// the reader exits. Returns false if it never does.
    fn force_reader_stop(&self, reader: &JoinHandle<()>) -> bool {
        eprintln!("tandem: ConPTY ignored the close; ending the child");
        unsafe { TerminateProcess(self.process, 1) };
        let thread = reader.as_raw_handle() as HANDLE;
        for _ in 0..50 {
            // A cancel only hits a read in progress, so repeat it until the reader is out.
            unsafe { CancelSynchronousIo(thread) };
            if self.reader_done.recv_timeout(Duration::from_millis(20)).is_ok() {
                return true;
            }
        }
        false
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        // Closing the pseudoconsole ends the child and closes the output pipe. The
        // reader thread keeps draining meanwhile, so the close cannot block on a full pipe.
        unsafe { (self.api.close)(self.hpc) };
        // The writer thread closes the input pipe once its queue ends or a write fails.
        self.writer.take();
        if let Some(reader) = self.reader.take() {
            if self.reader_done.recv_timeout(CLOSE_WAIT).is_err() && !self.force_reader_stop(&reader) {
                eprintln!("tandem: the pty reader did not stop; leaving it running");
                unsafe { CloseHandle(self.process) };
                return;
            }
            let _ = reader.join();
        }
        unsafe { CloseHandle(self.process) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conpty::{self, Source, dev_vendor_dir};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    fn apis() -> Vec<Api> {
        vec![
            conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap(),
            conpty::load(&Source::Inbox).unwrap(),
        ]
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[test]
    fn runs_a_command_and_reads_output() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c echo tandem-ok", 80, 24).unwrap();
            p.read_until(b"tandem-ok", secs(10))
                .unwrap_or_else(|got| panic!("{}: got {:?}", api.source.label(), text(&got)));
            assert_eq!(p.wait_exit(secs(10)), Some(0), "{}", api.source.label());
        }
    }

    #[test]
    fn reports_exit_code() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c exit 7", 80, 24).unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(7), "{}", api.source.label());
        }
    }

    #[test]
    fn input_reaches_the_child() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
            // cmd prints abc123 only when it runs the echo. The typed line shows abc^123.
            p.write(b"echo abc^123\r").unwrap();
            p.read_until(b"abc123", secs(10))
                .unwrap_or_else(|got| panic!("{}: got {:?}", api.source.label(), text(&got)));
            p.write(b"exit 3\r").unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(3), "{}", api.source.label());
        }
    }

    #[test]
    fn resize_succeeds() {
        // Shipped ConPTY only. The Windows copy loses the close if a resize arrives while it
        // is still starting (about 1 run in 8), and the drop then waits forever. Milestone 0
        // measured this; see docs/probe.
        let api = conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap();
        let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
        p.resize(120, 40).unwrap();
    }

    #[test]
    fn reader_survives_a_zero_byte_write() {
        let (mut r, mut w): (HANDLE, HANDLE) = (null_mut(), null_mut());
        assert_ne!(unsafe { CreatePipe(&mut r, &mut w, null(), 0) }, 0);
        let (rx, _done, reader) = start_reader(r, Arc::new(|| {}));
        let mut n = 0u32;
        unsafe {
            WriteFile(w, b"".as_ptr(), 0, &mut n, null_mut());
            WriteFile(w, b"x".as_ptr(), 1, &mut n, null_mut());
            CloseHandle(w);
        }
        reader.join().unwrap();
        let got: Vec<u8> = rx.try_iter().flatten().collect();
        assert_eq!(got, b"x");
    }

    #[test]
    fn pty_can_move_to_another_thread() {
        // Milestone 1 closes panes off the UI thread.
        fn assert_send<T: Send>() {}
        assert_send::<Pty>();
    }

    #[test]
    fn inbox_drop_after_early_resize_is_bounded() {
        // The Windows copy loses the close if a resize arrives while it starts (about 1 run
        // in 8, milestone 0 results). Drop must still return.
        let api = conpty::load(&Source::Inbox).unwrap();
        for run in 0..20 {
            let p = spawn(&api, "cmd.exe /q /k", 80, 24).unwrap();
            p.resize(120, 40).unwrap();
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "run {run}: drop took {:?}", start.elapsed());
        }
    }

    fn shipped() -> Api {
        conpty::load(&Source::Shipped(dev_vendor_dir())).unwrap()
    }

    #[test]
    fn notify_fires_on_output_and_exit() {
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let p = spawn_notify(&shipped(), "cmd.exe /c echo notify-me", None, 80, 24, Arc::new(move || {
            c.fetch_add(1, Ordering::SeqCst);
        }))
        .unwrap();
        assert_eq!(p.wait_exit(secs(10)), Some(0));
        let end = Instant::now() + secs(5);
        while count.load(Ordering::SeqCst) < 2 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(count.load(Ordering::SeqCst) >= 2, "notify ran {} times", count.load(Ordering::SeqCst));
    }

    #[test]
    fn try_read_returns_output_without_blocking() {
        let p = spawn(&shipped(), "cmd.exe /q /k echo try-read", 80, 24).unwrap();
        let mut all = Vec::new();
        let end = Instant::now() + secs(10);
        while !text(&all).contains("try-read") && Instant::now() < end {
            match p.try_read() {
                Read::Data(d) => all.extend(d),
                Read::Empty => std::thread::sleep(Duration::from_millis(20)),
                Read::Closed => panic!("output closed early"),
            }
        }
        assert!(text(&all).contains("try-read"), "{:?}", text(&all));
    }

    #[test]
    fn exit_code_is_none_while_running() {
        let p = spawn(&shipped(), "cmd.exe /q /k", 80, 24).unwrap();
        assert_eq!(p.exit_code(), None);
    }

    #[test]
    fn zero_size_is_an_error() {
        let err = spawn(&shipped(), "cmd.exe /q /k", 0, 24).err().unwrap();
        assert!(err.contains("size"), "{err}");
        let p = spawn(&shipped(), "cmd.exe /q /k", 80, 24).unwrap();
        assert!(p.resize(80, 0).unwrap_err().contains("size"));
    }

    #[test]
    fn write_does_not_block_when_the_child_is_not_reading() {
        // ping never reads its input, so a large write fills the pipe.
        let p = spawn(&shipped(), "ping -n 8 127.0.0.1", 80, 24).unwrap();
        let t = Instant::now();
        p.write(&vec![b'a'; 32 << 20]).unwrap();
        assert!(t.elapsed() < secs(1), "write blocked for {:?}", t.elapsed());
    }

    #[test]
    fn the_child_starts_in_the_given_directory() {
        let dir = std::env::temp_dir();
        let p = spawn_notify(&shipped(), "cmd.exe /c cd", Some(&dir), 200, 24, Arc::new(|| {})).unwrap();
        let want = dir.to_string_lossy().trim_end_matches('\\').to_string();
        p.read_until(want.as_bytes(), secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
    }

    #[test]
    fn bad_command_is_an_error() {
        let err = spawn(&apis()[0], "no-such-program-xyz.exe", 80, 24).err().unwrap();
        assert!(err.contains("CreateProcessW"), "{err}");
    }

    #[test]
    fn drop_returns_while_child_runs() {
        for api in apis() {
            let p = spawn(&api, "ping -n 30 127.0.0.1", 80, 24).unwrap();
            p.read_until(b"Pinging", secs(10)).unwrap_or_else(|got| panic!("{:?}", text(&got)));
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "{}: drop took {:?}", api.source.label(), start.elapsed());
        }
    }

    #[test]
    fn drop_returns_after_child_exits() {
        for api in apis() {
            let p = spawn(&api, "cmd.exe /c exit 0", 80, 24).unwrap();
            assert_eq!(p.wait_exit(secs(10)), Some(0));
            let start = Instant::now();
            drop(p);
            assert!(start.elapsed() < secs(5), "{}: drop took {:?}", api.source.label(), start.elapsed());
        }
    }
}

