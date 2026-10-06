//! Fetching the release feed with curl.exe (M13v2 Task 2): the one process this update path
//! starts, and the worker that runs one request at a time off the UI thread.

use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::pty::Notify;
use crate::script_provision::{KillJob, RealRunner};
use crate::update::{self, Fetch, Fetched};

/// What curl may write to stdout: the feed's cap and its status line.
pub(crate) const STDOUT_CAP: usize = update::MAX_FEED + 16;
/// The longest a request may run (curl's own `--max-time` is 30 s).
const TIME_CAP: Duration = Duration::from_secs(35);

/// A job handle is a kernel object, safe to use from any thread.
struct SharedJob(KillJob);
unsafe impl Send for SharedJob {}
unsafe impl Sync for SharedJob {}

/// The running curl's job, so quitting or cancelling can end it, and the cancel count a request
/// started under (a cancel between the two ends the request).
static RUNNING: Mutex<Option<Arc<SharedJob>>> = Mutex::new(None);
static CANCELS: AtomicU64 = AtomicU64::new(0);

/// Ends the running request, if any: Cancel, and the process teardown. Twice is safe.
pub fn cancel_running() {
    CANCELS.fetch_add(1, Ordering::SeqCst);
    if let Some(job) = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        job.0.terminate();
    }
}

/// Reads up to `cap` bytes (and a read more); true when there was more than that.
fn read_capped(r: &mut dyn Read, cap: usize) -> (Vec<u8>, bool) {
    let (mut all, mut buf) = (Vec::new(), [0u8; 8192]);
    loop {
        match r.read(&mut buf) {
            Ok(0) | Err(_) => return (all, false),
            Ok(n) => {
                all.extend_from_slice(&buf[..n]);
                if all.len() > cap {
                    return (all, true);
                }
            }
        }
    }
}

/// What curl's end means: its exit status, stdout (the body and the status line) and stderr.
fn outcome(code: Option<i32>, stdout: &[u8], stderr: &[u8], too_big: bool) -> Fetched {
    if too_big {
        return Fetched::Transport("feed too large".into());
    }
    let err = String::from_utf8_lossy(stderr).trim().to_string();
    match code {
        None => Fetched::Transport("curl was ended".into()),
        Some(0) => match update::split_status(&String::from_utf8_lossy(stdout)) {
            Some((code, body)) => Fetched::Status { code, body },
            None => Fetched::Transport(if err.is_empty() { "curl got no HTTP status".into() } else { err }),
        },
        Some(c) => Fetched::Transport(if err.is_empty() { format!("curl exited with code {c}") } else { err }),
    }
}

/// Tandem's environment for curl: the variables that choose its trust store or its config are
/// dropped (P2), and SSLKEYLOGFILE (it would write the session keys to a file); proxies stay curl's own.
fn curl_env(vars: impl IntoIterator<Item = (String, String)>) -> Vec<(String, String)> {
    const DROPPED: [&str; 6] = ["CURL_CA_BUNDLE", "SSL_CERT_FILE", "SSL_CERT_DIR", "CURL_HOME", "CURL_SSL_BACKEND", "SSLKEYLOGFILE"];
    vars.into_iter().filter(|(k, _)| !DROPPED.iter().any(|d| k.eq_ignore_ascii_case(d))).collect()
}

/// Runs `curl` on `url`: hidden, from its own folder, never through a shell, stdin empty, in a
/// job that ends it; stdout capped, the time capped, ended on cancel. `proto` is `=https`.
fn fetch(curl: &Path, url: &str, proto: &str) -> Fetched {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    let started = CANCELS.load(Ordering::SeqCst);
    let args = update::fetch_args(url, update::current_version(), proto);
    let job = match KillJob::new() {
        Ok(j) => Arc::new(SharedJob(j)),
        Err(e) => return Fetched::Transport(e),
    };
    let (mut out_r, out_w) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return Fetched::Transport(e.to_string()),
    };
    let (mut err_r, err_w) = match std::io::pipe() {
        Ok(p) => p,
        Err(e) => return Fetched::Transport(e.to_string()),
    };
    let inherited = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)));
    let mut cmd = Command::new(curl);
    cmd.args(&args).env_clear().envs(curl_env(inherited)).stdin(Stdio::null()).stdout(out_w).stderr(err_w).creation_flags(CREATE_NO_WINDOW | CREATE_SUSPENDED);
    if let Some(dir) = curl.parent() {
        cmd.current_dir(dir);
    }
    let spawned = cmd.spawn();
    drop(cmd);
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => return Fetched::Transport(format!("Could not run curl: {e}")),
    };
    if let Err(e) = job.0.adopt(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Fetched::Transport(e);
    }
    *RUNNING.lock().unwrap_or_else(|e| e.into_inner()) = Some(job.clone());
    let too_big = Arc::new(AtomicBool::new(false));
    let big = too_big.clone();
    let (out_tx, out_rx) = channel();
    std::thread::spawn(move || {
        let (bytes, over) = read_capped(&mut out_r, STDOUT_CAP);
        big.store(over, Ordering::SeqCst);
        let _ = out_tx.send(bytes);
    });
    let (err_tx, err_rx) = channel();
    std::thread::spawn(move || {
        let (bytes, _) = read_capped(&mut err_r, 8192);
        let _ = err_tx.send(bytes);
    });
    let deadline = Instant::now() + TIME_CAP;
    let end = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.code()),
            Ok(None) if CANCELS.load(Ordering::SeqCst) != started => break Err("cancelled"),
            Ok(None) if too_big.load(Ordering::SeqCst) => break Err("big"),
            Ok(None) if Instant::now() >= deadline => break Err("timed out"),
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => break Err("lost"),
        }
    };
    // Whatever curl left running holds the pipes: the job ends it, then the readers finish.
    job.0.terminate();
    let _ = child.kill();
    let _ = child.wait();
    {
        let mut running = RUNNING.lock().unwrap_or_else(|e| e.into_inner());
        if running.as_ref().is_some_and(|r| Arc::ptr_eq(r, &job)) {
            *running = None;
        }
    }
    let stdout = out_rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let stderr = err_rx.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    match end {
        // Ended by the job's terminate: that was a cancel, not curl's own exit.
        Ok(_) if CANCELS.load(Ordering::SeqCst) != started => Fetched::Transport("cancelled".into()),
        Ok(code) => outcome(code, &stdout, &stderr, too_big.load(Ordering::SeqCst)),
        Err("big") => Fetched::Transport("feed too large".into()),
        Err("cancelled") => Fetched::Transport("cancelled".into()),
        Err("timed out") => Fetched::Transport("the update check timed out".into()),
        Err(_) => Fetched::Transport("could not wait for curl".into()),
    }
}

/// The production fetch: `curl.exe` from System32, https only.
pub struct CurlFetch;

impl Fetch for CurlFetch {
    fn get(&self, url: &str) -> Fetched {
        match RealRunner::system_tool("curl.exe") {
            Ok(curl) => fetch(&curl, url, "=https"),
            Err(e) => Fetched::Transport(e),
        }
    }
}

/// The same for the loopback test: plain http allowed.
#[cfg(test)]
pub(crate) fn fetch_with(curl: &Path, url: &str) -> Fetched {
    fetch(curl, url, "=http")
}

struct State {
    busy: bool,
    /// Bumped by `cancel`: an answer for an older request is dropped.
    epoch: u64,
    result: Option<Fetched>,
}

/// One request at a time on a worker thread; the UI thread polls `try_result`.
pub struct Checker {
    tx: Sender<u64>,
    state: Arc<Mutex<State>>,
}

impl Checker {
    pub fn start(fetch: Box<dyn Fetch + Send>, notify: Notify) -> Checker {
        let state = Arc::new(Mutex::new(State { busy: false, epoch: 0, result: None }));
        let (tx, rx): (Sender<u64>, Receiver<u64>) = channel();
        let shared = state.clone();
        std::thread::spawn(move || {
            while let Ok(epoch) = rx.recv() {
                // Cancelled before it started: nothing to fetch.
                if shared.lock().unwrap_or_else(|e| e.into_inner()).epoch != epoch {
                    continue;
                }
                let got = fetch.get(update::RELEASES_URL);
                let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
                if s.epoch == epoch {
                    s.result = Some(got);
                    s.busy = false;
                }
                drop(s);
                notify();
            }
        });
        Checker { tx, state }
    }

    /// Starts a request unless one is running.
    pub fn check(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.busy {
            return;
        }
        s.busy = true;
        s.result = None;
        let _ = self.tx.send(s.epoch);
    }

    pub fn busy(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).busy
    }

    /// The finished answer, once.
    pub fn try_result(&self) -> Option<Fetched> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).result.take()
    }

    /// Ends the running request and drops its answer; a new check may start at once.
    pub fn cancel(&self) {
        {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.epoch += 1;
            s.busy = false;
            s.result = None;
        }
        cancel_running();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{Fetch, Fetched};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    /// The tests that start curl or cancel every running one never overlap.
    static SERIAL: Mutex<()> = Mutex::new(());

    /// A fake feed that answers once its gate is opened.
    struct Fake {
        calls: Arc<AtomicUsize>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Fetch for Fake {
        fn get(&self, _url: &str) -> Fetched {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (open, cv) = &*self.gate;
            let mut open = open.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            while !*open && Instant::now() < deadline {
                open = cv.wait_timeout(open, Duration::from_millis(50)).unwrap().0;
            }
            Fetched::Status { code: 200, body: "[]".into() }
        }
    }

    fn fake(open: bool) -> (Fake, Arc<AtomicUsize>, Arc<(Mutex<bool>, Condvar)>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((Mutex::new(open), Condvar::new()));
        (Fake { calls: calls.clone(), gate: gate.clone() }, calls, gate)
    }

    fn release(gate: &Arc<(Mutex<bool>, Condvar)>) {
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
    }

    fn wait_result(c: &Checker) -> Option<Fetched> {
        let end = Instant::now() + Duration::from_secs(2);
        while Instant::now() < end {
            if let Some(r) = c.try_result() {
                return Some(r);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    #[test]
    fn checker_uses_the_fake() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let woken = Arc::new(AtomicUsize::new(0));
        let w = woken.clone();
        let (f, calls, _gate) = fake(true);
        let c = Checker::start(Box::new(f), Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        c.check();
        assert_eq!(wait_result(&c), Some(Fetched::Status { code: 200, body: "[]".into() }));
        assert!(woken.load(Ordering::SeqCst) >= 1, "the event loop is woken when the answer is ready");
        assert_eq!(c.try_result(), None, "an answer is taken once");
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // A second check while one runs is ignored.
        let (f, calls, gate) = fake(false);
        let c = Checker::start(Box::new(f), Arc::new(|| {}));
        c.check();
        assert!(c.busy());
        c.check();
        release(&gate);
        assert!(wait_result(&c).is_some());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(c.try_result(), None);
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one request in flight at a time");
        assert!(!c.busy());

        // After cancel the pending answer is dropped, and a new check can start.
        let (f, calls, gate) = fake(false);
        let c = Checker::start(Box::new(f), Arc::new(|| {}));
        c.check();
        c.cancel();
        release(&gate);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(c.try_result(), None, "a cancelled check's answer is dropped");
        c.check();
        assert_eq!(wait_result(&c), Some(Fetched::Status { code: 200, body: "[]".into() }));
        // The cancelled request may never have started; the new one ran.
        assert!((1..=2).contains(&calls.load(Ordering::SeqCst)));
    }

    /// Answers each connection in turn with `status` and a small JSON body, then closes.
    fn serve(statuses: &'static [&'static str]) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for status in statuses {
                let Ok((mut s, _)) = listener.accept() else { return };
                let mut seen = Vec::new();
                let mut b = [0u8; 512];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut b) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&b[..n]),
                    }
                }
                let body = if status.starts_with("HTTP/1.1 200") { "[]" } else { "{\"message\":\"x\"}" };
                let _ = write!(s, "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
        });
        port
    }

    fn curl() -> std::path::PathBuf {
        crate::script_provision::RealRunner::system_tool("curl.exe").unwrap()
    }

    #[test]
    fn curl_reports_the_status() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let port = serve(&["HTTP/1.1 404 Not Found", "HTTP/1.1 401 Unauthorized", "HTTP/1.1 500 Internal Server Error", "HTTP/1.1 200 OK"]);
        let url = format!("http://127.0.0.1:{port}/");
        let at = |f: Fetched| match f {
            Fetched::Status { code, body } => (code, body),
            other => panic!("not a status: {other:?}"),
        };
        assert_eq!(at(fetch_with(&curl(), &url)).0, 404);
        assert_eq!(at(fetch_with(&curl(), &url)).0, 401);
        assert_eq!(at(fetch_with(&curl(), &url)).0, 500);
        assert_eq!(at(fetch_with(&curl(), &url)), (200, "[]".to_string()));
        // A port nobody listens on: curl reached no server.
        let dead = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        match fetch_with(&curl(), &format!("http://127.0.0.1:{dead}/")) {
            Fetched::Transport(text) => assert!(!text.is_empty(), "the transport error says something"),
            other => panic!("not a transport error: {other:?}"),
        }
    }

    #[test]
    fn quitting_cancels_a_running_fetch() {
        let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        // A server that accepts and never answers.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let holder = std::thread::spawn(move || {
            let conn = listener.accept().ok();
            std::thread::sleep(Duration::from_secs(6));
            drop(conn);
        });
        let url = format!("http://127.0.0.1:{port}/");
        let started = Instant::now();
        let worker = std::thread::spawn(move || fetch_with(&curl(), &url));
        std::thread::sleep(Duration::from_millis(800));
        cancel_running();
        let got = worker.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5), "curl was ended, not waited out");
        assert!(matches!(&got, Fetched::Transport(t) if t.contains("cancel")), "{got:?}");
        let _ = holder.join();
    }

    #[test]
    fn feed_over_the_cap_is_a_transport_error() {
        // A runner that writes 3 MiB: the read stops at the cap and says so.
        let mut big = std::io::repeat(b'x').take(3 * 1024 * 1024);
        let (kept, over) = read_capped(&mut big, STDOUT_CAP);
        assert!(over);
        assert!(kept.len() <= STDOUT_CAP + 8192, "no more than the cap and one read is kept");
        assert_eq!(outcome(Some(0), &kept, b"", over), Fetched::Transport("feed too large".into()));
        let mut small = std::io::Cursor::new(vec![b'y'; 1000]);
        assert_eq!(read_capped(&mut small, STDOUT_CAP), (vec![b'y'; 1000], false));
    }

    #[test]
    fn exits_become_fetches() {
        assert_eq!(outcome(Some(0), b"[]\n200", b"", false), Fetched::Status { code: 200, body: "[]".into() });
        assert_eq!(outcome(Some(0), b"{\"message\":\"Not Found\"}\n404", b"", false), Fetched::Status { code: 404, body: "{\"message\":\"Not Found\"}".into() });
        // No status line (curl wrote 000): no server was reached.
        assert_eq!(outcome(Some(0), b"\n000", b"curl: (28) timed out", false), Fetched::Transport("curl: (28) timed out".into()));
        assert_eq!(outcome(Some(7), b"\n000", b"curl: (7) Failed to connect\r\n", false), Fetched::Transport("curl: (7) Failed to connect".into()));
        assert_eq!(outcome(Some(7), b"", b"", false), Fetched::Transport("curl exited with code 7".into()));
        assert_eq!(outcome(None, b"", b"", false), Fetched::Transport("curl was ended".into()));
    }

    #[test]
    fn curl_never_reads_its_trust_from_the_environment() {
        let vars = ["PATH", "SystemRoot", "curl_ca_bundle", "SSL_CERT_FILE", "SSL_CERT_DIR", "CURL_HOME", "Curl_Ssl_Backend", "sslkeylogfile", "HTTPS_PROXY"];
        let kept: Vec<String> = curl_env(vars.iter().map(|k| (k.to_string(), "x".to_string()))).into_iter().map(|(k, _)| k).collect();
        assert_eq!(kept, ["PATH", "SystemRoot", "HTTPS_PROXY"], "the proxy is curl's own; the trust variables go");
    }

    #[test]
    fn one_curl_start_and_no_download() {
        let src = include_str!("update_fetch_win.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        assert_eq!(code.matches("Command::new(").count(), 1, "the one curl.exe start");
        for banned in ["\"-o\"", "\"-O\"", "\"--output\"", "std::fs", "ShellExecute", "CreateProcess", ".arg(\"/c\")"] {
            assert!(!code.contains(banned), "the update path downloads and writes nothing: {banned}");
        }
        let teardown = include_str!("app/mod.rs").split("fn teardown_process").nth(1).and_then(|r| r.split("\n}").next()).unwrap();
        assert!(teardown.contains("update_fetch_win::cancel_running"), "quitting ends a running check");
    }
}
